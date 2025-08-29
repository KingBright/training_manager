use anyhow::Result;
use nix::unistd::setsid;
use std::sync::Arc;
use tokio::{fs as tokio_fs, process::Command, sync::Mutex};
use tracing::{error, info};

use crate::models::{AppState, Task, TaskInfo, TaskStatus};

// --- Task Manager Background Service ---

pub struct TaskManager {
    state: AppState,
}

impl TaskManager {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    pub async fn run(self) {
        loop {
            let task_id = self.get_next_task_from_queue().await;

            if let Some(task_id) = task_id {
                let state = self.state.clone();
                tokio::spawn(async move {
                    if let Err(e) = Self::execute_task(state, &task_id).await {
                        error!("Failed to execute task {}: {}", task_id, e);
                    }
                });
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    async fn get_next_task_from_queue(&self) -> Option<String> {
        let mut queue = self.state.queue.lock().await;
        queue.pop()
    }

    async fn execute_task(state: AppState, task_id: &str) -> Result<()> {
        let mut task = match sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&state.db)
            .await
        {
            Ok(task) => task,
            Err(e) => {
                error!("Could not fetch task {} from DB: {}", task_id, e);
                return Ok(());
            }
        };

        if task.status != TaskStatus::Queued {
            info!(
                "Task {} has status {:?}, skipping execution.",
                task_id, task.status
            );
            return Ok(());
        }

        let config = state.config.read().await;
        let log_dir = std::path::Path::new(&config.storage.output_path).join(task_id);
        tokio_fs::create_dir_all(&log_dir).await?;
        let log_path = log_dir.join("task.log");
        let log_file = std::fs::File::create(&log_path)?;

        let working_dir = task.working_dir.clone().unwrap_or_else(|| {
            config
                .tasks
                .working_directory
                .to_string_lossy()
                .to_string()
        });

        let mut cmd = Command::new("bash");
        cmd.current_dir(&working_dir)
            .arg("-c")
            .arg(&task.command)
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        // Set the process group ID to ensure the process and its children can be killed together.
        unsafe {
            cmd.pre_exec(|| {
                setsid().map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, format!("setsid failed: {}", e))
                })?;
                Ok(())
            });
        }

        let child = cmd.spawn()?;
        let pid = child.id().map(|id| id as i64);

        let now = chrono::Utc::now();
        let log_path_str = log_path.to_str().map(|s| s.to_string());

        task.status = TaskStatus::Running;
        task.started_at = Some(now);
        task.log_path = log_path_str.clone();
        task.pid = pid;

        if let Err(e) =
            sqlx::query("UPDATE tasks SET status = ?, started_at = ?, log_path = ?, pid = ? WHERE id = ?")
                .bind(task.status)
                .bind(task.started_at)
                .bind(&task.log_path)
                .bind(task.pid)
                .bind(task_id)
                .execute(&state.db)
                .await
        {
            error!("Failed to update task {} to running state: {}", task_id, e);
            // If we can't update the DB, we shouldn't proceed.
            return Err(e.into());
        }

        let child_arc = Arc::new(Mutex::new(child));

        let task_info = TaskInfo {
            task: task.clone(),
            process: Some(child_arc.clone()),
        };
        state
            .tasks
            .write()
            .await
            .insert(task_id.to_string(), task_info);

        let wait_state = state.clone();
        let wait_task_id = task_id.to_string();
        tokio::spawn(async move {
            let status = match child_arc.lock().await.wait().await {
                Ok(status) => status,
                Err(e) => {
                    error!("Failed to wait for task {}: {}", wait_task_id, e);
                    return;
                }
            };

            // The task might have been stopped manually. If so, it will be removed from the map.
            // If we can remove it, it means it finished naturally.
            if let Some(_removed_task) = wait_state.tasks.write().await.remove(&wait_task_id) {
                let final_status = if status.success() {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                };
                let finished_at = chrono::Utc::now();

                if let Err(e) =
                    sqlx::query("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?")
                        .bind(final_status)
                        .bind(finished_at)
                        .bind(&wait_task_id)
                        .execute(&state.db)
                        .await
                {
                    error!(
                        "Failed to update task {} status after completion: {}",
                        wait_task_id, e
                    );
                }
                info!(
                    "Task {} finished with status: {:?}",
                    wait_task_id, final_status
                );
            } else {
                // If the task was not in the map, it means it was stopped via the API.
                // The stop_task_handler is responsible for updating the DB in this case.
                info!(
                    "Task {} was stopped manually, skipping final status update.",
                    wait_task_id
                );
            }
        });

        Ok(())
    }
}

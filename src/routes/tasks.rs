use axum::{
    extract::{Path, State},
    Json,
};
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tokio::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    error::AppError,
    metrics_parser,
    models::{AppState, CreateTaskRequest, Task, TaskStatus},
};

// --- Route Handlers ---

pub async fn list_tasks_handler(State(state): State<AppState>) -> Result<Json<Vec<Task>>, AppError> {
    let tasks = sqlx::query_as::<_, Task>("SELECT * FROM tasks ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(tasks))
}

pub async fn create_task_handler(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    let id = Uuid::new_v4().to_string();
    let config = state.config.read().await;
    let conda_env = request
        .conda_env
        .unwrap_or_else(|| config.isaaclab.default_conda_env.clone());
    let task_name = extract_task_name(&request.command);

    let task = Task {
        id: id.clone(),
        name: task_name,
        command: request.command.clone(),
        conda_env: Some(conda_env.clone()),
        working_dir: request.working_dir.clone(),
        status: TaskStatus::Queued,
        pid: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: None,
        log_path: None,
    };

    sqlx::query("INSERT INTO tasks (id, name, command, conda_env, working_dir, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&task.id)
        .bind(&task.name)
        .bind(&task.command)
        .bind(&task.conda_env)
        .bind(&task.working_dir)
        .bind(&task.status)
        .bind(task.created_at)
        .execute(&state.db)
        .await?;

    state.queue.lock().await.push(id.clone());
    info!(
        "Created task: {} with conda env: {} and command: {}",
        id, conda_env, request.command
    );
    Ok(Json(task))
}

pub async fn get_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, AppError> {
    sqlx::query_as("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::TaskNotFound(id))
}

pub async fn stop_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Remove the task from the live tasks map. This prevents the background `wait` task
    // from overwriting the status after we set it to "Stopped".
    let _ = state.tasks.write().await.remove(&id);

    // Fetch the task from the database to get the PID.
    let task = sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::TaskNotFound(id.clone()))?;

    if let Some(pid) = task.pid {
        if pid > 0 {
            info!("Attempting to stop process group with PID: {}", pid);
            // Use nix to kill the entire process group by passing a negative PID.
            let pgid = Pid::from_raw(-pid as i32);
            match signal::kill(pgid, Signal::SIGKILL) {
                Ok(_) => info!("Successfully sent SIGKILL to process group {}", pid),
                Err(e) => {
                    // It's not a critical error if the process doesn't exist (e.g., it already finished)
                    // We can just log a warning.
                    warn!(
                        "Failed to kill process group {}: {}. This might be because the process already stopped.",
                        pid, e
                    );
                }
            }
        }
    }

    let now = chrono::Utc::now();
    sqlx::query("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?")
        .bind(TaskStatus::Stopped)
        .bind(now)
        .bind(&id)
        .execute(&state.db)
        .await?;

    info!("Task {} marked as stopped.", id);
    Ok(Json(serde_json::json!({"message": "Task stopped"})))
}

pub async fn delete_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // First, ensure the task is stopped.
    let _ = stop_task_handler(State(state.clone()), Path(id.clone())).await;

    // Then, delete the record from the database.
    sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    state.tasks.write().await.remove(&id);
    Ok(Json(serde_json::json!({"message": "Task deleted"})))
}

pub async fn get_task_logs_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, AppError> {
    let task = get_task_handler(State(state), Path(id)).await?.0;
    match task.log_path {
        Some(log_path) => {
            let content = tokio::fs::read_to_string(&log_path)
                .await
                .unwrap_or_else(|_| "Log not found or empty.".to_string());
            let lines: Vec<&str> = content.lines().collect();
            let last_200_lines = lines
                .iter()
                .rev()
                .take(200)
                .rev()
                .map(|s| *s)
                .collect::<Vec<&str>>()
                .join("\n");
            Ok(last_200_lines)
        }
        None => Ok("Log path not set.".to_string()),
    }
}

pub async fn get_task_metrics_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<metrics_parser::MetricsData>, AppError> {
    let task = get_task_handler(State(state.clone()), Path(id.clone()))
        .await?
        .0;
    match task.log_path {
        Some(log_path) => {
            let content = tokio::fs::read_to_string(&log_path)
                .await
                .unwrap_or_else(|_| "".to_string());
            let metrics = tokio::task::spawn_blocking(move || {
                metrics_parser::parse_log_file(&content)
            })
            .await
            .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
            Ok(Json(metrics))
        }
        None => {
            // Return empty metrics if log path is not set
            Ok(Json(metrics_parser::MetricsData {
                latest_fixed_metrics: std::collections::HashMap::new(),
                historical_metrics: std::collections::HashMap::new(),
            }))
        }
    }
}

pub async fn get_queue_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.queue.lock().await.clone())
}

pub async fn get_conda_envs_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, AppError> {
    let config = state.config.read().await;
    let conda_path = config.isaaclab.conda_path.to_string_lossy().to_string();
    Ok(Json(get_conda_environments(&conda_path).await?))
}

// --- Utility Functions ---

fn extract_task_name(command: &str) -> String {
    command
        .split_whitespace()
        .find(|part| part.starts_with("--task="))
        .and_then(|part| part.strip_prefix("--task="))
        .unwrap_or("Training Task")
        .to_string()
}

async fn get_conda_environments(conda_path: &str) -> Result<Vec<String>, AppError> {
    let output = Command::new(format!("{}/bin/conda", conda_path))
        .args(["env", "list"])
        .output()
        .await?;
    if !output.status.success() {
        return Ok(vec!["base".to_string()]);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(String::from)
        .collect())
}

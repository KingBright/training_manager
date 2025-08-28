use anyhow::Result;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{
    collections::HashMap,
    os::unix::process::CommandExt,
    sync::Arc,
};
use tokio::{
    fs as tokio_fs,
    process::{Child, Command},
    sync::{Mutex, RwLock},
};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{error, info};
use axum::extract::multipart::MultipartError;

mod config;
mod metrics_parser;
mod routes;

/// IsaacLab Manager Server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Port to run the server on
    #[arg(short, long)]
    port: Option<u16>,
}

// --- Data Structures ---

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    pub conda_env: Option<String>,
    pub working_dir: Option<String>,
    pub status: TaskStatus,
    #[sqlx(default)]
    pub pid: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "task_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub command: String,
    pub conda_env: Option<String>,
    pub working_dir: Option<String>,
}

#[derive(Serialize)]
pub struct SyncConfigResponse {
    pub default_excludes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SyncRequest {
    pub remote_path: Option<String>,
}

// --- Application State and Error Handling ---

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub tasks: Arc<RwLock<HashMap<String, TaskInfo>>>,
    pub queue: Arc<Mutex<Vec<String>>>,
    pub current_task: Arc<Mutex<Option<String>>>,
    pub config: Arc<RwLock<config::Config>>,
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub task: Task,
    pub process: Option<Arc<Mutex<Child>>>,
}

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Task already running")]
    TaskAlreadyRunning,
    #[error("Multipart error: {0}")]
    Multipart(#[from] MultipartError),
    #[error("Config error: {0}")]
    Config(#[from] anyhow::Error),
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_message) = match self {
            AppError::Database(err) => {
                error!("Database error: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error".to_string())
            }
            AppError::Io(err) => {
                error!("IO error: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "IO error".to_string())
            }
            AppError::TaskNotFound(id) => (StatusCode::NOT_FOUND, format!("Task not found: {}", id)),
            AppError::TaskAlreadyRunning => (StatusCode::CONFLICT, "Task already running".to_string()),
            AppError::Multipart(err) => {
                error!("Multipart error: {}", err);
                (StatusCode::BAD_REQUEST, format!("Multipart form error: {}", err))
            }
            AppError::Config(err) => {
                error!("Configuration error: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Configuration error".to_string())
            }
        };
        (status, Json(serde_json::json!({ "error": error_message }))).into_response()
    }
}

// --- Main Application ---

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();
    
    let database_url = "sqlite:./isaaclab_manager.db";
    if !Sqlite::database_exists(database_url).await.unwrap_or(false) {
        Sqlite::create_database(database_url).await?;
    }
    
    let db = SqlitePool::connect(database_url).await?;
    info!("Running database migrations...");
    match sqlx::migrate!("./migrations").run(&db).await {
        Ok(_) => info!("Database migrations completed successfully."),
        Err(e) => {
            error!("Database migration failed: {}", e);
        }
    }
    
    let mut config = config::Config::load(&db).await?;
    if let Some(port) = args.port {
        config.server.port = port;
    }
    let state = AppState {
        db: db.clone(),
        tasks: Arc::new(RwLock::new(HashMap::new())),
        queue: Arc::new(Mutex::new(Vec::new())),
        current_task: Arc::new(Mutex::new(None)),
        config: Arc::new(RwLock::new(config)),
    };
    
    let task_manager = TaskManager::new(state.clone());
    tokio::spawn(task_manager.run());
    
    let app = Router::new()
        .route("/", get(index_handler))
        .merge(routes::create_api_router())
        .nest_service("/static", ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());
    
    let addr = {
        let config_guard = state.config.read().await;
        format!("{}:{}", config_guard.server.host, config_guard.server.port)
    };
    info!("Starting IsaacLab Manager on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

// --- Route Handlers ---

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

// --- Utility Functions ---

pub fn extract_task_name(command: &str) -> String {
    command.split_whitespace()
        .find(|part| part.starts_with("--task="))
        .and_then(|part| part.strip_prefix("--task="))
        .unwrap_or("Training Task")
        .to_string()
}

// --- Task Manager Background Service ---

struct TaskManager {
    state: AppState,
}

impl TaskManager {
    fn new(state: AppState) -> Self { Self { state } }

    async fn run(self) {
        loop {
            if let Some(task_id) = self.state.queue.lock().await.pop() {
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
            info!("Task {} has status {:?}, skipping execution.", task_id, task.status);
            return Ok(());
        }

        let config = state.config.read().await;
        let log_dir = std::path::Path::new(&config.storage.output_path).join(task_id);
        tokio_fs::create_dir_all(&log_dir).await?;
        let log_path = log_dir.join("task.log");
        let log_file = std::fs::File::create(&log_path)?;

        let working_dir = task.working_dir.clone().unwrap_or_else(|| config.tasks.working_directory.to_string_lossy().to_string());

        let mut cmd = Command::new("bash");
        cmd.current_dir(&working_dir)
            .arg("-c")
            .arg(&task.command)
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("setsid failed: {}", e)))?;
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

        if let Err(e) = sqlx::query("UPDATE tasks SET status = ?, started_at = ?, log_path = ?, pid = ? WHERE id = ?")
            .bind(task.status)
            .bind(task.started_at)
            .bind(&task.log_path)
            .bind(task.pid)
            .bind(task_id)
            .execute(&state.db).await {
            error!("Failed to update task {} to running state: {}", task_id, e);
            return Err(e.into());
        }

        let child_arc = Arc::new(Mutex::new(child));

        let task_info = TaskInfo {
            task: task.clone(),
            process: Some(child_arc.clone()),
        };
        state.tasks.write().await.insert(task_id.to_string(), task_info);

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

            if let Some(_removed_task) = wait_state.tasks.write().await.remove(&wait_task_id) {
                let final_status = if status.success() { TaskStatus::Completed } else { TaskStatus::Failed };
                let finished_at = chrono::Utc::now();

                if let Err(e) = sqlx::query("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?")
                    .bind(final_status)
                    .bind(finished_at)
                    .bind(&wait_task_id)
                    .execute(&wait_state.db).await {
                    error!("Failed to update task {} status after completion: {}", wait_task_id, e);
                }
                info!("Task {} finished with status: {:?}", wait_task_id, final_status);
            } else {
                info!("Task {} was stopped manually, skipping final status update.", wait_task_id);
            }
        });

        Ok(())
    }
}

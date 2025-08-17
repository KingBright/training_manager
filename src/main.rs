use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use axum_extra::extract::{multipart::MultipartError, Multipart};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{
    collections::HashMap,
    fs::File,
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    fs as tokio_fs, // Use an alias to avoid conflict with std::fs
    process::{Child, Command},
    sync::{Mutex, RwLock},
};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{error, info};
use uuid::Uuid;
use walkdir::WalkDir;

mod config;

// --- Data Structures ---

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    pub conda_env: Option<String>,
    pub working_dir: Option<String>,
    pub status: TaskStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub log_path: Option<String>,
    pub tensorboard_port: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "task_status", rename_all = "lowercase")]
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

// --- Application State and Error Handling ---

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub tasks: Arc<RwLock<HashMap<String, TaskInfo>>>,
    pub queue: Arc<Mutex<Vec<String>>>,
    pub current_task: Arc<Mutex<Option<String>>>,
    pub config: config::Config,
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
        };
        (status, Json(serde_json::json!({ "error": error_message }))).into_response()
    }
}

// --- Main Application ---

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    
    let database_url = "sqlite:./isaaclab_manager.db";
    if !Sqlite::database_exists(database_url).await.unwrap_or(false) {
        Sqlite::create_database(database_url).await?;
    }
    
    let db = SqlitePool::connect(database_url).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    
    let config = config::Config::load()?;
    let state = AppState {
        db: db.clone(),
        tasks: Arc::new(RwLock::new(HashMap::new())),
        queue: Arc::new(Mutex::new(Vec::new())),
        current_task: Arc::new(Mutex::new(None)),
        config,
    };
    
    let task_manager = TaskManager::new(state.clone());
    tokio::spawn(task_manager.run());
    
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/tasks", get(list_tasks_handler).post(create_task_handler))
        .route("/api/tasks/{id}", get(get_task_handler).delete(delete_task_handler))
        .route("/api/tasks/{id}/stop", post(stop_task_handler))
        .route("/api/tasks/{id}/logs", get(get_task_logs_handler))
        .route("/api/conda/envs", get(get_conda_envs_handler))
        .route("/api/queue", get(get_queue_handler))
        .route("/api/sync", post(sync_code_handler))
        .route("/api/sync/config", get(get_sync_config_handler))
        .route("/api/sync/manifest", get(get_sync_manifest_handler))
        .nest_service("/static", ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .with_state(state);
    
    info!("Starting IsaacLab Manager on http://0.0.0.0:6006");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:6006").await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

// --- Route Handlers ---

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn list_tasks_handler(State(state): State<AppState>) -> Result<Json<Vec<Task>>, AppError> {
    let tasks = sqlx::query_as::<_, Task>("SELECT * FROM tasks ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(tasks))
}

async fn create_task_handler(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    let id = Uuid::new_v4().to_string();
    let conda_env = request.conda_env.unwrap_or_else(|| state.config.isaaclab.default_conda_env.clone());
    let task_name = extract_task_name(&request.command);
    
    let task = Task {
        id: id.clone(),
        name: task_name,
        command: request.command.clone(),
        conda_env: Some(conda_env.clone()),
        working_dir: request.working_dir.clone(),
        status: TaskStatus::Queued,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: None,
        log_path: None,
        tensorboard_port: None,
    };
    
    sqlx::query!(
        "INSERT INTO tasks (id, name, command, conda_env, working_dir, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        task.id, task.name, task.command, task.conda_env, task.working_dir, task.status, task.created_at
    )
    .execute(&state.db)
    .await?;
    
    state.queue.lock().await.push(id.clone());
    info!("Created task: {} with conda env: {} and command: {}", id, conda_env, request.command);
    Ok(Json(task))
}

async fn get_task_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Task>, AppError> {
    sqlx::query_as("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .map(Json)
        .ok_or_else(|| AppError::TaskNotFound(id))
}

async fn stop_task_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(task_info) = state.tasks.read().await.get(&id) {
        if let Some(process) = &task_info.process {
            process.lock().await.kill().await?;
        }
    }
    let now = chrono::Utc::now();
    sqlx::query!("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?", TaskStatus::Stopped, now, id)
        .execute(&state.db)
        .await?;
    Ok(Json(serde_json::json!({"message": "Task stopped"})))
}

async fn delete_task_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
    sqlx::query!("DELETE FROM tasks WHERE id = ?", id)
        .execute(&state.db)
        .await?;
    state.tasks.write().await.remove(&id);
    Ok(Json(serde_json::json!({"message": "Task deleted"})))
}

async fn get_task_logs_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<String, AppError> {
    let task = get_task_handler(State(state), Path(id)).await?.0;
    match task.log_path {
        Some(log_path) => Ok(tokio::fs::read_to_string(&log_path).await.unwrap_or_else(|_| "Log not found or empty.".to_string())),
        None => Ok("Log path not set.".to_string()),
    }
}

async fn get_queue_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.queue.lock().await.clone())
}

async fn get_conda_envs_handler(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    let conda_path = state.config.isaaclab.conda_path.to_string_lossy().to_string();
    Ok(Json(get_conda_environments(&conda_path).await?))
}


// --- Sync Handlers ---

async fn get_sync_config_handler(State(state): State<AppState>) -> Json<SyncConfigResponse> {
    Json(SyncConfigResponse {
        default_excludes: state.config.sync.default_excludes.clone(),
    })
}

async fn get_sync_manifest_handler(State(state): State<AppState>) -> Result<Json<HashMap<String, String>>, AppError> {
    info!("Received request for sync manifest.");
    let target_path = PathBuf::from(&state.config.sync.target_path);
    info!(?target_path, "Target path for sync manifest.");

    if !target_path.exists() || !target_path.is_dir() {
        error!(?target_path, "Target path does not exist or is not a directory.");
        return Ok(Json(HashMap::new()));
    }

    let excludes = state.config.sync.default_excludes.clone();
    info!(?excludes, "Using exclusion patterns.");
    let target_path_for_closure = target_path.clone();

    let manifest = tokio::task::spawn_blocking(move || {
        info!("Starting blocking task for manifest generation.");
        let exclude_patterns: Vec<glob::Pattern> = excludes
            .iter()
            .map(|s| glob::Pattern::new(s).expect("Invalid glob pattern in config"))
            .collect();

        let walker = WalkDir::new(&target_path_for_closure).into_iter();

        let filtered_walker = walker.filter_entry(|e| {
            let path = e.path();
            let relative_path = match path.strip_prefix(&target_path_for_closure) {
                Ok(p) => p,
                Err(_) => return false, // If we can't get a relative path, exclude it.
            };

            if relative_path.as_os_str().is_empty() {
                return true; // Always include the root directory itself.
            }

            let excluded = exclude_patterns.iter().any(|p| p.matches_path(relative_path));
            if excluded {
                tracing::debug!(?path, "Excluding path");
            }
            !excluded
        });

        let mut manifest: HashMap<String, String> = HashMap::new();
        let mut file_count: u64 = 0;
        info!("Starting file traversal and hashing...");
        for result in filtered_walker {
            match result {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file() {
                        file_count += 1;
                        if file_count % 1000 == 0 {
                            info!(file_count, "Hashed {} files so far...", file_count);
                        }
                        info!(?path, "Hashing file");
                        if let Ok(relative_path) = path.strip_prefix(&target_path_for_closure) {
                            match File::open(path) {
                                Ok(mut file) => {
                                    let mut hasher = Sha256::new();
                                    if let Err(e) = std::io::copy(&mut file, &mut hasher) {
                                        error!(?path, "Failed to read file for hashing: {}", e);
                                        continue;
                                    }
                                    let hash = format!("{:x}", hasher.finalize());
                                    manifest.insert(relative_path.to_string_lossy().replace('\\', "/"), hash);
                                }
                                Err(e) => {
                                    error!(?path, "Failed to open file: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Error walking directory: {}", e);
                }
            }
        }
        info!(file_count, "Finished hashing all files.");
        manifest
    }).await.map_err(|e| {
        error!("Manifest generation task failed: {}", e);
        AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
    })?;

    info!(manifest_size = manifest.len(), "Manifest generation complete. Returning manifest.");
    Ok(Json(manifest))
}

async fn sync_code_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let target_path = PathBuf::from(state.config.sync.target_path.to_string_lossy().to_string());
    if target_path.to_string_lossy().is_empty() {
        return Ok(Json(serde_json::json!({"error": "Sync target path not configured"})));
    }
    
    let mut files_written = 0;
    while let Some(field) = multipart.next_field().await? {
        if let Some(relative_path_str) = field.file_name() {
            let relative_path = PathBuf::from(relative_path_str)
                .components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .collect::<PathBuf>();

            let data = field.bytes().await?;
            let dest_path = target_path.join(&relative_path);
            if let Some(parent) = dest_path.parent() {
                tokio_fs::create_dir_all(parent).await?;
            }
            tokio_fs::write(&dest_path, &data).await?;
            files_written += 1;
        }
    }

    Ok(Json(serde_json::json!({ "message": format!("Sync complete. Wrote {} files.", files_written) })))
}


// --- Utility Functions ---

fn extract_task_name(command: &str) -> String {
    command.split_whitespace()
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
    Ok(stdout.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(String::from)
        .collect())
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
        let log_dir = std::path::Path::new("./outputs").join(task_id).join("logs");
        tokio_fs::create_dir_all(&log_dir).await?;
        let log_path = log_dir.join("task.log");
        let log_file = std::fs::File::create(&log_path)?;

        let now = chrono::Utc::now();
        let log_path_str = log_path.to_str();
        sqlx::query!(
            "UPDATE tasks SET status = ?, started_at = ?, log_path = ? WHERE id = ?",
            TaskStatus::Running, now, log_path_str, task_id
        )
        .execute(&state.db).await?;

        let task = sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
            .bind(task_id).fetch_one(&state.db).await?;

        let working_dir = task.working_dir.unwrap_or_else(|| state.config.tasks.working_directory.to_string_lossy().to_string());

        let mut cmd = Command::new("bash");
        cmd.current_dir(working_dir)
            .arg("-c")
            .arg(&task.command)
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        let mut child = cmd.spawn()?;
        let status = child.wait().await?;

        let final_status = if status.success() { TaskStatus::Completed } else { TaskStatus::Failed };
        let finished_at = chrono::Utc::now();
        sqlx::query!(
            "UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?",
            final_status, finished_at, task_id
        )
        .execute(&state.db).await?;

        Ok(())
    }
}

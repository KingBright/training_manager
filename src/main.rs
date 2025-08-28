use anyhow::Result;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use axum::extract::{multipart::MultipartError, Multipart};
use clap::Parser;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    os::unix::process::CommandExt,
    sync::Arc,
};
use tokio::{
    fs as tokio_fs, // Use an alias to avoid conflict with std::fs
    process::{Child, Command},
    sync::{Mutex, RwLock},
};
use tokio_util::io::ReaderStream;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{error, info, warn};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::write::{FileOptions, ZipWriter};

mod config;
mod metrics_parser;

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
    sqlx::migrate!("./migrations").run(&db).await?;
    
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
        .route("/api/tasks", get(list_tasks_handler).post(create_task_handler))
        .route("/api/tasks/{id}", get(get_task_handler).delete(delete_task_handler))
        .route("/api/tasks/{id}/stop", post(stop_task_handler))
        .route("/api/tasks/{id}/logs", get(get_task_logs_handler))
        .route("/api/tasks/{id}/metrics", get(get_task_metrics_handler))
        .route("/api/conda/envs", get(get_conda_envs_handler))
        .route("/api/queue", get(get_queue_handler))
        .route("/api/config", get(get_config_handler).post(update_config_handler))
        .route("/api/sync", post(sync_code_handler).layer(DefaultBodyLimit::max(10 * 1024 * 1024)))
        .route("/api/sync/config", get(get_sync_config_handler))
        .route("/api/sync/manifest", get(get_sync_manifest_handler))
        .route("/api/sync/download/{*path}", get(download_file_handler))
        .route("/api/sync/download_zip", get(download_zip_handler))
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

async fn get_config_handler(State(state): State<AppState>) -> Json<config::Config> {
    let config = state.config.read().await;
    Json(config.clone())
}

async fn update_config_handler(
    State(state): State<AppState>,
    Json(new_config): Json<config::Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    new_config.validate()?;
    let mut config_guard = state.config.write().await;
    *config_guard = new_config;
    config_guard.save_to_db(&state.db).await?;
    Ok(Json(serde_json::json!({ "message": "Configuration updated successfully" })))
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
    let config = state.config.read().await;
    let conda_env = request.conda_env.unwrap_or_else(|| config.isaaclab.default_conda_env.clone());
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
                    warn!("Failed to kill process group {}: {}. This might be because the process already stopped.", pid, e);
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


async fn delete_task_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, AppError> {
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

async fn get_task_logs_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<String, AppError> {
    let task = get_task_handler(State(state), Path(id)).await?.0;
    match task.log_path {
        Some(log_path) => {
            let content = tokio::fs::read_to_string(&log_path).await.unwrap_or_else(|_| "Log not found or empty.".to_string());
            let lines: Vec<&str> = content.lines().collect();
            let last_200_lines = lines.iter().rev().take(200).rev().map(|s| *s).collect::<Vec<&str>>().join("\n");
            Ok(last_200_lines)
        }
        None => Ok("Log path not set.".to_string()),
    }
}

async fn get_task_metrics_handler(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<metrics_parser::MetricsData>, AppError> {
    let task = get_task_handler(State(state.clone()), Path(id.clone())).await?.0;
    match task.log_path {
        Some(log_path) => {
            let content = tokio::fs::read_to_string(&log_path).await.unwrap_or_else(|_| "".to_string());
            let metrics = tokio::task::spawn_blocking(move || {
                metrics_parser::parse_log_file(&content)
            }).await.map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
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

async fn get_queue_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.queue.lock().await.clone())
}

async fn get_conda_envs_handler(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    let config = state.config.read().await;
    let conda_path = config.isaaclab.conda_path.to_string_lossy().to_string();
    Ok(Json(get_conda_environments(&conda_path).await?))
}

// --- Sync Handlers ---

/// Resolves and validates a user-provided path against a base directory.
/// Ensures the final path is a descendant of the base path and exists.
async fn resolve_safe_sync_path(base_path: &std::path::Path, remote_path_opt: Option<&String>) -> Result<PathBuf, AppError> {
    let mut target_path = base_path.to_path_buf();

    if let Some(remote_path_str) = remote_path_opt {
        if !remote_path_str.is_empty() {
            target_path.push(sanitize_path(remote_path_str));
        }
    }

    let canonical_base = base_path.canonicalize().map_err(|e| {
        error!("Base sync directory '{}' not found or invalid: {}", base_path.display(), e);
        AppError::Io(e)
    })?;

    let canonical_target = target_path.canonicalize().map_err(|e| {
        error!("Target path '{}' not found or invalid: {}", target_path.display(), e);
        AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "The specified path does not exist."))
    })?;

    if !canonical_target.starts_with(&canonical_base) {
        error!("Security violation: Attempt to access path '{}' which is outside of sync root '{}'", canonical_target.display(), canonical_base.display());
        return Err(AppError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Access denied.")));
    }

    Ok(canonical_target)
}


/// A utility function to sanitize a path string, removing any directory traversal components.
fn sanitize_path(path_str: &str) -> PathBuf {
    PathBuf::from(path_str)
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect()
}

async fn get_sync_config_handler(State(state): State<AppState>) -> Json<SyncConfigResponse> {
    let config = state.config.read().await;
    Json(SyncConfigResponse {
        default_excludes: config.sync.default_excludes.clone(),
    })
}

async fn get_sync_manifest_handler(
    State(state): State<AppState>,
    Query(params): Query<SyncRequest>,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let config = state.config.read().await;
    let base_path = PathBuf::from(&config.sync.target_path);
    let target_path = resolve_safe_sync_path(&base_path, params.remote_path.as_ref()).await?;

    let excludes = config.sync.default_excludes.clone();
    let manifest = tokio::task::spawn_blocking(move || {
        let exclude_patterns: Vec<glob::Pattern> = excludes
            .iter()
            .map(|s| glob::Pattern::new(s).expect("Invalid glob pattern in config"))
            .collect();

        let walker = WalkDir::new(&target_path).into_iter();
        let filtered_walker = walker.filter_entry(|e| {
            let path = e.path();
            let relative_path = match path.strip_prefix(&target_path) {
                Ok(p) => p,
                Err(_) => return false,
            };
            if relative_path.as_os_str().is_empty() {
                return true;
            }
            !exclude_patterns.iter().any(|p| p.matches_path(relative_path))
        });

        let mut manifest: HashMap<String, String> = HashMap::new();
        for result in filtered_walker {
            if let Ok(entry) = result {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(relative_path) = path.strip_prefix(&target_path) {
                        if let Ok(mut file) = File::open(path) {
                            let mut hasher = Sha256::new();
                            if std::io::copy(&mut file, &mut hasher).is_ok() {
                                let hash = format!("{:x}", hasher.finalize());
                                manifest.insert(relative_path.to_string_lossy().replace('\\', "/"), hash);
                            }
                        }
                    }
                }
            }
        }
        manifest
    })
    .await
    .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    Ok(Json(manifest))
}

async fn download_file_handler(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<SyncRequest>,
) -> Result<impl IntoResponse, AppError> {
    let config = state.config.read().await;
    let remote_dir_str = params.remote_path.as_deref().unwrap_or(".");
    let remote_dir_path = std::path::Path::new(remote_dir_str);

    let base_dir = if remote_dir_path.is_absolute() {
        remote_dir_path.to_path_buf()
    } else {
        let sanitized_relative = remote_dir_path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<PathBuf>();
        config
            .tasks
            .working_directory
            .join(sanitized_relative)
    };

    let file_path = base_dir.join(sanitize_path(&path));

    let canonical_path = file_path.canonicalize().map_err(|_| {
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found",
        ))
    })?;

    if !remote_dir_path.is_absolute() {
        let canonical_base =
            config.tasks.working_directory.canonicalize().map_err(AppError::Io)?;
        if !canonical_path.starts_with(&canonical_base) {
            error!(
                "Potential directory traversal attempt blocked: {:?}",
                canonical_path
            );
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Access denied.",
            )));
        }
    }

    let file_path = canonical_path;

    if !file_path.is_file() {
        return Err(AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "Path is not a file")));
    }

    let file = tokio_fs::File::open(&file_path).await.map_err(AppError::Io)?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file_name),
        ),
    ];
    Ok((headers, body).into_response())
}

async fn download_zip_handler(
    State(state): State<AppState>,
    Query(params): Query<SyncRequest>,
) -> Result<impl IntoResponse, AppError> {
    let config = state.config.read().await;
    let remote_path_str = params.remote_path.as_deref().unwrap_or(".");
    let remote_path = std::path::Path::new(remote_path_str);

    let target_path = if remote_path.is_absolute() {
        remote_path.to_path_buf()
    } else {
        let sanitized_relative = remote_path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<PathBuf>();
        config
            .tasks
            .working_directory
            .join(sanitized_relative)
    };

    let canonical_target = target_path.canonicalize().map_err(|e| {
        error!(
            "Target path '{}' not found or invalid: {}",
            target_path.display(),
            e
        );
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "The specified path does not exist.",
        ))
    })?;

    if !remote_path.is_absolute() {
        let canonical_base =
            config.tasks.working_directory.canonicalize().map_err(|e| {
                error!(
                    "Working directory '{}' not found or invalid: {}",
                    config.tasks.working_directory.display(),
                    e
                );
                AppError::Io(e)
            })?;
        if !canonical_target.starts_with(&canonical_base) {
            error!(
                "Security violation: Attempt to access path '{}' which is outside of working directory '{}'",
                canonical_target.display(),
                canonical_base.display()
            );
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Access denied.",
            )));
        }
    }

    let target_path = canonical_target;

    let zip_buffer = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, std::io::Error> {
        // Find all .pt files and their modification times
        let mut pt_files = Vec::new();
        for entry in WalkDir::new(&target_path).into_iter().filter_map(|e| e.ok()) {
            if entry.path().extension().map_or(false, |ext| ext == "pt") {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        pt_files.push((entry.path().to_path_buf(), modified));
                    }
                }
            }
        }

        // Determine the newest .pt file
        let newest_pt_path = if !pt_files.is_empty() {
            pt_files.sort_by(|a, b| b.1.cmp(&a.1)); // Sort descending by time
            Some(pt_files[0].0.clone())
        } else {
            None
        };

        let mut buffer = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buffer);
            let mut zip = ZipWriter::new(cursor);
            let options = FileOptions::<'_, ()>::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o755);

            let walker = WalkDir::new(&target_path).into_iter();
            for entry in walker.filter_map(|e| e.ok()) {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.starts_with("events.out") {
                        continue;
                    }
                }

                // If it's a .pt file, only include the newest one
                if path.extension().map_or(false, |ext| ext == "pt") {
                    if let Some(newest) = &newest_pt_path {
                        if path != newest.as_path() {
                            continue; // Skip this file
                        }
                    }
                }

                let name = path.strip_prefix(&target_path).unwrap();
                if path.is_file() {
                    zip.start_file(name.to_string_lossy(), options)?;
                    let mut f = std::fs::File::open(path)?;
                    let mut file_buffer = Vec::new();
                    f.read_to_end(&mut file_buffer)?;
                    zip.write_all(&file_buffer)?;
                } else if !name.as_os_str().is_empty() {
                    zip.add_directory(name.to_string_lossy(), options)?;
                }
            }
            zip.finish()?;
        }
        Ok(buffer)
    })
    .await
    .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    let file_name = if let Some(remote_path) = &params.remote_path {
        let sanitized = sanitize_path(remote_path);
        let name = sanitized.file_name().and_then(|s| s.to_str()).unwrap_or("archive");
        format!("{}.zip", name)
    } else {
        "archive.zip".to_string()
    };

    let headers = [
        (header::CONTENT_TYPE, "application/zip".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file_name),
        ),
    ];

    let zip_data = zip_buffer?;
    Ok((headers, zip_data).into_response())
}


async fn sync_code_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = state.config.read().await;
    let target_path_str = config.sync.target_path.to_string_lossy();

    if target_path_str.is_empty() {
        return Ok(Json(serde_json::json!({"error": "Sync target path not configured"})));
    }

    let target_path = PathBuf::from(target_path_str.to_string());

    // Ensure the base directory exists
    tokio_fs::create_dir_all(&target_path).await?;
    
    // Canonicalize the path to resolve any relative parts and get a stable, absolute path.
    // This is the core of the fix.
    let canonical_target = target_path.canonicalize().map_err(|e| {
        error!("Sync target path '{}' not found or invalid: {}", target_path.display(), e);
        AppError::Io(e)
    })?;

    let mut files_written = 0;
    while let Some(field) = multipart.next_field().await? {
        if let Some(relative_path_str) = field.file_name() {
            let relative_path = sanitize_path(relative_path_str);

            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let dest_path = canonical_target.join(&relative_path);

            // Security check to ensure the final path is within the canonical target directory.
            if !dest_path.starts_with(&canonical_target) {
                error!("Security violation: file path '{}' escaped target directory '{}'", dest_path.display(), canonical_target.display());
                continue;
            }

            if let Some(parent) = dest_path.parent() {
                tokio_fs::create_dir_all(parent).await?;
            }
            let data = field.bytes().await?;
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

        // Set the process group ID to ensure the process and its children can be killed together.
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
            // If we can't update the DB, we shouldn't proceed.
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

            // The task might have been stopped manually. If so, it will be removed from the map.
            // If we can remove it, it means it finished naturally.
            if let Some(_removed_task) = wait_state.tasks.write().await.remove(&wait_task_id) {
                let final_status = if status.success() { TaskStatus::Completed } else { TaskStatus::Failed };
                let finished_at = chrono::Utc::now();

                if let Err(e) = sqlx::query("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?")
                    .bind(final_status)
                    .bind(finished_at)
                    .bind(&wait_task_id)
                    .execute(&state.db).await {
                    error!("Failed to update task {} status after completion: {}", wait_task_id, e);
                }
                info!("Task {} finished with status: {:?}", wait_task_id, final_status);
            } else {
                // If the task was not in the map, it means it was stopped via the API.
                // The stop_task_handler is responsible for updating the DB in this case.
                info!("Task {} was stopped manually, skipping final status update.", wait_task_id);
            }
        });

        Ok(())
    }
}

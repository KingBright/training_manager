use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{
    fs,
    process::{Child, Command},
    sync::{Mutex, RwLock},
};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{info, error};
use uuid::Uuid;
use axum_extra::extract::{multipart::MultipartError, Multipart};
use glob::Pattern;

mod config; // Added

// 数据结构定义
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    pub conda_env: Option<String>,  // 新增conda环境字段
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
    pub command: String,              // 完整的命令
    pub conda_env: Option<String>,    // conda环境
    pub working_dir: Option<String>,  // 工作目录
}

// 应用状态
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub tasks: Arc<RwLock<HashMap<String, TaskInfo>>>,
    pub queue: Arc<Mutex<Vec<String>>>,
    pub current_task: Arc<Mutex<Option<String>>>,
    pub config: config::Config, // Changed from AppConfig
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub task: Task,
    pub process: Option<Arc<Mutex<Child>>>,
}

// Removed AppConfig struct and its impl Default

// 错误类型
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
        
        (status, Json(serde_json::json!({"error": error_message}))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();
    
    // 创建数据库
    let database_url = "sqlite:./isaaclab_manager.db";
    if !Sqlite::database_exists(database_url).await.unwrap_or(false) {
        Sqlite::create_database(database_url).await?;
    }
    
    let db = SqlitePool::connect(database_url).await?;
    
    // 运行数据库迁移
    sqlx::migrate!("./migrations").run(&db).await?;
    
    // 创建应用状态
    let config = config::Config::load()?; // New: Load config from file or env
    let state = AppState {
        db: db.clone(),
        tasks: Arc::new(RwLock::new(HashMap::new())) ,
        queue: Arc::new(Mutex::new(Vec::new())) ,
        current_task: Arc::new(Mutex::new(None)) ,
        config: config, // Use the loaded config
    };
    
    // 启动任务管理器
    let task_manager = TaskManager::new(state.clone());
    tokio::spawn(task_manager.run());
    
    // 创建路由
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
        // .route("/api/tensorboard/{id}", get(get_tensorboard_handler))
        // .route("/api/download/{id}/onnx", get(download_onnx_handler))
        .nest_service("/static", ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .with_state(state);
    
    // 启动服务器
    info!("Starting IsaacLab Manager on http://0.0.0.0:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

// 处理器函数
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
    
    // 确定使用的conda环境
    let conda_env = request.conda_env.unwrap_or_else(|| state.config.isaaclab.default_conda_env.clone()); // Updated
    
    // 从命令中提取任务名称(简单解析)
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
    
    // 保存到数据库
    sqlx::query!(
        "INSERT INTO tasks (id, name, command, conda_env, working_dir, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        task.id,
        task.name,
        task.command,
        task.conda_env,
        task.working_dir,
        task.status,
        task.created_at
    )
    .execute(&state.db)
    .await?;
    
    // 添加到队列
    {
        let mut queue = state.queue.lock().await;
        queue.push(id.clone());
    }
    
    info!("Created task: {} with conda env: {} and command: {}", id, conda_env, request.command);
    
    Ok(Json(task))
}

// 辅助函数：从命令中提取任务名称
fn extract_task_name(command: &str) -> String {
    // 尝试从命令中提取有意义的名称
    if let Some(task_part) = command.split_whitespace()
        .find(|part| part.starts_with("--task=")) {
        if let Some(task_name) = task_part.strip_prefix("--task=") {
            return task_name.to_string();
        }
    }
    
    // 如果没找到--task参数，使用python文件名
    if let Some(py_file) = command.split_whitespace()
        .find(|part| part.ends_with(".py")) {
        if let Some(file_name) = py_file.split('/').last() {
            return file_name.trim_end_matches(".py").to_string();
        }
    }
    
    // 默认名称
    "训练任务".to_string()
}

async fn get_conda_envs_handler(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    let conda_envs = get_conda_environments(&state.config.isaaclab.conda_path.to_string_lossy()).await?; // Updated
    Ok(Json(conda_envs))
}

async fn get_conda_environments(conda_path: &str) -> Result<Vec<String>, AppError> {
    let conda_executable = std::path::Path::new(conda_path).join("bin/conda");
    info!("Using conda executable: {:?}", conda_executable);

    if !conda_executable.is_file() {
        error!("Conda executable not found at: {:?}", conda_executable);
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Conda executable not found at: {:?}", conda_executable),
        )));
    }

    let output = Command::new(conda_executable)
        .args(&["env", "list"])
        .output()
        .await?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        error!("Failed to list conda environments. stderr: {}", stderr);
        return Ok(vec!["base".to_string()]); // 默认返回base环境
    }
    
    info!("conda env list stdout: {}", stdout);

    let mut envs = Vec::new();
    
    for line in stdout.lines() {
        if !line.starts_with('#') && !line.trim().is_empty() {
            if let Some(env_name) = line.split_whitespace().next() {
                if env_name != "*" { // 跳过当前环境标记
                    envs.push(env_name.to_string());
                }
            }
        }
    }
    
    if envs.is_empty() {
        envs.push("base".to_string());
    }
    
    Ok(envs)
}

async fn get_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Task>, AppError> {
    let task = sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await? 
        .ok_or_else(|| AppError::TaskNotFound(id))?;
    
    Ok(Json(task))
}

async fn stop_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 实现任务停止逻辑
    let tasks = state.tasks.read().await;
    if let Some(task_info) = tasks.get(&id) {
        if let Some(process) = &task_info.process {
            let mut process = process.lock().await;
            let _ = process.kill().await;
        }
    }
    
    // 更新数据库状态
    let now = chrono::Utc::now();
    sqlx::query!("UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?", 
        TaskStatus::Stopped, now, id)
        .execute(&state.db)
        .await?;
    
    Ok(Json(serde_json::json!({"message": "Task stopped"})))
}

async fn delete_task_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Delete the task from the database
    sqlx::query!("DELETE FROM tasks WHERE id = ?", id)
        .execute(&state.db)
        .await?;

    // Remove the task from the in-memory map
    state.tasks.write().await.remove(&id);

    Ok(Json(serde_json::json!({"message": "Task deleted"})))
}

async fn get_task_logs_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, AppError> {
    let task = sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await? 
        .ok_or_else(|| AppError::TaskNotFound(id.clone()))?;

    if let Some(log_path) = task.log_path {
        match tokio::fs::read_to_string(&log_path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let start_index = if lines.len() > 200 { lines.len() - 200 } else { 0 };
                Ok(lines[start_index..].join("\n"))
            },
            Err(_) => Ok("Log file not found or empty".to_string()),
        }
    } else {
        Ok("Log path not set for this task".to_string())
    }
}

async fn get_queue_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    let queue = state.queue.lock().await;
    Json(queue.clone())
}

#[derive(Serialize)]
pub struct SyncConfigResponse {
    pub default_excludes: Vec<String>,
}

async fn get_sync_config_handler(
    State(state): State<AppState>,
) -> Result<Json<SyncConfigResponse>, AppError> {
    let response = SyncConfigResponse {
        default_excludes: state.config.sync.default_excludes.clone(),
    };
    Ok(Json(response))
}

async fn sync_code_handler(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    // 1. Get target path from config
    let target_path = PathBuf::from(state.config.sync.target_path.to_string_lossy().to_string());
    if target_path.to_string_lossy().is_empty() {
        error!("Sync target path is not configured.");
        return Ok(Json(serde_json::json!({"error": "代码同步目标路径未配置"})));
    }
    
    // 2. Compile exclusion patterns
    let exclude_patterns: Vec<Pattern> = state.config.sync.default_excludes
        .iter()
        .map(|p| Pattern::new(p).expect("Invalid glob pattern in config"))
        .collect();

    let mut files_written = 0;
    let mut files_excluded = 0;

    // 3. Process multipart stream
    while let Some(field) = multipart.next_field().await? {
        if let Some(relative_path_str) = field.name().map(|s| s.to_string()) {
            if relative_path_str.is_empty() {
                continue;
            }

            // Security: clean the path to prevent directory traversal
            let relative_path = PathBuf::from(relative_path_str)
                .components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .collect::<PathBuf>();

            let data = field.bytes().await?;

            // 4. Check against exclusion patterns
            let is_excluded = exclude_patterns.iter().any(|p| p.matches_path(&relative_path));

            if is_excluded {
                files_excluded += 1;
                info!("Excluding file: {}", relative_path.display());
                continue;
            }

            // 5. Write file to disk
            let dest_path = target_path.join(&relative_path);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&dest_path, &data).await?;
            files_written += 1;
            info!("Synced file: {}", dest_path.display());
        }
    }

    let message = format!(
        "同步完成。\n成功写入 {} 个文件。\n排除了 {} 个文件。",
        files_written, files_excluded
    );

    Ok(Json(serde_json::json!({ "message": message })))
}

// TaskManager
struct TaskManager {
    state: AppState,
}

impl TaskManager {
    fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn run(self) {
        loop {
            let task_id = {
                let mut queue = self.state.queue.lock().await;
                queue.pop()
            };

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

    async fn execute_task(state: AppState, task_id: &str) -> Result<()> {
        // Create log file
        let log_dir = std::path::Path::new("./outputs").join(task_id).join("logs");
        tokio::fs::create_dir_all(&log_dir).await?;
        let log_path = log_dir.join("task.log");
        let log_file = tokio::fs::File::create(&log_path).await?;

        // Update task status and log_path
        let now = chrono::Utc::now();
        let log_path_str = log_path.to_str().unwrap_or_default();
        sqlx::query!(
            "UPDATE tasks SET status = ?, started_at = ?, log_path = ? WHERE id = ?",
            TaskStatus::Running, 
            now,
            log_path_str,
            task_id
        )
        .execute(&state.db)
        .await?;

        // Get task details
        let task = sqlx::query_as::<_, Task>("SELECT * FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&state.db)
            .await?;

        // Set working directory
        let working_dir = task.working_dir.clone().unwrap_or_else(|| state.config.tasks.working_directory.to_string_lossy().to_string());

        // Execute the command
        let mut cmd = Command::new("bash");
        cmd.current_dir(working_dir);
        cmd.arg("-c");
        cmd.arg(&task.command);

        let std_log_file = log_file.into_std().await;
        cmd.stdout(std_log_file.try_clone()?);
        cmd.stderr(std_log_file);

        let mut child = cmd.spawn()?;

        // Wait for the command to finish
        let output = child.wait_with_output().await?;

        // Update task status based on exit code
        let status = if output.status.success() {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed
        };

        let finished_at = chrono::Utc::now();
        sqlx::query!(
            "UPDATE tasks SET status = ?, finished_at = ? WHERE id = ?",
            status,
            finished_at,
            task_id
        )
        .execute(&state.db)
        .await?;

        Ok(())
    }
}

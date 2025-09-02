use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{collections::HashMap, sync::Arc};
use tokio::{process::Child, sync::{Mutex, RwLock}};

use crate::config;

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

#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ListFilesRequest {
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteFileRequest {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ListFilesResponse {
    pub parent: Option<String>,
    pub files: Vec<FileInfo>,
}

// --- Application State ---

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

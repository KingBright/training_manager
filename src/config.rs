// src/config.rs
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub isaaclab: IsaacLabConfig,
    pub storage: StorageConfig,
    pub tensorboard: TensorBoardConfig,
    pub sync: SyncConfig,
    pub tasks: TaskConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsaacLabConfig {
    pub path: PathBuf,
    pub python_executable: String,
    pub conda_path: PathBuf,
    pub default_conda_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub output_path: PathBuf,
    pub log_path: PathBuf,
    pub database_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TensorBoardConfig {
    pub base_port: u16,
    pub max_instances: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub target_path: PathBuf,
    pub default_excludes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    pub max_concurrent: u32,
    pub default_headless: bool,
    pub timeout_seconds: u64,
    pub working_directory: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 6006,
            },
            isaaclab: IsaacLabConfig {
                path: PathBuf::from("/home/ecs-user"),
                python_executable: "python".to_string(),
                conda_path: PathBuf::from("/home/ecs-user/anaconda3"),
                default_conda_env: "isaaclab".to_string(),
            },
            storage: StorageConfig {
                output_path: PathBuf::from("/home/ecs-user/outputs"),
                log_path: PathBuf::from("/home/ecs-user/logs"),
                database_url: "sqlite:./isaaclab_manager.db".to_string(),
            },
            tensorboard: TensorBoardConfig {
                base_port: 6007,
                max_instances: 10,
            },
            sync: SyncConfig {
                target_path: PathBuf::from("/home/ecs-user/moves"),
                default_excludes: vec![
                    "__pycache__".to_string(),
                    "*.pyc".to_string(),
                    ".git".to_string(),
                    "logs/".to_string(),
                    "outputs/".to_string(),
                    ".vscode/".to_string(),
                    "*.tmp".to_string(),
                    ".DS_Store".to_string(),
                    "target".to_string(),
                ],
            },
            tasks: TaskConfig {
                max_concurrent: 1,
                default_headless: true,
                timeout_seconds: 86400,
                working_directory: PathBuf::from("/home/ecs-user"),
            },
        }
    }
}

impl Config {
    pub async fn load(db: &SqlitePool) -> Result<Self> {
        let rows = sqlx::query("SELECT key, value FROM config")
            .fetch_all(db)
            .await?;

        if rows.is_empty() {
            tracing::info!("No configuration found in database. Loading defaults and saving.");
            let config = Self::default();
            config.save_to_db(db).await?;
            return Ok(config);
        }

        let mut db_config = rows
            .into_iter()
            .map(|row| (row.get::<String, _>("key"), row.get::<String, _>("value")))
            .collect::<HashMap<String, String>>();

        let default_config = Self::default();

        let config = Self {
            server: ServerConfig {
                host: db_config
                    .remove("server_host")
                    .unwrap_or(default_config.server.host),
                port: db_config
                    .remove("server_port")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.server.port),
            },
            isaaclab: IsaacLabConfig {
                path: db_config
                    .remove("isaaclab_path")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.isaaclab.path),
                python_executable: db_config
                    .remove("isaaclab_python_executable")
                    .unwrap_or(default_config.isaaclab.python_executable),
                conda_path: db_config
                    .remove("isaaclab_conda_path")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.isaaclab.conda_path),
                default_conda_env: db_config
                    .remove("isaaclab_default_conda_env")
                    .unwrap_or(default_config.isaaclab.default_conda_env),
            },
            storage: StorageConfig {
                output_path: db_config
                    .remove("storage_output_path")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.storage.output_path),
                log_path: db_config
                    .remove("storage_log_path")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.storage.log_path),
                database_url: default_config.storage.database_url,
            },
            tensorboard: TensorBoardConfig {
                base_port: db_config
                    .remove("tensorboard_base_port")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.tensorboard.base_port),
                max_instances: db_config
                    .remove("tensorboard_max_instances")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.tensorboard.max_instances),
            },
            sync: SyncConfig {
                target_path: db_config
                    .remove("sync_target_path")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.sync.target_path),
                default_excludes: db_config
                    .remove("sync_default_excludes")
                    .and_then(|v| serde_json::from_str(&v).ok())
                    .unwrap_or(default_config.sync.default_excludes),
            },
            tasks: TaskConfig {
                max_concurrent: db_config
                    .remove("tasks_max_concurrent")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.tasks.max_concurrent),
                default_headless: db_config
                    .remove("tasks_default_headless")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.tasks.default_headless),
                timeout_seconds: db_config
                    .remove("tasks_timeout_seconds")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default_config.tasks.timeout_seconds),
                working_directory: db_config
                    .remove("tasks_working_directory")
                    .map(PathBuf::from)
                    .unwrap_or(default_config.tasks.working_directory),
            },
        };
        
        if !db_config.is_empty() {
            tracing::warn!("Unused configuration keys found in database: {:?}", db_config.keys());
        }

        Ok(config)
    }

    pub async fn save_to_db(&self, db: &SqlitePool) -> Result<()> {
        let mut tx = db.begin().await?;

        let query_str = "INSERT INTO config (key, value, updated_at) VALUES (?, ?, datetime('now')) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at";

        sqlx::query(query_str).bind("server_host").bind(&self.server.host).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("server_port").bind(self.server.port.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("isaaclab_path").bind(self.isaaclab.path.to_string_lossy()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("isaaclab_python_executable").bind(&self.isaaclab.python_executable).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("isaaclab_conda_path").bind(self.isaaclab.conda_path.to_string_lossy()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("isaaclab_default_conda_env").bind(&self.isaaclab.default_conda_env).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("storage_output_path").bind(self.storage.output_path.to_string_lossy()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("storage_log_path").bind(self.storage.log_path.to_string_lossy()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tensorboard_base_port").bind(self.tensorboard.base_port.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tensorboard_max_instances").bind(self.tensorboard.max_instances.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("sync_target_path").bind(self.sync.target_path.to_string_lossy()).execute(&mut *tx).await?;
        let excludes_json = serde_json::to_string(&self.sync.default_excludes)?;
        sqlx::query(query_str).bind("sync_default_excludes").bind(&excludes_json).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tasks_max_concurrent").bind(self.tasks.max_concurrent.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tasks_default_headless").bind(self.tasks.default_headless.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tasks_timeout_seconds").bind(self.tasks.timeout_seconds.to_string()).execute(&mut *tx).await?;
        sqlx::query(query_str).bind("tasks_working_directory").bind(self.tasks.working_directory.to_string_lossy()).execute(&mut *tx).await?;

        tx.commit().await?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if !self.isaaclab.path.exists() {
            anyhow::bail!("IsaacLab path does not exist: {:?}", self.isaaclab.path);
        }
        if !self.isaaclab.conda_path.exists() {
            anyhow::bail!("Conda path does not exist: {:?}", self.isaaclab.conda_path);
        }
        let conda_script = self.isaaclab.conda_path.join("etc/profile.d/conda.sh");
        if !conda_script.exists() {
            anyhow::bail!("Conda script not found: {:?}", conda_script);
        }
        std::fs::create_dir_all(&self.storage.output_path)?;
        std::fs::create_dir_all(&self.storage.log_path)?;

        let db_path_str = self.storage.database_url.strip_prefix("sqlite:").unwrap_or(&self.storage.database_url);
        if let Some(parent) = PathBuf::from(db_path_str).parent() {
            if !parent.as_os_str().is_empty() {
                 std::fs::create_dir_all(parent)?;
            }
        }
        Ok(())
    }
}

// src/metrics.rs
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Metrics {
    pub tasks_created: Arc<AtomicU64>,
    pub tasks_completed: Arc<AtomicU64>,
    pub tasks_failed: Arc<AtomicU64>,
    pub sync_operations: Arc<AtomicU64>,
    pub uptime_start: std::time::Instant,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            tasks_created: Arc::new(AtomicU64::new(0)),
            tasks_completed: Arc::new(AtomicU64::new(0)),
            tasks_failed: Arc::new(AtomicU64::new(0)),
            sync_operations: Arc::new(AtomicU64::new(0)),
            uptime_start: std::time::Instant::now(),
        }
    }
}

#[derive(Serialize)]
pub struct MetricsSnapshot {
    pub tasks_created: u64,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub sync_operations: u64,
    pub uptime_seconds: u64,
}

impl Metrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            tasks_created: self.tasks_created.load(Ordering::Relaxed),
            tasks_completed: self.tasks_completed.load(Ordering::Relaxed),
            tasks_failed: self.tasks_failed.load(Ordering::Relaxed),
            sync_operations: self.sync_operations.load(Ordering::Relaxed),
            uptime_seconds: self.uptime_start.elapsed().as_secs(),
        }
    }

    pub fn increment_tasks_created(&self) {
        self.tasks_created.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_tasks_completed(&self) {
        self.tasks_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_tasks_failed(&self) {
        self.tasks_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_sync_operations(&self) {
        self.sync_operations.fetch_add(1, Ordering::Relaxed);
    }
}

// src/notifications.rs
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationType {
    TaskCreated,
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    TaskStopped,
    SyncCompleted,
    SyncFailed,
    SystemError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub notification_type: NotificationType,
    pub title: String,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub task_id: Option<String>,
}

impl Notification {
    pub fn new(
        notification_type: NotificationType,
        title: String,
        message: String,
        task_id: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            notification_type,
            title,
            message,
            timestamp: chrono::Utc::now(),
            task_id,
        }
    }

    pub fn task_created(task_name: &str, task_id: &str) -> Self {
        Self::new(
            NotificationType::TaskCreated,
            "任务已创建".to_string(),
            format!("任务 '{}' 已添加到队列", task_name),
            Some(task_id.to_string()),
        )
    }

    pub fn task_started(task_name: &str, task_id: &str) -> Self {
        Self::new(
            NotificationType::TaskStarted,
            "任务已开始".to_string(),
            format!("任务 '{}' 开始执行", task_name),
            Some(task_id.to_string()),
        )
    }

    pub fn task_completed(task_name: &str, task_id: &str) -> Self {
        Self::new(
            NotificationType::TaskCompleted,
            "任务已完成".to_string(),
            format!("任务 '{}' 执行完成", task_name),
            Some(task_id.to_string()),
        )
    }

    pub fn task_failed(task_name: &str, task_id: &str, error: &str) -> Self {
        Self::new(
            NotificationType::TaskFailed,
            "任务执行失败".to_string(),
            format!("任务 '{}' 执行失败: {}", task_name, error),
            Some(task_id.to_string()),
        )
    }

    pub fn sync_completed() -> Self {
        Self::new(
            NotificationType::SyncCompleted,
            "代码同步完成".to_string(),
            "代码同步操作已成功完成".to_string(),
            None,
        )
    }

    pub fn sync_failed(error: &str) -> Self {
        Self::new(
            NotificationType::SyncFailed,
            "代码同步失败".to_string(),
            format!("代码同步失败: {}", error),
            None,
        )
    }
}

#[derive(Clone)]
pub struct NotificationService {
    sender: broadcast::Sender<Notification>,
}

impl NotificationService {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1000);
        Self { sender }
    }

    pub fn send(&self, notification: Notification) {
        let _ = self.sender.send(notification);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.sender.subscribe()
    }
}

// src/utils.rs
use tokio::process::Command;

pub async fn check_system_requirements() -> Result<()> {
    // 检查Python
    let python_output = Command::new("python")
        .args(&["--version"])
        .output()
        .await?;
    
    if !python_output.status.success() {
        anyhow::bail!("Python not found or not working");
    }

    // 检查rsync
    let rsync_output = Command::new("rsync")
        .args(&["--version"])
        .output()
        .await?;
    
    if !rsync_output.status.success() {
        anyhow::bail!("rsync not found or not working");
    }

    // 检查TensorBoard
    let tb_output = Command::new("tensorboard")
        .args(&["--version"])
        .output()
        .await?;
    
    if !tb_output.status.success() {
        tracing::warn!("TensorBoard not found, some features may not work");
    }

    Ok(())
}

pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

pub async fn get_system_info() -> serde_json::Value {
    let mut sys = sysinfo::System::new_all();
    sys.refresh_all();

    let disks = sysinfo::Disks::new_with_refreshed_list();

    serde_json::json!({
        "hostname": sysinfo::System::host_name().unwrap_or_default(),
        "os": format!("{} {}", sysinfo::System::name().unwrap_or_default(), sysinfo::System::os_version().unwrap_or_default()),
        "cpu_count": sys.cpus().len(),
        "cpu_usage": sys.global_cpu_usage(),
        "memory_total": sys.total_memory(),
        "memory_used": sys.used_memory(),
        "disk_info": disks.iter().map(|disk| {
            serde_json::json!({
                "mount_point": disk.mount_point().to_str(),
                "total_space": disk.total_space(),
                "available_space": disk.available_space(),
            })
        }).collect::<Vec<_>>()
    })
}

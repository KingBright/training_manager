// src/config.rs
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub isaaclab: IsaacLabConfig,
    pub storage: StorageConfig,
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
                port: 3000,
            },
            isaaclab: IsaacLabConfig {
                path: PathBuf::from("/opt/isaaclab"),
                python_executable: "python".to_string(),
                conda_path: PathBuf::from("/opt/miniconda3"),
                default_conda_env: "isaaclab".to_string(),
            },
            storage: StorageConfig {
                output_path: PathBuf::from("./outputs"),
                log_path: PathBuf::from("./logs"),
                database_url: "sqlite:./data/isaaclab_manager.db".to_string(),
            },
            sync: SyncConfig {
                target_path: PathBuf::from("/opt/isaaclab/source"),
                default_excludes: vec![
                    "__pycache__".to_string(),
                    "*.pyc".to_string(),
                    ".git".to_string(),
                    "logs/".to_string(),
                    "outputs/".to_string(),
                    ".vscode/".to_string(),
                    "*.tmp".to_string(),
                ],
            },
            tasks: TaskConfig {
                max_concurrent: 1,
                default_headless: true,
                timeout_seconds: 86400, // 24 hours
                working_directory: std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("/tmp")),
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        // 尝试从配置文件加载
        if let Ok(config_str) = std::fs::read_to_string("config/.app.toml") {
            match toml::from_str(&config_str) {
                Ok(config) => return Ok(config),
                Err(e) => tracing::warn!("Failed to parse config file: {}", e),
            }
        }

        // 尝试从环境变量加载
        let mut config = Self::default();
        
        if let Ok(host) = std::env::var("SERVER_HOST") {
            config.server.host = host;
        }
        
        if let Ok(port) = std::env::var("SERVER_PORT") {
            if let Ok(port) = port.parse() {
                config.server.port = port;
            }
        }
        
        if let Ok(isaaclab_path) = std::env::var("ISAACLAB_PATH") {
            config.isaaclab.path = PathBuf::from(isaaclab_path);
        }
        
        if let Ok(conda_path) = std::env::var("CONDA_PATH") {
            config.isaaclab.conda_path = PathBuf::from(conda_path);
        }
        
        if let Ok(conda_env) = std::env::var("CONDA_ENV") {
            config.isaaclab.default_conda_env = conda_env;
        }
        
        if let Ok(database_url) = std::env::var("DATABASE_URL") {
            config.storage.database_url = database_url;
        }

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all("config")?;
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write("config/app.toml", toml_str)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        // 验证IsaacLab路径
        if !self.isaaclab.path.exists() {
            anyhow::bail!("IsaacLab path does not exist: {:?}", self.isaaclab.path);
        }

        // 验证Conda路径
        if !self.isaaclab.conda_path.exists() {
            anyhow::bail!("Conda path does not exist: {:?}", self.isaaclab.conda_path);
        }

        // 验证conda.sh文件存在
        let conda_script = self.isaaclab.conda_path.join("etc/profile.d/conda.sh");
        if !conda_script.exists() {
            anyhow::bail!("Conda script not found: {:?}", conda_script);
        }

        // 创建必要的目录
        std::fs::create_dir_all(&self.storage.output_path)?;
        std::fs::create_dir_all(&self.storage.log_path)?;
        std::fs::create_dir_all("data")?;

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

use axum::{routing::get, Json, Router};
use serde::Serialize;
use sysinfo::System;
use tokio::process::Command;
use tracing::{error, warn};
use crate::AppState;

#[derive(Serialize, Debug, Clone)]
pub struct SystemInfo {
    cpu: CpuInfo,
    gpus: Vec<GpuInfo>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CpuInfo {
    /// Overall CPU usage percentage
    total_usage: f32,
    /// Total system memory in MB
    total_memory_mb: u64,
    /// Used system memory in MB
    used_memory_mb: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct GpuInfo {
    name: String,
    total_memory_mb: u32,
    used_memory_mb: u32,
    gpu_utilization: u32,
    performance_state: String,
    process_count: u32,
}

pub fn create_router() -> Router<AppState> {
    Router::new().route("/api/system/resources", get(get_system_resources_handler))
}

async fn get_system_resources_handler() -> Json<SystemInfo> {
    let cpu_info = get_cpu_info();
    let gpu_info = get_gpu_info().await;
    Json(SystemInfo {
        cpu: cpu_info,
        gpus: gpu_info,
    })
}

fn get_cpu_info() -> CpuInfo {
    let mut sys = System::new_all();
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    // The first CPU in the list is the aggregate of all others.
    let total_usage = sys.cpus().first().map_or(0.0, |cpu| cpu.cpu_usage());
    let total_memory_mb = sys.total_memory() / 1024 / 1024;
    let used_memory_mb = sys.used_memory() / 1024 / 1024;

    CpuInfo {
        total_usage,
        total_memory_mb,
        used_memory_mb,
    }
}

async fn get_gpu_info() -> Vec<GpuInfo> {
    let output = Command::new("nvidia-smi")
        .arg("--query-gpu=name,memory.total,memory.used,utilization.gpu,pstate,processes.count")
        .arg("--format=csv,noheader,nounits")
        .output()
        .await;

    match output {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_nvidia_smi_csv(&stdout)
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!("nvidia-smi command failed with status {}: {}", output.status, stderr);
                Vec::new()
            }
        }
        Err(e) => {
            warn!("Failed to execute nvidia-smi command: {}. This might be because NVIDIA drivers are not installed or the command is not in the system's PATH.", e);
            Vec::new()
        }
    }
}

fn parse_nvidia_smi_csv(csv_data: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in csv_data.trim().lines() {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() == 6 {
            let gpu_info = GpuInfo {
                name: parts[0].to_string(),
                total_memory_mb: parts[1].parse().unwrap_or(0),
                used_memory_mb: parts[2].parse().unwrap_or(0),
                gpu_utilization: parts[3].parse().unwrap_or(0),
                performance_state: parts[4].to_string(),
                process_count: parts[5].parse().unwrap_or(0),
            };
            gpus.push(gpu_info);
        } else {
            error!("Failed to parse nvidia-smi output line, expected 6 parts but got {}: '{}'", parts.len(), line);
        }
    }
    gpus
}

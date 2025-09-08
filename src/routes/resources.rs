use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;
use tokio::time::{sleep, Duration};

use crate::{error::AppError, models::AppState};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CpuInfo {
    pub brand: String,
    pub frequency: u64,
    pub usage: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: String,
    pub memory_total: u64,
    pub memory_used: u64,
    pub utilization: u32,
    pub temperature: u32,
    pub power_draw: u32,
    pub power_limit: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CpuProcessInfo {
    pub pid: u32,
    pub command: String,
    pub usage: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GpuProcessInfo {
    pub pid: u32,
    pub command: String,
    pub memory_used: u64, // in bytes
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SystemResourceInfo {
    pub cpus: Vec<CpuInfo>,
    pub memory: MemoryInfo,
    pub gpus: Vec<GpuInfo>,
    pub top_cpu_processes: Vec<CpuProcessInfo>,
    pub top_gpu_processes: Vec<GpuProcessInfo>,
}

pub async fn get_resources_handler(
    State(_state): State<AppState>,
) -> Result<Json<SystemResourceInfo>, AppError> {
    let (cpus, memory, (top_cpu_processes, process_map)) =
        tokio::try_join!(get_cpu_info(), get_memory_info(), get_all_process_info())?;

    let gpus = get_gpu_info().await.unwrap_or_else(|e| {
        tracing::warn!("Could not retrieve GPU info: {}", e);
        Vec::new()
    });

    let top_gpu_processes = get_top_gpu_processes(process_map).await.unwrap_or_else(|e| {
        tracing::warn!("Could not retrieve GPU process info: {}", e);
        Vec::new()
    });

    Ok(Json(SystemResourceInfo {
        cpus,
        memory,
        gpus,
        top_cpu_processes,
        top_gpu_processes,
    }))
}

async fn get_cpu_info() -> Result<Vec<CpuInfo>, AppError> {
    let cpuinfo_content = fs::read_to_string("/proc/cpuinfo").await?;
    let mut brand = "Unknown".to_string();
    let mut frequency: u64 = 0;
    for line in cpuinfo_content.lines() {
        if line.starts_with("model name") {
            brand = line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
        if line.starts_with("cpu MHz") {
            frequency = line
                .split(':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .parse::<f64>()
                .unwrap_or(0.0) as u64;
        }
    }

    let stat_before = read_proc_stat().await?;
    sleep(Duration::from_millis(200)).await;
    let stat_after = read_proc_stat().await?;

    let mut cpus = Vec::new();
    for (cpu_id, before) in stat_before.iter() {
        if let Some(after) = stat_after.get(cpu_id) {
            let idle_before = before.idle + before.iowait;
            let non_idle_before = before.user + before.nice + before.system + before.irq + before.softirq + before.steal;
            let total_before = idle_before + non_idle_before;

            let idle_after = after.idle + after.iowait;
            let non_idle_after = after.user + after.nice + after.system + after.irq + after.softirq + after.steal;
            let total_after = idle_after + non_idle_after;

            let total_diff = total_after - total_before;
            let idle_diff = idle_after - idle_before;

            let usage = if total_diff > 0 {
                (1.0 - (idle_diff as f32 / total_diff as f32)) * 100.0
            } else {
                0.0
            };

            if cpu_id.starts_with("cpu") && cpu_id != "cpu" {
                 cpus.push(CpuInfo {
                    brand: brand.clone(),
                    frequency,
                    usage,
                });
            }
        }
    }

    // Fallback if we couldn't parse individual cores for some reason
    if cpus.is_empty() {
        let num_cpus = std::thread::available_parallelism().map_err(|e| AppError::Io(e))?.get();
        for _ in 0..num_cpus {
             cpus.push(CpuInfo {
                brand: brand.clone(),
                frequency,
                usage: 0.0, // Cannot calculate total usage easily this way
            });
        }
    }

    Ok(cpus)
}

#[derive(Default)]
struct ProcStat {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

async fn read_proc_stat() -> Result<HashMap<String, ProcStat>, AppError> {
    let content = fs::read_to_string("/proc/stat").await?;
    let mut stats = HashMap::new();
    for line in content.lines() {
        if line.starts_with("cpu") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 8 {
                let cpu_id = parts[0].to_string();
                let stat = ProcStat {
                    user: parts[1].parse().unwrap_or(0),
                    nice: parts[2].parse().unwrap_or(0),
                    system: parts[3].parse().unwrap_or(0),
                    idle: parts[4].parse().unwrap_or(0),
                    iowait: parts[5].parse().unwrap_or(0),
                    irq: parts[6].parse().unwrap_or(0),
                    softirq: parts[7].parse().unwrap_or(0),
                    steal: parts[8].parse().unwrap_or(0),
                };
                stats.insert(cpu_id, stat);
            }
        }
    }
    Ok(stats)
}

async fn get_memory_info() -> Result<MemoryInfo, AppError> {
    let meminfo_content = fs::read_to_string("/proc/meminfo").await?;
    let mut total = 0;
    let mut available = 0;

    for line in meminfo_content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let val = parts[1].parse::<u64>().unwrap_or(0);
            match parts[0] {
                "MemTotal:" => total = val * 1024, // Assuming KB -> Bytes
                "MemAvailable:" => available = val * 1024,
                _ => {}
            }
        }
    }

    let used = total - available;

    Ok(MemoryInfo {
        total,
        used,
        available,
    })
}

async fn get_gpu_info() -> Result<Vec<GpuInfo>, anyhow::Error> {
    let output = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,driver_version,memory.total,memory.used,utilization.gpu,temperature.gpu,power.draw,power.limit",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "nvidia-smi command failed with status: {}",
            output.status
        ));
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut gpus = Vec::new();

    for line in stdout.trim().lines() {
        let values: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if values.len() < 8 {
            continue;
        }

        let gpu_info = GpuInfo {
            name: values[0].to_string(),
            driver_version: values[1].to_string(),
            memory_total: values[2].parse::<u64>()? * 1024 * 1024, // Assuming MiB -> Bytes
            memory_used: values[3].parse::<u64>()? * 1024 * 1024, // Assuming MiB -> Bytes
            utilization: values[4].parse()?,
            temperature: values[5].parse()?,
            power_draw: values[6].parse::<f32>()? as u32,
            power_limit: values[7].parse::<f32>()? as u32,
        };
        gpus.push(gpu_info);
    }

    Ok(gpus)
}

async fn get_all_process_info() -> Result<(Vec<CpuProcessInfo>, HashMap<u32, String>), AppError> {
    let output = tokio::process::Command::new("ps")
        .args(["-eo", "%cpu,pid,args", "--sort=-%cpu"])
        .output()
        .await?;

    if !output.status.success() {
        tracing::error!(
            "ps command failed with status: {}. stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(AppError::CommandFailed("ps".to_string()));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| AppError::CommandFailed("ps output not utf8".to_string()))?;

    let mut top_cpu_processes = Vec::new();
    let mut process_map = HashMap::new();

    for line in stdout.lines().skip(1) {
        let mut parts = line.trim().split_whitespace();
        if let (Some(cpu_str), Some(pid_str)) = (parts.next(), parts.next()) {
            let command = parts.collect::<Vec<&str>>().join(" ");
            if let (Ok(usage), Ok(pid)) = (cpu_str.parse::<f32>(), pid_str.parse::<u32>()) {
                if !command.is_empty() {
                    if top_cpu_processes.len() < 10 {
                        top_cpu_processes.push(CpuProcessInfo {
                            pid,
                            command: command.clone(),
                            usage,
                        });
                    }
                    process_map.insert(pid, command);
                }
            }
        }
    }
    Ok((top_cpu_processes, process_map))
}

async fn get_top_gpu_processes(
    process_map: HashMap<u32, String>,
) -> Result<Vec<GpuProcessInfo>, AppError> {
    let output = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_gpu_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await?;

    if !output.status.success() {
        // This can happen if nvidia-smi is not installed, which is not a critical error.
        return Err(AppError::CommandFailed("nvidia-smi".to_string()));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| AppError::CommandFailed("nvidia-smi output not utf8".to_string()))?;
    let mut processes = Vec::new();

    for line in stdout.trim().lines() {
        let values: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if values.len() >= 3 {
            if let (Ok(pid), name, Ok(memory_used_mib)) =
                (values[0].parse(), values[1], values[2].parse::<u64>())
            {
                let command = process_map
                    .get(&pid)
                    .cloned()
                    .unwrap_or_else(|| name.to_string());
                processes.push(GpuProcessInfo {
                    pid,
                    command,
                    memory_used: memory_used_mib * 1024 * 1024, // MiB to Bytes
                });
            }
        }
    }

    // Sort by memory usage and take top 10
    processes.sort_by(|a, b| b.memory_used.cmp(&a.memory_used));
    processes.truncate(10);

    Ok(processes)
}

#[derive(Serialize)]
pub struct KillResponse {
    message: String,
}

pub async fn kill_process_handler(
    axum::extract::Path(pid): axum::extract::Path<u32>,
) -> Result<Json<KillResponse>, AppError> {
    tracing::info!("Attempting to kill process with PID: {}", pid);

    let status = tokio::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .await
        .map_err(|e| {
            tracing::error!("Failed to execute kill command: {}", e);
            AppError::CommandFailed(format!("Failed to execute kill command for PID {}", pid))
        })?;

    if status.success() {
        Ok(Json(KillResponse {
            message: format!("Process {} terminated successfully.", pid),
        }))
    } else {
        tracing::error!(
            "Failed to kill process {}. `kill` command exited with status: {}",
            pid,
            status
        );
        Err(AppError::CommandFailed(format!(
            "Failed to kill process {}. It might not exist or you may lack permissions.",
            pid
        )))
    }
}

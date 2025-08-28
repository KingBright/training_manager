use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use tokio::process::Command;

use crate::{config, AppError, AppState};

pub fn create_router() -> Router<AppState> {
    Router::new()
        .route("/api/config", get(get_config_handler).post(update_config_handler))
        .route("/api/conda/envs", get(get_conda_envs_handler))
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

async fn get_conda_envs_handler(State(state): State<AppState>) -> Result<Json<Vec<String>>, AppError> {
    let config = state.config.read().await;
    let conda_path = config.isaaclab.conda_path.to_string_lossy().to_string();
    Ok(Json(get_conda_environments(&conda_path).await?))
}

pub async fn get_conda_environments(conda_path: &str) -> Result<Vec<String>, AppError> {
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

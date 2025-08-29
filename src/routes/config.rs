use axum::{extract::State, Json};

use crate::{config, error::AppError, models::AppState};

pub async fn get_config_handler(State(state): State<AppState>) -> Json<config::Config> {
    let config = state.config.read().await;
    Json(config.clone())
}

pub async fn update_config_handler(
    State(state): State<AppState>,
    Json(new_config): Json<config::Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    new_config.validate()?;
    let mut config_guard = state.config.write().await;
    *config_guard = new_config;
    config_guard.save_to_db(&state.db).await?;
    Ok(Json(
        serde_json::json!({ "message": "Configuration updated successfully" }),
    ))
}

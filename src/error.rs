use axum::{
    extract::multipart::MultipartError,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;

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

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_message) = match self {
            AppError::Database(err) => {
                error!("Database error: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            AppError::Io(err) => {
                error!("IO error: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "IO error".to_string())
            }
            AppError::TaskNotFound(id) => (StatusCode::NOT_FOUND, format!("Task not found: {}", id)),
            AppError::TaskAlreadyRunning => (
                StatusCode::CONFLICT,
                "Task already running".to_string(),
            ),
            AppError::Multipart(err) => {
                error!("Multipart error: {}", err);
                (
                    StatusCode::BAD_REQUEST,
                    format!("Multipart form error: {}", err),
                )
            }
            AppError::Config(err) => {
                error!("Configuration error: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Configuration error".to_string(),
                )
            }
        };
        (
            status,
            Json(serde_json::json!({ "error": error_message })),
        )
            .into_response()
    }
}

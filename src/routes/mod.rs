use axum::{
    routing::{get, post},
    Router,
};
use tower_http::{cors::CorsLayer, services::ServeDir};

use crate::{
    models::AppState,
    routes::{
        config::{get_config_handler, update_config_handler},
        files::{delete_file_handler, list_files_handler},
        static_files::index_handler,
        sync::{
            download_file_handler, download_zip_handler, get_sync_config_handler,
            get_sync_manifest_handler, sync_code_handler,
        },
        tasks::{
            create_task_handler, delete_task_handler, get_conda_envs_handler, get_queue_handler,
            get_task_handler, get_task_logs_handler, get_task_metrics_handler, list_tasks_handler,
            stop_task_handler,
        },
    },
};

use crate::routes::resources::get_resources_handler;

pub mod config;
pub mod files;
pub mod resources;
pub mod static_files;
pub mod sync;
pub mod tasks;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route(
            "/api/tasks",
            get(list_tasks_handler).post(create_task_handler),
        )
        .route(
            "/api/tasks/{id}",
            get(get_task_handler).delete(delete_task_handler),
        )
        .route("/api/tasks/{id}/stop", post(stop_task_handler))
        .route("/api/tasks/{id}/logs", get(get_task_logs_handler))
        .route("/api/tasks/{id}/metrics", get(get_task_metrics_handler))
        .route("/api/conda/envs", get(get_conda_envs_handler))
        .route("/api/queue", get(get_queue_handler))
        .route(
            "/api/config",
            get(get_config_handler).post(update_config_handler),
        )
        .route("/api/sync", post(sync_code_handler))
        .route("/api/sync/config", get(get_sync_config_handler))
        .route("/api/sync/manifest", get(get_sync_manifest_handler))
        .route(
            "/api/files",
            get(list_files_handler).delete(delete_file_handler),
        )
        .route("/api/sync/download/{*path}", get(download_file_handler))
        .route("/api/sync/download_zip", get(download_zip_handler))
        .route("/api/resources", get(get_resources_handler))
        .nest_service("/static", ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

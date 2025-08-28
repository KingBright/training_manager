use axum::Router;
use crate::AppState;

pub mod config_routes;
pub mod sync_routes;
pub mod task_routes;
pub mod system_resources;

pub fn create_api_router() -> Router<AppState> {
    Router::new()
        .merge(task_routes::create_router())
        .merge(config_routes::create_router())
        .merge(sync_routes::create_router())
        .merge(system_resources::create_router())
}

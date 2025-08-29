use anyhow::Result;
use clap::Parser;
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info};

mod config;
mod error;
mod metrics_parser;
mod models;
mod routes;
mod task_manager;

use models::AppState;
use task_manager::TaskManager;

/// IsaacLab Manager Server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Port to run the server on
    #[arg(short, long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    let database_url = "sqlite:./isaaclab_manager.db";
    if !Sqlite::database_exists(database_url).await.unwrap_or(false) {
        Sqlite::create_database(database_url).await?;
    }

    let db = SqlitePool::connect(database_url).await?;
    info!("Running database migrations...");
    match sqlx::migrate!("./migrations").run(&db).await {
        Ok(_) => info!("Database migrations completed successfully."),
        Err(e) => {
            error!("Database migration failed: {}", e);
        }
    }

    let mut config = config::Config::load(&db).await?;
    if let Some(port) = args.port {
        config.server.port = port;
    }
    let state = AppState {
        db: db.clone(),
        tasks: Arc::new(RwLock::new(HashMap::new())),
        queue: Arc::new(Mutex::new(Vec::new())),
        current_task: Arc::new(Mutex::new(None)),
        config: Arc::new(RwLock::new(config)),
    };

    let task_manager = TaskManager::new(state.clone());
    tokio::spawn(task_manager.run());

    let app = routes::create_router(state.clone());

    let addr = {
        let config_guard = state.config.read().await;
        format!("{}:{}", config_guard.server.host, config_guard.server.port)
    };
    info!("Starting IsaacLab Manager on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

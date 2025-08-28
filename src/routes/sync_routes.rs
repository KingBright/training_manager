use axum::{
    body::Body,
    extract::{multipart::Multipart, DefaultBodyLimit, Path, Query, State},
    http::header,
    response::{IntoResponse},
    routing::get,
    Json, Router,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};
use tokio::fs as tokio_fs;
use tokio_util::io::ReaderStream;
use tracing::error;
use walkdir::WalkDir;
use zip::write::{FileOptions, ZipWriter};

use crate::{AppError, AppState, SyncConfigResponse, SyncRequest};

pub fn create_router() -> Router<AppState> {
    Router::new()
        .route("/api/sync", get(|| async { "GET not supported for sync" }).post(sync_code_handler))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .route("/api/sync/config", get(get_sync_config_handler))
        .route("/api/sync/manifest", get(get_sync_manifest_handler))
        .route("/api/sync/download/:path", get(download_file_handler))
        .route("/api/sync/download_zip", get(download_zip_handler))
}

async fn resolve_sync_path(
    config_path: &std::path::Path,
    remote_path_opt: Option<&String>,
) -> Result<PathBuf, AppError> {
    let target_path = match remote_path_opt {
        Some(remote_path_str) if !remote_path_str.is_empty() => {
            let p = std::path::PathBuf::from(remote_path_str);
            if !p.is_absolute() {
                error!("Remote path must be absolute: {}", remote_path_str);
                return Err(AppError::Config(anyhow::anyhow!(
                    "The provided remote_dir must be an absolute path."
                )));
            }
            p
        }
        _ => config_path.to_path_buf(),
    };

    let canonical_target = target_path.canonicalize().map_err(|e| {
        error!("Sync path '{}' not found or invalid: {}", target_path.display(), e);
        AppError::Io(e)
    })?;

    if let Some(home_dir) = home::home_dir() {
        if !canonical_target.starts_with(&home_dir) {
            error!(
                "Security violation: Attempt to sync to a path outside of the user's home directory. Target: {}, Home: {}",
                canonical_target.display(),
                home_dir.display()
            );
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Sync directory must be within the user's home directory.",
            )));
        }
    } else {
        return Err(AppError::Config(anyhow::anyhow!(
            "Could not determine user's home directory."
        )));
    }

    Ok(canonical_target)
}

fn sanitize_path(path_str: &str) -> PathBuf {
    PathBuf::from(path_str)
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect()
}

async fn get_sync_config_handler(State(state): State<AppState>) -> Json<SyncConfigResponse> {
    let config = state.config.read().await;
    Json(SyncConfigResponse {
        default_excludes: config.sync.default_excludes.clone(),
    })
}

async fn get_sync_manifest_handler(
    State(state): State<AppState>,
    Query(params): Query<SyncRequest>,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let config = state.config.read().await;
    let base_path = PathBuf::from(&config.sync.target_path);
    let target_path = resolve_sync_path(&base_path, params.remote_path.as_ref()).await?;

    let excludes = config.sync.default_excludes.clone();
    let manifest = tokio::task::spawn_blocking(move || {
        let exclude_patterns: Vec<glob::Pattern> = excludes
            .iter()
            .map(|s| glob::Pattern::new(s).expect("Invalid glob pattern in config"))
            .collect();

        let walker = WalkDir::new(&target_path).into_iter();
        let filtered_walker = walker.filter_entry(|e| {
            let path = e.path();
            let relative_path = match path.strip_prefix(&target_path) {
                Ok(p) => p,
                Err(_) => return false,
            };
            if relative_path.as_os_str().is_empty() {
                return true;
            }
            !exclude_patterns.iter().any(|p| p.matches_path(relative_path))
        });

        let mut manifest: HashMap<String, String> = HashMap::new();
        for result in filtered_walker {
            if let Ok(entry) = result {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(relative_path) = path.strip_prefix(&target_path) {
                        if let Ok(mut file) = File::open(path) {
                            let mut hasher = Sha256::new();
                            if std::io::copy(&mut file, &mut hasher).is_ok() {
                                let hash = format!("{:x}", hasher.finalize());
                                manifest.insert(relative_path.to_string_lossy().replace('\\', "/"), hash);
                            }
                        }
                    }
                }
            }
        }
        manifest
    })
    .await
    .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    Ok(Json(manifest))
}

async fn download_file_handler(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<SyncRequest>,
) -> Result<impl IntoResponse, AppError> {
    let config = state.config.read().await;
    let remote_dir_str = params.remote_path.as_deref().unwrap_or(".");
    let remote_dir_path = std::path::Path::new(remote_dir_str);

    let base_dir = if remote_dir_path.is_absolute() {
        remote_dir_path.to_path_buf()
    } else {
        let sanitized_relative = remote_dir_path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<PathBuf>();
        config
            .tasks
            .working_directory
            .join(sanitized_relative)
    };

    let file_path = base_dir.join(sanitize_path(&path));

    let canonical_path = file_path.canonicalize().map_err(|_| {
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "File not found",
        ))
    })?;

    if !remote_dir_path.is_absolute() {
        let canonical_base =
            config.tasks.working_directory.canonicalize().map_err(AppError::Io)?;
        if !canonical_path.starts_with(&canonical_base) {
            error!(
                "Potential directory traversal attempt blocked: {:?}",
                canonical_path
            );
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Access denied.",
            )));
        }
    }

    let file_path = canonical_path;

    if !file_path.is_file() {
        return Err(AppError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "Path is not a file")));
    }

    let file = tokio_fs::File::open(&file_path).await.map_err(AppError::Io)?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file_name),
        ),
    ];
    Ok((headers, body).into_response())
}

async fn download_zip_handler(
    State(state): State<AppState>,
    Query(params): Query<SyncRequest>,
) -> Result<impl IntoResponse, AppError> {
    let config = state.config.read().await;
    let remote_path_str = params.remote_path.as_deref().unwrap_or(".");
    let remote_path = std::path::Path::new(remote_path_str);

    let target_path = if remote_path.is_absolute() {
        remote_path.to_path_buf()
    } else {
        let sanitized_relative = remote_path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect::<PathBuf>();
        config
            .tasks
            .working_directory
            .join(sanitized_relative)
    };

    let canonical_target = target_path.canonicalize().map_err(|e| {
        error!(
            "Target path '{}' not found or invalid: {}",
            target_path.display(),
            e
        );
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "The specified path does not exist.",
        ))
    })?;

    if !remote_path.is_absolute() {
        let canonical_base =
            config.tasks.working_directory.canonicalize().map_err(|e| {
                error!(
                    "Working directory '{}' not found or invalid: {}",
                    config.tasks.working_directory.display(),
                    e
                );
                AppError::Io(e)
            })?;
        if !canonical_target.starts_with(&canonical_base) {
            error!(
                "Security violation: Attempt to access path '{}' which is outside of working directory '{}'",
                canonical_target.display(),
                canonical_base.display()
            );
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Access denied.",
            )));
        }
    }

    let target_path = canonical_target;

    let zip_buffer = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, std::io::Error> {
        let mut pt_files = Vec::new();
        for entry in WalkDir::new(&target_path).into_iter().filter_map(|e| e.ok()) {
            if entry.path().extension().map_or(false, |ext| ext == "pt") {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        pt_files.push((entry.path().to_path_buf(), modified));
                    }
                }
            }
        }

        let newest_pt_path = if !pt_files.is_empty() {
            pt_files.sort_by(|a, b| b.1.cmp(&a.1));
            Some(pt_files[0].0.clone())
        } else {
            None
        };

        let mut buffer = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buffer);
            let mut zip = ZipWriter::new(cursor);
            let options = FileOptions::<'_, ()>::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o755);

            let walker = WalkDir::new(&target_path).into_iter();
            for entry in walker.filter_map(|e| e.ok()) {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.starts_with("events.out") {
                        continue;
                    }
                }

                if path.extension().map_or(false, |ext| ext == "pt") {
                    if let Some(newest) = &newest_pt_path {
                        if path != newest.as_path() {
                            continue;
                        }
                    }
                }

                let name = path.strip_prefix(&target_path).unwrap();
                if path.is_file() {
                    zip.start_file(name.to_string_lossy(), options)?;
                    let mut f = std::fs::File::open(path)?;
                    let mut file_buffer = Vec::new();
                    f.read_to_end(&mut file_buffer)?;
                    zip.write_all(&file_buffer)?;
                } else if !name.as_os_str().is_empty() {
                    zip.add_directory(name.to_string_lossy(), options)?;
                }
            }
            zip.finish()?;
        }
        Ok(buffer)
    })
    .await
    .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    let file_name = if let Some(remote_path) = &params.remote_path {
        let sanitized = sanitize_path(remote_path);
        let name = sanitized.file_name().and_then(|s| s.to_str()).unwrap_or("archive");
        format!("{}.zip", name)
    } else {
        "archive.zip".to_string()
    };

    let headers = [
        (header::CONTENT_TYPE, "application/zip".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file_name),
        ),
    ];

    let zip_data = zip_buffer?;
    Ok((headers, zip_data).into_response())
}

async fn sync_code_handler(
    State(state): State<AppState>,
    Query(params): Query<SyncRequest>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = state.config.read().await;
    let base_path = PathBuf::from(&config.sync.target_path);
    let canonical_target = resolve_sync_path(&base_path, params.remote_path.as_ref()).await?;

    tokio_fs::create_dir_all(&canonical_target).await?;

    let mut files_written = 0;
    while let Some(field) = multipart.next_field().await? {
        if let Some(relative_path_str) = field.file_name() {
            let relative_path = sanitize_path(relative_path_str);

            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let dest_path = canonical_target.join(&relative_path);

            if !dest_path.starts_with(&canonical_target) {
                error!("Security violation: file path '{}' escaped target directory '{}'", dest_path.display(), canonical_target.display());
                continue;
            }

            if let Some(parent) = dest_path.parent() {
                tokio_fs::create_dir_all(parent).await?;
            }
            let data = field.bytes().await?;
            tokio_fs::write(&dest_path, &data).await?;
            files_written += 1;
        }
    }

    Ok(Json(serde_json::json!({ "message": format!("Sync complete. Wrote {} files.", files_written) })))
}

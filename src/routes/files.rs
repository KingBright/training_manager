use std::path::{Path, PathBuf};

use axum::{
    extract::{Query, State},
    Json,
};
use glob::Pattern;
use tokio::fs as tokio_fs;
use tracing::{error, info};

use crate::{
    error::AppError,
    models::{AppState, DeleteFileRequest, FileInfo, ListFilesRequest, ListFilesResponse},
};

/// A utility function to sanitize a path string, removing any directory traversal components.
fn sanitize_path(path_str: &str) -> PathBuf {
    PathBuf::from(path_str)
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect()
}

pub async fn list_files_handler(
    State(state): State<AppState>,
    Query(params): Query<ListFilesRequest>,
) -> Result<Json<ListFilesResponse>, AppError> {
    let config = state.config.read().await;
    let base_dir = &config.tasks.working_directory;
    let ignore_patterns: Vec<Pattern> = config
        .files
        .ignore_patterns
        .iter()
        .filter_map(|p| Pattern::new(p).ok())
        .collect();

    let mut current_path = base_dir.clone();
    if let Some(p) = &params.path {
        let requested_path = sanitize_path(p);
        current_path = base_dir.join(requested_path);
    }

    // Security check: Ensure the resolved path is within the base working directory.
    let canonical_current = current_path.canonicalize().map_err(AppError::Io)?;
    let canonical_base = base_dir.canonicalize().map_err(AppError::Io)?;
    if !canonical_current.starts_with(&canonical_base) {
        error!(
            "Security violation: Attempt to access path '{}' which is outside of working directory '{}'",
            canonical_current.display(),
            canonical_base.display()
        );
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access denied.",
        )));
    }

    let parent_path = if canonical_current != canonical_base {
        canonical_current.parent().and_then(|p| {
            p.strip_prefix(&canonical_base).ok().and_then(|rel_p| {
                if rel_p == Path::new("") {
                    Some("/".to_string())
                } else {
                    rel_p.to_str().map(String::from)
                }
            })
        })
    } else {
        None
    };

    let mut files = Vec::new();
    let mut read_dir = tokio_fs::read_dir(&canonical_current).await?;

    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Filter out hidden files/directories
        if ignore_patterns.iter().any(|p| p.matches(&name)) {
            continue;
        }

        let metadata = tokio_fs::metadata(&path).await;
        let (created_at, modified_at) = if let Ok(meta) = metadata {
            (
                meta.created().ok().map(chrono::DateTime::from),
                meta.modified().ok().map(chrono::DateTime::from),
            )
        } else {
            (None, None)
        };

        let is_dir = path.is_dir();

        let relative_path = path
            .strip_prefix(&canonical_base)
            .unwrap()
            .to_string_lossy()
            .to_string();

        files.push(FileInfo {
            name,
            path: relative_path,
            is_dir,
            created_at,
            modified_at,
        });
    }

    files.sort_by(|a, b| {
        if a.is_dir && !b.is_dir {
            std::cmp::Ordering::Less
        } else if !a.is_dir && b.is_dir {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    let response = ListFilesResponse {
        parent: parent_path,
        files,
    };

    Ok(Json(response))
}

pub async fn delete_file_handler(
    State(state): State<AppState>,
    Json(payload): Json<DeleteFileRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = state.config.read().await;
    let base_dir = &config.tasks.working_directory;

    let target_path = base_dir.join(sanitize_path(&payload.path));

    // Security check: Ensure the resolved path is within the base working directory.
    let canonical_target = target_path.canonicalize().map_err(AppError::Io)?;
    let canonical_base = base_dir.canonicalize().map_err(AppError::Io)?;
    if !canonical_target.starts_with(&canonical_base) {
        error!(
            "Security violation: Attempt to delete path '{}' which is outside of working directory '{}'",
            canonical_target.display(),
            canonical_base.display()
        );
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access denied.",
        )));
    }

    if canonical_target == canonical_base {
        error!("Security violation: Attempt to delete the root working directory.");
        return Err(AppError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Cannot delete root directory.",
        )));
    }

    if canonical_target.is_dir() {
        tokio_fs::remove_dir_all(&canonical_target).await?;
        info!("Deleted directory: {}", canonical_target.display());
    } else {
        tokio_fs::remove_file(&canonical_target).await?;
        info!("Deleted file: {}", canonical_target.display());
    }

    Ok(Json(
        serde_json::json!({ "message": "File or directory deleted successfully" }),
    ))
}

use crate::agent::runtime::AgentRuntime;
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncReadExt;

type BrowseError = (StatusCode, Json<Value>);
fn failure(status: StatusCode, message: impl ToString) -> BrowseError {
    (status, Json(json!({"error": message.to_string()})))
}

#[derive(Default, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    root: usize,
    #[serde(default)]
    path: String,
}

async fn confined_path(root: &std::path::Path, path: &str) -> Result<PathBuf, BrowseError> {
    // Canonical containment also fences junctions and symlinks.
    let target = tokio::fs::canonicalize(root.join(path))
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    if !target.starts_with(root) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "Path is outside this workspace root",
        ));
    }
    Ok(target)
}

pub async fn browse(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<Value>, BrowseError> {
    let workspace = agent
        .get_session_workspace(&session_id)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    let roots: Vec<String> = if workspace.paths.is_empty() {
        vec![workspace.resolved_path()]
    } else {
        workspace.paths.iter().map(|p| p.path.clone()).collect()
    };
    let roots: Vec<String> = roots
        .iter()
        .map(|p| shellexpand::tilde(p).to_string())
        .collect();
    let configured = roots
        .get(query.root)
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST, "Unknown workspace root"))?;
    let root = tokio::fs::canonicalize(configured)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    let target = confined_path(&root, &query.path).await?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    let relative = target
        .strip_prefix(&root)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/");
    if metadata.is_file() {
        const MAX_BYTES: u64 = 2 * 1024 * 1024;
        let file = tokio::fs::File::open(&target)
            .await
            .map_err(|e| failure(StatusCode::FORBIDDEN, e))?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "File exceeds the 2 MB preview limit",
            ));
        }
        if bytes.contains(&0) {
            return Err(failure(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Binary files cannot be shown as text",
            ));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "File is not UTF-8 text"))?;
        return Ok(Json(
            json!({"kind":"file", "path":relative, "root":query.root, "roots":roots, "content":content, "size":metadata.len()}),
        ));
    }
    if !metadata.is_dir() {
        return Err(failure(StatusCode::BAD_REQUEST, "Not a file or directory"));
    }
    let mut directory = tokio::fs::read_dir(&target)
        .await
        .map_err(|e| failure(StatusCode::FORBIDDEN, e))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    while let Some(entry) = directory
        .next_entry()
        .await
        .map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))?
    {
        if entries.len() >= 2000 {
            truncated = true;
            break;
        }
        let Ok(metadata) = tokio::fs::metadata(entry.path()).await else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        entries.push(json!({"name": name, "path":path, "directory":metadata.is_dir(), "size":metadata.len()}));
    }
    entries.sort_by(|a, b| {
        b["directory"]
            .as_bool()
            .cmp(&a["directory"].as_bool())
            .then_with(|| {
                a["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .cmp(&b["name"].as_str().unwrap_or_default().to_lowercase())
            })
    });
    Ok(Json(
        json!({"kind":"directory", "workspace":workspace.name, "path":relative, "root":query.root, "roots":roots, "entries":entries, "truncated":truncated}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn preview_paths_are_confined_to_the_canonical_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        tokio::fs::create_dir(&root).await.unwrap();
        tokio::fs::write(root.join("inside.txt"), "inside")
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("outside.txt"), "outside")
            .await
            .unwrap();
        let root = tokio::fs::canonicalize(root).await.unwrap();
        assert!(confined_path(&root, "inside.txt").await.is_ok());
        assert_eq!(
            confined_path(&root, "../outside.txt").await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert!(confined_path(&root, "missing.txt").await.is_err());
    }
}

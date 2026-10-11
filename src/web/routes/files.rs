use crate::agent::runtime::AgentRuntime;
use axum::{
    body::Body,
    extract::{Extension, Path, Query},
    http::{header, StatusCode},
    response::Response,
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

// ── Showing images and revealing files from chat ───────────────────
// A chat can embed an image from disk or link a file. Both stay inside the
// workspaces and OSA's own data folder, the places the agent may already read.

const MAX_SHARED_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

/// Canonical folders a chat may show or reveal files from.
async fn shareable_roots(agent: &AgentRuntime) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for workspace in agent.get_workspaces().await {
        let paths: Vec<String> = if workspace.paths.is_empty() {
            vec![workspace.resolved_path()]
        } else {
            workspace.paths.iter().map(|p| p.path.clone()).collect()
        };
        for path in paths {
            if let Ok(root) = tokio::fs::canonicalize(shellexpand::tilde(&path).to_string()).await {
                roots.push(root);
            }
        }
    }
    let config_dir = agent.get_config().await.config_dir();
    if let Ok(root) = tokio::fs::canonicalize(&config_dir).await {
        roots.push(root);
    }
    roots
}

/// Resolves a requested path to an existing file inside a shareable root.
async fn shareable_file(agent: &AgentRuntime, requested: &str) -> Result<PathBuf, BrowseError> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(failure(StatusCode::BAD_REQUEST, "No path given"));
    }
    let target = tokio::fs::canonicalize(shellexpand::tilde(requested).to_string())
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    if !metadata.is_file() {
        return Err(failure(StatusCode::BAD_REQUEST, "Not a file"));
    }
    let roots = shareable_roots(agent).await;
    if !roots.iter().any(|root| target.starts_with(root)) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "This file is outside the workspaces and OSA's data folder",
        ));
    }
    Ok(target)
}

/// Raster image types only. SVG is left out because it can carry script.
fn image_content_type(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct SharedFileQuery {
    path: String,
}

/// Serves an image so the chat can embed it. Requires the normal API token.
pub async fn raw(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(query): Query<SharedFileQuery>,
) -> Result<Response, BrowseError> {
    let target = shareable_file(&agent, &query.path).await?;
    let content_type = image_content_type(&target).ok_or_else(|| {
        failure(
            StatusCode::BAD_REQUEST,
            "Only image files can be shown in chat",
        )
    })?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    if metadata.len() > MAX_SHARED_IMAGE_BYTES {
        return Err(failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Image is larger than 25 MB",
        ));
    }
    let bytes = tokio::fs::read(&target)
        .await
        .map_err(|e| failure(StatusCode::NOT_FOUND, e))?;
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(bytes))
        .map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
pub struct RevealRequest {
    path: String,
}

/// Selects the file in Explorer. It never opens or runs the file.
pub async fn reveal(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<RevealRequest>,
) -> Result<Json<Value>, BrowseError> {
    let target = shareable_file(&agent, &payload.path).await?;
    reveal_in_file_manager(&target).map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "success": true })))
}

fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let shown = path.display().to_string();
        // canonicalize adds the \\?\ verbatim prefix, which Explorer does not understand.
        let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string();
        // Explorer wants the path quoted exactly as /select,"path". Explorer also
        // exits non-zero even on success, so the status is not checked.
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{shown}\""))
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let folder = path.parent().unwrap_or(path);
        std::process::Command::new("xdg-open").arg(folder).spawn()?;
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct OpenUrlRequest {
    url: String,
}

/// Only plain web links, with no whitespace or control characters, may be opened.
fn validated_web_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.len() > 2048 {
        return Err("Link is too long".to_string());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only http and https links can be opened".to_string());
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Link contains characters that cannot be opened".to_string());
    }
    Ok(url.to_string())
}

/// Opens a web link in the default browser of the machine running OSA.
pub async fn open_url(Json(payload): Json<OpenUrlRequest>) -> Result<Json<Value>, BrowseError> {
    let url = validated_web_url(&payload.url).map_err(|e| failure(StatusCode::BAD_REQUEST, e))?;
    open_with_system(&url).map_err(|e| failure(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "success": true })))
}

fn open_with_system(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // explorer.exe hands the link to the default browser. It exits non-zero
        // even on success, so the status is not checked.
        std::process::Command::new("explorer.exe")
            .arg(url)
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        Ok(())
    }
}

#[cfg(test)]
mod open_url_tests {
    use super::validated_web_url;

    #[test]
    fn only_plain_web_links_are_accepted() {
        assert!(validated_web_url("https://example.com/a?b=c").is_ok());
        assert!(validated_web_url("  http://localhost:8765/x  ").is_ok());
        assert!(validated_web_url("file:///C:/Windows/notepad.exe").is_err());
        assert!(validated_web_url("javascript:alert(1)").is_err());
        assert!(validated_web_url("https://example.com/a b").is_err());
        assert!(validated_web_url("https://example.com/\nx").is_err());
        assert!(validated_web_url(&format!("https://e.com/{}", "a".repeat(2100))).is_err());
    }
}

#[cfg(test)]
mod shared_file_tests {
    use super::image_content_type;
    use std::path::Path;

    #[test]
    fn only_raster_images_are_served() {
        assert_eq!(image_content_type(Path::new("shot.PNG")), Some("image/png"));
        assert_eq!(
            image_content_type(Path::new("photo.jpeg")),
            Some("image/jpeg")
        );
        assert_eq!(image_content_type(Path::new("icon.svg")), None);
        assert_eq!(image_content_type(Path::new("notes.txt")), None);
        assert_eq!(image_content_type(Path::new("no_extension")), None);
    }
}

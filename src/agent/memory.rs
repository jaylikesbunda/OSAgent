use crate::error::{OSAgentError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// "agent" or "user"
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryFile {
    pub version: u32,
    pub memories: Vec<MemoryEntry>,
}

impl Default for MemoryFile {
    fn default() -> Self {
        Self {
            version: 1,
            memories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStatus {
    pub enabled: bool,
    pub file_path: String,
}

pub struct MemoryStore {
    enabled: AtomicBool,
    file_path: RwLock<PathBuf>,
    io_lock: Mutex<()>,
}

impl MemoryStore {
    pub fn new(enabled: bool, file_path: String) -> Result<Self> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        if enabled {
            Self::ensure_initialized(&file_path)?;
        }

        Ok(Self {
            enabled: AtomicBool::new(enabled),
            file_path: RwLock::new(file_path),
            io_lock: Mutex::new(()),
        })
    }

    pub fn status(&self) -> MemoryStatus {
        let file_path = self.file_path.read().unwrap();
        MemoryStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            file_path: file_path.to_string_lossy().to_string(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            let file_path = self.file_path.read().unwrap();
            Self::ensure_initialized(&file_path)?;
        }
        Ok(())
    }

    pub fn set_config(&self, enabled: bool, file_path: String) -> Result<()> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        {
            let mut current = self.file_path.write().unwrap();
            *current = file_path.clone();
        }

        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            Self::ensure_initialized(&file_path)?;
        }
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<MemoryEntry>> {
        if !self.is_enabled() {
            return Ok(vec![]);
        }
        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        state
            .memories
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(state.memories)
    }

    pub async fn add(
        &self,
        title: String,
        content: String,
        tags: Vec<String>,
        source: String,
    ) -> Result<MemoryEntry> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Memory system is disabled".to_string(),
            ));
        }

        let title = title.trim().to_string();
        let content = content.trim().to_string();
        if title.is_empty() || content.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Memory title and content are required".to_string(),
            ));
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let now = Utc::now();

        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            title,
            content,
            tags,
            source,
            created_at: now,
            updated_at: now,
        };

        state.memories.push(entry.clone());
        Self::write_state(&file_path, &state)?;
        Ok(entry)
    }

    pub async fn update(
        &self,
        id: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<MemoryEntry> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Memory system is disabled".to_string(),
            ));
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;

        let entry = state
            .memories
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| OSAgentError::ToolExecution(format!("Memory '{}' not found", id)))?;

        if let Some(t) = title {
            let t = t.trim().to_string();
            if !t.is_empty() {
                entry.title = t;
            }
        }
        if let Some(c) = content {
            let c = c.trim().to_string();
            if !c.is_empty() {
                entry.content = c;
            }
        }
        if let Some(tg) = tags {
            entry.tags = tg;
        }
        entry.updated_at = Utc::now();

        let updated = entry.clone();
        Self::write_state(&file_path, &state)?;
        Ok(updated)
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let before = state.memories.len();
        state.memories.retain(|m| m.id != id);

        if state.memories.len() == before {
            return Ok(false);
        }

        Self::write_state(&file_path, &state)?;
        Ok(true)
    }

    /// Returns a system prompt block injected before each request when memories exist.
    pub async fn prompt_block(&self) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let memories = self.list().await?;
        if memories.is_empty() {
            return Ok(None);
        }

        let mut lines = Vec::new();
        lines.push("# User Memory".to_string());
        lines.push("The following memories have been recorded about the user. Treat them as durable facts unless the user corrects them. Use them to personalize responses and avoid asking for information already known.".to_string());
        lines.push("If the user asks a direct personal question such as their name, job, preferences, or project details, answer from these memories plainly and confidently.".to_string());
        lines.push("Do not confuse user facts with your own identity. You are OSA; these memories are about the user.".to_string());

        for m in memories.iter().take(50) {
            let tag_str = if m.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", m.tags.join(", "))
            };
            lines.push(format!("\n## {}{}", m.title, tag_str));
            lines.push(format!("Fact: {}", m.content));
        }

        Ok(Some(lines.join("\n")))
    }

    fn ensure_initialized(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            let content = serde_json::to_string_pretty(&MemoryFile::default()).map_err(|e| {
                OSAgentError::Parse(format!("Failed to serialize memory file: {}", e))
            })?;
            fs::write(path, content)?;
        }
        Ok(())
    }

    fn current_file_path(&self) -> PathBuf {
        self.file_path.read().unwrap().clone()
    }

    fn read_state(path: &Path) -> Result<MemoryFile> {
        Self::ensure_initialized(path)?;
        let raw = fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            return Ok(MemoryFile::default());
        }
        serde_json::from_str(&raw).map_err(|e| {
            OSAgentError::Parse(format!("Failed to parse memory file {:?}: {}", path, e))
        })
    }

    fn write_state(path: &Path, state: &MemoryFile) -> Result<()> {
        let body = serde_json::to_string_pretty(state)
            .map_err(|e| OSAgentError::Parse(format!("Failed to serialize memory file: {}", e)))?;
        fs::write(path, body)?;
        Ok(())
    }
}

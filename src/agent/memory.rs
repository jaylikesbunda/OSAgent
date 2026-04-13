use crate::config::LearningMode;
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
    #[serde(default)]
    pub category: MemoryCategory,
    #[serde(default = "default_true")]
    pub confirmed: bool,
    /// "agent" or "user"
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    UserPreference,
    ProjectContext,
    Workflow,
    Fact,
    #[default]
    General,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemorySuggestionStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySuggestion {
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub category: MemoryCategory,
    pub source: String,
    pub suggested_by: String,
    pub rationale: Option<String>,
    pub status: MemorySuggestionStatus,
    pub suggested_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryFile {
    pub version: u32,
    pub memories: Vec<MemoryEntry>,
    #[serde(default)]
    pub suggestions: Vec<MemorySuggestion>,
}

impl Default for MemoryFile {
    fn default() -> Self {
        Self {
            version: 2,
            memories: Vec::new(),
            suggestions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStatus {
    pub enabled: bool,
    pub file_path: String,
    pub learning_mode: LearningMode,
}

pub struct MemoryStore {
    enabled: AtomicBool,
    file_path: RwLock<PathBuf>,
    learning_mode: RwLock<LearningMode>,
    io_lock: Mutex<()>,
    cached_prompt_block: std::sync::RwLock<Option<String>>,
    cache_dirty: AtomicBool,
}

impl MemoryStore {
    pub fn new(enabled: bool, file_path: String, learning_mode: LearningMode) -> Result<Self> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        if enabled {
            Self::ensure_initialized(&file_path)?;
        }

        Ok(Self {
            enabled: AtomicBool::new(enabled),
            file_path: RwLock::new(file_path),
            learning_mode: RwLock::new(learning_mode),
            io_lock: Mutex::new(()),
            cached_prompt_block: std::sync::RwLock::new(None),
            cache_dirty: AtomicBool::new(true),
        })
    }

    pub fn status(&self) -> MemoryStatus {
        let file_path = self.file_path.read().unwrap();
        MemoryStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            file_path: file_path.to_string_lossy().to_string(),
            learning_mode: *self.learning_mode.read().unwrap(),
        }
    }

    pub fn learning_mode(&self) -> LearningMode {
        *self.learning_mode.read().unwrap()
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

    pub fn set_config(
        &self,
        enabled: bool,
        file_path: String,
        learning_mode: LearningMode,
    ) -> Result<()> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        {
            let mut current = self.file_path.write().unwrap();
            *current = file_path.clone();
        }

        {
            let mut current_mode = self.learning_mode.write().unwrap();
            *current_mode = learning_mode;
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
        category: Option<MemoryCategory>,
        confirmed: bool,
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
            category: category.unwrap_or_default(),
            confirmed,
            source,
            created_at: now,
            updated_at: now,
        };

        state.memories.push(entry.clone());
        Self::write_state(&file_path, &state)?;
        self.invalidate_cache();
        Ok(entry)
    }

    pub async fn update(
        &self,
        id: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
        category: Option<MemoryCategory>,
        confirmed: Option<bool>,
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
        if let Some(cat) = category {
            entry.category = cat;
        }
        if let Some(is_confirmed) = confirmed {
            entry.confirmed = is_confirmed;
        }
        entry.updated_at = Utc::now();

        let updated = entry.clone();
        Self::write_state(&file_path, &state)?;
        self.invalidate_cache();
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
        self.invalidate_cache();
        Ok(true)
    }

    pub async fn suggest(
        &self,
        title: String,
        content: String,
        tags: Vec<String>,
        category: Option<MemoryCategory>,
        source: String,
        suggested_by: String,
        rationale: Option<String>,
    ) -> Result<MemorySuggestion> {
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

        let suggestion = MemorySuggestion {
            id: Uuid::new_v4().to_string(),
            title,
            content,
            tags,
            category: category.unwrap_or_default(),
            source,
            suggested_by,
            rationale,
            status: MemorySuggestionStatus::Pending,
            suggested_at: now,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
        };

        state.suggestions.push(suggestion.clone());
        Self::write_state(&file_path, &state)?;
        Ok(suggestion)
    }

    pub async fn list_suggestions(&self) -> Result<Vec<MemorySuggestion>> {
        if !self.is_enabled() {
            return Ok(vec![]);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        state
            .suggestions
            .sort_by(|a, b| b.suggested_at.cmp(&a.suggested_at));
        Ok(state.suggestions)
    }

    pub async fn approve_suggestion(
        &self,
        suggestion_id: &str,
        actor: String,
    ) -> Result<MemoryEntry> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Memory system is disabled".to_string(),
            ));
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let now = Utc::now();

        let suggestion = state
            .suggestions
            .iter_mut()
            .find(|s| s.id == suggestion_id)
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!(
                    "Memory suggestion '{}' not found",
                    suggestion_id
                ))
            })?;

        if suggestion.status != MemorySuggestionStatus::Pending {
            return Err(OSAgentError::ToolExecution(
                "Only pending suggestions can be approved".to_string(),
            ));
        }

        suggestion.status = MemorySuggestionStatus::Approved;
        suggestion.resolved_at = Some(now);
        suggestion.resolved_by = Some(actor);

        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            title: suggestion.title.clone(),
            content: suggestion.content.clone(),
            tags: suggestion.tags.clone(),
            category: suggestion.category.clone(),
            confirmed: true,
            source: suggestion.source.clone(),
            created_at: now,
            updated_at: now,
        };

        state.memories.push(entry.clone());
        Self::write_state(&file_path, &state)?;
        self.invalidate_cache();
        Ok(entry)
    }

    pub async fn reject_suggestion(
        &self,
        suggestion_id: &str,
        actor: String,
        note: Option<String>,
    ) -> Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let now = Utc::now();

        let Some(suggestion) = state.suggestions.iter_mut().find(|s| s.id == suggestion_id) else {
            return Ok(false);
        };

        if suggestion.status != MemorySuggestionStatus::Pending {
            return Ok(false);
        }

        suggestion.status = MemorySuggestionStatus::Rejected;
        suggestion.resolved_at = Some(now);
        suggestion.resolved_by = Some(actor);
        suggestion.resolution_note = note.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

        Self::write_state(&file_path, &state)?;
        Ok(true)
    }

    /// Returns a system prompt block injected before each request when memories exist.
    pub async fn prompt_block(&self) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        if !self.cache_dirty.load(Ordering::Relaxed) {
            if let Ok(guard) = self.cached_prompt_block.read() {
                if guard.is_some() {
                    return Ok(guard.clone());
                }
            }
        }

        let memories = self.list().await?;
        if memories.is_empty() {
            let mut guard = self.cached_prompt_block.write().unwrap();
            *guard = None;
            self.cache_dirty.store(false, Ordering::Relaxed);
            return Ok(None);
        }

        let mut lines = Vec::new();
        lines.push("# User Memory".to_string());

        lines.push(
            "Use confirmed entries when relevant, but always prioritize the user's current request."
                .to_string(),
        );

        for m in memories.iter().filter(|m| m.confirmed).take(10) {
            lines.push(format!("[{:?}] {}: {}", m.category, m.title, m.content));
        }

        let block = lines.join("\n");
        let mut guard = self.cached_prompt_block.write().unwrap();
        *guard = Some(block.clone());
        self.cache_dirty.store(false, Ordering::Relaxed);
        Ok(Some(block))
    }

    fn invalidate_cache(&self) {
        self.cache_dirty.store(true, Ordering::Relaxed);
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

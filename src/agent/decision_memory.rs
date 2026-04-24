use crate::config::{CaptureMode, LearningMode};
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
pub struct DecisionEntry {
    pub id: String,
    pub key: String,
    pub value: String,
    pub rationale: Option<String>,
    pub source: String,
    pub approved_by: String,
    pub approved_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionAuditEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub decision_id: String,
    pub key: String,
    pub actor: String,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSuggestionStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionSuggestion {
    pub id: String,
    pub key: String,
    pub value: String,
    pub rationale: Option<String>,
    pub source: String,
    pub suggested_by: String,
    pub status: DecisionSuggestionStatus,
    pub suggested_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum DecisionCaptureOutcome {
    Ignored,
    Recorded(DecisionEntry),
    Suggested(DecisionSuggestion),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DecisionMemoryFile {
    pub version: u32,
    pub decisions: Vec<DecisionEntry>,
    pub audit: Vec<DecisionAuditEvent>,
    #[serde(default)]
    pub suggestions: Vec<DecisionSuggestion>,
}

impl Default for DecisionMemoryFile {
    fn default() -> Self {
        Self {
            version: 2,
            decisions: Vec::new(),
            audit: Vec::new(),
            suggestions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionMemoryStatus {
    pub enabled: bool,
    pub file_path: String,
    pub learning_mode: LearningMode,
    pub capture_mode: CaptureMode,
}

pub struct DecisionMemory {
    enabled: AtomicBool,
    file_path: RwLock<PathBuf>,
    learning_mode: RwLock<LearningMode>,
    capture_mode: RwLock<CaptureMode>,
    io_lock: Mutex<()>,
}

impl DecisionMemory {
    pub fn new(
        enabled: bool,
        file_path: String,
        learning_mode: LearningMode,
        capture_mode: CaptureMode,
    ) -> Result<Self> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        if enabled {
            Self::ensure_file_initialized(&file_path)?;
        }

        Ok(Self {
            enabled: AtomicBool::new(enabled),
            file_path: RwLock::new(file_path),
            learning_mode: RwLock::new(learning_mode),
            capture_mode: RwLock::new(capture_mode),
            io_lock: Mutex::new(()),
        })
    }

    pub fn status(&self) -> DecisionMemoryStatus {
        let file_path = self.file_path.read().unwrap();
        DecisionMemoryStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            file_path: file_path.to_string_lossy().to_string(),
            learning_mode: *self.learning_mode.read().unwrap(),
            capture_mode: *self.capture_mode.read().unwrap(),
        }
    }

    pub fn learning_mode(&self) -> LearningMode {
        *self.learning_mode.read().unwrap()
    }

    pub fn capture_mode(&self) -> CaptureMode {
        *self.capture_mode.read().unwrap()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_config(
        &self,
        enabled: bool,
        file_path: String,
        learning_mode: LearningMode,
        capture_mode: CaptureMode,
    ) -> Result<()> {
        let expanded = shellexpand::tilde(&file_path).to_string();
        let file_path = PathBuf::from(expanded);

        {
            let mut current = self.file_path.write().unwrap();
            *current = file_path.clone();
        }

        {
            let mut mode = self.learning_mode.write().unwrap();
            *mode = learning_mode;
        }

        {
            let mut capture = self.capture_mode.write().unwrap();
            *capture = capture_mode;
        }

        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            Self::ensure_file_initialized(&file_path)?;
        }

        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<DecisionEntry>> {
        if !self.is_enabled() {
            return Ok(vec![]);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        state
            .decisions
            .sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(state.decisions)
    }

    pub async fn upsert_approved(
        &self,
        key: String,
        value: String,
        rationale: Option<String>,
        source: String,
        actor: String,
    ) -> Result<DecisionEntry> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Decision memory is disabled".to_string(),
            ));
        }

        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Decision key and value are required".to_string(),
            ));
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let now = Utc::now();

        let decision = Self::upsert_in_state(&mut state, key, value, rationale, source, actor, now);

        Self::write_state(&file_path, &state)?;
        Ok(decision)
    }

    fn upsert_in_state(
        state: &mut DecisionMemoryFile,
        key: String,
        value: String,
        rationale: Option<String>,
        source: String,
        actor: String,
        now: DateTime<Utc>,
    ) -> DecisionEntry {
        let maybe_existing = state
            .decisions
            .iter_mut()
            .find(|d| d.key.eq_ignore_ascii_case(&key));

        if let Some(existing) = maybe_existing {
            existing.value = value.clone();
            existing.rationale = rationale.clone().map(|s| s.trim().to_string());
            existing.source = source;
            existing.approved_by = actor.clone();
            existing.updated_at = now;

            state.audit.push(DecisionAuditEvent {
                id: Uuid::new_v4().to_string(),
                timestamp: now,
                action: "updated".to_string(),
                decision_id: existing.id.clone(),
                key: existing.key.clone(),
                actor,
                details: format!("Updated approved decision to '{}'.", value),
            });

            existing.clone()
        } else {
            let created = DecisionEntry {
                id: Uuid::new_v4().to_string(),
                key: key.clone(),
                value: value.clone(),
                rationale: rationale.map(|s| s.trim().to_string()),
                source,
                approved_by: actor.clone(),
                approved_at: now,
                updated_at: now,
            };

            state.audit.push(DecisionAuditEvent {
                id: Uuid::new_v4().to_string(),
                timestamp: now,
                action: "added".to_string(),
                decision_id: created.id.clone(),
                key: created.key.clone(),
                actor,
                details: format!("Added approved decision '{}'.", created.value),
            });

            state.decisions.push(created.clone());
            created
        }
    }

    pub async fn delete(&self, decision_id: &str, actor: String) -> Result<bool> {
        if !self.is_enabled() {
            return Ok(false);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let initial_len = state.decisions.len();

        let removed = state
            .decisions
            .iter()
            .find(|d| d.id == decision_id)
            .cloned();

        state.decisions.retain(|d| d.id != decision_id);
        if state.decisions.len() == initial_len {
            return Ok(false);
        }

        if let Some(decision) = removed {
            state.audit.push(DecisionAuditEvent {
                id: Uuid::new_v4().to_string(),
                timestamp: Utc::now(),
                action: "deleted".to_string(),
                decision_id: decision.id,
                key: decision.key,
                actor,
                details: "Deleted approved decision".to_string(),
            });
        }

        Self::write_state(&file_path, &state)?;
        Ok(true)
    }

    pub async fn suggest(
        &self,
        key: String,
        value: String,
        rationale: Option<String>,
        source: String,
        suggested_by: String,
    ) -> Result<DecisionSuggestion> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Decision memory is disabled".to_string(),
            ));
        }

        let key = key.trim().to_string();
        let value = value.trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Decision key and value are required".to_string(),
            ));
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        let now = Utc::now();

        let suggestion = DecisionSuggestion {
            id: Uuid::new_v4().to_string(),
            key,
            value,
            rationale,
            source,
            suggested_by,
            status: DecisionSuggestionStatus::Pending,
            suggested_at: now,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
        };

        state.suggestions.push(suggestion.clone());
        Self::write_state(&file_path, &state)?;
        Ok(suggestion)
    }

    pub async fn list_suggestions(&self) -> Result<Vec<DecisionSuggestion>> {
        if !self.is_enabled() {
            return Ok(vec![]);
        }

        let _guard = self.io_lock.lock().await;
        let file_path = self.current_file_path();
        let mut state = Self::read_state(&file_path)?;
        state
            .suggestions
            .sort_by_key(|b| std::cmp::Reverse(b.suggested_at));
        Ok(state.suggestions)
    }

    pub async fn approve_suggestion(
        &self,
        suggestion_id: &str,
        actor: String,
    ) -> Result<DecisionEntry> {
        if !self.is_enabled() {
            return Err(OSAgentError::ToolExecution(
                "Decision memory is disabled".to_string(),
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
                    "Decision suggestion '{}' not found",
                    suggestion_id
                ))
            })?;

        if suggestion.status != DecisionSuggestionStatus::Pending {
            return Err(OSAgentError::ToolExecution(
                "Only pending suggestions can be approved".to_string(),
            ));
        }

        suggestion.status = DecisionSuggestionStatus::Approved;
        suggestion.resolved_at = Some(now);
        suggestion.resolved_by = Some(actor.clone());

        let suggested_key = suggestion.key.clone();
        let suggested_value = suggestion.value.clone();
        let suggested_rationale = suggestion.rationale.clone();
        let suggested_source = suggestion.source.clone();

        let decision = Self::upsert_in_state(
            &mut state,
            suggested_key,
            suggested_value,
            suggested_rationale,
            format!("{}-approved", suggested_source),
            actor,
            now,
        );

        Self::write_state(&file_path, &state)?;
        Ok(decision)
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

        if suggestion.status != DecisionSuggestionStatus::Pending {
            return Ok(false);
        }

        suggestion.status = DecisionSuggestionStatus::Rejected;
        suggestion.resolved_at = Some(now);
        suggestion.resolved_by = Some(actor);
        suggestion.resolution_note = note.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

        Self::write_state(&file_path, &state)?;
        Ok(true)
    }

    pub async fn maybe_capture_from_user_message(
        &self,
        message: &str,
        actor: &str,
    ) -> Result<DecisionCaptureOutcome> {
        if !self.is_enabled() {
            return Ok(DecisionCaptureOutcome::Ignored);
        }

        let trimmed = message.trim();
        let lower = trimmed.to_lowercase();

        if let Some(decision) = self.try_explicit_prefix(trimmed, &lower, actor).await? {
            return Ok(DecisionCaptureOutcome::Recorded(decision));
        }

        self.try_natural_patterns(trimmed, &lower, actor).await
    }

    async fn try_explicit_prefix(
        &self,
        trimmed: &str,
        lower: &str,
        actor: &str,
    ) -> Result<Option<DecisionEntry>> {
        let prefixes = ["approved decision:", "decision approved:", "approved:"];
        let mut payload: Option<&str> = None;

        for prefix in prefixes {
            if lower.starts_with(prefix) {
                payload = Some(trimmed[prefix.len()..].trim());
                break;
            }
        }

        let Some(payload) = payload else {
            return Ok(None);
        };

        self.parse_key_value_payload(payload, "chat-explicit", actor)
            .await
    }

    async fn try_natural_patterns(
        &self,
        trimmed: &str,
        lower: &str,
        actor: &str,
    ) -> Result<DecisionCaptureOutcome> {
        let capture_mode = self.capture_mode();
        if capture_mode == CaptureMode::Off {
            return Ok(DecisionCaptureOutcome::Ignored);
        }

        let patterns: &[(&str, fn(&str, &str) -> Option<(String, String)>)] = &[
            ("always use ", Self::parse_always_use),
            ("from now on use ", Self::parse_from_now_on),
            ("from now on, use ", Self::parse_from_now_on),
            ("going forward, use ", Self::parse_from_now_on),
            ("going forward use ", Self::parse_from_now_on),
            ("i prefer to use ", Self::parse_prefer),
            ("i prefer using ", Self::parse_prefer),
            ("i prefer ", Self::parse_prefer),
            ("use ", Self::parse_use_instead),
            ("make sure to use ", Self::parse_from_now_on),
            ("keep using ", Self::parse_from_now_on),
            ("stick with ", Self::parse_from_now_on),
        ];

        for (prefix, extractor) in patterns {
            if lower.starts_with(prefix) {
                if let Some((key, value)) = extractor(trimmed, lower) {
                    let rationale =
                        Some("Captured from natural-language preference pattern".to_string());
                    if capture_mode == CaptureMode::Auto {
                        let decision = self
                            .upsert_approved(
                                key,
                                value,
                                rationale,
                                "chat-detected".to_string(),
                                actor.to_string(),
                            )
                            .await?;
                        return Ok(DecisionCaptureOutcome::Recorded(decision));
                    }

                    let suggestion = self
                        .suggest(
                            key,
                            value,
                            rationale,
                            "chat-detected".to_string(),
                            actor.to_string(),
                        )
                        .await?;
                    return Ok(DecisionCaptureOutcome::Suggested(suggestion));
                }
            }
        }

        Ok(DecisionCaptureOutcome::Ignored)
    }

    fn parse_always_use(trimmed: &str, _lower: &str) -> Option<(String, String)> {
        let rest = trimmed["always use ".len()..].trim();
        if rest.is_empty() {
            return None;
        }
        let clean = rest.trim_end_matches('.');
        Some(("preferred_tool".to_string(), clean.to_string()))
    }

    fn parse_from_now_on(trimmed: &str, _lower: &str) -> Option<(String, String)> {
        let after = trimmed.find("use ").map(|i| &trimmed[i + 4..])?;
        let clean = after.trim().trim_end_matches('.');
        if clean.is_empty() {
            return None;
        }
        Some(("preferred_tool".to_string(), clean.to_string()))
    }

    fn parse_prefer(trimmed: &str, _lower: &str) -> Option<(String, String)> {
        let mut rest = trimmed;
        for prefix in &["I prefer to use ", "I prefer using ", "I prefer "] {
            if rest.starts_with(prefix) {
                rest = &rest[prefix.len()..];
                break;
            }
        }
        let clean = rest.trim().trim_end_matches('.');
        if clean.is_empty() {
            return None;
        }
        Some(("preference".to_string(), clean.to_string()))
    }

    fn parse_use_instead(trimmed: &str, lower: &str) -> Option<(String, String)> {
        if !lower.contains(" instead of ") && !lower.contains(" instead ") {
            return None;
        }
        let after_use = trimmed.find("use ").map(|i| &trimmed[i + 4..])?;
        let clean = after_use.trim().trim_end_matches('.');
        if clean.is_empty() {
            return None;
        }
        Some(("preferred_tool".to_string(), clean.to_string()))
    }

    async fn parse_key_value_payload(
        &self,
        payload: &str,
        source: &str,
        actor: &str,
    ) -> Result<Option<DecisionEntry>> {
        let Some((key, value, rationale)) = Self::parse_payload(payload) else {
            return Ok(None);
        };

        let decision = self
            .upsert_approved(
                key.to_string(),
                value.to_string(),
                rationale,
                source.to_string(),
                actor.to_string(),
            )
            .await?;

        Ok(Some(decision))
    }

    fn parse_payload(payload: &str) -> Option<(String, String, Option<String>)> {
        let mut rationale = None;
        let mut body = payload;
        if let Some((left, right)) = payload.split_once('|') {
            body = left.trim();
            let note = right.trim();
            if !note.is_empty() {
                rationale = Some(note.to_string());
            }
        }

        let parsed = if let Some((k, v)) = body.split_once("->") {
            Some((k.trim(), v.trim()))
        } else if let Some((k, v)) = body.split_once('=') {
            Some((k.trim(), v.trim()))
        } else if let Some((k, v)) = body.split_once(':') {
            Some((k.trim(), v.trim()))
        } else {
            None
        };

        parsed.map(|(k, v)| (k.to_string(), v.to_string(), rationale))
    }

    pub async fn prompt_block(&self) -> Result<Option<String>> {
        if !self.is_enabled() {
            return Ok(None);
        }

        let decisions = self.list().await?;
        if decisions.is_empty() {
            return Ok(None);
        }

        let mut lines = Vec::new();
        lines.push("# Approved Decision Memory (mandatory)".to_string());
        lines.push(
            "Apply these decisions consistently unless the user explicitly approves a change."
                .to_string(),
        );

        for decision in decisions.iter().take(20) {
            let mut line = format!("- {} = {}", decision.key, decision.value);
            if let Some(rationale) = &decision.rationale {
                if !rationale.trim().is_empty() {
                    line.push_str(&format!(" (why: {})", rationale.trim()));
                }
            }
            lines.push(line);
        }

        lines.push(
            "If a new conflicting preference appears, ask for explicit approval using: Approved decision: <key>=<value>."
                .to_string(),
        );

        Ok(Some(lines.join("\n")))
    }

    fn current_file_path(&self) -> PathBuf {
        self.file_path.read().unwrap().clone()
    }

    fn ensure_file_initialized(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        if !path.exists() {
            let content =
                serde_json::to_string_pretty(&DecisionMemoryFile::default()).map_err(|e| {
                    OSAgentError::Parse(format!("Failed to serialize decision memory: {}", e))
                })?;
            fs::write(path, content)?;
        }

        Ok(())
    }

    fn read_state(path: &Path) -> Result<DecisionMemoryFile> {
        Self::ensure_file_initialized(path)?;
        let raw = fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            return Ok(DecisionMemoryFile::default());
        }

        serde_json::from_str(&raw).map_err(|e| {
            OSAgentError::Parse(format!(
                "Failed to parse decision memory file {:?}: {}",
                path, e
            ))
        })
    }

    fn write_state(path: &Path, state: &DecisionMemoryFile) -> Result<()> {
        let body = serde_json::to_string_pretty(state).map_err(|e| {
            OSAgentError::Parse(format!("Failed to serialize decision memory: {}", e))
        })?;
        fs::write(path, body)?;
        Ok(())
    }
}

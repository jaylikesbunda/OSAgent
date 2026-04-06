use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalendarEvent {
    id: String,
    title: String,
    start: String,
    end: Option<String>,
    description: Option<String>,
    location: Option<String>,
    created_at: String,
    updated_at: String,
}

pub struct CalendarTool {
    storage_path: PathBuf,
}

impl CalendarTool {
    pub fn new(config: Config) -> Self {
        let base_dir = std::env::var("OSAGENT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string())
                    .join(".osagent")
            });
        Self {
            storage_path: base_dir.join("calendar").join("events.json"),
        }
    }

    #[cfg(test)]
    fn new_for_path(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    fn load_events(&self) -> Result<Vec<CalendarEvent>> {
        if !self.storage_path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.storage_path).map_err(OSAgentError::Io)?;
        serde_json::from_str(&raw).map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to parse calendar data: {}", e))
        })
    }

    fn save_events(&self, events: &[CalendarEvent]) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).map_err(OSAgentError::Io)?;
        }
        let raw = serde_json::to_string_pretty(events).map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to serialize calendar data: {}", e))
        })?;
        fs::write(&self.storage_path, raw).map_err(OSAgentError::Io)?;
        Ok(())
    }

    fn parse_datetime(input: &str) -> Result<DateTime<FixedOffset>> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Datetime value cannot be empty".to_string(),
            ));
        }

        if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
            return Ok(dt);
        }

        for fmt in [
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M",
        ] {
            if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, fmt) {
                return Self::local_naive_to_fixed(naive);
            }
        }

        if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
            let naive = date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| OSAgentError::ToolExecution("Invalid date value".to_string()))?;
            return Self::local_naive_to_fixed(naive);
        }

        Err(OSAgentError::ToolExecution(
            "Invalid datetime. Use RFC3339 or local formats like '2026-04-04 15:30'".to_string(),
        ))
    }

    fn local_naive_to_fixed(naive: NaiveDateTime) -> Result<DateTime<FixedOffset>> {
        match Local.from_local_datetime(&naive) {
            LocalResult::Single(dt) => Ok(dt.fixed_offset()),
            LocalResult::Ambiguous(dt, _) => Ok(dt.fixed_offset()),
            LocalResult::None => Err(OSAgentError::ToolExecution(
                "Datetime does not exist in the local timezone".to_string(),
            )),
        }
    }

    fn sorted_events(mut events: Vec<CalendarEvent>) -> Vec<CalendarEvent> {
        events.sort_by_key(|event| {
            DateTime::parse_from_rfc3339(&event.start)
                .map(|dt| dt.timestamp())
                .unwrap_or(i64::MAX)
        });
        events
    }

    fn render_event(event: &CalendarEvent) -> String {
        let mut lines = vec![
            format!("{} [{}]", event.title, event.id),
            format!("Start: {}", event.start),
        ];
        if let Some(end) = &event.end {
            lines.push(format!("End: {}", end));
        }
        if let Some(location) = &event.location {
            lines.push(format!("Location: {}", location));
        }
        if let Some(description) = &event.description {
            lines.push(format!("Description: {}", description));
        }
        lines.join("\n")
    }

    fn parse_limit(args: &Value) -> usize {
        args["limit"].as_u64().unwrap_or(10).clamp(1, 100) as usize
    }

    fn create_event(&self, args: &Value) -> Result<String> {
        let title = args["title"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("title is required for create".to_string())
        })?;
        if title.trim().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "title cannot be empty".to_string(),
            ));
        }
        let start = Self::parse_datetime(args["start"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("start is required for create".to_string())
        })?)?;
        let end = args["end"].as_str().map(Self::parse_datetime).transpose()?;
        if let Some(end) = end {
            if end < start {
                return Err(OSAgentError::ToolExecution(
                    "end must be after start".to_string(),
                ));
            }
        }

        let now = Utc::now().to_rfc3339();
        let event = CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title: title.trim().to_string(),
            start: start.to_rfc3339(),
            end: end.map(|value| value.to_rfc3339()),
            description: args["description"]
                .as_str()
                .map(|value| value.trim().to_string()),
            location: args["location"]
                .as_str()
                .map(|value| value.trim().to_string()),
            created_at: now.clone(),
            updated_at: now,
        };

        let mut events = self.load_events()?;
        events.push(event.clone());
        self.save_events(&events)?;
        Ok(format!(
            "Created calendar event\n\n{}",
            Self::render_event(&event)
        ))
    }

    fn list_events(&self, args: &Value, upcoming_only: bool) -> Result<String> {
        let limit = Self::parse_limit(args);
        let now = Utc::now().timestamp();
        let from = args["from"]
            .as_str()
            .map(Self::parse_datetime)
            .transpose()?;
        let to = args["to"].as_str().map(Self::parse_datetime).transpose()?;

        let events = Self::sorted_events(self.load_events()?)
            .into_iter()
            .filter(|event| {
                let Ok(start) = DateTime::parse_from_rfc3339(&event.start) else {
                    return false;
                };
                if upcoming_only && start.timestamp() < now {
                    return false;
                }
                if let Some(from) = from {
                    if start < from {
                        return false;
                    }
                }
                if let Some(to) = to {
                    if start > to {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .collect::<Vec<_>>();

        if events.is_empty() {
            return Ok(if upcoming_only {
                "No upcoming calendar events".to_string()
            } else {
                "No calendar events found".to_string()
            });
        }

        Ok(events
            .iter()
            .map(Self::render_event)
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    fn get_event(&self, args: &Value) -> Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("id is required for get".to_string()))?;
        let event = self
            .load_events()?
            .into_iter()
            .find(|event| event.id == id)
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!("Calendar event not found: {}", id))
            })?;
        Ok(Self::render_event(&event))
    }

    fn update_event(&self, args: &Value) -> Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("id is required for update".to_string()))?;
        let mut events = self.load_events()?;
        let event = events
            .iter_mut()
            .find(|event| event.id == id)
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!("Calendar event not found: {}", id))
            })?;

        if let Some(title) = args["title"].as_str() {
            if title.trim().is_empty() {
                return Err(OSAgentError::ToolExecution(
                    "title cannot be empty".to_string(),
                ));
            }
            event.title = title.trim().to_string();
        }
        if let Some(start) = args["start"].as_str() {
            event.start = Self::parse_datetime(start)?.to_rfc3339();
        }
        if args.get("end").is_some() {
            event.end = args["end"]
                .as_str()
                .map(Self::parse_datetime)
                .transpose()?
                .map(|dt| dt.to_rfc3339());
        }
        if args.get("description").is_some() {
            event.description = args["description"]
                .as_str()
                .map(|value| value.trim().to_string());
        }
        if args.get("location").is_some() {
            event.location = args["location"]
                .as_str()
                .map(|value| value.trim().to_string());
        }

        let start = DateTime::parse_from_rfc3339(&event.start).map_err(|e| {
            OSAgentError::ToolExecution(format!("Stored event has invalid start time: {}", e))
        })?;
        if let Some(end) = &event.end {
            let end = DateTime::parse_from_rfc3339(end).map_err(|e| {
                OSAgentError::ToolExecution(format!("Stored event has invalid end time: {}", e))
            })?;
            if end < start {
                return Err(OSAgentError::ToolExecution(
                    "end must be after start".to_string(),
                ));
            }
        }

        event.updated_at = Utc::now().to_rfc3339();
        let updated = event.clone();
        self.save_events(&events)?;
        Ok(format!(
            "Updated calendar event\n\n{}",
            Self::render_event(&updated)
        ))
    }

    fn delete_event(&self, args: &Value) -> Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("id is required for delete".to_string()))?;
        let mut events = self.load_events()?;
        let before = events.len();
        events.retain(|event| event.id != id);
        if events.len() == before {
            return Err(OSAgentError::ToolExecution(format!(
                "Calendar event not found: {}",
                id
            )));
        }
        self.save_events(&events)?;
        Ok(format!("Deleted calendar event {}", id))
    }
}

#[async_trait]
impl Tool for CalendarTool {
    fn name(&self) -> &str {
        "calendar"
    }

    fn description(&self) -> &str {
        "Manage a persistent local calendar with create, list, update, get, delete, and upcoming event actions"
    }

    fn when_to_use(&self) -> &str {
        "Use for scheduling, reviewing upcoming events, and storing calendar plans in OSA's local calendar"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for alarms, timers, or external provider calendar sync"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Create a meeting".to_string(),
                input: json!({
                    "action": "create",
                    "title": "Project sync",
                    "start": "2026-04-06 14:00",
                    "end": "2026-04-06 14:30",
                    "location": "Conference Room A"
                }),
            },
            ToolExample {
                description: "See upcoming events".to_string(),
                input: json!({
                    "action": "upcoming",
                    "limit": 5
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "upcoming", "get", "update", "delete"],
                    "description": "Calendar action"
                },
                "id": {
                    "type": "string",
                    "description": "Event ID for get, update, or delete"
                },
                "title": {
                    "type": "string",
                    "description": "Event title"
                },
                "start": {
                    "type": "string",
                    "description": "Start datetime in RFC3339 or local format like 2026-04-04 15:30"
                },
                "end": {
                    "type": ["string", "null"],
                    "description": "Optional end datetime"
                },
                "description": {
                    "type": ["string", "null"],
                    "description": "Optional event description"
                },
                "location": {
                    "type": ["string", "null"],
                    "description": "Optional location"
                },
                "from": {
                    "type": "string",
                    "description": "Optional lower datetime bound for list"
                },
                "to": {
                    "type": "string",
                    "description": "Optional upper datetime bound for list"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum number of events to return"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("action is required".to_string()))?;

        match action {
            "create" => self.create_event(&args),
            "list" => self.list_events(&args, false),
            "upcoming" => self.list_events(&args, true),
            "get" => self.get_event(&args),
            "update" => self.update_event(&args),
            "delete" => self.delete_event(&args),
            _ => Err(OSAgentError::ToolExecution(format!(
                "Unknown calendar action: {}",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_local_datetime_format() {
        let parsed = CalendarTool::parse_datetime("2026-04-04 15:30").unwrap();
        assert_eq!(
            parsed.format("%Y-%m-%d %H:%M").to_string(),
            "2026-04-04 15:30"
        );
    }

    #[tokio::test]
    async fn create_update_and_delete_event_round_trip() {
        let temp = tempdir().unwrap();
        let tool = CalendarTool::new_for_path(temp.path().join("events.json"));

        let created = tool
            .execute(json!({
                "action": "create",
                "title": "Project sync",
                "start": "2026-04-04 15:30",
                "end": "2026-04-04 16:00",
                "location": "Room A"
            }))
            .await
            .unwrap();
        assert!(created.contains("Created calendar event"));

        let events = tool.load_events().unwrap();
        assert_eq!(events.len(), 1);
        let id = events[0].id.clone();

        let updated = tool
            .execute(json!({
                "action": "update",
                "id": id,
                "description": "Weekly status"
            }))
            .await
            .unwrap();
        assert!(updated.contains("Weekly status"));

        let listed = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(listed.contains("Project sync"));

        let deleted = tool
            .execute(json!({"action": "delete", "id": events[0].id}))
            .await
            .unwrap();
        assert!(deleted.contains("Deleted calendar event"));
        assert!(tool.load_events().unwrap().is_empty());
    }
}

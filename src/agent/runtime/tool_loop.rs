//! Tool-loop detection, fingerprints, context condensing helpers
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub(super) fn estimate_tokens(text: &str) -> usize {
        let chars = text.chars().count();
        (chars / 4).max(1)
    }

    pub(super) fn summarize_tool_output_for_context(
        tool_name: &str,
        outcome: ToolOutcome,
        output: &str,
    ) -> String {
        let preview = truncation::preview_tool_output_for_context(tool_name, output);
        let rendered = match tool_name {
            "read_file" => format!(
                "Tool: {}\nOutput summary (trimmed file content for context):\n{}",
                tool_name, preview
            ),
            "list_files" | "glob" | "grep" => format!(
                "Tool: {}\nOutput summary (trimmed search results for context):\n{}",
                tool_name, preview
            ),
            _ => format!("Tool: {}\nOutput:\n{}", tool_name, preview),
        };

        format!("[tool outcome: {}]\n{}", outcome.as_str(), rendered)
    }

    pub(super) fn stable_json_string(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::String(value) => format!("{:?}", value),
            serde_json::Value::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(Self::stable_json_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            serde_json::Value::Object(map) => {
                let mut entries = map.iter().collect::<Vec<_>>();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                format!(
                    "{{{}}}",
                    entries
                        .into_iter()
                        .map(|(key, value)| {
                            format!("{}:{}", key, Self::stable_json_string(value))
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
    }

    pub(super) fn tool_call_signature(name: &str, args: &serde_json::Value) -> String {
        format!("{}:{}", name, Self::stable_json_string(args))
    }

    pub(super) fn normalize_loop_text(value: &str) -> String {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    }

    pub(super) fn normalized_arg(args: &serde_json::Value, key: &str, default: &str) -> String {
        args.get(key)
            .and_then(|value| value.as_str())
            .map(Self::normalize_loop_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    pub(super) fn tool_intent_signature(name: &str, args: &serde_json::Value) -> Option<String> {
        match name {
            "grep" => Some(format!(
                "grep:{}:{}",
                Self::normalized_arg(args, "pattern", "*"),
                Self::normalized_arg(args, "file_pattern", "*"),
            )),
            "glob" => Some(format!(
                "glob:{}:{}",
                Self::normalized_arg(args, "pattern", "*"),
                Self::normalized_arg(args, "path", "."),
            )),
            "list_files" => Some(format!(
                "list_files:{}:{}",
                Self::normalized_arg(args, "path", "."),
                args.get("recursive")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            )),
            "read_file" => {
                let path = args
                    .get("filePath")
                    .and_then(|value| value.as_str())
                    .or_else(|| args.get("path").and_then(|value| value.as_str()))
                    .unwrap_or("");
                let offset = args
                    .get("offset")
                    .and_then(|value| value.as_u64())
                    .or_else(|| args.get("start_line").and_then(|value| value.as_u64()))
                    .unwrap_or(1);
                let limit = args
                    .get("limit")
                    .and_then(|value| value.as_u64())
                    .map(|value| {
                        let start = offset.max(1);
                        start.saturating_add(value).saturating_sub(1).to_string()
                    })
                    .or_else(|| {
                        args.get("end_line")
                            .and_then(|value| value.as_u64())
                            .map(|value| value.to_string())
                    })
                    .unwrap_or_else(|| "end".to_string());
                Some(format!(
                    "read_file:{}:{}:{}",
                    Self::normalize_loop_text(path),
                    offset.max(1),
                    limit
                ))
            }
            "bash" => Some(format!(
                "bash:{}:{}",
                Self::normalized_arg(args, "command", ""),
                Self::normalized_arg(args, "workdir", "."),
            )),
            "code_python" | "code_node" | "code_bash" => Some(format!(
                "{}:{}",
                name,
                args.get("code")
                    .and_then(|value| value.as_str())
                    .map(|code| Self::normalize_loop_text(
                        &code.chars().take(160).collect::<String>()
                    ))
                    .unwrap_or_default()
            )),
            _ => None,
        }
    }

    pub(super) fn consecutive_repeat_count(history: &[String], signature: &str) -> usize {
        let mut count = 0usize;
        for previous in history.iter().rev() {
            if previous == signature {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Records a completed tool call in the rolling loop detector and, when it
    /// reports a loop, emits and persists a `LoopDetected` event. Returns
    /// `true` when the loop is critical so the caller can nudge the model
    /// before the next iteration.
    pub(super) fn check_tool_loop(
        &self,
        detector: &mut ToolLoopDetector,
        session: &mut Session,
        tool_name: &str,
        arguments: &serde_json::Value,
        success: bool,
    ) -> Result<bool> {
        let LoopDetectionResult::Stuck {
            detector: kind,
            level,
            count,
            message,
        } = detector.record_and_check(tool_name, arguments, success)
        else {
            return Ok(false);
        };

        let level_str = match level {
            LoopSeverity::Critical => "critical",
            LoopSeverity::Warning => "warning",
        };
        let kind_str = format!("{:?}", kind);
        warn!(
            "Tool loop detected for session {} ({} {} x{}): {}",
            session.id, level_str, kind_str, count, message
        );
        self.record_session_event(
            session,
            "loop_detected",
            serde_json::json!({
                "detector": &kind_str,
                "level": level_str,
                "count": count,
                "tool_name": tool_name,
                "message": &message,
            }),
        )?;
        self.event_bus.emit(AgentEvent::LoopDetected {
            session_id: session.id.clone(),
            sequence: 0,
            detector: kind_str,
            level: level_str.to_string(),
            count,
            message,
            timestamp: SystemTime::now(),
        });
        Ok(matches!(level, LoopSeverity::Critical))
    }

    pub(super) fn tool_loop_guidance(tool_name: &str) -> &'static str {
        match tool_name {
            "grep" | "glob" | "list_files" => {
                "Change the path or pattern, or inspect one of the files you already found instead of repeating the same search."
            }
            "read_file" => {
                "Paging the same file forward (offset=1, 200, 400, ...) is normal exploration. Only identical offset+limit repeats count as loops. Read a narrower window (limit=80 or less) of a new section instead of rereading the same lines unchanged."
            }
            "bash" | "code_python" | "code_node" | "code_bash" => {
                "Adjust the command or script, inspect the prior output, or summarize the blocker instead of rerunning it unchanged."
            }
            _ => {
                "Use the previous result, change strategy, or explain the blocker instead of repeating the same call."
            }
        }
    }

    pub(super) fn message_tokens(message: &Message) -> usize {
        let mut total = Self::estimate_tokens(&message.content) + 8;
        if let Some(thinking) = &message.thinking {
            total += Self::estimate_tokens(thinking);
        }
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                total += Self::estimate_tokens(&call.name);
                total += Self::estimate_tokens(&call.arguments.to_string());
            }
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            total += Self::estimate_tokens(tool_call_id);
        }
        // Images ride the request as base64 payloads; chars/4 of the text
        // alone would make multimodal sessions invisible to the trigger.
        for image in &message.images {
            total += image.data_url.chars().count().div_ceil(4).max(256);
        }
        total
    }

    pub(super) fn fingerprint_tools<T: serde::Serialize>(tools: &T) -> u64 {
        let serialized = serde_json::to_string(tools).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        serialized.hash(&mut hasher);
        hasher.finish()
    }

    pub(super) fn aggregate_cache_reason(reasons: &[String]) -> Option<String> {
        let mut unique = reasons.iter().collect::<Vec<_>>();
        unique.sort();
        unique.dedup();
        match unique.as_slice() {
            [] => None,
            [reason] => Some((*reason).clone()),
            _ => Some("mixed".to_string()),
        }
    }

    pub(super) fn condense_messages(messages: &[Message], context_window: usize) -> Vec<Message> {
        let budget = ((context_window as f32) * 0.8) as usize;
        let mut total: usize = messages.iter().map(Self::message_tokens).sum();
        if total <= budget {
            return messages.to_vec();
        }

        let mut system_message: Option<Message> = None;
        let mut user_message: Option<Message> = None;
        let mut other: Vec<Message> = Vec::new();

        for message in messages {
            if message.role == "system" && system_message.is_none() {
                system_message = Some(message.clone());
            } else if message.role == "user" {
                user_message = Some(message.clone());
                other.push(message.clone());
            } else {
                other.push(message.clone());
            }
        }

        let summary = Self::summarize_for_context(&other, budget / 2);
        let summary_message = Message::assistant(format!("Context summary:\n{}", summary), None);

        let mut condensed: Vec<Message> = Vec::new();
        if let Some(system) = system_message {
            condensed.push(system);
        }
        condensed.push(summary_message);
        if let Some(user) = user_message {
            condensed.push(user);
        }

        total = condensed.iter().map(Self::message_tokens).sum();
        if total > budget {
            return condensed;
        }

        let mut remaining: Vec<Message> = Vec::new();
        for message in messages.iter().rev() {
            if message.role == "system" {
                continue;
            }
            let msg_tokens = Self::message_tokens(message);
            if total + msg_tokens > budget {
                break;
            }
            total += msg_tokens;
            remaining.push(message.clone());
        }

        remaining.reverse();
        condensed.extend(remaining);
        condensed
    }

    pub(super) fn summarize_for_context(messages: &[Message], token_budget: usize) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0;

        for message in messages.iter().rev() {
            if message.role == "system"
                || (Self::is_synthetic_message(message)
                    && Self::message_kind(message) != Some("compaction_summary"))
            {
                continue;
            }

            let Some(line) = Self::compactable_message_content(message) else {
                continue;
            };
            let line = format!("- {}", line);
            let line_tokens = Self::estimate_tokens(&line);
            if used + line_tokens > token_budget {
                break;
            }

            used += line_tokens;
            lines.push(line);
        }

        if lines.is_empty() {
            return "No prior context retained.".to_string();
        }

        lines.reverse();
        lines.join("\n")
    }
}

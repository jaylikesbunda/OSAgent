/// Advisory repeat-tool-call reminders.
///
/// Ported from DeepSeek Harness's `dsh-repeat-tool-reminder` guard.
/// Unlike the hard loop guard (which blocks after N identical calls),
/// this is a *nudge*: when the same tool is called with identical
/// canonicalized arguments a threshold number of times in a row, an
/// escalating reminder is injected as a user-role message. The tool
/// result stays the tool's own auditable output; the reminder travels
/// separately so the model sees both.
use crate::config::RepeatReminderConfig;
use serde_json::Value;
use std::collections::HashMap;

/// Per-agent chain state. Chains are keyed by (tool name, canonicalized
/// arguments) so identical calls form a chain while a changed argument
/// starts a fresh one.
#[derive(Debug, Clone, Default)]
pub struct RepeatReminderState {
    chains: HashMap<(String, String), u32>,
}

impl RepeatReminderState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A user prompt resets the submitting agent's chain: the loop the
    /// reminder was watching is over once a human steers.
    pub fn reset(&mut self) {
        self.chains.clear();
    }

    /// Deep key-sort + stringify. Property order is irrelevant, so two
    /// JSON objects with the same fields in different orders canonicalize
    /// identically.
    pub fn canonicalize_arguments(args: &Value) -> String {
        fn sort_value(value: Value) -> Value {
            match value {
                Value::Object(map) => {
                    let mut entries: Vec<(String, Value)> = map
                        .into_iter()
                        .map(|(key, value)| (key, sort_value(value)))
                        .collect();
                    entries.sort_by(|a, b| a.0.cmp(&b.0));
                    Value::Object(entries.into_iter().collect())
                }
                Value::Array(items) => {
                    Value::Array(items.into_iter().map(sort_value).collect())
                }
                other => other,
            }
        }
        serde_json::to_string(&sort_value(args.clone())).unwrap_or_default()
    }

    /// Observe one tool call. Returns a reminder string when the chain
    /// for this (tool, canonical args) hits one of the configured
    /// thresholds, escalating from a gentle nudge to a detailed message.
    pub fn note_tool_call(
        &mut self,
        tool_name: &str,
        args: &Value,
        config: &RepeatReminderConfig,
    ) -> Option<String> {
        if !config.enabled {
            return None;
        }

        let included = config
            .include
            .iter()
            .any(|pattern| wildcard_matches(pattern, tool_name));
        let excluded = config
            .exclude
            .iter()
            .any(|pattern| wildcard_matches(pattern, tool_name));
        if !included || excluded {
            return None;
        }

        let canonical = Self::canonicalize_arguments(args);
        let count = self
            .chains
            .entry((tool_name.to_string(), canonical.clone()))
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let count = *count;

        let thresholds: Vec<u32> = {
            let mut sorted = config
                .thresholds
                .iter()
                .copied()
                .filter(|threshold| *threshold > 0)
                .collect::<Vec<_>>();
            sorted.sort_unstable();
            sorted.dedup();
            sorted
        };
        if thresholds.is_empty() {
            return None;
        }

        let hit = thresholds.iter().position(|threshold| *threshold == count);
        let index = match hit {
            Some(index) => index,
            None => return None,
        };

        let gentle = index == 0;
        if gentle {
            Some(format!(
                "Reminder: you have called the tool \"{tool_name}\" with the same arguments \
                 {count} times in a row. If this is not intentional, reconsider your approach \
                 instead of retrying the same call."
            ))
        } else {
            let preview = canonical.chars().take(config.arguments_preview_chars).collect::<String>();
            let overflow = canonical.chars().count() > config.arguments_preview_chars;
            let suffix = if overflow {
                format!(
                    " (+{} more chars)",
                    canonical.chars().count() - config.arguments_preview_chars
                )
            } else {
                String::new()
            };
            Some(format!(
                "You are repeating yourself: the tool \"{tool_name}\" has now been called \
                 {count} consecutive times with identical arguments: {preview}{suffix}. \
                 Stop retrying the same call — the result is unlikely to change. Re-evaluate \
                 the plan and try a different approach."
            ))
        }
    }
}

/// Minimal `*`-wildcard matcher: `*` matches any run of characters.
pub fn wildcard_matches(pattern: &str, name: &str) -> bool {
    fn inner(pattern: &[char], name: &[char]) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => {
                inner(rest, name)
                    || (!name.is_empty() && inner(pattern, &name[1..]))
            }
            Some((ch, rest)) => {
                name.first() == Some(ch) && inner(rest, &name[1..])
            }
        }
    }

    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    inner(&pattern, &name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> RepeatReminderConfig {
        RepeatReminderConfig::default()
    }

    #[test]
    fn canonicalization_ignores_key_order() {
        let a = json!({"b": 1, "a": {"z": true, "y": [1, 2]}});
        let b = json!({"a": {"y": [1, 2], "z": true}, "b": 1});
        assert_eq!(
            RepeatReminderState::canonicalize_arguments(&a),
            RepeatReminderState::canonicalize_arguments(&b)
        );
    }

    #[test]
    fn first_threshold_is_gentle_and_later_is_detailed() {
        let mut state = RepeatReminderState::new();
        let args = json!({"path": "x"});

        let mut reminders = Vec::new();
        for _ in 0..10 {
            if let Some(text) = state.note_tool_call("write_file", &args, &config()) {
                reminders.push(text);
            }
        }

        assert_eq!(reminders.len(), 3);
        assert!(reminders[0].starts_with("Reminder:"));
        assert!(reminders[1].contains("You are repeating yourself"));
        assert!(reminders[2].contains("8 consecutive times"));
    }

    #[test]
    fn changed_arguments_break_the_chain() {
        let mut state = RepeatReminderState::new();
        for _ in 0..3 {
            state.note_tool_call("bash", &json!({"command": "git status"}), &config());
        }
        let reminder = state.note_tool_call("bash", &json!({"command": "git diff"}), &config());
        assert!(reminder.is_none());
    }

    #[test]
    fn excluded_tools_are_transparent() {
        let mut state = RepeatReminderState::new();
        let mut config = config();
        config.exclude = vec!["todo*".to_string()];

        for _ in 0..5 {
            assert!(state
                .note_tool_call("todowrite", &json!({}), &config)
                .is_none());
        }
        // Excluded calls neither count nor reset.
        let reminder = state.note_tool_call("bash", &json!({"command": "x"}), &config());
        assert!(reminder.is_none());
    }

    #[test]
    fn reset_clears_all_chains() {
        let mut state = RepeatReminderState::new();
        state.note_tool_call("bash", &json!({"command": "x"}), &config());
        state.reset();
        let reminder = state.note_tool_call("bash", &json!({"command": "x"}), &config());
        assert!(reminder.is_none());
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_matches("*", "anything"));
        assert!(wildcard_matches("todo*", "todowrite"));
        assert!(!wildcard_matches("todo*", "bash"));
        assert!(wildcard_matches("read_file", "read_file"));
        assert!(!wildcard_matches("read_file", "write_file"));
    }
}

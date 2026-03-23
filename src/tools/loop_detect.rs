/// Tool loop detection: prevents runaway repeated tool calls.
///
/// Ported from OpenClaw's `tool-loop-detection.ts`.
/// Maintains a rolling hash history of recent tool calls and detects:
/// - Generic repeats (same tool + args)
/// - Known poll patterns with no progress
/// - Ping-pong patterns between two tools
/// - Global circuit breaker (total consecutive failures)
use std::collections::VecDeque;

const DEFAULT_HISTORY_SIZE: usize = 30;
const DEFAULT_WARNING_THRESHOLD: usize = 10;
const DEFAULT_CRITICAL_THRESHOLD: usize = 20;
const DEFAULT_GLOBAL_CIRCUIT_BREAKER: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDetectorKind {
    GenericRepeat,
    KnownPollNoProgress,
    PingPong,
    GlobalCircuitBreaker,
}

#[derive(Debug, Clone)]
pub enum LoopDetectionResult {
    NotStuck,
    Stuck {
        detector: LoopDetectorKind,
        level: LoopSeverity,
        count: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopSeverity {
    Warning,
    Critical,
}

/// Known polling tools that are expected to be called repeatedly with different state.
const KNOWN_POLL_TOOLS: &[&str] = &["process", "bash"];

#[derive(Debug, Clone)]
pub struct LoopDetectionConfig {
    pub enabled: bool,
    pub history_size: usize,
    pub warning_threshold: usize,
    pub critical_threshold: usize,
    pub global_circuit_breaker_threshold: usize,
    pub detect_generic_repeat: bool,
    pub detect_poll_no_progress: bool,
    pub detect_ping_pong: bool,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            history_size: DEFAULT_HISTORY_SIZE,
            warning_threshold: DEFAULT_WARNING_THRESHOLD,
            critical_threshold: DEFAULT_CRITICAL_THRESHOLD,
            global_circuit_breaker_threshold: DEFAULT_GLOBAL_CIRCUIT_BREAKER,
            detect_generic_repeat: true,
            detect_poll_no_progress: true,
            detect_ping_pong: true,
        }
    }
}

#[derive(Debug, Clone)]
struct ToolCallEntry {
    tool_name: String,
    /// Fast hash of (tool_name + arguments).
    signature: u64,
    /// Whether the call succeeded.
    success: bool,
}

/// State tracker for loop detection across an agent session.
#[derive(Debug)]
pub struct ToolLoopDetector {
    config: LoopDetectionConfig,
    history: VecDeque<ToolCallEntry>,
}

fn hash_tool_call(tool_name: &str, arguments: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    // Sort keys for consistent hashing of JSON objects.
    let normalized = normalize_json_for_hash(arguments);
    normalized.hash(&mut hasher);
    hasher.finish()
}

fn normalize_json_for_hash(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k, normalize_json_for_hash(&map[*k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(normalize_json_for_hash).collect();
            format!("[{}]", parts.join(","))
        }
        _ => value.to_string(),
    }
}

impl ToolLoopDetector {
    pub fn new(config: LoopDetectionConfig) -> Self {
        Self {
            config,
            history: VecDeque::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Record a tool call and check for loops.
    /// Returns a detection result indicating whether a loop was detected.
    pub fn record_and_check(
        &mut self,
        tool_name: &str,
        arguments: &serde_json::Value,
        success: bool,
    ) -> LoopDetectionResult {
        if !self.config.enabled {
            return LoopDetectionResult::NotStuck;
        }

        let signature = hash_tool_call(tool_name, arguments);

        // Check global circuit breaker
        let consecutive_failures = self.count_consecutive_failures();
        if consecutive_failures >= self.config.global_circuit_breaker_threshold {
            return LoopDetectionResult::Stuck {
                detector: LoopDetectorKind::GlobalCircuitBreaker,
                level: LoopSeverity::Critical,
                count: consecutive_failures,
                message: format!(
                    "Global circuit breaker: {} consecutive tool failures. \
                     The agent appears stuck in an error loop.",
                    consecutive_failures
                ),
            };
        }

        // Check generic repeat
        if self.config.detect_generic_repeat {
            let repeat_count = self.count_consecutive_signature(signature);
            if repeat_count >= self.config.critical_threshold {
                return LoopDetectionResult::Stuck {
                    detector: LoopDetectorKind::GenericRepeat,
                    level: LoopSeverity::Critical,
                    count: repeat_count,
                    message: format!(
                        "Critical: Tool '{}' has been called identically {} consecutive times. \
                         Stopping to prevent infinite loop. Try a different approach.",
                        tool_name,
                        repeat_count + 1
                    ),
                };
            }
            if repeat_count >= self.config.warning_threshold {
                return LoopDetectionResult::Stuck {
                    detector: LoopDetectorKind::GenericRepeat,
                    level: LoopSeverity::Warning,
                    count: repeat_count,
                    message: format!(
                        "Warning: Tool '{}' has been called identically {} consecutive times. \
                         Consider changing strategy.",
                        tool_name,
                        repeat_count + 1
                    ),
                };
            }
        }

        // Check ping-pong between two tools
        if self.config.detect_ping_pong {
            if let Some((tool_a, tool_b)) = self.detect_ping_pong_pattern() {
                return LoopDetectionResult::Stuck {
                    detector: LoopDetectorKind::PingPong,
                    level: LoopSeverity::Warning,
                    count: self.count_ping_pong_cycles(&tool_a, &tool_b),
                    message: format!(
                        "Ping-pong detected: Alternating between '{}' and '{}'. \
                         Both tools may be returning the same state.",
                        tool_a, tool_b
                    ),
                };
            }
        }

        // Check poll with no progress (same tool, different args but no success change)
        if self.config.detect_poll_no_progress && KNOWN_POLL_TOOLS.contains(&tool_name) {
            let poll_count = self.count_consecutive_same_tool(tool_name);
            if poll_count >= self.config.warning_threshold {
                let all_failed = self
                    .history
                    .iter()
                    .rev()
                    .take(poll_count)
                    .all(|e| !e.success);
                if all_failed {
                    return LoopDetectionResult::Stuck {
                        detector: LoopDetectorKind::KnownPollNoProgress,
                        level: LoopSeverity::Warning,
                        count: poll_count,
                        message: format!(
                            "Polling '{}' {} times with no success. \
                             The polled resource may be unavailable.",
                            tool_name,
                            poll_count + 1
                        ),
                    };
                }
            }
        }

        // Record the call
        self.history.push_back(ToolCallEntry {
            tool_name: tool_name.to_string(),
            signature,
            success,
        });
        while self.history.len() > self.config.history_size {
            self.history.pop_front();
        }

        LoopDetectionResult::NotStuck
    }

    /// Get guidance text for a specific tool loop.
    pub fn tool_loop_guidance(tool_name: &str) -> &'static str {
        match tool_name {
            "read_file" | "read" => {
                "You already have the file content. Move to the next step: analyze, edit, or summarize."
            }
            "grep" | "glob" => {
                "You already searched. Read the files you found or adjust the search pattern."
            }
            "bash" | "exec" => {
                "The command failed or repeated. Check error output and try a different command or arguments."
            }
            "write_file" | "write" => {
                "The file was already written. Move to testing or the next file."
            }
            _ => "Repeat detected. Change strategy: try a different tool, adjust parameters, or explain the blocker.",
        }
    }

    /// Clear the history (e.g., on strategy change).
    pub fn reset(&mut self) {
        self.history.clear();
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    fn count_consecutive_signature(&self, signature: u64) -> usize {
        self.history
            .iter()
            .rev()
            .take_while(|entry| entry.signature == signature)
            .count()
    }

    fn count_consecutive_same_tool(&self, tool_name: &str) -> usize {
        self.history
            .iter()
            .rev()
            .take_while(|entry| entry.tool_name == tool_name)
            .count()
    }

    fn count_consecutive_failures(&self) -> usize {
        self.history
            .iter()
            .rev()
            .take_while(|entry| !entry.success)
            .count()
    }

    /// Detect if the last N calls alternate between exactly two tools.
    fn detect_ping_pong_pattern(&self) -> Option<(String, String)> {
        let lookback = 10;
        let recent: Vec<&ToolCallEntry> = self.history.iter().rev().take(lookback).collect();

        if recent.len() < 4 {
            return None;
        }

        let first = &recent[0].tool_name;
        let second = recent
            .iter()
            .skip(1)
            .find(|e| e.tool_name != *first)
            .map(|e| e.tool_name.clone())?;

        // Check if it strictly alternates
        let mut expected_first = true;
        for entry in &recent {
            let expected = if expected_first { first } else { &second };
            if entry.tool_name != *expected {
                return None;
            }
            expected_first = !expected_first;
        }

        Some((first.clone(), second))
    }

    fn count_ping_pong_cycles(&self, tool_a: &str, tool_b: &str) -> usize {
        let mut count = 0usize;
        let mut expect_a = true;
        for entry in self.history.iter().rev() {
            let expected = if expect_a { tool_a } else { tool_b };
            if entry.tool_name == expected {
                if expect_a {
                    count += 1;
                }
                expect_a = !expect_a;
            } else {
                break;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn default_detector() -> ToolLoopDetector {
        ToolLoopDetector::new(LoopDetectionConfig::default())
    }

    #[test]
    fn no_loop_on_first_call() {
        let mut det = default_detector();
        let result = det.record_and_check("read_file", &json!({"path": "a.txt"}), true);
        assert!(matches!(result, LoopDetectionResult::NotStuck));
    }

    #[test]
    fn detects_generic_repeat() {
        let mut det = default_detector();
        let args = json!({"path": "a.txt"});
        for _ in 0..11 {
            det.record_and_check("read_file", &args, true);
        }
        let result = det.record_and_check("read_file", &args, true);
        assert!(matches!(
            result,
            LoopDetectionResult::Stuck {
                detector: LoopDetectorKind::GenericRepeat,
                level: LoopSeverity::Warning,
                ..
            }
        ));
    }

    #[test]
    fn detects_critical_repeat() {
        let mut det = default_detector();
        let args = json!({"path": "a.txt"});
        for _ in 0..21 {
            det.record_and_check("read_file", &args, true);
        }
        let result = det.record_and_check("read_file", &args, true);
        assert!(matches!(
            result,
            LoopDetectionResult::Stuck {
                detector: LoopDetectorKind::GenericRepeat,
                level: LoopSeverity::Critical,
                ..
            }
        ));
    }

    #[test]
    fn different_args_no_loop() {
        let mut det = default_detector();
        for i in 0..15 {
            let result =
                det.record_and_check("read_file", &json!({"path": format!("file_{i}.txt")}), true);
            assert!(matches!(result, LoopDetectionResult::NotStuck));
        }
    }

    #[test]
    fn detects_ping_pong() {
        let mut det = default_detector();
        for _ in 0..5 {
            det.record_and_check("grep", &json!({"pattern": "a"}), true);
            det.record_and_check("bash", &json!({"command": "ls"}), true);
        }
        let result = det.record_and_check("grep", &json!({"pattern": "a"}), true);
        assert!(matches!(
            result,
            LoopDetectionResult::Stuck {
                detector: LoopDetectorKind::PingPong,
                ..
            }
        ));
    }

    #[test]
    fn reset_clears_history() {
        let mut det = default_detector();
        let args = json!({"path": "a.txt"});
        for _ in 0..10 {
            det.record_and_check("read_file", &args, true);
        }
        assert_eq!(det.history_len(), 10);
        det.reset();
        assert_eq!(det.history_len(), 0);
    }

    #[test]
    fn global_circuit_breaker() {
        let mut det = default_detector();
        for _ in 0..31 {
            det.record_and_check("bash", &json!({"command": "fail"}), false);
        }
        let result = det.record_and_check("bash", &json!({"command": "fail"}), false);
        assert!(matches!(
            result,
            LoopDetectionResult::Stuck {
                detector: LoopDetectorKind::GlobalCircuitBreaker,
                ..
            }
        ));
    }

    #[test]
    fn loop_guidance_returns_text() {
        let guidance = ToolLoopDetector::tool_loop_guidance("read_file");
        assert!(!guidance.is_empty());
    }

    #[test]
    fn disabled_detector_passes_through() {
        let mut det = ToolLoopDetector::new(LoopDetectionConfig {
            enabled: false,
            ..Default::default()
        });
        let args = json!({"path": "a.txt"});
        for _ in 0..50 {
            let result = det.record_and_check("read_file", &args, true);
            assert!(matches!(result, LoopDetectionResult::NotStuck));
        }
    }
}

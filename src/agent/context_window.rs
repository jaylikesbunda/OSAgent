/// Context window guard: resolves and evaluates context window size limits.
///
/// Ported from OpenClaw's `context-window-guard.ts`.
/// Resolves the effective context window from model config, agent config,
/// or defaults, and provides warn/block thresholds.

/// Hard minimum context window size in tokens.
/// Any model reporting fewer tokens than this is unusable.
pub const CONTEXT_WINDOW_HARD_MIN_TOKENS: usize = 16_000;

/// Warn when context window falls below this threshold.
pub const CONTEXT_WINDOW_WARN_BELOW_TOKENS: usize = 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextWindowSource {
    Model,
    AgentConfig,
    Default,
}

#[derive(Debug, Clone)]
pub struct ContextWindowInfo {
    pub tokens: usize,
    pub source: ContextWindowSource,
}

impl ContextWindowInfo {
    pub fn new(tokens: usize, source: ContextWindowSource) -> Self {
        Self { tokens, source }
    }
}

#[derive(Debug, Clone)]
pub struct ContextWindowGuardResult {
    pub info: ContextWindowInfo,
    pub should_warn: bool,
    pub should_block: bool,
}

impl ContextWindowGuardResult {
    pub fn tokens(&self) -> usize {
        self.info.tokens
    }

    pub fn source(&self) -> ContextWindowSource {
        self.info.source
    }
}

/// Resolve the effective context window size from available sources.
///
/// Priority:
/// 1. `agent_context_tokens` (explicit cap from agent config)
/// 2. `model_context_window` (from model metadata)
/// 3. `default_tokens` fallback
///
/// If `agent_context_tokens` is set and is smaller than the resolved value,
/// it acts as a cap (from "source: agentContextTokens").
pub fn resolve_context_window_info(
    model_context_window: Option<usize>,
    agent_context_tokens: Option<usize>,
    default_tokens: usize,
) -> ContextWindowInfo {
    let base = if let Some(model_tokens) = model_context_window {
        if model_tokens > 0 {
            ContextWindowInfo::new(model_tokens, ContextWindowSource::Model)
        } else {
            ContextWindowInfo::new(default_tokens, ContextWindowSource::Default)
        }
    } else {
        ContextWindowInfo::new(default_tokens, ContextWindowSource::Default)
    };

    if let Some(cap) = agent_context_tokens {
        if cap > 0 && cap < base.tokens {
            return ContextWindowInfo::new(cap, ContextWindowSource::AgentConfig);
        }
    }

    base
}

/// Evaluate whether the context window triggers warning or blocking thresholds.
pub fn evaluate_context_window_guard(
    info: ContextWindowInfo,
    warn_below_tokens: Option<usize>,
    hard_min_tokens: Option<usize>,
) -> ContextWindowGuardResult {
    let warn_below = warn_below_tokens
        .unwrap_or(CONTEXT_WINDOW_WARN_BELOW_TOKENS)
        .max(1);
    let hard_min = hard_min_tokens
        .unwrap_or(CONTEXT_WINDOW_HARD_MIN_TOKENS)
        .max(1);
    let tokens = info.tokens;

    ContextWindowGuardResult {
        info,
        should_warn: tokens > 0 && tokens < warn_below,
        should_block: tokens > 0 && tokens < hard_min,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_from_model_when_available() {
        let info = resolve_context_window_info(Some(128_000), None, 16_000);
        assert_eq!(info.tokens, 128_000);
        assert_eq!(info.source, ContextWindowSource::Model);
    }

    #[test]
    fn falls_back_to_default() {
        let info = resolve_context_window_info(None, None, 16_000);
        assert_eq!(info.tokens, 16_000);
        assert_eq!(info.source, ContextWindowSource::Default);
    }

    #[test]
    fn agent_cap_overrides_when_smaller() {
        let info = resolve_context_window_info(Some(128_000), Some(32_000), 16_000);
        assert_eq!(info.tokens, 32_000);
        assert_eq!(info.source, ContextWindowSource::AgentConfig);
    }

    #[test]
    fn agent_cap_ignored_when_larger() {
        let info = resolve_context_window_info(Some(128_000), Some(256_000), 16_000);
        assert_eq!(info.tokens, 128_000);
        assert_eq!(info.source, ContextWindowSource::Model);
    }

    #[test]
    fn zero_model_falls_back() {
        let info = resolve_context_window_info(Some(0), None, 16_000);
        assert_eq!(info.tokens, 16_000);
        assert_eq!(info.source, ContextWindowSource::Default);
    }

    #[test]
    fn guard_blocks_below_hard_min() {
        let info = ContextWindowInfo::new(8_000, ContextWindowSource::Model);
        let result = evaluate_context_window_guard(info, None, None);
        assert!(result.should_block);
        assert!(result.should_warn);
    }

    #[test]
    fn guard_warns_below_threshold() {
        let info = ContextWindowInfo::new(24_000, ContextWindowSource::Model);
        let result = evaluate_context_window_guard(info, None, None);
        assert!(!result.should_block);
        assert!(result.should_warn);
    }

    #[test]
    fn guard_passes_large_window() {
        let info = ContextWindowInfo::new(128_000, ContextWindowSource::Model);
        let result = evaluate_context_window_guard(info, None, None);
        assert!(!result.should_block);
        assert!(!result.should_warn);
    }

    #[test]
    fn custom_thresholds() {
        let info = ContextWindowInfo::new(50_000, ContextWindowSource::Model);
        let result = evaluate_context_window_guard(info, Some(60_000), Some(40_000));
        assert!(!result.should_block);
        assert!(result.should_warn);
    }
}

use std::fmt;
use std::time::Duration;

use thiserror::Error;

impl From<tokio::task::JoinError> for OSAgentError {
    fn from(e: tokio::task::JoinError) -> Self {
        OSAgentError::Unknown(format!("Task join error: {}", e))
    }
}

/// Structured details extracted from a provider HTTP error response.
///
/// Unlike the free-form `Provider(String)` variant, this carries the
/// HTTP status code, any `Retry-After` hint, and the provider's error
/// code so retry/rate-limit/context classification can be done
/// structurally instead of by sniffing message text.
#[derive(Debug, Clone)]
pub struct ProviderErrorInfo {
    pub message: String,
    pub status_code: Option<u16>,
    pub retry_after: Option<Duration>,
    pub error_code: Option<String>,
}

impl ProviderErrorInfo {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: None,
            retry_after: None,
            error_code: None,
        }
    }
}

impl fmt::Display for ProviderErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status_code {
            Some(status) => write!(f, "{} (status code {})", self.message, status),
            None => write!(f, "{}", self.message),
        }
    }
}

#[derive(Error, Debug)]
pub enum OSAgentError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Provider error: {0}")]
    ProviderStructured(ProviderErrorInfo),

    #[error("Tool execution error: {0}")]
    ToolExecution(String),

    #[error("Tool not allowed: {0}")]
    ToolNotAllowed(String),

    #[error("External path access requires permission: {path}")]
    ExternalPathAccess { path: String },

    #[allow(dead_code)]
    #[error("Invalid parameters: expected {expected}, got {got}")]
    InvalidParameters { expected: String, got: String },

    #[allow(dead_code)]
    #[error("Tool timeout after {seconds}s")]
    ToolTimeout { seconds: u64 },

    #[error("Storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Timeout error")]
    Timeout,

    #[error("Output too large (max {max_bytes} bytes)")]
    OutputTooLarge { max_bytes: usize },

    #[error("Parse error: {0}")]
    Parse(String),

    #[allow(dead_code)]
    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("TTS error: {0}")]
    Tts(String),

    #[error("Workflow error: {0}")]
    Workflow(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, OSAgentError>;

impl OSAgentError {
    pub fn is_rate_limited(&self) -> bool {
        match self {
            Self::ProviderStructured(info) => {
                info.status_code == Some(429)
                    || info
                        .error_code
                        .as_deref()
                        .is_some_and(|code| {
                            let lower = code.to_lowercase();
                            lower.contains("rate_limit")
                                || lower.contains("rate-limit")
                                || lower.contains("too_many")
                                || lower.contains("quota")
                        })
                    || contains_any(
                        &info.message.to_lowercase(),
                        &[
                            "rate limit",
                            "too many requests",
                            "status code 429",
                            "429",
                            "retry-after",
                            "retry after",
                            "quota exceeded",
                            "tokens per min",
                            "requests per min",
                            "request limit",
                            "capacity",
                        ],
                    )
            }
            Self::Provider(message) => contains_any(
                &message.to_lowercase(),
                &[
                    "rate limit",
                    "too many requests",
                    "status code 429",
                    "429",
                    "retry-after",
                    "retry after",
                    "quota exceeded",
                    "tokens per min",
                    "requests per min",
                    "request limit",
                    "capacity",
                ],
            ),
            _ => false,
        }
    }

    pub fn is_context_limit(&self) -> bool {
        match self {
            Self::ProviderStructured(info) => {
                info.status_code == Some(413)
                    || info.error_code.as_deref() == Some("context_length_exceeded")
                    || contains_any(
                        &info.message.to_lowercase(),
                        &[
                            "maximum context length",
                            "max context length",
                            "context window",
                            "requested about",
                            "reduce the length",
                            "prompt is too long",
                            "too many input tokens",
                            "context_length_exceeded",
                            "middle-out",
                            // Bedrock
                            "input is too long for",
                            // xAI/Grok
                            "maximum prompt length is",
                            // GitHub Copilot
                            "exceeds the limit",
                            // llama.cpp
                            "exceeds the available context",
                            // LM Studio
                            "greater than the context",
                            // MiniMax
                            "context window exceeds",
                            // Kimi/Moonshot
                            "exceeded model token limit",
                            // HTTP 413
                            "request entity too large",
                            // vLLM
                            "context length is only",
                            // Mistral
                            "too large for model",
                            // z.ai
                            "model_context_window_exceeded",
                        ],
                    )
                    || (info.message.to_lowercase().contains("input length")
                        && info.message.to_lowercase().contains("exceeds")
                        && info.message.to_lowercase().contains("context length"))
            }
            Self::Provider(message) => {
                let lower = message.to_lowercase();
                contains_any(
                    &lower,
                    &[
                        "maximum context length",
                        "max context length",
                        "context window",
                        "requested about",
                        "reduce the length",
                        "prompt is too long",
                        "too many input tokens",
                        "context_length_exceeded",
                        "middle-out",
                        // Bedrock
                        "input is too long for",
                        // xAI/Grok
                        "maximum prompt length is",
                        // GitHub Copilot
                        "exceeds the limit",
                        // llama.cpp
                        "exceeds the available context",
                        // LM Studio
                        "greater than the context",
                        // MiniMax
                        "context window exceeds",
                        // Kimi/Moonshot
                        "exceeded model token limit",
                        // HTTP 413
                        "request entity too large",
                        // vLLM
                        "context length is only",
                        // Mistral
                        "too large for model",
                        // z.ai
                        "model_context_window_exceeded",
                    ],
                ) || (lower.contains("input length")
                    && lower.contains("exceeds")
                    && lower.contains("context length"))
            }
            _ => false,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Timeout => true,
            Self::Http(error) => error.is_timeout() || error.is_connect() || error.is_request(),
            Self::ProviderStructured(info) => {
                // 5xx are transient server failures and should always be retried,
                // even when the error text doesn't say so explicitly.
                info.status_code.is_some_and(|status| status >= 500)
                    || self.is_rate_limited()
                    || contains_any(
                        &info.message.to_lowercase(),
                        &[
                            "timeout",
                            "timed out",
                            "connection reset",
                            "connection closed",
                            "broken pipe",
                            "temporarily unavailable",
                            "service unavailable",
                            "bad gateway",
                            "gateway timeout",
                            "internal server error",
                            "overloaded",
                            "try again",
                            "status code 500",
                            "status code 502",
                            "status code 503",
                            "status code 504",
                            "status code 524",
                            "(500",
                            "(502",
                            "(503",
                            "(504",
                            "(524",
                        ],
                    )
            }
            Self::Provider(message) => {
                let lower = message.to_lowercase();
                self.is_rate_limited()
                    || contains_any(
                        &lower,
                        &[
                            "timeout",
                            "timed out",
                            "connection reset",
                            "connection closed",
                            "broken pipe",
                            "temporarily unavailable",
                            "service unavailable",
                            "bad gateway",
                            "gateway timeout",
                            "internal server error",
                            "overloaded",
                            "try again",
                            "status code 500",
                            "status code 502",
                            "status code 503",
                            "status code 504",
                            "status code 524",
                            "(500",
                            "(502",
                            "(503",
                            "(504",
                            "(524",
                        ],
                    )
            }
            _ => false,
        }
    }

    /// OpenAI may return 404 for models that are actually available.
    pub fn is_openai_retryable(&self) -> bool {
        match self {
            Self::ProviderStructured(info) => {
                info.status_code == Some(404) || self.is_retryable()
            }
            Self::Provider(message) => {
                let lower = message.to_lowercase();
                lower.contains("status code 404") || lower.contains("(404") || self.is_retryable()
            }
            _ => self.is_retryable(),
        }
    }

    /// The server's requested retry delay, when the provider error carries
    /// a `Retry-After` hint. Capped to avoid pathological sleeps.
    pub fn retry_after_delay(&self) -> Option<Duration> {
        match self {
            Self::ProviderStructured(info) => info
                .retry_after
                .map(|delay| delay.min(Duration::from_secs(MAX_RETRY_AFTER_DELAY_SECS))),
            _ => None,
        }
    }

    pub fn is_recoverable(&self) -> bool {
        self.is_retryable() || self.is_rate_limited()
    }
}

/// Upper bound for honoring a provider's `Retry-After` hint.
pub const MAX_RETRY_AFTER_DELAY_SECS: u64 = 300;

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structured(status: Option<u16>, error_code: Option<&str>, message: &str) -> OSAgentError {
        OSAgentError::ProviderStructured(ProviderErrorInfo {
            message: message.to_string(),
            status_code: status,
            retry_after: None,
            error_code: error_code.map(|code| code.to_string()),
        })
    }

    #[test]
    fn rate_limited_by_status_429() {
        let err = structured(Some(429), None, "Request was throttled");
        assert!(err.is_rate_limited());
        assert!(err.is_retryable());
        assert!(err.is_recoverable());
    }

    #[test]
    fn rate_limited_by_error_code() {
        let err = structured(None, Some("rate_limit_exceeded"), "slow down");
        assert!(err.is_rate_limited());
    }

    #[test]
    fn rate_limited_by_message_text() {
        let err = structured(Some(403), None, "Rate limit reached for tokens per min");
        assert!(err.is_rate_limited());
    }

    #[test]
    fn context_limit_by_status_413() {
        let err = structured(Some(413), None, "Request Entity Too Large");
        assert!(err.is_context_limit());
        assert!(!err.is_retryable());
    }

    #[test]
    fn context_limit_by_error_code() {
        let err = structured(None, Some("context_length_exceeded"), "nope");
        assert!(err.is_context_limit());
    }

    #[test]
    fn retryable_5xx_even_with_unrecognized_message() {
        let err = structured(Some(503), None, "weird unknown wording");
        assert!(err.is_retryable());
    }

    #[test]
    fn retryable_429_always() {
        let err = structured(Some(429), None, "weird unknown wording");
        assert!(err.is_retryable());
    }

    #[test]
    fn not_retryable_4xx_unrecognized() {
        let err = structured(Some(400), None, "weird unknown wording");
        assert!(!err.is_retryable());
    }

    #[test]
    fn openai_404_is_retryable() {
        let err = structured(Some(404), None, "model not found");
        assert!(err.is_openai_retryable());
        assert!(!err.is_retryable());
    }

    #[test]
    fn retry_after_delay_respected_and_capped() {
        let mut info = ProviderErrorInfo::new("slow down");
        info.status_code = Some(429);
        info.retry_after = Some(Duration::from_secs(5));
        let err = OSAgentError::ProviderStructured(info);
        assert_eq!(err.retry_after_delay(), Some(Duration::from_secs(5)));

        let mut info = ProviderErrorInfo::new("slow down");
        info.retry_after = Some(Duration::from_secs(10_000));
        let err = OSAgentError::ProviderStructured(info);
        assert_eq!(
            err.retry_after_delay(),
            Some(Duration::from_secs(MAX_RETRY_AFTER_DELAY_SECS))
        );

        let err = OSAgentError::Provider("rate limit".to_string());
        assert_eq!(err.retry_after_delay(), None);
    }

    #[test]
    fn legacy_string_provider_behavior_preserved() {
        let err = OSAgentError::Provider("API request failed (429): too many requests".to_string());
        assert!(err.is_rate_limited());
        assert!(err.is_retryable());

        let err = OSAgentError::Provider("status code 503".to_string());
        assert!(err.is_retryable());

        let err = OSAgentError::Provider("maximum context length exceeded".to_string());
        assert!(err.is_context_limit());
    }

    #[test]
    fn display_includes_status_code() {
        let err = structured(Some(429), None, "API request failed (429): slow down");
        assert!(err.to_string().contains("429"));
    }
}

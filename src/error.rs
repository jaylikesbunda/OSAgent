use thiserror::Error;

#[derive(Error, Debug)]
pub enum OSAgentError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Tool execution error: {0}")]
    ToolExecution(String),

    #[error("Tool not allowed: {0}")]
    ToolNotAllowed(String),

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
            Self::Provider(message) => contains_any(
                &message.to_lowercase(),
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
                ],
            ),
            _ => false,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Timeout => true,
            Self::Http(error) => error.is_timeout() || error.is_connect() || error.is_request(),
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

    pub fn is_recoverable(&self) -> bool {
        self.is_retryable() || self.is_rate_limited()
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned status {status}: {body}")]
    Provider { status: u16, body: String },
    #[error("tool error: {0}")]
    Tool(String),
    #[error("store error: {0}")]
    Store(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("cancelled")]
    Cancelled,
    #[error("approval channel closed")]
    ApprovalClosed,
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_provider_error() {
        let e = Error::Provider { status: 401, body: "bad key".into() };
        let s = e.to_string();
        assert!(s.contains("401"), "{s}");
        assert!(s.contains("bad key"), "{s}");
    }

    #[test]
    fn from_json_error() {
        let r: std::result::Result<serde_json::Value, _> = serde_json::from_str("{nope");
        let e: Error = r.unwrap_err().into();
        assert!(matches!(e, Error::Json(_)));
    }

    #[test]
    fn cancelled_display() {
        assert!(!Error::Cancelled.to_string().is_empty());
    }
}

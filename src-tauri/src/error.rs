use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git: {0}")]
    Git(String),
    #[error("github: {0}")]
    GitHub(String),
    #[error("version: {0}")]
    Version(String),
    #[error("store: {0}")]
    Store(String),
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.to_string())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Store(e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Other(e.to_string())
    }
}
impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors can carry the URL but never headers, so no token leaks.
        Error::GitHub(e.to_string())
    }
}

// Tauri commands return errors to the UI as plain strings.
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&crate::redact::redact(&self.to_string()))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

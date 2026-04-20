use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, TachyonError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ByteOffset(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineNumber(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: ByteOffset,
    pub end: ByteOffset,
}

impl ByteRange {
    pub fn new(start: ByteOffset, end: ByteOffset) -> Result<Self> {
        if start.0 > end.0 {
            return Err(TachyonError::InvalidByteRange {
                start: start.0,
                end: end.0,
            });
        }
        Ok(Self { start, end })
    }

    pub fn len(self) -> u64 {
        self.end.0 - self.start.0
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    Substring { case_sensitive: bool },
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub pattern: String,
    pub mode: SearchMode,
}

impl SearchQuery {
    pub fn substring(pattern: impl Into<String>, case_sensitive: bool) -> Result<Self> {
        let pattern = pattern.into();
        if pattern.is_empty() {
            return Err(TachyonError::InvalidQuery(
                "substring pattern must not be empty".to_owned(),
            ));
        }
        Ok(Self {
            pattern,
            mode: SearchMode::Substring { case_sensitive },
        })
    }

    pub fn regex(pattern: impl Into<String>) -> Result<Self> {
        let pattern = pattern.into();
        if pattern.is_empty() {
            return Err(TachyonError::InvalidQuery(
                "regex pattern must not be empty".to_owned(),
            ));
        }
        Ok(Self {
            pattern,
            mode: SearchMode::Regex,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub service: String,
    pub name: String,
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Error)]
pub enum TachyonError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid byte range: start={start}, end={end}")]
    InvalidByteRange { start: u64, end: u64 },
    #[error("line out of bounds: requested={requested}, total={total}")]
    LineOutOfBounds { requested: u64, total: u64 },
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("parse error: {0}")]
    Parse(String),
}

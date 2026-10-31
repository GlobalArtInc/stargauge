use chrono::{DateTime, Utc};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApiError {
    #[error("repository not found")]
    NotFound,
    #[error("GitHub rejected the token")]
    BadToken,
    #[error("GitHub rate limit reached")]
    RateLimited {
        reset_at: Option<DateTime<Utc>>,
        secondary: bool,
    },
    #[error("GitHub serves at most {max} pages for this repository, it needs {pages}")]
    TooLarge { pages: u64, max: u64 },
    #[error("{pages} pages needed, only {remaining} requests left in the rate limit budget")]
    BudgetTooSmall {
        pages: u64,
        remaining: u32,
        reset_at: Option<DateTime<Utc>>,
    },
    #[error("network error: {0}")]
    Network(String),
    #[error("GitHub API returned HTTP {status}: {snippet}")]
    Http { status: u16, snippet: String },
    #[error("interrupted")]
    Interrupted,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    Input(String),
    #[error("GitHub API: {0}")]
    Api(#[from] ApiError),
    #[error("I/O error at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("state file {} is invalid: {message}", .path.display())]
    State { path: PathBuf, message: String },
    #[error("config error: {0}")]
    Config(String),
}

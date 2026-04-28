pub mod app;
pub mod git;

pub type AppResult<T> = Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git {command}: {message}")]
    Git { command: String, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("branch diverged from remote")]
    Diverged,

    #[error("no branches found")]
    NoBranches,

    #[error("invalid number from git: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
}

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

impl Error {
    /// True when this wraps a Ctrl+C (`Interrupted`) raised by an interactive
    /// prompt. Callers that otherwise swallow prompt errors must still honour it
    /// so the user can cancel — raw mode delivers Ctrl+C as this error rather
    /// than a SIGINT.
    #[must_use]
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Error::Io(io) if io.kind() == std::io::ErrorKind::Interrupted)
    }
}

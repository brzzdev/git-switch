pub mod app;
pub mod git;

pub type AppResult<T> = Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git {command}: {message}")]
    Git { command: String, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("not on a branch (detached HEAD); nothing to refresh")]
    Detached,

    #[error("branch diverged from remote")]
    Diverged,

    /// `br` was asked to check out a branch git already holds in another
    /// worktree. The message names the path and the verb that does reach it,
    /// because that hand-off is the whole difference between the two verbs.
    #[error(
        "{branch} is checked out at {path}; run `perch {}` to go there",
        app::shell_quote(branch)
    )]
    HeldByWorktree { branch: String, path: String },

    #[error("no branches found")]
    NoBranches,

    /// One of the long spellings dropped at 2.0.0. Worth its own error because
    /// `wt` would otherwise read the retired word as a branch to create.
    #[error("`perch {given}` is gone; use `perch {keep}`")]
    Retired {
        given: &'static str,
        keep: &'static str,
    },

    #[error("invalid number from git: {0}")]
    ParseInt(#[from] std::num::ParseIntError),

    /// A destructive action was declined because its risk could not be shown:
    /// there was no terminal to warn on or ask in, and `--force` wasn't given.
    #[error("{0}")]
    Unconfirmed(String),
}

impl Error {
    /// A retired spelling, named alongside the one that replaced it.
    #[must_use]
    pub fn retired(given: &'static str, keep: &'static str) -> Self {
        Error::Retired { given, keep }
    }

    /// True when this wraps a Ctrl+C (`Interrupted`) raised by an interactive
    /// prompt. Callers that otherwise swallow prompt errors must still honour it
    /// so the user can cancel — raw mode delivers Ctrl+C as this error rather
    /// than a SIGINT.
    #[must_use]
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Error::Io(io) if io.kind() == std::io::ErrorKind::Interrupted)
    }
}

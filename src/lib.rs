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
        app::go_there_argument(branch)
    )]
    HeldByWorktree { branch: String, path: String },

    #[error("no branches found")]
    NoBranches,

    /// One of the `wt` subverb spellings dropped at 2.0.0. Worth its own error
    /// because `wt` would otherwise read the retired word as a branch to create
    /// — which is also why the message has to name the `--` form, the only way
    /// left to reach a branch that really is called `list` or `remove`.
    #[error(
        "`perch wt {word}` is gone; use `perch wt {keep}`, or `perch wt -- {word}` for a branch by that name"
    )]
    Retired {
        word: &'static str,
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
    /// A retired `wt` subverb, named alongside the one that replaced it.
    #[must_use]
    pub fn retired(word: &'static str, keep: &'static str) -> Self {
        Error::Retired { word, keep }
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

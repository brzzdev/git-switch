/// Declares an enum together with the word that selects each variant on the
/// command line, generating `parse` from that same list.
///
/// What this buys is that there is no *second list* to keep in step: the words
/// sit on the variants, so what the dispatcher parses and what the completions
/// subtract come from the declaration itself. The hand-kept array it replaces
/// could be left missing a variant while everything still compiled — the
/// dispatcher's own match was satisfied by the arm alone, and a test that
/// iterated the array could not see the gap — and the word would then be eaten
/// as a verb but offered as a branch, which is the drift [#96] set out to
/// remove.
///
/// It is not a proof: a variant written bare simply has no word, and nothing
/// here objects. That is deliberate, because some variants have none — as
/// [`Verb::Go`](app::Verb::Go) hasn't, being what a bare `perch` already means.
/// What catches a word left off by mistake is the same exhaustive `match` that
/// caught it before.
///
/// [#96]: https://github.com/brzzdev/perch/issues/96
macro_rules! spelled {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident $(= $word:literal)?),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// The variant `word` selects, where it selects one. A word this
            /// rejects is a branch name.
            #[must_use]
            pub fn parse(word: &str) -> Option<Self> {
                match word {
                    $($($word => Some(Self::$variant),)?)+
                    _ => None,
                }
            }
        }
    };
}

pub mod app;
pub mod git;

pub type AppResult<T> = Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git {command}: {message}")]
    Git { command: String, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("invalid `perch br rm` invocation: {0}")]
    BrRmUsage(String),

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

    #[error("{branch} is checked out at {path}; {hint}")]
    HeldForRemoval {
        branch: String,
        path: String,
        hint: String,
    },

    #[error("branch '{branch}' does not exist locally")]
    LocalBranchNotFound { branch: String },

    #[error("no branches found")]
    NoBranches,

    #[error("{branch} has no removable upstream: {reason}")]
    NoRemovableUpstream { branch: String, reason: String },

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

    /// A `wt` creation option was combined with a `wt` subverb.
    #[error("`--no-switch` does not apply to `perch wt {subverb}`")]
    NoSwitchWithSubverb { subverb: String },

    #[error("invalid number from git: {0}")]
    ParseInt(#[from] std::num::ParseIntError),

    #[error("one or more requested removals failed")]
    RemovalFailed,

    /// A destructive action was declined because its risk could not be shown:
    /// there was no terminal to warn on or ask in, and `--force` wasn't given.
    #[error("{0}")]
    Unconfirmed(String),

    /// A branch picked from a row went away between the list being drawn and
    /// the selection being resolved. Its own error because the alternative is
    /// silent: with nothing left to find, the name would otherwise read as a
    /// branch to create, and the user would be handed a fresh branch off the
    /// default in place of the one they chose.
    #[error("{branch} no longer exists; it was deleted after the list was drawn")]
    Vanished { branch: String },
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

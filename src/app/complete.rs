//! What the shell completions offer where a branch name goes.
//!
//! The dispatcher decides which words it reads as commands before it reads one
//! as a branch, so it is the only thing that can say which branches are
//! reachable at which position. Each `--complete` flag names a position, and
//! prints the branches that position will accept — the whole branch set, minus
//! the words its own `match` arm eats first.
//!
//! The subtraction is a table here rather than a list in each completion file,
//! and `dispatch` parses through that same table: a new verb gets a variant, the
//! exhaustive `match` in `main.rs` demands an arm for it, and the completions
//! drop it without being touched.

use std::collections::HashSet;

use crate::{AppResult, git};

/// A word `dispatch` reads as a verb before it reads it as a branch name.
/// `perch -- <name>` is the way past it to a branch spelled the same.
#[derive(Clone, Copy)]
pub enum TopWord {
    Br,
    Wt,
}

impl TopWord {
    const TABLE: [(&'static str, Self); 2] = [("br", Self::Br), ("wt", Self::Wt)];

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        parse(&Self::TABLE, word)
    }
}

/// A word `dispatch_wt` reads as a *Subverb* before it reads it as a branch
/// name. `list` and `remove` are the spellings retired at 2.0.0: they are eaten
/// to be refused, which counts here for the same reason `ls` does — a name
/// completion offers there reaches an error rather than a branch.
#[derive(Clone, Copy)]
pub enum WtWord {
    List,
    Ls,
    Remove,
    Rm,
}

impl WtWord {
    const TABLE: [(&'static str, Self); 4] = [
        ("list", Self::List),
        ("ls", Self::Ls),
        ("remove", Self::Remove),
        ("rm", Self::Rm),
    ];

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        parse(&Self::TABLE, word)
    }
}

fn parse<T: Copy>(table: &[(&'static str, T)], word: &str) -> Option<T> {
    table
        .iter()
        .find(|(spelling, _)| *spelling == word)
        .map(|(_, value)| *value)
}

fn spellings<T>(table: &[(&'static str, T)]) -> HashSet<&'static str> {
    table.iter().map(|(spelling, _)| *spelling).collect()
}

/// Where on the command line the next word would be read as a branch name. One
/// per dispatcher that has such a position — which is all three, since every
/// one of them falls through to a branch.
#[derive(Clone, Copy)]
pub enum Position {
    /// `perch <branch>`, where a *Verb* is read first.
    Top,
    /// `perch br <branch>`, where nothing is.
    Br,
    /// `perch wt <branch>`, where a *Subverb* is read first.
    Wt,
}

impl Position {
    /// The words this position's dispatcher takes for itself.
    fn eaten(self) -> HashSet<&'static str> {
        match self {
            Position::Top => spellings(&TopWord::TABLE),
            // `br` reads everything after it as a branch, so its list is the
            // unfiltered one — which is also what a `--` buys at any level.
            Position::Br => HashSet::new(),
            Position::Wt => spellings(&WtWord::TABLE),
        }
    }
}

/// `perch [br|wt] --complete` — the branches reachable as the next word at
/// `position`, one per line, for the shell completions to offer.
///
/// The set is `local_branches` plus `remote_only_branches`, the same two reads
/// [`build_catalogue`](super::build_catalogue) makes, so what the picker lists
/// and what a named target resolves against is what TAB offers. The `git branch`
/// the completion files used to run saw only the first of the two, which left a
/// remote-only branch accepted on the command line but never completed.
///
/// The verbs and subverbs themselves are left to the completion files, each of
/// which carries a description for them; this answers for branch names alone.
/// A repo with no branches prints nothing and succeeds — there is nothing to
/// complete, which is not an error the way it is for the picker.
pub fn run(position: Position) -> AppResult<()> {
    let remote = git::current_remote(git::current_branch()?.as_deref());
    let local = git::local_branches()?;
    let remote_only = git::remote_only_branches(&local, &remote).unwrap_or_default();

    let eaten = position.eaten();
    for branch in local.iter().chain(&remote_only) {
        if !eaten.contains(branch.as_str()) {
            println!("{branch}");
        }
    }
    Ok(())
}

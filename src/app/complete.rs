//! What the shell completions offer where a branch name goes.
//!
//! The dispatcher decides which words it reads as commands before it reads one
//! as a branch, so it is the only thing that can say which branches are
//! reachable at which position. Every position on the command line answers
//! `--complete` with what it would accept there — worktree names after
//! `wt rm`, branch names everywhere else — and each subtracts the words its own
//! `match` arm takes first.
//!
//! That subtraction is `Verb::spelling` and `wt::Subverb::spelling`, the same
//! reading the dispatcher parses through, rather than a list restated in each
//! completion file. Both are exhaustive matches, so a new verb has to be given
//! a word there, and the arm it needs in `main.rs` is a compile error until it
//! has one. The one step neither the compiler nor a test can force is adding it
//! to `ALL` beside them — miss that and the verb simply never parses, which the
//! dispatcher notices long before a completion does.
//!
//! What the completion files still hold is the positional logic — which is
//! genuinely per-shell — and the verbs and subverbs themselves, each of which
//! carries a description there that no list of words here could supply.

use crate::{AppResult, git};

use super::{Verb, wt};

/// Where on the command line the next word would be read as a branch name,
/// which is the whole of what decides the words eaten before it.
#[derive(Clone, Copy)]
pub enum Position {
    /// `perch <branch>`, where a *Verb* is read first.
    Bare,
    /// `perch br <branch>`. `br` has no *Subverb*s, so nothing is.
    Br,
    /// `perch wt <branch>`, where a *Subverb* is read first.
    Wt,
    /// `perch -- <branch>`, and the same after either verb — `--` has ended
    /// parsing, so nothing is read first.
    ///
    /// Its own position rather than a borrow of [`Position::Br`], which today
    /// eats nothing either: `--` exists precisely to reach a word some position
    /// would otherwise eat, so answering it with a verb's list would silently
    /// start filtering the escape hatch the day that verb gained a subverb.
    Escaped,
}

impl Position {
    /// Whether the dispatcher at this position takes `word` for itself.
    fn eats(self, word: &str) -> bool {
        match self {
            Position::Bare => Verb::parse(word).is_some(),
            Position::Br | Position::Escaped => false,
            Position::Wt => wt::Subverb::parse(word).is_some(),
        }
    }
}

/// `perch [br|wt] [--] --complete` — the branches reachable as the next word at
/// `position`, one per line, for the shell completions to offer.
///
/// The set is `reachable_branches`, the same read the *Catalogue* is built
/// from, so what the picker lists and what a named target resolves against is
/// what TAB offers. The `git branch` the completion files used to run saw only
/// the local half, which left a remote-only branch accepted on the command line
/// but never completed.
///
/// A repo with no branches prints nothing and succeeds, where the picker raises
/// [`Error::NoBranches`](crate::Error::NoBranches) over the same empty read:
/// nothing to complete is an ordinary answer to a TAB, and a completion has
/// nowhere to show an error anyway.
pub fn run(position: Position) -> AppResult<()> {
    let remote = git::current_remote(git::current_branch()?.as_deref());
    let (local, remote_only) = super::reachable_branches(&remote)?;

    // One `write` for the lot: `println!` flushes a line at a time, which in a
    // repo with thousands of refs is thousands of syscalls on a keypress.
    let mut out = String::new();
    for branch in local.iter().chain(&remote_only) {
        if !position.eats(branch) {
            out.push_str(branch);
            out.push('\n');
        }
    }
    print!("{out}");
    Ok(())
}

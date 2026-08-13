//! Every *Marker* glyph the project renders, defined once.
//!
//! The picker rows and `wt ls`'s status column once spelled `●` and `↑N` out
//! separately, which let the two drift on what a glyph means. There is now one
//! vocabulary and they cannot.
//!
//! Sharing the *glyphs* is not sharing the *facts*. `↑` says two different
//! things: in a picker row an *Unmerged* branch, holding commits `git branch -d`
//! would refuse to discard; in `wt ls` a branch merely ahead of its upstream.
//! Neither implies the other, and [ADR
//! 0001](../../docs/adr/0001-warned-means-forceable.md) turns on the difference
//! — reading ahead-of-upstream as unmerged would license forcing over commits
//! nobody was warned about. So they are separate cases that happen to render
//! alike, and passing one where the other belongs is a compile error rather
//! than a review catch.

use std::fmt;

use console::style;

use super::Risk;
use crate::git;

/// The rendering of a *Risk*, or of a worktree's position against its upstream.
/// A marker is a warning, and per ADR 0001 a shown warning is what licenses
/// forcing — so what a glyph means is a fact about the whole project, not about
/// the row that happens to draw it.
#[derive(Clone, Copy)]
pub(crate) enum Marker {
    /// Commits the upstream doesn't have. Says nothing about whether they are
    /// merged, so it never licenses anything; `wt ls` alone draws it.
    AheadOfUpstream(u32),
    /// Commits on the upstream that aren't local yet. Nothing is at risk from
    /// being behind, so this too is `wt ls`'s alone.
    BehindUpstream(u32),
    /// A *Dirty* worktree: uncommitted or untracked changes.
    Dirty,
    /// An *Unmerged* branch, holding commits `git branch -d` would refuse to
    /// discard. `None` where there is no upstream to count against, which
    /// renders as a bare `↑`.
    Unmerged(Option<u32>),
}

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Marker::AheadOfUpstream(n) | Marker::Unmerged(Some(n)) => {
                write!(f, "{}", style(format!("↑{n}")).green())
            }
            Marker::BehindUpstream(n) => write!(f, "{}", style(format!("↓{n}")).red()),
            Marker::Dirty => write!(f, "{}", style("●").yellow()),
            Marker::Unmerged(None) => write!(f, "{}", style("↑").green()),
        }
    }
}

/// Joins markers into a column, space-separated.
fn join(markers: &[Marker]) -> String {
    markers
        .iter()
        .map(Marker::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The markers for a picker row: `●` for a dirty worktree, `↑N` for an unmerged
/// branch. Empty when there is nothing to lose, which is what keeps a non-empty
/// column meaningful — and what withholds the license to force.
pub(crate) fn markers(risk: Risk) -> String {
    let mut marks = Vec::new();
    if risk.dirty {
        marks.push(Marker::Dirty);
    }
    match risk.unmerged {
        Some(git::Unmerged::Ahead(n)) => marks.push(Marker::Unmerged(Some(n))),
        Some(git::Unmerged::NoUpstream) => marks.push(Marker::Unmerged(None)),
        None => {}
    }
    join(&marks)
}

/// The status column for `wt ls`: the same `●` the pickers draw, plus where the
/// worktree sits against its upstream. Empty when the tree is clean and in sync.
pub(crate) fn worktree_status(dirty: bool, ahead: u32, behind: u32) -> String {
    let mut marks = Vec::new();
    if dirty {
        marks.push(Marker::Dirty);
    }
    if ahead > 0 {
        marks.push(Marker::AheadOfUpstream(ahead));
    }
    if behind > 0 {
        marks.push(Marker::BehindUpstream(behind));
    }
    join(&marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strips ANSI styling so assertions read as the user sees the column.
    fn plain(s: &str) -> String {
        console::strip_ansi_codes(s).into_owned()
    }

    /// Dirtiness belongs to the worktree and unmerged commits to the branch, so
    /// a row at risk both ways carries both glyphs.
    #[test]
    fn markers_render_a_dirty_worktree_and_an_unmerged_branch() {
        let risk = Risk {
            dirty: true,
            unmerged: Some(git::Unmerged::Ahead(2)),
        };
        assert_eq!(plain(&markers(risk)), "● ↑2");
    }

    /// With no upstream there is no count to give, but the risk is real — so the
    /// glyph still appears rather than being dropped for want of a number.
    #[test]
    fn unmerged_without_upstream_renders_a_bare_arrow() {
        let risk = Risk {
            dirty: false,
            unmerged: Some(git::Unmerged::NoUpstream),
        };
        assert_eq!(plain(&markers(risk)), "↑");
    }

    /// An empty marker column is what keeps a non-empty one meaningful — and per
    /// ADR 0001 it is also what withholds the license to force.
    #[test]
    fn no_risk_renders_no_markers() {
        assert!(markers(Risk::default()).is_empty());
    }

    /// `wt ls` draws the same glyphs as the pickers, plus the one only it has.
    #[test]
    fn worktree_status_shares_the_picker_glyphs_and_adds_behind() {
        assert_eq!(plain(&worktree_status(true, 2, 3)), "● ↑2 ↓3");
        assert_eq!(plain(&worktree_status(false, 0, 1)), "↓1");
        assert!(worktree_status(false, 0, 0).is_empty());
    }

    /// The two facts `↑` stands for are different judgements, so they are
    /// different cases — but a user reading a column shouldn't have to know
    /// that, and doesn't.
    #[test]
    fn an_unmerged_branch_and_one_merely_ahead_render_alike() {
        assert_eq!(
            plain(&Marker::Unmerged(Some(2)).to_string()),
            plain(&Marker::AheadOfUpstream(2).to_string())
        );
    }
}

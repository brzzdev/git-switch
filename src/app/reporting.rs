//! Every outcome line the destruction flows print, and the warnings they print
//! on the way there.
//!
//! [`removal`](super::removal) decides what happens; this decides how it reads.
//! Both destruction flows — the stale-branch prompt that follows a switch, and
//! `git-switch wt rm` — hand their [`removal::Report`] to [`removal_outcome`]
//! and print what comes back, so the answer never depends on which command you
//! arrived through. The *Marker* glyphs a picker row draws instead belong to
//! [`marker`](super::marker).
//!
//! Nothing here runs a git process or writes to a stream — every function takes
//! values and returns the lines its caller prints — so the wording is asserted
//! against values rather than by running the binary.

use std::path::Path;

use console::{StyledObject, style};

use super::{Risk, display_path, removal, shell_quote};
use crate::git;

/// The glyph fronting a line that reports something going wrong, or a risk about
/// to be taken.
fn warn() -> StyledObject<&'static str> {
    style("!").yellow().bold()
}

/// The glyph fronting a line that reports something done.
fn done() -> StyledObject<&'static str> {
    style("✓").green().bold()
}

/// The risks as they are *shown*, one warning line each — what stands in for the
/// markers when a target was named on the command line and no row was ever
/// drawn. Per [ADR 0001](../../docs/adr/0001-warned-means-forceable.md) this is
/// the warning that licenses the forcing the confirmation then asks about.
pub(crate) fn warnings(risk: Risk, subject: &str, path: &Path) -> Vec<String> {
    describe(risk, subject, path)
        .into_iter()
        .map(|line| format!("{} {line}", warn()))
        .collect()
}

/// One line per risk, in prose. [`warnings`] renders these for the terminal;
/// the non-interactive refusal joins them into its error message instead, which
/// is why the bare wording is worth having on its own. `subject` names what the
/// risk is attached to.
pub(crate) fn describe(risk: Risk, subject: &str, path: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    if risk.dirty {
        lines.push(format!("{} has uncommitted changes", display_path(path)));
    }
    match risk.unmerged {
        Some(git::Unmerged::Ahead(n)) => {
            lines.push(format!("{subject} has {n} unmerged commit(s)"));
        }
        Some(git::Unmerged::NoUpstream) => {
            lines.push(format!("{subject} has unmerged commits and no upstream"));
        }
        None => {}
    }
    lines
}

/// What a removal did, as the lines to print — the target says which steps could
/// have run, and the report says what each one did.
///
/// A branch kept for unmerged commits reads differently from one whose delete
/// failed, a failure carries git's own reason, and a kept branch carries the
/// `git branch -D` hint for forcing it by hand. The match is exhaustive in both
/// directions: a new outcome has to be worded rather than falling through to a
/// line that claims something else happened.
pub(crate) fn removal_outcome(report: &removal::Report<'_>) -> Vec<String> {
    match report.target {
        removal::Target::Branch { name } => report
            .branch
            .iter()
            .map(|outcome| branch_line("", name, outcome))
            .collect(),

        removal::Target::Held { name, path } => match &report.worktree {
            // A worktree that refused took the branch step with it, so say why
            // and stop — the branch is still there, holding whatever it held.
            Some(git::WorktreeRemoveOutcome::Failed(detail)) => {
                worktree_failure(path, Some(name), detail)
            }
            Some(git::WorktreeRemoveOutcome::Removed) => {
                // The worktree goes first, so one that went is reported
                // alongside whatever the branch did afterwards.
                let prefix = format!("removed worktree at {}, ", display_path(path));
                report
                    .branch
                    .iter()
                    .map(|outcome| branch_line(&prefix, name, outcome))
                    .collect()
            }
            // Unreachable: [`removal::remove`] always runs the worktree step for
            // a target that names one. Saying nothing is the safe reading if
            // that ever changes — better silence than a line claiming a removal
            // that never happened.
            None => Vec::new(),
        },

        removal::Target::Worktree { path } => match &report.worktree {
            Some(git::WorktreeRemoveOutcome::Failed(detail)) => {
                worktree_failure(path, None, detail)
            }
            // A detached worktree goes alone: there was no branch step to report.
            Some(git::WorktreeRemoveOutcome::Removed) => vec![format!(
                "{} removed worktree at {}",
                done(),
                display_path(path),
            )],
            // Unreachable, for the same reason as the `Held` arm above.
            None => Vec::new(),
        },
    }
}

/// A worktree that git refused to remove, with its own reason indented beneath.
/// `branch` names the branch left alone, where the worktree held one.
fn worktree_failure(path: &Path, branch: Option<&str>, detail: &str) -> Vec<String> {
    let at = display_path(path);
    let headline = match branch {
        Some(branch) => format!(
            "{} failed to remove the worktree at {at}; leaving {branch} alone:",
            warn(),
        ),
        None => format!("{} failed to remove the worktree at {at}:", warn()),
    };
    std::iter::once(headline)
        .chain(detail.lines().map(|line| format!("  {line}")))
        .collect()
}

/// What became of the branch, as one line. `prefix` carries the worktree that
/// already went, so success and failure both report it.
fn branch_line(prefix: &str, branch: &str, outcome: &git::BranchDeleteOutcome) -> String {
    match outcome {
        git::BranchDeleteOutcome::Deleted => {
            format!("{} {prefix}deleted {branch}", done())
        }
        git::BranchDeleteOutcome::NotMerged => format!(
            "{} {prefix}kept {branch} with unmerged commits \
             (run `git branch -D -- {}` to force-delete)",
            warn(),
            shell_quote(branch),
        ),
        git::BranchDeleteOutcome::Failed(detail) => {
            format!("{} {prefix}could not delete {branch}: {detail}", warn())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strips ANSI styling so assertions read as the user sees the line.
    fn plain(s: &str) -> String {
        console::strip_ansi_codes(s).into_owned()
    }

    fn plain_all(lines: &[String]) -> Vec<String> {
        lines.iter().map(|l| plain(l)).collect()
    }

    fn held() -> removal::Target<'static> {
        removal::Target::Held {
            name: "fix/typo",
            path: Path::new("/tmp/wt"),
        }
    }

    fn report(
        target: removal::Target<'_>,
        worktree: Option<git::WorktreeRemoveOutcome>,
        branch: Option<git::BranchDeleteOutcome>,
    ) -> removal::Report<'_> {
        removal::Report {
            target,
            worktree,
            branch,
        }
    }

    /// A branch no worktree held has no worktree step, so nothing is claimed
    /// about one.
    #[test]
    fn a_branch_that_went_alone_names_only_itself() {
        let report = report(
            removal::Target::Branch { name: "fix/typo" },
            None,
            Some(git::BranchDeleteOutcome::Deleted),
        );
        assert_eq!(plain_all(&removal_outcome(&report)), ["✓ deleted fix/typo"]);
    }

    /// The worktree goes first, so a removal that happened is reported alongside
    /// whatever the branch did afterwards rather than on a line of its own.
    #[test]
    fn a_worktree_that_went_with_its_branch_is_named_too() {
        let report = report(
            held(),
            Some(git::WorktreeRemoveOutcome::Removed),
            Some(git::BranchDeleteOutcome::Deleted),
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            ["✓ removed worktree at /tmp/wt, deleted fix/typo"]
        );
    }

    /// A detached worktree has no branch step, so nothing is claimed about one.
    #[test]
    fn a_detached_worktree_goes_alone() {
        let report = report(
            removal::Target::Worktree {
                path: Path::new("/tmp/wt"),
            },
            Some(git::WorktreeRemoveOutcome::Removed),
            None,
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            ["✓ removed worktree at /tmp/wt"]
        );
    }

    /// Kept and failed are different answers: this one says the branch is still
    /// there on purpose, and hands over the command that would force it.
    #[test]
    fn a_kept_branch_carries_the_hint_for_forcing_it() {
        let report = report(
            removal::Target::Branch {
                name: "spike/abandoned",
            },
            None,
            Some(git::BranchDeleteOutcome::NotMerged),
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            ["! kept spike/abandoned with unmerged commits \
                 (run `git branch -D -- spike/abandoned` to force-delete)"]
        );
    }

    /// The hint is meant to be pasted into a shell, and git allows `$` and
    /// backticks in a ref name — so the name has to survive the paste as a
    /// literal rather than running.
    #[test]
    fn a_branch_name_that_could_run_as_a_command_is_quoted() {
        let report = report(
            removal::Target::Branch {
                name: "topic$(touch${IFS}/tmp/pwned)",
            },
            None,
            Some(git::BranchDeleteOutcome::NotMerged),
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            [
                "! kept topic$(touch${IFS}/tmp/pwned) with unmerged commits \
                 (run `git branch -D -- 'topic$(touch${IFS}/tmp/pwned)'` to force-delete)"
            ]
        );
    }

    /// git knows why it refused and the user doesn't, so its reason is passed
    /// through rather than summarised away.
    #[test]
    fn a_failed_delete_surfaces_gits_reason() {
        let report = report(
            removal::Target::Branch { name: "old/thing" },
            None,
            Some(git::BranchDeleteOutcome::Failed(
                "error: cannot delete branch".into(),
            )),
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            ["! could not delete old/thing: error: cannot delete branch"]
        );
    }

    /// Git's reason is indented beneath the headline, one line per line of it.
    #[test]
    fn a_refusing_worktree_says_which_branch_it_kept_alive() {
        let report = report(
            held(),
            Some(git::WorktreeRemoveOutcome::Failed(
                "fatal: is locked\nreason: in use".into(),
            )),
            None,
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            [
                "! failed to remove the worktree at /tmp/wt; leaving fix/typo alone:",
                "  fatal: is locked",
                "  reason: in use",
            ]
        );
    }

    /// With no branch to keep alive, the headline says only what refused.
    #[test]
    fn a_refusing_detached_worktree_names_no_branch() {
        let report = report(
            removal::Target::Worktree {
                path: Path::new("/tmp/wt"),
            },
            Some(git::WorktreeRemoveOutcome::Failed(
                "fatal: is locked".into(),
            )),
            None,
        );
        assert_eq!(
            plain_all(&removal_outcome(&report)),
            [
                "! failed to remove the worktree at /tmp/wt:",
                "  fatal: is locked",
            ]
        );
    }

    /// The risk is attached to two different things, so each line names the one
    /// it belongs to: the worktree by path, the branch by name.
    #[test]
    fn describe_names_the_worktree_by_path_and_the_branch_by_name() {
        let risk = Risk {
            dirty: true,
            unmerged: Some(git::Unmerged::Ahead(2)),
        };
        assert_eq!(
            describe(risk, "feature", Path::new("/tmp/wt")),
            [
                "/tmp/wt has uncommitted changes",
                "feature has 2 unmerged commit(s)",
            ]
        );
    }

    #[test]
    fn describe_says_when_there_is_no_upstream_to_count_against() {
        let risk = Risk {
            dirty: false,
            unmerged: Some(git::Unmerged::NoUpstream),
        };
        assert_eq!(
            describe(risk, "feature", Path::new("/tmp/wt")),
            ["feature has unmerged commits and no upstream"]
        );
    }

    /// The warning the user is shown is the whole line, glyph included — the
    /// caller prints it rather than finishing it off.
    #[test]
    fn warnings_front_each_risk_with_the_warning_glyph() {
        let risk = Risk {
            dirty: true,
            unmerged: Some(git::Unmerged::Ahead(2)),
        };
        assert_eq!(
            plain_all(&warnings(risk, "feature", Path::new("/tmp/wt"))),
            [
                "! /tmp/wt has uncommitted changes",
                "! feature has 2 unmerged commit(s)",
            ]
        );
    }
}

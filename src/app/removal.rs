//! Destroying a branch, a worktree, or a branch together with the worktree
//! holding it — the one place the rules of [ADR
//! 0001](../../docs/adr/0001-warned-means-forceable.md) live.
//!
//! Both destruction flows — the stale-branch prompt that follows a switch, and
//! `git-switch wt rm` — come through [`remove`]. It performs the steps in the
//! only order that works, forces only what its [`License`] covers, and returns
//! what happened. It prints nothing: the wording belongs to the call sites.

use std::path::{Path, PathBuf};

use super::Risk;
use crate::{AppResult, git};

/// What is being removed. Every case names something real, so "neither a branch
/// nor a worktree" cannot be asked for.
pub(crate) enum Target {
    /// A branch no worktree holds.
    Branch { name: String },
    /// A branch and the worktree *holding* it, which go together.
    Held {
        name: String,
        worktree: git::Worktree,
    },
    /// A worktree with no branch to delete alongside it (a detached one).
    Worktree { worktree: git::Worktree },
}

impl Target {
    fn branch(&self) -> Option<&str> {
        match self {
            Target::Branch { name } | Target::Held { name, .. } => Some(name),
            Target::Worktree { .. } => None,
        }
    }

    fn worktree(&self) -> Option<&git::Worktree> {
        match self {
            Target::Branch { .. } => None,
            Target::Held { worktree, .. } | Target::Worktree { worktree } => Some(worktree),
        }
    }
}

/// What licenses forcing, per ADR 0001: a warning the user has already seen, or
/// an explicit `--force`. It has no public fields and exactly two constructors,
/// so forcing something nobody was warned about is unrepresentable rather than
/// merely against convention.
#[derive(Clone, Copy)]
pub(crate) struct License {
    worktree: bool,
    branch: bool,
}

impl License {
    /// The markers the user was shown: a *dirty* worktree licenses discarding
    /// its files, an *unmerged* branch its commits, and nothing licenses
    /// anything else. A risk that arose after the markers were drawn is absent
    /// here, so git's own guard refuses instead.
    pub(crate) fn shown(risk: Risk) -> Self {
        Self {
            worktree: risk.dirty,
            branch: risk.unmerged.is_some(),
        }
    }

    /// `wt rm --force`, blanket over both steps.
    pub(crate) fn forced() -> Self {
        Self {
            worktree: true,
            branch: true,
        }
    }
}

/// What happened, one field per step. `None` means the step never ran: because
/// the target had nothing for it to act on, or — for the branch — because the
/// worktree refused to go and left it alone.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) worktree: Option<git::WorktreeRemoveOutcome>,
    pub(crate) branch: Option<git::BranchDeleteOutcome>,
}

/// The two git operations a removal performs. Putting them behind a trait lets
/// the ordering and licensing rules be driven by scripted outcomes in tests,
/// exactly as the key source drives the interactive pickers; [`GitSteps`] is the
/// real implementation.
pub(crate) trait Steps {
    fn remove_worktree(
        &mut self,
        path: &Path,
        force: bool,
    ) -> AppResult<git::WorktreeRemoveOutcome>;

    fn delete_branch(&mut self, branch: &str, force: bool) -> AppResult<git::BranchDeleteOutcome>;
}

/// The real steps, run against the repo on disk.
///
/// It carries the main worktree so no call site has to remember to: `git branch
/// -d` judges merged-ness against the HEAD it runs under, and the main worktree
/// is both where the markers were measured from and the one worktree that can
/// never be the one being removed.
pub(crate) struct GitSteps {
    main: Option<PathBuf>,
}

impl GitSteps {
    pub(crate) fn at_main(main: Option<&Path>) -> Self {
        Self {
            main: main.map(Path::to_path_buf),
        }
    }
}

impl Steps for GitSteps {
    fn remove_worktree(
        &mut self,
        path: &Path,
        force: bool,
    ) -> AppResult<git::WorktreeRemoveOutcome> {
        git::worktree_remove(path, force)
    }

    fn delete_branch(&mut self, branch: &str, force: bool) -> AppResult<git::BranchDeleteOutcome> {
        let dir = self.main.as_deref();
        if force {
            git::force_delete_branch(dir, branch)
        } else {
            git::delete_branch_if_merged(dir, branch)
        }
    }
}

/// Removes `target`, forcing only what `license` covers.
///
/// The worktree goes first — git will not delete a branch something still holds
/// — and one that refuses to go leaves its branch alone, which the returned
/// [`Report`] shows as an absent branch step. Git refusing is a value either
/// way; only a git process that cannot be spawned is an error.
pub(crate) fn remove(
    target: &Target,
    license: License,
    steps: &mut impl Steps,
) -> AppResult<Report> {
    let mut report = Report::default();

    if let Some(worktree) = target.worktree() {
        let outcome = steps.remove_worktree(&worktree.path, license.worktree)?;
        let refused = !matches!(outcome, git::WorktreeRemoveOutcome::Removed);
        report.worktree = Some(outcome);
        if refused {
            return Ok(report);
        }
    }

    if let Some(branch) = target.branch() {
        report.branch = Some(steps.delete_branch(branch, license.branch)?);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One recorded call, as the rules care about it: which step, and whether it
    /// was forced.
    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        RemoveWorktree { force: bool },
        DeleteBranch { force: bool },
    }

    /// Runs the steps from scripted outcomes and records what it was asked to
    /// do, so ordering and licensing can be asserted without a repo on disk.
    struct FakeSteps {
        worktree: git::WorktreeRemoveOutcome,
        branch: git::BranchDeleteOutcome,
        calls: Vec<Call>,
    }

    impl FakeSteps {
        /// Both steps succeed; tests that care about an outcome override it.
        fn new() -> Self {
            Self {
                worktree: git::WorktreeRemoveOutcome::Removed,
                branch: git::BranchDeleteOutcome::Deleted,
                calls: Vec::new(),
            }
        }
    }

    impl Steps for FakeSteps {
        fn remove_worktree(
            &mut self,
            _path: &Path,
            force: bool,
        ) -> AppResult<git::WorktreeRemoveOutcome> {
            self.calls.push(Call::RemoveWorktree { force });
            Ok(self.worktree.clone())
        }

        fn delete_branch(
            &mut self,
            _branch: &str,
            force: bool,
        ) -> AppResult<git::BranchDeleteOutcome> {
            self.calls.push(Call::DeleteBranch { force });
            Ok(self.branch.clone())
        }
    }

    fn held() -> Target {
        Target::Held {
            name: "feature".to_string(),
            worktree: git::Worktree {
                path: PathBuf::from("/tmp/wt"),
                branch: Some("feature".to_string()),
                is_main: false,
                prunable: false,
            },
        }
    }

    /// Git will not delete a branch a worktree still holds, so the order is not
    /// a preference.
    #[test]
    fn the_worktree_goes_before_the_branch() {
        let mut steps = FakeSteps::new();
        remove(&held(), License::shown(Risk::default()), &mut steps).expect("no git to fail");
        assert_eq!(
            steps.calls,
            vec![
                Call::RemoveWorktree { force: false },
                Call::DeleteBranch { force: false },
            ]
        );
    }

    /// A locked worktree, say: deleting its branch would leave a directory with
    /// nothing behind it.
    #[test]
    fn a_worktree_that_refuses_leaves_its_branch_alone() {
        let mut steps = FakeSteps::new();
        steps.worktree = git::WorktreeRemoveOutcome::Failed("locked".to_string());
        let report = remove(&held(), License::forced(), &mut steps).expect("no git to fail");
        assert_eq!(steps.calls, vec![Call::RemoveWorktree { force: true }]);
        assert!(matches!(
            report.worktree,
            Some(git::WorktreeRemoveOutcome::Failed(_))
        ));
        assert!(
            report.branch.is_none(),
            "no branch step ran, got: {:?}",
            report.branch
        );
    }

    /// ADR 0001 made structural: a `●` licenses discarding files and an `↑N`
    /// licenses discarding commits, each on its own.
    #[test]
    fn a_license_from_markers_forces_only_what_was_marked() {
        let mut dirty_only = FakeSteps::new();
        let risk = Risk {
            dirty: true,
            unmerged: None,
        };
        remove(&held(), License::shown(risk), &mut dirty_only).expect("no git to fail");
        assert_eq!(
            dirty_only.calls,
            vec![
                Call::RemoveWorktree { force: true },
                Call::DeleteBranch { force: false },
            ]
        );

        let mut unmerged_only = FakeSteps::new();
        let risk = Risk {
            dirty: false,
            unmerged: Some(git::Unmerged::Ahead(2)),
        };
        remove(&held(), License::shown(risk), &mut unmerged_only).expect("no git to fail");
        assert_eq!(
            unmerged_only.calls,
            vec![
                Call::RemoveWorktree { force: false },
                Call::DeleteBranch { force: true },
            ]
        );
    }

    /// `wt rm --force` stays blanket over both steps.
    #[test]
    fn an_explicit_force_covers_both_steps() {
        let mut steps = FakeSteps::new();
        remove(&held(), License::forced(), &mut steps).expect("no git to fail");
        assert_eq!(
            steps.calls,
            vec![
                Call::RemoveWorktree { force: true },
                Call::DeleteBranch { force: true },
            ]
        );
    }

    /// Each step reports itself, and a step that never ran is absent rather than
    /// dressed up as a failure.
    #[test]
    fn the_report_carries_each_step_that_ran() {
        let mut steps = FakeSteps::new();
        steps.branch = git::BranchDeleteOutcome::NotMerged;
        let report =
            remove(&held(), License::shown(Risk::default()), &mut steps).expect("no git to fail");
        assert!(matches!(
            report.worktree,
            Some(git::WorktreeRemoveOutcome::Removed)
        ));
        assert!(matches!(
            report.branch,
            Some(git::BranchDeleteOutcome::NotMerged)
        ));

        let mut steps = FakeSteps::new();
        steps.branch = git::BranchDeleteOutcome::Failed("in use".to_string());
        let branch_only = Target::Branch {
            name: "feature".to_string(),
        };
        let report = remove(&branch_only, License::shown(Risk::default()), &mut steps)
            .expect("no git to fail");
        assert!(report.worktree.is_none(), "no worktree to remove");
        assert!(matches!(
            report.branch,
            Some(git::BranchDeleteOutcome::Failed(_))
        ));

        let mut steps = FakeSteps::new();
        let detached = Target::Worktree {
            worktree: git::Worktree {
                path: PathBuf::from("/tmp/wt"),
                branch: None,
                is_main: false,
                prunable: false,
            },
        };
        let report = remove(&detached, License::forced(), &mut steps).expect("no git to fail");
        assert!(matches!(
            report.worktree,
            Some(git::WorktreeRemoveOutcome::Removed)
        ));
        assert!(report.branch.is_none(), "no branch to delete");
    }

    /// The constructors are the whole of the rule, so they are asserted
    /// directly.
    #[test]
    fn the_license_constructors_say_what_they_cover() {
        let nothing = License::shown(Risk::default());
        assert!(!nothing.worktree && !nothing.branch);

        let both = License::shown(Risk {
            dirty: true,
            unmerged: Some(git::Unmerged::NoUpstream),
        });
        assert!(both.worktree && both.branch);

        let forced = License::forced();
        assert!(forced.worktree && forced.branch);
    }
}

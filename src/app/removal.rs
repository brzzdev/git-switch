//! Destroying a branch, a worktree, or a branch together with the worktree
//! holding it — the one place the rules of [ADR
//! 0001](../../docs/adr/0001-warned-means-forceable.md) live.
//!
//! Both destruction flows — the stale-branch prompt that follows a switch, and
//! `git-switch wt rm` — come through [`remove`]. It performs the steps in the
//! only order that works, forces only what its [`License`] covers, and returns
//! what happened. It prints nothing: the wording belongs to
//! [`reporting`](super::reporting), which the [`Report`] is handed to whole.

use std::path::{Path, PathBuf};

use super::Risk;
use crate::{AppResult, git};

/// What is being removed. Every case names something real, so "neither a branch
/// nor a worktree" cannot be asked for. It borrows from the row or worktree the
/// caller is already holding to render the outcome from, and travels on in the
/// [`Report`] so the wording never has to be told twice which steps could run.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Target<'a> {
    /// A branch no worktree holds.
    Branch { name: &'a str },
    /// A branch and the worktree *holding* it, which go together.
    Held { name: &'a str, path: &'a Path },
    /// A worktree with no branch to delete alongside it (a detached one).
    Worktree { path: &'a Path },
}

impl<'a> Target<'a> {
    fn branch(self) -> Option<&'a str> {
        match self {
            Target::Branch { name } | Target::Held { name, .. } => Some(name),
            Target::Worktree { .. } => None,
        }
    }

    fn path(self) -> Option<&'a Path> {
        match self {
            Target::Branch { .. } => None,
            Target::Held { path, .. } | Target::Worktree { path } => Some(path),
        }
    }
}

/// The branch half of a [`License`]: on what authority the delete may discard
/// commits. The worktree half needs no such thing — only a warning ever licenses
/// discarding files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchLicense {
    /// Nothing licenses it, so git's own guard decides.
    None,
    /// Licensed outright, on something already settled before the delete began:
    /// a *Marker* the user has seen, or an explicit `--force`. The two are
    /// distinct sources of *License* in the glossary and this does not conflate
    /// them — it says only that neither is conditional on anything still true.
    Outright,
    /// Proof that the branch is *Equivalent*, which is conditional: per [ADR
    /// 0005](../../docs/adr/0005-proof-of-equivalence-is-a-license.md) a license
    /// covers what was established and nothing more, so it lapses the moment
    /// either the branch or the anchor moves off what was proven.
    Proven(git::Proof),
}

/// What licenses forcing: a warning the user has already seen (ADR 0001), proof
/// that a branch is *Equivalent* (ADR 0005), or an explicit `--force`. It has no
/// public fields and exactly three constructors, so forcing something nobody was
/// warned about and nothing was proven of is unrepresentable rather than merely
/// against convention.
pub(crate) struct License {
    worktree: bool,
    branch: BranchLicense,
}

impl License {
    /// The risk the user was warned about — as row markers in a picker, or as
    /// the confirmation that stands in for them where a target was named on the
    /// command line. A *dirty* worktree licenses discarding its files, an
    /// *unmerged* branch its commits, and nothing licenses anything else: a risk
    /// that arose after the warning was given is absent here, so git's own guard
    /// refuses instead.
    pub(crate) fn shown(risk: Risk) -> Self {
        Self {
            worktree: risk.dirty,
            branch: if risk.unmerged.is_some() {
                BranchLicense::Outright
            } else {
                BranchLicense::None
            },
        }
    }

    /// A branch proven *Equivalent*, with the worktree half still answering to
    /// the risk shown. Nothing was warned of about the branch — that is the
    /// point of the proof — so the proof stands in for the marker, and only for
    /// as long as what it was established on still holds.
    pub(crate) fn proven(risk: Risk, proof: &git::Proof) -> Self {
        Self {
            branch: BranchLicense::Proven(proof.clone()),
            ..Self::shown(risk)
        }
    }

    /// `wt rm --force`, blanket over both steps.
    pub(crate) fn forced() -> Self {
        Self {
            worktree: true,
            branch: BranchLicense::Outright,
        }
    }
}

/// What happened, one field per step. `None` means the step never ran: because
/// the target had nothing for it to act on, or — for the branch — because the
/// worktree refused to go and left it alone. It carries the [`Target`] so
/// [`reporting`](super::reporting) can word the outcome from the report alone.
#[derive(Debug)]
pub(crate) struct Report<'a> {
    pub(crate) target: Target<'a>,
    pub(crate) worktree: Option<git::WorktreeRemoveOutcome>,
    pub(crate) branch: Option<git::BranchDeleteOutcome>,
}

impl Report<'_> {
    /// Whether the worktree itself went. False covers both a target that never
    /// had one and one git refused to remove — in either case there is no
    /// directory to report gone.
    pub(crate) fn worktree_removed(&self) -> bool {
        matches!(self.worktree, Some(git::WorktreeRemoveOutcome::Removed))
    }
}

/// What a removal asks of git: the two operations it performs, and the one
/// question it asks before deciding to force. Putting them behind a trait lets
/// the ordering and licensing rules be driven by scripted outcomes in tests,
/// exactly as the key source drives the interactive pickers; [`GitSteps`] is the
/// real implementation.
pub(crate) trait Steps {
    /// What `refname` points at *now* — asked to check a proof still covers what
    /// it was established on, so it must read the repo rather than anything
    /// remembered.
    fn resolve(&mut self, refname: &str) -> Option<String>;

    fn delete_branch(&mut self, branch: &str, force: bool) -> AppResult<git::BranchDeleteOutcome>;

    fn remove_worktree(
        &mut self,
        path: &Path,
        force: bool,
    ) -> AppResult<git::WorktreeRemoveOutcome>;
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
    fn resolve(&mut self, refname: &str) -> Option<String> {
        git::resolve(self.main.as_deref(), refname)
    }

    fn delete_branch(&mut self, branch: &str, force: bool) -> AppResult<git::BranchDeleteOutcome> {
        let dir = self.main.as_deref();
        if force {
            git::force_delete_branch(dir, branch)
        } else {
            git::delete_branch_if_merged(dir, branch)
        }
    }

    fn remove_worktree(
        &mut self,
        path: &Path,
        force: bool,
    ) -> AppResult<git::WorktreeRemoveOutcome> {
        git::worktree_remove(path, force)
    }
}

/// Removes `target`, forcing only what `license` covers.
///
/// The worktree goes first — git will not delete a branch something still holds
/// — and one that refuses to go leaves its branch alone, which the returned
/// [`Report`] shows as an absent branch step. Git refusing is a value either
/// way; only a git process that cannot be spawned is an error.
pub(crate) fn remove<'a>(
    target: Target<'a>,
    license: &License,
    steps: &mut impl Steps,
) -> AppResult<Report<'a>> {
    let mut report = Report {
        target,
        worktree: None,
        branch: None,
    };

    if let Some(path) = target.path() {
        report.worktree = Some(steps.remove_worktree(path, license.worktree)?);
        if !report.worktree_removed() {
            return Ok(report);
        }
    }

    if let Some(branch) = target.branch() {
        // A proof is re-checked rather than trusted. It was established on two
        // things — where the branch stood and what the anchor held — and a
        // license covers both or neither: a branch that moved has work nobody
        // proved, and an anchor rewound out from under it (by a removal hook on
        // an earlier row, say) no longer holds the content that made the branch
        // safe to discard. Either way it falls to `-d` and meets git's own
        // guard, exactly as an unmarked worktree does.
        let force = match &license.branch {
            BranchLicense::None => false,
            BranchLicense::Outright => true,
            BranchLicense::Proven(proof) => {
                steps.resolve(&format!("refs/heads/{branch}")).as_ref() == Some(&proof.tip)
                    && steps.resolve(&proof.anchor_ref).as_ref() == Some(&proof.anchor_tip)
            }
        };
        report.branch = Some(steps.delete_branch(branch, force)?);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::mem::discriminant;

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
        /// What the refs read when [`remove`] asks. The proof tests move one out
        /// from under the license and leave the other where it was.
        refs: HashMap<String, String>,
        calls: Vec<Call>,
    }

    impl FakeSteps {
        /// Both steps succeed and both refs sit where [`proof`] says; tests that
        /// care about an outcome, or about something moving, override it.
        fn new() -> Self {
            Self {
                worktree: git::WorktreeRemoveOutcome::Removed,
                branch: git::BranchDeleteOutcome::Deleted,
                refs: HashMap::from([
                    ("refs/heads/feature".to_string(), PROVEN_TIP.to_string()),
                    (ANCHOR_REF.to_string(), PROVEN_ANCHOR.to_string()),
                ]),
                calls: Vec::new(),
            }
        }

        /// Move a ref off what the proof was established on.
        fn moved(mut self, refname: &str) -> Self {
            self.refs.insert(refname.to_string(), "moved".to_string());
            self
        }
    }

    const ANCHOR_REF: &str = "refs/heads/main";
    /// What the proof tests establish their license on.
    const PROVEN_TIP: &str = "abc123";
    const PROVEN_ANCHOR: &str = "def456";

    fn proof() -> git::Proof {
        git::Proof {
            anchor_ref: ANCHOR_REF.to_string(),
            anchor_tip: PROVEN_ANCHOR.to_string(),
            tip: PROVEN_TIP.to_string(),
        }
    }

    impl Steps for FakeSteps {
        fn resolve(&mut self, refname: &str) -> Option<String> {
            self.refs.get(refname).cloned()
        }

        fn delete_branch(
            &mut self,
            _branch: &str,
            force: bool,
        ) -> AppResult<git::BranchDeleteOutcome> {
            self.calls.push(Call::DeleteBranch { force });
            Ok(self.branch.clone())
        }

        fn remove_worktree(
            &mut self,
            _path: &Path,
            force: bool,
        ) -> AppResult<git::WorktreeRemoveOutcome> {
            self.calls.push(Call::RemoveWorktree { force });
            Ok(self.worktree.clone())
        }
    }

    fn held() -> Target<'static> {
        Target::Held {
            name: "feature",
            path: Path::new("/tmp/wt"),
        }
    }

    /// Git will not delete a branch a worktree still holds, so the order is not
    /// a preference.
    #[test]
    fn the_worktree_goes_before_the_branch() {
        let mut steps = FakeSteps::new();
        remove(held(), &License::shown(Risk::default()), &mut steps).expect("no git to fail");
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
        let report = remove(held(), &License::forced(), &mut steps).expect("no git to fail");
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
        remove(held(), &License::shown(risk), &mut dirty_only).expect("no git to fail");
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
        remove(held(), &License::shown(risk), &mut unmerged_only).expect("no git to fail");
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
        remove(held(), &License::forced(), &mut steps).expect("no git to fail");
        assert_eq!(
            steps.calls,
            vec![
                Call::RemoveWorktree { force: true },
                Call::DeleteBranch { force: true },
            ]
        );
    }

    /// Every combination of step outcomes, plus the two targets that skip a step
    /// entirely — an absent step means there was no such step, never a failure
    /// dressed up as one. A removed worktree is paired with each branch outcome
    /// in turn; a worktree that refused is
    /// [`a_worktree_that_refuses_leaves_its_branch_alone`], which owns that case.
    #[test]
    fn the_report_carries_each_step_that_ran() {
        for outcome in [
            git::BranchDeleteOutcome::Deleted,
            git::BranchDeleteOutcome::NotMerged,
            git::BranchDeleteOutcome::Failed("in use".to_string()),
        ] {
            let mut steps = FakeSteps::new();
            steps.branch = outcome.clone();
            let report = remove(held(), &License::shown(Risk::default()), &mut steps)
                .expect("no git to fail");
            assert!(
                matches!(report.worktree, Some(git::WorktreeRemoveOutcome::Removed)),
                "the worktree went, got: {:?}",
                report.worktree
            );
            assert_eq!(
                report.branch.as_ref().map(discriminant),
                Some(discriminant(&outcome)),
                "the branch step is reported as it happened, got: {:?}",
                report.branch
            );
        }

        let mut steps = FakeSteps::new();
        let branch_only = Target::Branch { name: "feature" };
        let report = remove(branch_only, &License::shown(Risk::default()), &mut steps)
            .expect("no git to fail");
        assert!(report.worktree.is_none(), "no worktree to remove");
        assert!(matches!(
            report.branch,
            Some(git::BranchDeleteOutcome::Deleted)
        ));

        let mut steps = FakeSteps::new();
        let detached = Target::Worktree {
            path: Path::new("/tmp/wt"),
        };
        let report = remove(detached, &License::forced(), &mut steps).expect("no git to fail");
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
        assert!(!nothing.worktree && nothing.branch == BranchLicense::None);

        let both = License::shown(Risk {
            dirty: true,
            unmerged: Some(git::Unmerged::NoUpstream),
        });
        assert!(both.worktree && both.branch == BranchLicense::Outright);

        let forced = License::forced();
        assert!(forced.worktree && forced.branch == BranchLicense::Outright);
    }

    /// Proof is the third source of license (ADR 0005): an equivalent branch
    /// draws no marker, so nothing else in its license would force anything, and
    /// the proof alone is what discards the commits git would refuse.
    #[test]
    fn a_proof_forces_the_delete_while_what_it_was_established_on_still_holds() {
        let mut steps = FakeSteps::new();
        remove(
            held(),
            &License::proven(Risk::default(), &proof()),
            &mut steps,
        )
        .expect("no git to fail");
        assert_eq!(
            steps.calls,
            vec![
                Call::RemoveWorktree { force: false },
                Call::DeleteBranch { force: true },
            ]
        );
    }

    /// A license covers what was established and nothing more, and equivalence
    /// was established on two things at once. Move either — the branch grows a
    /// commit nobody proved, or the anchor is rewound and no longer holds the
    /// content that made the branch safe to discard — and the delete falls to
    /// `-d` to meet git's own guard.
    #[test]
    fn a_proof_lapses_when_either_the_branch_or_the_anchor_moves() {
        for moved in ["refs/heads/feature", ANCHOR_REF] {
            let mut steps = FakeSteps::new().moved(moved);
            remove(
                held(),
                &License::proven(Risk::default(), &proof()),
                &mut steps,
            )
            .expect("no git to fail");
            assert_eq!(
                steps.calls,
                vec![
                    Call::RemoveWorktree { force: false },
                    Call::DeleteBranch { force: false },
                ],
                "{moved} moved, so the proof no longer covers the delete"
            );
        }
    }
}

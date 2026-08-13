use std::collections::HashSet;
use std::path::{Path, PathBuf};

use console::{Key, Term, style};
use indicatif::ProgressBar;

use crate::{AppResult, Error, git};

pub(crate) mod hook;
pub(crate) mod marker;
pub(crate) mod picker;
pub(crate) mod removal;
pub(crate) mod reporting;
pub mod wt;

use picker::{
    Availability, Pick, PickKind, PickerOptions, Section, Selection, align_labels,
    interactive_keys, multi_select, pick,
};

pub(crate) struct CursorGuard(Term);

impl CursorGuard {
    pub(crate) fn hide() -> Self {
        let term = Term::stderr();
        let _ = term.hide_cursor();
        Self(term)
    }
}

impl Drop for CursorGuard {
    fn drop(&mut self) {
        let _ = self.0.show_cursor();
    }
}

/// The stderr terminal, but only when it's interactive. Returns `None` in
/// piped/CI runs where there's no TTY to drive a prompt — callers fall back to
/// doing nothing rather than blocking on key input.
fn interactive_term() -> Option<Term> {
    let term = Term::stderr();
    term.is_term().then_some(term)
}

pub fn run(target: Option<&str>) -> AppResult<()> {
    let old_branch = git::current_branch()?;
    let remote = git::current_remote(old_branch.as_deref());

    // `git-switch .` refreshes the branch we're already on against its remote,
    // rather than switching anywhere.
    if target == Some(".") {
        let Some(current) = old_branch.as_deref() else {
            return Err(Error::Detached);
        };
        return refresh_current(&remote, current);
    }

    let target = match target {
        Some(name) => name.to_string(),
        None => match select_branch(old_branch.as_deref(), &remote)? {
            Some(t) => t,
            None => return Ok(()),
        },
    };

    // `git checkout` refuses for a branch already checked out in another
    // worktree; hand off to the shell wrapper instead.
    // A prunable worktree (directory gone) still holds the branch but can't be
    // entered; skip it so we fall through to the self-healing checkout below.
    if old_branch.as_deref() != Some(target.as_str())
        && let Some(held_by) =
            git::worktree_for_branch(&git::worktree_list()?, &target).filter(|w| !w.prunable)
    {
        // The target may track a different remote than the current branch.
        let target_remote = git::current_remote(Some(target.as_str()));
        if let Err(e) = wt::update_in(&held_by.path, &target, &target_remote) {
            eprintln!(
                "{} update of {} failed: {e}",
                style("!").yellow().bold(),
                target,
            );
        }
        eprintln!(
            "{} {} is checked out at {}",
            style("→").cyan().bold(),
            target,
            held_by.path.display()
        );
        // `target` is where we're about to hand off, so it must not be on offer.
        if let Err(e) = prompt_delete_stale_branches(None, Some(&target), &remote) {
            if e.is_interrupt() {
                return Err(e);
            }
            eprintln!(
                "{} stale-branch check failed: {e}",
                style("!").yellow().bold()
            );
        }
        handoff_cd(&held_by.path);
        return Ok(());
    }

    let stashed = if git::has_tracked_changes()? {
        git::stash_push()?;
        true
    } else {
        false
    };

    let result = switch_and_update(&target, old_branch.as_deref(), &remote);

    if stashed {
        if result.is_err()
            && let Some(old) = old_branch.as_deref()
        {
            eprintln!("Switching back to {old} and restoring stashed changes.");
            let _ = git::checkout(old);
        }
        report_stash_pop();
    }

    result
}

/// Pop the auto-stash and report the outcome — a clean restore, a conflict to
/// resolve, or an outright failure. Shared by the switch and refresh flows.
fn report_stash_pop() {
    match git::stash_pop() {
        Ok(git::StashPopOutcome::Clean) => {}
        Ok(git::StashPopOutcome::Conflict) => eprintln!(
            "Conflicts detected restoring stashed changes. Resolve them, then run `git stash drop` to clean up the stash entry."
        ),
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "Stash pop failed. Inspect `git status` and `git stash list` to recover manually."
            );
        }
    }
}

/// Outcome of the keep/discard prompt shown when `git-switch .` finds local
/// work that a refresh would otherwise overwrite.
enum RefreshChoice {
    /// Rebase local commits onto the remote, restoring any stashed edits.
    Keep,
    /// Hard-reset to the remote, discarding local commits and tracked edits.
    Discard,
}

/// Refresh the branch we're on against its remote (`git-switch .`).
///
/// With nothing to pull it just reports status. When the remote has new
/// commits, a clean tree integrates them with no prompt — fast-forwarding, or
/// (when the branch has diverged, e.g. after rebasing through a web UI)
/// rebasing local commits on top, which drops any already upstream and replays
/// genuine new work. A dirty tree would be disturbed by that, so there it
/// prompts to keep the uncommitted work (stash, rebase, restore) or discard it
/// (hard reset to the remote).
fn refresh_current(remote: &str, current: &str) -> AppResult<()> {
    let remote_ref = format!("{remote}/{current}");

    let has_remote = {
        let spinner = ProgressBar::new_spinner().with_message(format!("Fetching {remote}…"));
        let _cursor_guard = CursorGuard::hide();
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));

        let fetch_outcome =
            git::fetch(None, remote).unwrap_or_else(|e| git::FetchOutcome::Failed(e.to_string()));
        let has_remote = git::remote_branch_exists(remote, current);

        spinner.finish_and_clear();

        report_fetch_failure(&fetch_outcome);
        has_remote
    };

    if !has_remote {
        report_update(&git::MergeReport::NoRemote);
        return Ok(());
    }

    let (ahead, behind) = git::ahead_behind_remote(remote, current)?;

    // Nothing incoming: report status and leave the working tree (and any
    // uncommitted edits) untouched.
    if behind == 0 {
        if ahead == 0 {
            report_update(&git::MergeReport::UpToDate);
        } else {
            eprintln!(
                "{current} is {ahead} {} ahead of {remote_ref}; nothing to pull.",
                plural(ahead, "commit")
            );
        }
        return Ok(());
    }

    // A dirty tree would be disturbed by integrating, so let the user decide.
    if git::has_tracked_changes()? {
        match prompt_keep_discard(current, &remote_ref, ahead, behind)? {
            Some(RefreshChoice::Keep) => keep_local_work(&remote_ref)?,
            Some(RefreshChoice::Discard) => {
                git::reset_hard(remote, current)?;
                eprintln!("Reset {current} to {remote_ref}.");
            }
            None => eprintln!("Left {current} unchanged."),
        }
        return Ok(());
    }

    // Clean tree: integrate seamlessly. A fast-forward when the branch hasn't
    // diverged; otherwise rebase local commits onto the remote.
    if ahead == 0 {
        match git::fast_forward_merge(None, current, remote)? {
            git::FastForwardResult::Merged(report) => report_update(&report),
            // History always fast-forwards here, so a refusal means an untracked
            // file is in the way (`has_tracked_changes` ignores those).
            git::FastForwardResult::Diverged => eprintln!(
                "{} couldn't fast-forward {current} to {remote_ref}; an untracked file is likely blocking it",
                style("!").yellow().bold()
            ),
        }
    } else {
        rebase_onto(&remote_ref)?;
    }
    Ok(())
}

/// Keep uncommitted work while integrating remote commits: stash the edits,
/// rebase onto `remote_ref` (a fast-forward when the branch hasn't diverged),
/// then restore the edits. Mirrors the auto-stash dance in [`run`] so conflicts
/// and pop failures are surfaced the same way.
fn keep_local_work(remote_ref: &str) -> AppResult<()> {
    git::stash_push()?;
    // A clean rebase replays the stash cleanly; an abort leaves the tree at the
    // original HEAD, so the stash still applies. Restore either way.
    let result = rebase_onto(remote_ref);
    report_stash_pop();
    result
}

/// `"{word}"` for one, `"{word}s"` otherwise.
fn plural(n: u32, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Describe the incoming remote work alongside the dirty tree, then ask whether
/// to keep the uncommitted work or discard it. Reached only with a dirty tree
/// and commits to integrate. Returns `None` on cancel or in non-interactive
/// runs, where acting destructively without confirmation would be unsafe.
fn prompt_keep_discard(
    branch: &str,
    remote_ref: &str,
    ahead: u32,
    behind: u32,
) -> AppResult<Option<RefreshChoice>> {
    if ahead > 0 {
        eprintln!(
            "{branch} has diverged from {remote_ref}: {ahead} local {} not on the remote, {behind} on the remote not yet local.",
            plural(ahead, "commit")
        );
    } else {
        eprintln!(
            "{remote_ref} has {behind} new {}.",
            plural(behind, "commit")
        );
    }
    eprintln!("Working tree has uncommitted changes.");

    let Some(term) = interactive_term() else {
        return Ok(None);
    };

    eprint!(
        "{} {} {} ",
        style("?").green().bold(),
        style(format!("Refresh to {remote_ref}?")).bold(),
        style("[k]eep (stash & rebase) / [d]iscard (hard reset) / esc").dim(),
    );
    let _cursor_guard = CursorGuard::hide();
    loop {
        let choice = match term.read_key()? {
            Key::Char('k' | 'K') => Some(RefreshChoice::Keep),
            Key::Char('d' | 'D') => Some(RefreshChoice::Discard),
            Key::Char('n' | 'N') | Key::Escape => None,
            _ => continue,
        };
        eprintln!(
            "{}",
            match choice {
                Some(RefreshChoice::Keep) => "keep",
                Some(RefreshChoice::Discard) => "discard",
                None => "cancel",
            }
        );
        return Ok(choice);
    }
}

fn switch_and_update(target: &str, old_branch: Option<&str>, remote: &str) -> AppResult<()> {
    let already_on_target = old_branch.is_some_and(|b| b == target);

    if !already_on_target {
        git::checkout(target)?;
    }

    match fetch_and_ff(None, target, remote)? {
        git::FastForwardResult::Diverged => reconcile_diverged(target, remote)?,
        git::FastForwardResult::Merged(report) => report_update(&report),
    }

    // An in-place switch stays put, so there's no worktree to protect.
    prompt_delete_stale_branches(
        if already_on_target { None } else { old_branch },
        None,
        remote,
    )?;

    Ok(())
}

/// Fetch `remote` then fast-forward `branch` onto it, optionally inside the
/// worktree at `dir` (via `git -C`). Shows a spinner and surfaces fetch
/// failures; the caller decides how to handle the [`git::FastForwardResult`]
/// (the in-place switch offers a rebase on diverge; worktree updates don't).
pub(crate) fn fetch_and_ff(
    dir: Option<&std::path::Path>,
    branch: &str,
    remote: &str,
) -> AppResult<git::FastForwardResult> {
    let (fetch_outcome, merge_result) = {
        let spinner = ProgressBar::new_spinner().with_message(format!("Updating {branch}…"));
        let _cursor_guard = CursorGuard::hide();
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));

        let fetch_outcome =
            git::fetch(dir, remote).unwrap_or_else(|e| git::FetchOutcome::Failed(e.to_string()));
        let result = git::fast_forward_merge(dir, branch, remote);

        spinner.finish_and_clear();
        (fetch_outcome, result)
    };

    report_fetch_failure(&fetch_outcome);

    merge_result
}

/// Warn that a fetch failed (so callers know results may be stale), printing
/// git's detail lines. A no-op on success. Shared by the switch and refresh
/// flows, both of which fetch behind a spinner.
fn report_fetch_failure(outcome: &git::FetchOutcome) {
    let git::FetchOutcome::Failed(detail) = outcome else {
        return;
    };
    eprintln!(
        "{} fetch failed; results may be stale",
        style("!").yellow().bold()
    );
    for line in detail.lines() {
        eprintln!("  {line}");
    }
}

pub(crate) fn report_update(result: &git::MergeReport) {
    match result {
        git::MergeReport::UpToDate => eprintln!("Already up to date."),
        git::MergeReport::Pulled(1) => eprintln!("Pulled 1 commit."),
        git::MergeReport::Pulled(n) => eprintln!("Pulled {n} commits."),
        git::MergeReport::NoRemote => eprintln!("No remote tracking branch."),
    }
}

fn reconcile_diverged(branch: &str, remote: &str) -> AppResult<()> {
    let remote_ref = format!("{remote}/{branch}");
    eprintln!("Local branch has diverged from {remote_ref}.");

    if !confirm(&format!("Rebase onto {remote_ref}?"), false)? {
        eprintln!("{}", reconcile_hint(&remote_ref));
        return Err(Error::Diverged);
    }

    rebase_onto(&remote_ref)
}

/// The hint left behind when the user declines the rebase and has to reconcile
/// by hand. Kept apart from the printing so the wording can be tested without a
/// repo on disk.
fn reconcile_hint(remote_ref: &str) -> String {
    let quoted = shell_quote(remote_ref);
    format!("Run `git rebase -- {quoted}` or `git merge -- {quoted}` to reconcile.")
}

/// Rebase the current branch onto `remote_ref`, reporting an aborted rebase the
/// same way for every caller (refresh's keep path and the diverged-switch
/// reconcile).
fn rebase_onto(remote_ref: &str) -> AppResult<()> {
    match git::rebase(remote_ref)? {
        git::RebaseOutcome::Clean => Ok(()),
        git::RebaseOutcome::Aborted => {
            eprintln!("{}", aborted_rebase_hint(remote_ref));
            Err(Error::Diverged)
        }
    }
}

/// The hint left behind when a rebase conflicts and git puts the branch back
/// where it was. Kept apart from the printing for the same reason as
/// [`reconcile_hint`].
fn aborted_rebase_hint(remote_ref: &str) -> String {
    format!(
        "Rebase aborted due to conflicts. Run `git rebase -- {}` manually to reconcile.",
        shell_quote(remote_ref)
    )
}

pub(crate) fn confirm(prompt: &str, default_yes: bool) -> AppResult<bool> {
    let Some(term) = interactive_term() else {
        return Ok(default_yes);
    };
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    eprint!(
        "{} {} {} ",
        style("?").green().bold(),
        style(prompt).bold(),
        style(hint).dim(),
    );
    let _cursor_guard = CursorGuard::hide();
    loop {
        let answer = match term.read_key()? {
            Key::Char('y' | 'Y') => true,
            Key::Char('n' | 'N') | Key::Escape => false,
            Key::Enter => default_yes,
            _ => continue,
        };
        eprintln!("{}", if answer { "y" } else { "n" });
        return Ok(answer);
    }
}

fn select_branch(current: Option<&str>, remote: &str) -> AppResult<Option<String>> {
    let sections = build_sections(current, remote, &HashSet::new())?;
    // Non-interactive (piped/CI): we can't prompt, so report nothing to switch
    // to rather than blocking on key input.
    let Some(keys) = interactive_keys() else {
        return Ok(None);
    };
    let selection = pick(
        current,
        &sections,
        PickerOptions {
            prompt: "Switch to",
            allow_create_from_filter: false,
        },
        keys,
    )?;
    Ok(selection.map(|s| match s {
        Selection::Existing { name, .. } => name,
        Selection::Create(_) => unreachable!("create-from-filter disabled"),
    }))
}

pub(crate) fn build_sections(
    current: Option<&str>,
    remote: &str,
    exclude: &HashSet<String>,
) -> AppResult<Vec<Section>> {
    let local = git::local_branches()?;
    let remote_only = git::remote_only_branches(&local, remote).unwrap_or_default();

    if local.is_empty() && remote_only.is_empty() {
        return Err(Error::NoBranches);
    }

    let pinned_names = git::pinned_branches(remote);
    let local_set: HashSet<&str> = local.iter().map(String::as_str).collect();
    let remote_set: HashSet<&str> = remote_only.iter().map(String::as_str).collect();
    let pinned_set: HashSet<&str> = pinned_names.iter().map(String::as_str).collect();

    let pinned_picks: Vec<Pick> = pinned_names
        .iter()
        .filter(|name| !exclude.contains(name.as_str()))
        .map(|name| {
            let in_local = local_set.contains(name.as_str());
            let in_remote = remote_set.contains(name.as_str());
            let availability = if in_local {
                Availability::Local
            } else if in_remote {
                Availability::RemoteOnly
            } else {
                Availability::Missing
            };
            Pick {
                name: name.clone(),
                is_current: current == Some(name.as_str()),
                availability,
                kind: PickKind::Branch,
            }
        })
        .collect();

    let local_picks: Vec<Pick> = local
        .iter()
        .filter(|b| !pinned_set.contains(b.as_str()) && !exclude.contains(b.as_str()))
        .map(|b| Pick {
            name: b.clone(),
            is_current: current == Some(b.as_str()),
            availability: Availability::Local,
            kind: PickKind::Branch,
        })
        .collect();

    let remote_picks: Vec<Pick> = remote_only
        .iter()
        .filter(|b| !pinned_set.contains(b.as_str()) && !exclude.contains(b.as_str()))
        .map(|b| Pick {
            name: b.clone(),
            is_current: false,
            availability: Availability::Local,
            kind: PickKind::Branch,
        })
        .collect();

    let mut sections = Vec::new();
    if !pinned_picks.is_empty() {
        sections.push(Section {
            heading: "Pinned",
            items: pinned_picks,
        });
    }
    if !local_picks.is_empty() {
        sections.push(Section {
            heading: "Local",
            items: local_picks,
        });
    }
    if !remote_picks.is_empty() {
        sections.push(Section {
            heading: "Remote",
            items: remote_picks,
        });
    }
    Ok(sections)
}

/// What removing something would irreversibly destroy.
///
/// The project rule is *warned means forceable*: a destructive step may skip
/// confirmation only where the user was shown the specific risk first. In a
/// picker the row markers are that warning; for a target named on the command
/// line no marker is possible, so a confirmation naming these risks stands in
/// for it. Either way, forcing is licensed only by a `Risk` the user has seen.
#[derive(Default, Clone, Copy)]
pub(crate) struct Risk {
    /// The worktree holds uncommitted or untracked changes.
    pub(crate) dirty: bool,
    /// The branch has commits `git branch -d` would refuse to discard.
    pub(crate) unmerged: Option<git::Unmerged>,
}

impl Risk {
    pub(crate) fn any(self) -> bool {
        self.dirty || self.unmerged.is_some()
    }
}

/// A stale branch, together with the ground it is offered on, the worktree
/// holding it (if any) and what deleting it would destroy.
struct StaleRow {
    branch: String,
    ground: git::Ground,
    worktree: Option<git::Worktree>,
    risk: Risk,
}

/// Offers to delete stale branches, and the worktrees holding them.
///
/// `old_branch` is pre-ticked, being the branch just switched away from.
/// `destination` is the branch we're about to hand the shell off into: it is
/// never offered, since deleting it would remove the very worktree the caller
/// is about to `cd` to.
pub(crate) fn prompt_delete_stale_branches(
    old_branch: Option<&str>,
    destination: Option<&str>,
    remote: &str,
) -> AppResult<()> {
    let stale = git::stale_branches(remote)?;
    let worktrees = git::worktree_list().unwrap_or_default();
    // Judge risk — and later delete — from the main worktree, where HEAD is
    // normally the branch staleness was measured against. Asking from the
    // current worktree would call a branch merged into the anchor "unmerged"
    // whenever that worktree sits on something unrelated, and per ADR 0001 the
    // marker would then license a force-delete it never warned about.
    let main_dir = worktrees.iter().find(|w| w.is_main).map(|w| w.path.clone());
    let unmerged = git::unmerged_branches(main_dir.as_deref()).unwrap_or_default();
    let rows = stale_rows(stale, &worktrees, &unmerged, destination, &|path| {
        git::worktree_dirty(path)
    });
    if rows.is_empty() {
        return Ok(());
    }

    // A branch held by a worktree is never the one just left — git forbids the
    // same branch in two worktrees — so worktree rows always start unticked.
    let defaults: Vec<bool> = rows
        .iter()
        .map(|r| old_branch.is_some_and(|old| old == r.branch))
        .collect();
    let items = align_labels(&rows.iter().map(stale_label).collect::<Vec<_>>());

    // Non-interactive (piped/CI): we can't prompt, so delete nothing rather
    // than blocking on key input or silently acting on the defaults.
    let Some(keys) = interactive_keys() else {
        return Ok(());
    };
    let legend = risk_legend(&rows.iter().map(|r| r.risk).collect::<Vec<_>>());
    let selections = multi_select(
        "Delete stale branches (space to toggle, →/← all/none)",
        legend.as_deref(),
        &items,
        &defaults,
        keys,
    )?;

    // Delete from the main worktree, the same HEAD the risk was judged against.
    let mut steps = removal::GitSteps::at_main(main_dir.as_deref());
    for &i in &selections {
        delete_stale_row(&mut steps, &rows[i], main_dir.as_deref())?;
    }

    Ok(())
}

/// Builds the picker rows, pairing each stale branch with the worktree holding
/// it and what deleting it would destroy. `dirty` is injected so the rule can be
/// tested without a repo on disk.
fn stale_rows(
    stale: Vec<git::StaleBranch>,
    worktrees: &[git::Worktree],
    unmerged: &std::collections::HashMap<String, git::Unmerged>,
    destination: Option<&str>,
    dirty: &dyn Fn(&Path) -> bool,
) -> Vec<StaleRow> {
    stale
        .into_iter()
        .filter(|b| destination != Some(b.name.as_str()))
        .map(|b| {
            let worktree = git::worktree_for_branch(worktrees, &b.name);
            let risk = Risk {
                dirty: worktree
                    .as_ref()
                    .is_some_and(|w| !w.prunable && dirty(&w.path)),
                unmerged: unmerged.get(&b.name).copied(),
            };
            StaleRow {
                branch: b.name,
                ground: b.ground,
                worktree,
                risk,
            }
        })
        .collect()
}

/// Glosses the *Marker* glyphs these rows actually carry, or `None` where they
/// carry none. Only what is on screen is explained: a legend for a glyph nobody
/// can see is noise, and the common riskless list gets no legend at all. Grounds
/// need no gloss — they are already words.
pub(crate) fn risk_legend(risks: &[Risk]) -> Option<String> {
    let mut parts = Vec::new();
    if risks.iter().any(|r| r.dirty) {
        parts.push(format!("{} uncommitted changes", marker::Marker::Dirty));
    }
    if risks.iter().any(|r| r.unmerged.is_some()) {
        parts.push(format!(
            "{} unmerged commits",
            marker::Marker::Unmerged(None)
        ));
    }
    (!parts.is_empty()).then(|| parts.join("   "))
}

/// The *Ground* a row is offered on, as the word the glossary uses. Dim, and
/// deliberately not a [`marker::Marker`]: a ground warns of no loss, and per ADR
/// 0001 only a marker licenses forcing. See [ADR
/// 0003](../docs/adr/0003-a-ground-is-not-a-marker.md).
fn ground_label(ground: git::Ground) -> String {
    let word = match ground {
        git::Ground::Gone => "gone",
        git::Ground::Landed => "landed",
    };
    style(word).dim().to_string()
}

/// The picker row for a stale branch, as a (name, annotation) pair for
/// [`align_labels`]. The ground leads, answering why the row is here at all;
/// the risks follow, answering what deleting it would cost. Dirtiness belongs to
/// the worktree so it sits inside the parentheses; unmerged commits belong to
/// the branch so they sit outside. The worktree's path is deliberately absent —
/// it appears in the outcome line.
fn stale_label(row: &StaleRow) -> (String, String) {
    let worktree = match &row.worktree {
        None => String::new(),
        Some(w) if w.prunable => "(+ worktree, missing)".to_string(),
        Some(_) if row.risk.dirty => {
            format!("(+ worktree {})", marker::Marker::Dirty)
        }
        Some(_) => "(+ worktree)".to_string(),
    };
    let branch_risk = marker::markers(Risk {
        dirty: false,
        ..row.risk
    });

    let annotation = [ground_label(row.ground), worktree, branch_risk]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (row.branch.clone(), annotation)
}

/// Removes a ticked stale row and prints what happened. The ordering, and the
/// rule that only a shown marker licenses forcing, belong to [`removal`]; the
/// wording belongs to [`reporting`]. This is what's left: which target the row
/// describes, and telling the [`hook`] about a worktree that went with it — a
/// held stale branch takes its worktree along, which is as much a removal as
/// `wt rm` is. A hook mirrors what happened to the repo, not which command you
/// happened to type.
fn delete_stale_row(
    steps: &mut impl removal::Steps,
    row: &StaleRow,
    main: Option<&Path>,
) -> AppResult<()> {
    let target = match &row.worktree {
        Some(wt) => removal::Target::Held {
            name: &row.branch,
            path: &wt.path,
        },
        None => removal::Target::Branch { name: &row.branch },
    };
    let report = removal::remove(target, removal::License::shown(row.risk), steps)?;

    for line in reporting::removal_outcome(&report) {
        eprintln!("{line}");
    }

    // Only where a worktree was really there and really went. `main` is the
    // worktree the deletes ran from, and the one a hook is run from.
    if let Some(wt) = &row.worktree
        && report.worktree_removed()
        && let Some(main) = main
    {
        hook::fire(hook::Event::Removed, &wt.path, Some(&row.branch), main);
    }
    Ok(())
}

/// Renders `word` so a shell reads it as the single literal it is. Git allows
/// `$`, backticks, `;` and `&` in a ref name, so a branch called
/// ``topic$(rm -rf ~)`` would otherwise run its own payload the moment someone
/// pasted a command we printed. Names needing nothing are returned bare, which
/// keeps the overwhelmingly common case readable; anything else is single-quoted,
/// where the only character with meaning is `'` itself.
///
/// Quoting alone doesn't cover a name that looks like an option, so the commands
/// built from this pass `--` before the ref as well.
pub(crate) fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "._/@+-".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Contracts a leading home directory to `~` so paths stay readable in prompts.
pub(crate) fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// True when there's a terminal to show a warning on and read an answer from.
pub(crate) fn is_interactive() -> bool {
    interactive_term().is_some()
}

/// Hand a target directory to the shell wrapper, which reads it from stdout and
/// runs `cd`. When stdout is a terminal the wrapper isn't capturing it, so a
/// bare path would just be dumped to the screen with no `cd` — print an
/// actionable hint to stderr instead.
pub(crate) fn handoff_cd(path: &std::path::Path) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        eprintln!(
            "{} shell integration not active — can't cd for you. Run:",
            style("!").yellow().bold(),
        );
        eprintln!("  cd {}", path.display());
        eprintln!("  (enable auto-cd: see README \"Shell integration\")");
    } else {
        println!("{}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row stale on the *Landed* ground, which the label tests take as given
    /// so they can hold the risk half still — `ground_label_names_each_ground`
    /// covers the other one.
    fn stale_row(branch: &str, worktree: Option<git::Worktree>, risk: Risk) -> StaleRow {
        StaleRow {
            branch: branch.to_string(),
            ground: git::Ground::Landed,
            worktree,
            risk,
        }
    }

    /// Builds the input `stale_rows` now takes, all on one ground.
    fn landed(names: &[&str]) -> Vec<git::StaleBranch> {
        names
            .iter()
            .map(|n| git::StaleBranch {
                ground: git::Ground::Landed,
                name: (*n).to_string(),
            })
            .collect()
    }

    fn worktree(prunable: bool) -> git::Worktree {
        git::Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: None,
            is_main: false,
            prunable,
        }
    }

    /// Strips ANSI styling so assertions read as the user sees the row.
    fn plain(s: &str) -> String {
        console::strip_ansi_codes(s).into_owned()
    }

    #[test]
    fn reconcile_hint_names_both_ways_out() {
        assert_eq!(
            reconcile_hint("origin/main"),
            "Run `git rebase -- origin/main` or `git merge -- origin/main` to reconcile."
        );
    }

    #[test]
    fn aborted_rebase_hint_points_back_at_the_upstream() {
        assert_eq!(
            aborted_rebase_hint("origin/main"),
            "Rebase aborted due to conflicts. Run `git rebase -- origin/main` manually to reconcile."
        );
    }

    /// Both hints are meant to be pasted into a shell, and git allows `$` and
    /// backticks in a ref name — so the ref has to survive the paste as a
    /// literal rather than running.
    #[test]
    fn the_reconcile_hints_quote_a_ref_that_could_run_as_a_command() {
        let remote_ref = "origin/topic$(touch${IFS}/tmp/pwned)";
        assert_eq!(
            reconcile_hint(remote_ref),
            "Run `git rebase -- 'origin/topic$(touch${IFS}/tmp/pwned)'` \
             or `git merge -- 'origin/topic$(touch${IFS}/tmp/pwned)'` to reconcile."
        );
        assert_eq!(
            aborted_rebase_hint(remote_ref),
            "Rebase aborted due to conflicts. \
             Run `git rebase -- 'origin/topic$(touch${IFS}/tmp/pwned)'` manually to reconcile."
        );
    }

    #[test]
    fn shell_quote_leaves_an_ordinary_branch_name_bare() {
        assert_eq!(shell_quote("feature/login-2"), "feature/login-2");
    }

    #[test]
    fn shell_quote_wraps_anything_a_shell_would_interpret() {
        assert_eq!(shell_quote("topic;ls"), "'topic;ls'");
        assert_eq!(shell_quote("topic&&ls"), "'topic&&ls'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
    }

    /// A single quote can't be escaped inside single quotes, so it has to close
    /// the quoting, contribute an escaped `'`, and reopen.
    #[test]
    fn shell_quote_handles_a_quote_in_the_name() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// The ground is on every row: a branch with nothing at stake still owes the
    /// user an answer to "why is this being offered at all?".
    #[test]
    fn stale_label_without_worktree_carries_only_its_ground() {
        let row = stale_row("fix/typo", None, Risk::default());
        let (name, annotation) = stale_label(&row);
        assert_eq!(name, "fix/typo");
        assert_eq!(plain(&annotation), "landed");
    }

    #[test]
    fn ground_label_names_each_ground() {
        assert_eq!(plain(&ground_label(git::Ground::Gone)), "gone");
        assert_eq!(plain(&ground_label(git::Ground::Landed)), "landed");
    }

    #[test]
    fn stale_label_marks_a_dirty_worktree_inside_the_parens() {
        let row = stale_row(
            "chore/deps",
            Some(worktree(false)),
            Risk {
                dirty: true,
                unmerged: None,
            },
        );
        assert_eq!(plain(&stale_label(&row).1), "landed (+ worktree ●)");
    }

    #[test]
    fn stale_label_marks_a_missing_worktree() {
        let row = stale_row("old/thing", Some(worktree(true)), Risk::default());
        assert_eq!(plain(&stale_label(&row).1), "landed (+ worktree, missing)");
    }

    /// The ground leads — why the row is here — and the risks follow: what it
    /// would cost. Dirtiness belongs to the worktree, unmerged commits to the
    /// branch, so one sits inside the parentheses and the other outside.
    #[test]
    fn stale_label_leads_with_the_ground_then_the_risks() {
        let row = stale_row(
            "spike/abandoned",
            Some(worktree(false)),
            Risk {
                dirty: true,
                unmerged: Some(git::Unmerged::Ahead(2)),
            },
        );
        assert_eq!(plain(&stale_label(&row).1), "landed (+ worktree ●) ↑2");
    }

    /// Grounds are words already, so the legend glosses only glyphs — and only
    /// the glyphs some row actually carries.
    #[test]
    fn risk_legend_glosses_only_the_glyphs_on_screen() {
        let dirty = Risk {
            dirty: true,
            unmerged: None,
        };
        let unmerged = Risk {
            dirty: false,
            unmerged: Some(git::Unmerged::Ahead(2)),
        };
        assert_eq!(
            risk_legend(&[dirty]).as_deref().map(plain).as_deref(),
            Some("● uncommitted changes")
        );
        assert_eq!(
            risk_legend(&[unmerged]).as_deref().map(plain).as_deref(),
            Some("↑ unmerged commits")
        );
        assert_eq!(
            risk_legend(&[dirty, unmerged]).as_deref().map(plain),
            Some("● uncommitted changes   ↑ unmerged commits".to_string())
        );
    }

    /// The common case is a list with nothing at stake, which earns no legend.
    #[test]
    fn risk_legend_is_absent_when_nothing_is_at_risk() {
        assert!(risk_legend(&[Risk::default(), Risk::default()]).is_none());
    }

    /// Nothing to lose is what lets a removal skip the prompt entirely, so the
    /// default has to answer no.
    #[test]
    fn a_default_risk_has_nothing_to_lose() {
        assert!(!Risk::default().any());
    }

    fn named_worktree(branch: &str, path: &str) -> git::Worktree {
        git::Worktree {
            path: PathBuf::from(path),
            branch: Some(branch.to_string()),
            is_main: false,
            prunable: false,
        }
    }

    fn names(rows: &[StaleRow]) -> Vec<&str> {
        rows.iter().map(|r| r.branch.as_str()).collect()
    }

    /// Regression: the caller hands the shell off into a worktree right after
    /// this prompt. Offering that worktree for deletion means a `→` select-all
    /// removes the directory we're about to `cd` into.
    #[test]
    fn stale_rows_never_offer_the_handoff_destination() {
        let worktrees = vec![named_worktree("feature", "/tmp/wt")];
        let stale = landed(&["feature", "fix/typo"]);
        let rows = stale_rows(
            stale,
            &worktrees,
            &std::collections::HashMap::new(),
            Some("feature"),
            &|_| false,
        );
        assert_eq!(names(&rows), vec!["fix/typo"]);
    }

    #[test]
    fn stale_rows_without_a_destination_offer_everything() {
        let worktrees = vec![named_worktree("feature", "/tmp/wt")];
        let stale = landed(&["feature", "fix/typo"]);
        let rows = stale_rows(
            stale,
            &worktrees,
            &std::collections::HashMap::new(),
            None,
            &|_| false,
        );
        assert_eq!(names(&rows), vec!["feature", "fix/typo"]);
    }

    #[test]
    fn stale_rows_flag_a_dirty_held_worktree() {
        let worktrees = vec![named_worktree("feature", "/tmp/wt")];
        let rows = stale_rows(
            landed(&["feature"]),
            &worktrees,
            &std::collections::HashMap::new(),
            None,
            &|path| path == Path::new("/tmp/wt"),
        );
        assert!(rows[0].risk.dirty);
        assert!(rows[0].worktree.is_some());
    }
}

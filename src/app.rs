use std::collections::HashSet;
use std::path::{Path, PathBuf};

use console::{Key, Term, measure_text_width, style};
use indicatif::ProgressBar;

use crate::{AppResult, Error, git};

pub(crate) mod marker;
pub(crate) mod removal;
pub(crate) mod reporting;
pub mod wt;

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

/// Source of key events for the interactive pickers. Abstracting input behind a
/// trait lets the event loops be driven by a scripted sequence in tests; the
/// real implementation is [`TermKeys`].
pub(crate) trait KeySource {
    fn read_key(&mut self) -> std::io::Result<Key>;
}

/// The real key source backing the interactive pickers. It holds the terminal in
/// raw mode for the picker's lifetime and lets `crossterm` parse key events.
pub(crate) struct TermKeys {
    term: Term,
    raw: Option<raw::RawMode>,
}

impl KeySource for TermKeys {
    fn read_key(&mut self) -> std::io::Result<Key> {
        if let Some(raw) = &self.raw {
            return raw.read_key();
        }
        self.term.read_key()
    }
}

/// The stderr terminal, but only when it's interactive. Returns `None` in
/// piped/CI runs where there's no TTY to drive a prompt — callers fall back to
/// doing nothing rather than blocking on key input.
fn interactive_term() -> Option<Term> {
    let term = Term::stderr();
    term.is_term().then_some(term)
}

/// A key source for an interactive prompt, or `None` in piped/CI runs. Mirrors
/// [`interactive_term`] but acquires raw mode so arrow keys are read reliably.
fn interactive_keys() -> Option<TermKeys> {
    let term = interactive_term()?;
    Some(TermKeys {
        term,
        // Acquiring raw mode can fail; fall back to `console`.
        raw: raw::RawMode::acquire().ok(),
    })
}

/// Raw-mode key reader. `console::read_key` re-arms raw mode on every keystroke
/// and has been fragile around split escape sequences; `crossterm` keeps raw
/// mode active and uses its battle-tested event parser instead.
mod raw {
    use std::io;

    use console::Key;
    use crossterm::{
        event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, read},
        terminal::{disable_raw_mode, enable_raw_mode},
    };

    /// Zero-sized guard: enabling raw mode is process-global, and [`Drop`]
    /// disables it again.
    pub(crate) struct RawMode;

    impl RawMode {
        pub(crate) fn acquire() -> io::Result<Self> {
            enable_raw_mode()?;
            Ok(Self)
        }

        // `&self` is a capability token: holding the guard proves raw mode is
        // active, even though reading uses crossterm's global event source.
        #[allow(clippy::unused_self)]
        pub(crate) fn read_key(&self) -> io::Result<Key> {
            loop {
                let Event::Key(event) = read()? else {
                    continue;
                };
                if event.kind == KeyEventKind::Release {
                    continue;
                }
                return translate_key(event);
            }
        }
    }

    fn translate_key(event: KeyEvent) -> io::Result<Key> {
        if event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(event.code, KeyCode::Char('c' | 'C'))
        {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        }

        Ok(match event.code {
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Enter => Key::Enter,
            KeyCode::Left => Key::ArrowLeft,
            KeyCode::Right => Key::ArrowRight,
            KeyCode::Up => Key::ArrowUp,
            KeyCode::Down => Key::ArrowDown,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Delete => Key::Del,
            KeyCode::Insert => Key::Insert,
            KeyCode::Esc => Key::Escape,
            KeyCode::Char('a' | 'A') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Key::Home
            }
            KeyCode::Char('e' | 'E') if event.modifiers.contains(KeyModifiers::CONTROL) => Key::End,
            KeyCode::Char(c)
                if !event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Key::Char(c)
            }
            _ => Key::Unknown,
        })
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
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

#[derive(Clone, Copy)]
pub(crate) enum Availability {
    Local,
    RemoteOnly,
    Missing,
}

impl Availability {
    fn is_missing(self) -> bool {
        matches!(self, Availability::Missing)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickKind {
    Branch,
    Worktree,
}

#[derive(Clone)]
pub(crate) struct Pick {
    pub name: String,
    pub is_current: bool,
    pub availability: Availability,
    pub kind: PickKind,
}

pub(crate) struct Section {
    pub heading: &'static str,
    pub items: Vec<Pick>,
}

enum RowKind {
    Heading(String),
    Item(Pick),
    CreateNew(String),
}

struct RenderRow {
    kind: RowKind,
    section_idx: usize,
}

struct View {
    rows: Vec<RenderRow>,
    selectable: Vec<usize>,
}

pub(crate) enum Selection {
    Existing { name: String, kind: PickKind },
    Create(String),
}

#[derive(Clone, Copy)]
pub(crate) struct PickerOptions {
    pub prompt: &'static str,
    pub allow_create_from_filter: bool,
}

fn select_branch(current: Option<&str>, remote: &str) -> AppResult<Option<String>> {
    let sections = build_sections(current, remote, &HashSet::new())?;
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

/// Subsequence match against a pre-lowered needle. Lowering happens at the
/// call site so the needle is normalized once per filter, not once per item.
fn fuzzy_match(needle_lower: &str, haystack: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let mut hi = haystack.chars().flat_map(char::to_lowercase);
    'next: for nc in needle_lower.chars() {
        for hc in hi.by_ref() {
            if hc == nc {
                continue 'next;
            }
        }
        return false;
    }
    true
}

fn build_view(sections: &[Section], filter: &str, opts: PickerOptions) -> View {
    let needle: String = filter.chars().flat_map(char::to_lowercase).collect();
    let mut rows: Vec<RenderRow> = Vec::new();
    let mut selectable: Vec<usize> = Vec::new();

    for (sec_idx, sec) in sections.iter().enumerate() {
        let matching: Vec<&Pick> = sec
            .items
            .iter()
            .filter(|p| fuzzy_match(&needle, &p.name))
            .collect();
        if matching.is_empty() {
            continue;
        }
        rows.push(RenderRow {
            kind: RowKind::Heading(sec.heading.to_string()),
            section_idx: sec_idx,
        });
        for pick in matching {
            let idx = rows.len();
            let is_selectable = !pick.availability.is_missing();
            rows.push(RenderRow {
                kind: RowKind::Item(pick.clone()),
                section_idx: sec_idx,
            });
            if is_selectable {
                selectable.push(idx);
            }
        }
    }

    if opts.allow_create_from_filter && selectable.is_empty() && !filter.is_empty() {
        let idx = rows.len();
        rows.push(RenderRow {
            kind: RowKind::CreateNew(filter.to_string()),
            section_idx: 0,
        });
        selectable.push(idx);
    }

    View { rows, selectable }
}

fn cursor_selection(view: &View, cursor: usize) -> Option<Selection> {
    let &row_idx = view.selectable.get(cursor)?;
    match &view.rows[row_idx].kind {
        RowKind::Item(p) => Some(Selection::Existing {
            name: p.name.clone(),
            kind: p.kind,
        }),
        RowKind::CreateNew(name) => Some(Selection::Create(name.clone())),
        RowKind::Heading(_) => None,
    }
}

fn selectable_position(view: &View, name: &str) -> Option<usize> {
    view.selectable
        .iter()
        .position(|&i| matches!(&view.rows[i].kind, RowKind::Item(p) if p.name == name))
}

/// The single-select picker. `keys` is taken by value so the raw mode it holds
/// is released when this returns: under raw mode a newline moves down without
/// returning to column 0, so anything a caller printed while still holding the
/// key source would staircase across the terminal.
pub(crate) fn pick(
    current: Option<&str>,
    sections: &[Section],
    opts: PickerOptions,
    mut keys: impl KeySource,
) -> AppResult<Option<Selection>> {
    let term = Term::stderr();
    let _cursor_guard = CursorGuard::hide();

    let mut filter = String::new();
    let mut view = build_view(sections, &filter, opts);
    if view.selectable.is_empty() && !opts.allow_create_from_filter {
        return Ok(None);
    }

    let mut cursor: usize = current
        .and_then(|c| selectable_position(&view, c))
        .unwrap_or(0);

    let mut drawn = render(&term, &view, cursor, &filter, opts.prompt);

    loop {
        let key = keys.read_key()?;
        let preserved = match cursor_selection(&view, cursor) {
            Some(Selection::Existing { name, .. }) => Some(name),
            _ => None,
        };
        let mut filter_changed = false;

        match key {
            Key::ArrowUp => {
                if !view.selectable.is_empty() {
                    cursor = if cursor == 0 {
                        view.selectable.len() - 1
                    } else {
                        cursor - 1
                    };
                }
            }
            Key::ArrowDown => {
                if !view.selectable.is_empty() {
                    cursor = (cursor + 1) % view.selectable.len();
                }
            }
            Key::PageUp => {
                let page = page_size(&term);
                cursor = cursor.saturating_sub(page);
            }
            Key::PageDown => {
                let page = page_size(&term);
                let last = view.selectable.len().saturating_sub(1);
                cursor = (cursor + page).min(last);
            }
            Key::Home => cursor = 0,
            Key::End => {
                cursor = view.selectable.len().saturating_sub(1);
            }
            Key::Char(c) if !c.is_control() => {
                filter.push(c);
                filter_changed = true;
            }
            Key::Backspace => {
                if filter.pop().is_some() {
                    filter_changed = true;
                }
            }
            Key::Enter => {
                let selection = cursor_selection(&view, cursor);
                let _ = term.clear_last_lines(drawn);
                return Ok(selection);
            }
            Key::Escape => {
                if filter.is_empty() {
                    let _ = term.clear_last_lines(drawn);
                    return Ok(None);
                }
                filter.clear();
                filter_changed = true;
            }
            _ => continue,
        }

        if filter_changed {
            view = build_view(sections, &filter, opts);
            cursor = preserved
                .as_deref()
                .and_then(|n| selectable_position(&view, n))
                .unwrap_or(0);
        }

        let _ = term.clear_last_lines(drawn);
        drawn = render(&term, &view, cursor, &filter, opts.prompt);
    }
}

fn page_size(term: &Term) -> usize {
    let h = term.size().0 as usize;
    h.saturating_sub(2).max(1)
}

fn render(term: &Term, view: &View, cursor: usize, filter: &str, prompt_label: &str) -> usize {
    let (rows_term, cols_term) = term.size();
    let height = rows_term as usize;
    let width = cols_term as usize;

    let prompt = format!(
        "{} {} {} {}",
        style("?").green().bold(),
        style(prompt_label).bold(),
        style("(type to filter):").dim(),
        filter,
    );
    render_line(&prompt);
    let mut drawn = visual_rows(&prompt, width);

    if view.selectable.is_empty() {
        let line = style("  (no matches)").dim().to_string();
        render_line(&line);
        drawn += visual_rows(&line, width);
        return drawn;
    }

    // Reserve one trailing line of headroom. If the render filled the full
    // terminal height, the final newline would scroll the screen up by a line
    // each redraw; `clear_last_lines` then can't reach the scrolled-off prompt
    // (cursor-up clamps at the top row), so stale prompt lines pile up and the
    // live prompt scrolls out of view.
    let viewport_h = height.saturating_sub(drawn + 1).max(3);
    let total_rows = view.rows.len();
    let cursor_row = view.selectable.get(cursor).copied().unwrap_or(0);

    let cursor_section = view.rows[cursor_row].section_idx;
    let cursor_heading_row = view
        .rows
        .iter()
        .position(|r| r.section_idx == cursor_section && matches!(r.kind, RowKind::Heading(_)));

    let mut scroll = if total_rows <= viewport_h || cursor_row + 1 < viewport_h {
        0
    } else {
        cursor_row + 1 - viewport_h
    };

    let sticky = cursor_heading_row.is_some_and(|h| h < scroll);
    let content_h = if sticky {
        viewport_h.saturating_sub(1).max(1)
    } else {
        viewport_h
    };

    if sticky && cursor_row >= scroll + content_h {
        scroll = cursor_row + 1 - content_h;
    }

    if sticky
        && let Some(h) = cursor_heading_row
        && let RowKind::Heading(text) = &view.rows[h].kind
    {
        let line = style(text).bold().dim().to_string();
        render_line(&line);
        drawn += visual_rows(&line, width);
    }

    let end = (scroll + content_h).min(total_rows);
    for r in scroll..end {
        let line = format_row(&view.rows[r], r == cursor_row);
        render_line(&line);
        drawn += visual_rows(&line, width);
    }

    drawn
}

fn format_row(row: &RenderRow, is_cursor: bool) -> String {
    match &row.kind {
        RowKind::Heading(text) => style(text).bold().dim().to_string(),
        RowKind::Item(pick) => {
            let cursor = if is_cursor { ">" } else { " " };
            let name_with_mark = if pick.is_current {
                format!("* {}", pick.name)
            } else {
                pick.name.clone()
            };
            let suffix = match pick.availability {
                Availability::Local => "",
                Availability::RemoteOnly => " ☁",
                Availability::Missing => " (missing)",
            };
            let line = format!("  {cursor} {name_with_mark}{suffix}");
            if pick.availability.is_missing() {
                style(line).dim().to_string()
            } else {
                line
            }
        }
        RowKind::CreateNew(name) => {
            let cursor = if is_cursor { ">" } else { " " };
            format!(
                "  {cursor} {} {}",
                style("+").green().bold(),
                style(format!("Create new: {name}")).italic()
            )
        }
    }
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

/// A stale branch, together with the worktree holding it (if any) and what
/// deleting it would destroy.
struct StaleRow {
    branch: String,
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
    let selections = multi_select(
        "Delete stale branches (space to toggle, →/← all/none)",
        &items,
        &defaults,
        keys,
    )?;

    // Delete from the main worktree, the same HEAD the risk was judged against.
    let mut steps = removal::GitSteps::at_main(main_dir.as_deref());
    for &i in &selections {
        delete_stale_row(&mut steps, &rows[i])?;
    }

    Ok(())
}

/// Builds the picker rows, pairing each stale branch with the worktree holding
/// it and what deleting it would destroy. `dirty` is injected so the rule can be
/// tested without a repo on disk.
fn stale_rows(
    stale: Vec<String>,
    worktrees: &[git::Worktree],
    unmerged: &std::collections::HashMap<String, git::Unmerged>,
    destination: Option<&str>,
    dirty: &dyn Fn(&Path) -> bool,
) -> Vec<StaleRow> {
    stale
        .into_iter()
        .filter(|branch| destination != Some(branch.as_str()))
        .map(|branch| {
            let worktree = git::worktree_for_branch(worktrees, &branch);
            let risk = Risk {
                dirty: worktree
                    .as_ref()
                    .is_some_and(|w| !w.prunable && dirty(&w.path)),
                unmerged: unmerged.get(&branch).copied(),
            };
            StaleRow {
                branch,
                worktree,
                risk,
            }
        })
        .collect()
}

/// The picker row for a stale branch, as a (name, annotation) pair for
/// [`align_labels`]. Dirtiness belongs to the worktree so it sits inside the
/// parentheses; unmerged commits belong to the branch so they sit outside. The
/// worktree's path is deliberately absent — it appears in the outcome line.
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

    let annotation = [worktree, branch_risk]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (row.branch.clone(), annotation)
}

/// Removes a ticked stale row and prints what happened. The ordering, and the
/// rule that only a shown marker licenses forcing, belong to [`removal`]; the
/// wording belongs to [`reporting`]. This is what's left: which target the row
/// describes.
fn delete_stale_row(steps: &mut impl removal::Steps, row: &StaleRow) -> AppResult<()> {
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

/// Pads (name, annotation) pairs so annotations line up in a column. Rows
/// without an annotation are left bare, so an unannotated list gains no
/// trailing whitespace.
pub(crate) fn align_labels(rows: &[(String, String)]) -> Vec<String> {
    let width = rows
        .iter()
        .filter(|(_, a)| !a.is_empty())
        .map(|(name, _)| measure_text_width(name))
        .max()
        .unwrap_or(0);

    rows.iter()
        .map(|(name, annotation)| {
            if annotation.is_empty() {
                return name.clone();
            }
            let pad = " ".repeat(width.saturating_sub(measure_text_width(name)));
            format!("{name}{pad}  {annotation}")
        })
        .collect()
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

fn visual_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let w = measure_text_width(text);
    if w == 0 { 1 } else { w.div_ceil(width) }
}

fn render_line(line: &str) {
    // Emit `\r\n` explicitly: raw mode disables the terminal's `\n`→`\r\n`
    // translation. Routing through `eprint!` (rather than a raw fd write) keeps
    // libtest's output capture working, so passing picker tests stay quiet.
    eprint!("{line}\r\n");
}

/// The multi-select picker. `keys` is taken by value for the same reason as
/// [`pick`]: raw mode ends with the call, not with the caller's scope.
pub(crate) fn multi_select(
    prompt: &str,
    items: &[String],
    defaults: &[bool],
    mut keys: impl KeySource,
) -> AppResult<Vec<usize>> {
    let term = Term::stderr();
    let mut selected = defaults.to_vec();
    let mut cursor = 0usize;
    let header = format!("{} {}", style("?").green().bold(), style(prompt).bold());

    let _cursor_guard = CursorGuard::hide();

    let draw = |cursor: usize, selected: &[bool]| -> usize {
        let (rows_term, cols_term) = term.size();
        let (height, width) = (rows_term as usize, cols_term as usize);
        let mut rows = visual_rows(&header, width);
        render_line(&header);

        // Scroll a window of items around the cursor and reserve a trailing line
        // of headroom, so a long list never overflows the screen and scrolls the
        // prompt out of `clear_last_lines`' reach (see `render`).
        let viewport = height.saturating_sub(rows + 1).max(1);
        let total = items.len();
        let scroll = if total <= viewport || cursor + 1 < viewport {
            0
        } else {
            (cursor + 1 - viewport).min(total - viewport)
        };
        let end = (scroll + viewport).min(total);
        for i in scroll..end {
            let arrow = if i == cursor { ">" } else { " " };
            let check = if selected[i] { "[x]" } else { "[ ]" };
            let line = format!("  {arrow} {check} {}", items[i]);
            rows += visual_rows(&line, width);
            render_line(&line);
        }
        rows
    };

    let clear = |n: usize| {
        let _ = term.clear_last_lines(n);
    };

    let mut drawn = draw(cursor, &selected);

    loop {
        match keys.read_key()? {
            Key::ArrowUp if cursor > 0 => cursor -= 1,
            Key::ArrowDown if cursor + 1 < items.len() => cursor += 1,
            Key::Char(' ') => selected[cursor] = !selected[cursor],
            Key::ArrowRight => selected.fill(true),
            Key::ArrowLeft => selected.fill(false),
            Key::Enter => break,
            Key::Escape => {
                clear(drawn);
                return Ok(vec![]);
            }
            _ => continue,
        }

        clear(drawn);
        drawn = draw(cursor, &selected);
    }

    Ok(selected
        .iter()
        .enumerate()
        .filter(|(_, s)| **s)
        .map(|(i, _)| i)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives an event loop from a fixed list of keys. Once exhausted it yields
    /// `Escape` so a test that under-specifies its script bails out of the loop
    /// rather than hanging.
    struct ScriptedKeys(std::vec::IntoIter<Key>);

    impl ScriptedKeys {
        fn new(keys: Vec<Key>) -> Self {
            Self(keys.into_iter())
        }
    }

    impl KeySource for ScriptedKeys {
        fn read_key(&mut self) -> std::io::Result<Key> {
            Ok(self.0.next().unwrap_or(Key::Escape))
        }
    }

    fn section(heading: &'static str, names: &[&str]) -> Section {
        Section {
            heading,
            items: names
                .iter()
                .map(|n| Pick {
                    name: (*n).to_string(),
                    is_current: false,
                    availability: Availability::Local,
                    kind: PickKind::Branch,
                })
                .collect(),
        }
    }

    /// Keys for typing a literal string into the filter.
    fn typed(s: &str) -> Vec<Key> {
        s.chars().map(Key::Char).collect()
    }

    const SELECT_OPTS: PickerOptions = PickerOptions {
        prompt: "Test",
        allow_create_from_filter: false,
    };

    const CREATE_OPTS: PickerOptions = PickerOptions {
        prompt: "Test",
        allow_create_from_filter: true,
    };

    fn run_pick(sections: &[Section], opts: PickerOptions, keys: Vec<Key>) -> Option<Selection> {
        pick(None, sections, opts, ScriptedKeys::new(keys)).expect("pick should not error")
    }

    fn picked_name(sel: Option<Selection>) -> Option<String> {
        match sel {
            Some(Selection::Existing { name, .. }) => Some(name),
            _ => None,
        }
    }

    #[test]
    fn type_to_filter_then_enter_selects_match() {
        let sections = vec![section("Local", &["main", "feature", "develop"])];
        let mut keys = typed("feat");
        keys.push(Key::Enter);
        let sel = run_pick(&sections, SELECT_OPTS, keys);
        assert_eq!(picked_name(sel).as_deref(), Some("feature"));
    }

    #[test]
    fn arrow_up_from_first_wraps_to_last() {
        let sections = vec![section("Local", &["a", "b", "c"])];
        let sel = run_pick(&sections, SELECT_OPTS, vec![Key::ArrowUp, Key::Enter]);
        assert_eq!(picked_name(sel).as_deref(), Some("c"));
    }

    #[test]
    fn arrow_down_from_last_wraps_to_first() {
        let sections = vec![section("Local", &["a", "b", "c"])];
        // End lands on the last row; ArrowDown should wrap back to the first.
        let sel = run_pick(
            &sections,
            SELECT_OPTS,
            vec![Key::End, Key::ArrowDown, Key::Enter],
        );
        assert_eq!(picked_name(sel).as_deref(), Some("a"));
    }

    #[test]
    fn non_matching_filter_with_enter_creates() {
        let sections = vec![section("Local", &["main"])];
        let mut keys = typed("xyz");
        keys.push(Key::Enter);
        let sel = run_pick(&sections, CREATE_OPTS, keys);
        match sel {
            Some(Selection::Create(name)) => assert_eq!(name, "xyz"),
            _ => panic!("expected Selection::Create"),
        }
    }

    #[test]
    fn cursor_navigation_skips_headings() {
        let sections = vec![section("Pinned", &["p1"]), section("Local", &["l1", "l2"])];
        // From p1, one ArrowDown should land on l1, stepping over the "Local"
        // heading row rather than onto it.
        let sel = run_pick(&sections, SELECT_OPTS, vec![Key::ArrowDown, Key::Enter]);
        assert_eq!(picked_name(sel).as_deref(), Some("l1"));
    }

    #[test]
    fn escape_on_empty_filter_returns_none() {
        let sections = vec![section("Local", &["a", "b"])];
        let sel = run_pick(&sections, SELECT_OPTS, vec![Key::Escape]);
        assert!(sel.is_none());
    }

    #[test]
    fn escape_on_nonempty_filter_clears_it() {
        let sections = vec![section("Local", &["alpha", "beta"])];
        // Filter down to "beta" (hiding alpha), Escape to clear the filter, then
        // Home + Enter selects alpha — proving it is back in the list.
        let mut keys = typed("beta");
        keys.extend([Key::Escape, Key::Home, Key::Enter]);
        let sel = run_pick(&sections, SELECT_OPTS, keys);
        assert_eq!(picked_name(sel).as_deref(), Some("alpha"));
    }

    fn run_multi_select(items: &[&str], defaults: &[bool], keys: Vec<Key>) -> Vec<usize> {
        let items: Vec<String> = items.iter().map(|s| (*s).to_string()).collect();
        multi_select("Test", &items, defaults, ScriptedKeys::new(keys))
            .expect("multi_select should not error")
    }

    #[test]
    fn multi_select_space_toggles_returns_index_set() {
        let got = run_multi_select(
            &["a", "b", "c"],
            &[false, false, false],
            vec![
                Key::Char(' '),
                Key::ArrowDown,
                Key::ArrowDown,
                Key::Char(' '),
                Key::Enter,
            ],
        );
        assert_eq!(got, vec![0, 2]);
    }

    #[test]
    fn multi_select_right_selects_all() {
        let got = run_multi_select(
            &["a", "b"],
            &[false, false],
            vec![Key::ArrowRight, Key::Enter],
        );
        assert_eq!(got, vec![0, 1]);
    }

    #[test]
    fn multi_select_left_selects_none() {
        let got = run_multi_select(&["a", "b"], &[true, true], vec![Key::ArrowLeft, Key::Enter]);
        assert!(got.is_empty());
    }

    #[test]
    fn multi_select_escape_returns_empty() {
        let got = run_multi_select(&["a", "b"], &[true, true], vec![Key::Escape]);
        assert!(got.is_empty());
    }

    fn stale_row(branch: &str, worktree: Option<git::Worktree>, risk: Risk) -> StaleRow {
        StaleRow {
            branch: branch.to_string(),
            worktree,
            risk,
        }
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

    #[test]
    fn stale_label_without_worktree_is_bare() {
        let row = stale_row("fix/typo", None, Risk::default());
        let (name, annotation) = stale_label(&row);
        assert_eq!(name, "fix/typo");
        assert!(annotation.is_empty(), "got: {annotation}");
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
        assert_eq!(plain(&stale_label(&row).1), "(+ worktree ●)");
    }

    #[test]
    fn stale_label_marks_a_missing_worktree() {
        let row = stale_row("old/thing", Some(worktree(true)), Risk::default());
        assert_eq!(plain(&stale_label(&row).1), "(+ worktree, missing)");
    }

    /// Dirtiness belongs to the worktree, unmerged commits to the branch — so
    /// one sits inside the parentheses and the other outside.
    #[test]
    fn stale_label_puts_unmerged_outside_the_parens() {
        let row = stale_row(
            "spike/abandoned",
            Some(worktree(false)),
            Risk {
                dirty: true,
                unmerged: Some(git::Unmerged::Ahead(2)),
            },
        );
        assert_eq!(plain(&stale_label(&row).1), "(+ worktree ●) ↑2");
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
        let stale = vec!["feature".to_string(), "fix/typo".to_string()];
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
        let stale = vec!["feature".to_string(), "fix/typo".to_string()];
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
            vec!["feature".to_string()],
            &worktrees,
            &std::collections::HashMap::new(),
            None,
            &|path| path == Path::new("/tmp/wt"),
        );
        assert!(rows[0].risk.dirty);
        assert!(rows[0].worktree.is_some());
    }

    #[test]
    fn align_labels_pads_annotations_into_a_column() {
        let rows = vec![
            ("short".to_string(), "(+ worktree)".to_string()),
            ("much-longer-name".to_string(), "↑1".to_string()),
        ];
        let got = align_labels(&rows);
        assert_eq!(got[0], "short             (+ worktree)");
        assert_eq!(got[1], "much-longer-name  ↑1");
    }

    #[test]
    fn align_labels_leaves_unannotated_rows_bare() {
        let rows = vec![
            ("a".to_string(), String::new()),
            ("bb".to_string(), "↑1".to_string()),
        ];
        let got = align_labels(&rows);
        assert_eq!(got[0], "a", "no trailing padding on a bare row");
        assert_eq!(got[1], "bb  ↑1");
    }
}

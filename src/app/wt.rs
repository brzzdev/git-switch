use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use console::{measure_text_width, style};
use indicatif::ProgressBar;

use super::{
    Availability, CursorGuard, Pick, PickKind, PickerOptions, Risk, Section, Selection,
    build_sections, confirm, display_path, fetch_and_ff, handoff_cd, interactive_keys, marker,
    multi_select, pick, prompt_delete_stale_branches, removal, report_update, reporting,
};
use crate::{AppResult, Error, git};

enum Action {
    CdToExisting(git::Worktree),
    CreateForBranch(String),
    CreateNewBranch(String),
}

pub fn run(target: Option<&str>) -> AppResult<()> {
    // Drop worktrees whose directory was deleted by hand: they can't be entered,
    // so treat their branch as one to (re)create. `worktree_add`/`checkout` prune
    // the stale registration when it gets in the way.
    let worktrees: Vec<git::Worktree> = git::worktree_list()?
        .into_iter()
        .filter(|w| !w.prunable)
        .collect();
    let main = main_of(&worktrees)?;
    let current_branch = git::current_branch()?;
    let remote = git::current_remote(current_branch.as_deref());

    let action = match target {
        Some(name) => resolve_target(name, &worktrees, &remote)?,
        None => match select(&worktrees, current_branch.as_deref(), &remote)? {
            Some(a) => a,
            None => return Ok(()),
        },
    };

    // The branch comes back alongside the path so the stale prompt can leave the
    // worktree we're about to enter alone.
    let (target_path, target_branch) = match action {
        Action::CdToExisting(wt) => {
            let branch = wt.branch.clone().unwrap_or_default();
            // The worktree's branch may track a different remote than ours.
            let branch_remote = git::current_remote(Some(branch.as_str()));
            if let Err(e) = update_in(&wt.path, &branch, &branch_remote) {
                eprintln!(
                    "{} update of {} failed: {e}",
                    style("!").yellow().bold(),
                    branch,
                );
            }
            eprintln!(
                "{} switched to worktree at {}",
                style("→").cyan().bold(),
                wt.path.display()
            );
            (wt.path, branch)
        }
        Action::CreateForBranch(branch) => {
            let branch_remote = git::current_remote(Some(branch.as_str()));
            let path = create_worktree(&main.path, &branch, None, &branch_remote)?;
            (path, branch)
        }
        Action::CreateNewBranch(branch) => {
            let default = git::default_branch(&remote).ok_or_else(|| Error::Git {
                command: "worktree add".into(),
                message: format!("no default branch on {remote}; cannot pick a base"),
            })?;
            let base = format!("{remote}/{default}");
            let path = create_worktree(&main.path, &branch, Some(&base), &remote)?;
            (path, branch)
        }
    };

    if let Err(e) = prompt_delete_stale_branches(None, Some(&target_branch), &remote) {
        if e.is_interrupt() {
            return Err(e);
        }
        eprintln!(
            "{} stale-branch check failed: {e}",
            style("!").yellow().bold()
        );
    }
    handoff_cd(&target_path);
    Ok(())
}

pub fn run_ls() -> AppResult<()> {
    let worktrees = git::worktree_list()?;
    let track = git::ahead_behind_map();

    let max_branch = worktrees
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or("(detached)").len())
        .max()
        .unwrap_or(0);

    // Build the styled status segment per row up front so the path column can be
    // aligned to the widest one (ANSI codes inflate byte length, so pad by
    // visible width rather than relying on `{:width$}`).
    let statuses: Vec<String> = worktrees
        .iter()
        .map(|w| {
            let dirty = !w.prunable && git::worktree_dirty(&w.path);
            let (ahead, behind) = w
                .branch
                .as_deref()
                .and_then(|b| track.get(b).copied())
                .unwrap_or((0, 0));
            marker::worktree_status(dirty, ahead, behind)
        })
        .collect();
    let status_width = statuses
        .iter()
        .map(|s| measure_text_width(s))
        .max()
        .unwrap_or(0);

    for (w, status) in worktrees.iter().zip(&statuses) {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        let main_mark = if w.is_main { "*" } else { " " };
        if status_width == 0 {
            println!("{main_mark} {label:max_branch$}  {}", w.path.display());
        } else {
            let pad = " ".repeat(status_width - measure_text_width(status));
            println!(
                "{main_mark} {label:max_branch$}  {status}{pad}  {}",
                w.path.display()
            );
        }
    }
    Ok(())
}

pub fn run_rm(target: Option<&str>, force: bool) -> AppResult<()> {
    let worktrees = git::worktree_list()?;
    let main = main_of(&worktrees)?.clone();
    // Include branchless (detached) and missing worktrees too: a worktree whose
    // directory was deleted by hand often shows up detached, and cleaning up its
    // dead registration is exactly what `wt rm` is for.
    let removable: Vec<git::Worktree> = worktrees.into_iter().filter(|w| !w.is_main).collect();

    if removable.is_empty() {
        eprintln!("No worktrees to remove.");
        return Ok(());
    }

    // Canonicalize both sides: `env::current_dir()` and git's reported path can
    // disagree on symlinks (e.g. macOS /var vs /private/var), which would let a
    // plain `starts_with` miss a cwd that really is inside a doomed worktree.
    let cwd = env::current_dir().ok().and_then(|c| c.canonicalize().ok());
    let contains_cwd = |w: &git::Worktree| {
        let wt_path = w.path.canonicalize().unwrap_or_else(|_| w.path.clone());
        cwd.as_ref().is_some_and(|c| c.starts_with(&wt_path))
    };

    // The worktree the cwd sits in, for the `(current)` picker marker. Longest
    // path wins so a nested worktree beats its enclosing one.
    let current = removable
        .iter()
        .enumerate()
        .filter(|(_, w)| contains_cwd(w))
        .max_by_key(|(_, w)| w.path.as_os_str().len())
        .map(|(i, _)| i);

    // Judge merged-ness from the main worktree: it's the HEAD that will be
    // current when the branch delete runs, and it can't be one of the branches
    // being removed. Asking from a doomed worktree would call its own branch
    // merged and skip the warning.
    let unmerged = git::unmerged_branches(Some(&main.path)).unwrap_or_default();
    let risks: Vec<Risk> = removable
        .iter()
        .map(|w| Risk {
            dirty: !w.prunable && git::worktree_dirty(&w.path),
            unmerged: w.branch.as_deref().and_then(|b| unmerged.get(b).copied()),
        })
        .collect();

    let selected_indices = select_for_removal(&removable, &risks, current, target, force)?;
    if selected_indices.is_empty() {
        return Ok(());
    }

    let cwd_will_vanish = selected_indices
        .iter()
        .any(|&i| contains_cwd(&removable[i]));

    if cwd_will_vanish {
        env::set_current_dir(&main.path)?;
    }

    // Delete from the same worktree the risk was judged in, so `-d`'s idea of
    // merged matches the marker that licensed the removal.
    let mut steps = removal::GitSteps::at_main(Some(&main.path));
    for &i in &selected_indices {
        remove_one(&mut steps, &removable[i], risks[i], force)?;
    }

    if cwd_will_vanish {
        handoff_cd(&main.path);
    }
    Ok(())
}

/// Resolves which worktrees to remove: a single named target (`.` for the one
/// the cwd sits in), or a multi-select whose rows carry risk markers.
fn select_for_removal(
    removable: &[git::Worktree],
    risks: &[Risk],
    current: Option<usize>,
    target: Option<&str>,
    force: bool,
) -> AppResult<Vec<usize>> {
    let Some(name) = target else {
        // Non-interactive (piped/CI): we can't prompt, so remove nothing rather
        // than blocking on key input.
        let Some(mut keys) = interactive_keys() else {
            return Ok(vec![]);
        };
        let items = super::align_labels(
            &removable
                .iter()
                .enumerate()
                .map(|(i, w)| (rm_label(w, current == Some(i)), marker::markers(risks[i])))
                .collect::<Vec<_>>(),
        );
        let defaults = vec![false; items.len()];
        return multi_select(
            "Remove worktrees (space to toggle, →/← all/none)",
            &items,
            &defaults,
            &mut keys,
        );
    };

    // `.` means the worktree the cwd sits in, matching `git-switch .` for the
    // current branch. The main worktree isn't removable, so standing in it
    // leaves nothing for `.` to name.
    let i = if name == "." {
        current.ok_or_else(|| Error::Git {
            command: "worktree remove".into(),
            message: "the main worktree cannot be removed".into(),
        })?
    } else {
        removable
            .iter()
            .position(|w| rm_matches(w, name))
            .ok_or_else(|| Error::Git {
                command: "worktree remove".into(),
                message: format!("no worktree matching '{name}'"),
            })?
    };

    // A named target never passed through a picker, so no marker warned about
    // what it would destroy; the confirmation stands in for one.
    Ok(if confirm_removal(&removable[i], risks[i], force)? {
        vec![i]
    } else {
        vec![]
    })
}

/// Removes one worktree and, when it has a branch, deletes that too, then prints
/// what happened. The ordering and the licensing rule belong to [`removal`], the
/// wording to [`reporting`] — which is what makes the answer here word for word
/// the stale-branch prompt's, so it doesn't depend on how you got here.
fn remove_one(
    steps: &mut impl removal::Steps,
    wt: &git::Worktree,
    risk: Risk,
    force: bool,
) -> AppResult<()> {
    let target = match wt.branch.as_deref() {
        Some(name) => removal::Target::Held {
            name,
            path: &wt.path,
        },
        None => removal::Target::Worktree { path: &wt.path },
    };
    let license = if force {
        removal::License::forced()
    } else {
        removal::License::shown(risk)
    };
    let report = removal::remove(target, license, steps)?;

    for line in reporting::removal_outcome(&report) {
        eprintln!("{line}");
    }
    Ok(())
}

/// Fetch + fast-forward `branch` in the worktree at `path`. Unlike the in-place
/// switch, a diverged branch is only reported (we don't drive an interactive
/// rebase in a worktree the user isn't sitting in).
pub(crate) fn update_in(path: &Path, branch: &str, remote: &str) -> AppResult<()> {
    match fetch_and_ff(Some(path), branch, remote)? {
        git::FastForwardResult::Diverged => eprintln!(
            "{} {} has diverged from {}/{}; not updating.",
            style("!").yellow().bold(),
            branch,
            remote,
            branch,
        ),
        // A worktree branch with no upstream is unremarkable here — unlike the
        // in-place switch, stay quiet rather than printing "No remote…".
        git::FastForwardResult::Merged(git::MergeReport::NoRemote) => {}
        git::FastForwardResult::Merged(report) => report_update(&report),
    }
    Ok(())
}

fn create_worktree(
    main_path: &Path,
    branch: &str,
    base: Option<&str>,
    remote: &str,
) -> AppResult<PathBuf> {
    let path = worktree_path_for(main_path, branch)?;
    ensure_path_clear(&path)?;
    ensure_parent(&path);

    let result = {
        let spinner = ProgressBar::new_spinner();
        let _g = CursorGuard::hide();
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        spinner.set_message(format!("Fetching {remote}…"));
        let _ = git::fetch(None, remote);
        spinner.set_message(format!("Creating worktree for {branch}…"));
        let outcome = git::worktree_add(&path, branch, base);
        spinner.finish_and_clear();
        outcome
    };
    result?;

    if let Some(base) = base {
        eprintln!(
            "{} created {} from {} at {}",
            style("✓").green().bold(),
            branch,
            base,
            path.display()
        );
    } else {
        eprintln!(
            "{} created worktree at {}",
            style("✓").green().bold(),
            path.display()
        );
    }
    Ok(path)
}

fn select(
    worktrees: &[git::Worktree],
    current_branch: Option<&str>,
    remote: &str,
) -> AppResult<Option<Action>> {
    let sections = build_wt_sections(worktrees, current_branch, remote)?;
    let Some(mut keys) = interactive_keys() else {
        return Ok(None);
    };
    let selection = pick(
        current_branch,
        &sections,
        PickerOptions {
            prompt: "Worktree",
            allow_create_from_filter: true,
        },
        &mut keys,
    )?;
    let action = match selection {
        None => return Ok(None),
        Some(Selection::Existing {
            name,
            kind: PickKind::Worktree,
        }) => git::worktree_for_branch(worktrees, &name)
            .map(Action::CdToExisting)
            .unwrap_or(Action::CreateForBranch(name)),
        Some(Selection::Existing {
            name,
            kind: PickKind::Branch,
        }) => Action::CreateForBranch(name),
        Some(Selection::Create(name)) => Action::CreateNewBranch(name),
    };
    Ok(Some(action))
}

fn build_wt_sections(
    worktrees: &[git::Worktree],
    current_branch: Option<&str>,
    remote: &str,
) -> AppResult<Vec<Section>> {
    let held: HashSet<String> = worktrees.iter().filter_map(|w| w.branch.clone()).collect();

    let mut wt_picks: Vec<Pick> = worktrees
        .iter()
        .filter_map(|w| {
            w.branch.as_ref().map(|b| Pick {
                name: b.clone(),
                is_current: current_branch == Some(b.as_str()),
                availability: Availability::Local,
                kind: PickKind::Worktree,
            })
        })
        .collect();
    wt_picks.sort_by(|a, b| a.name.cmp(&b.name));

    let mut sections = Vec::new();
    if !wt_picks.is_empty() {
        sections.push(Section {
            heading: "Worktrees",
            items: wt_picks,
        });
    }
    sections.extend(build_sections(current_branch, remote, &held)?);
    Ok(sections)
}

fn resolve_target(name: &str, worktrees: &[git::Worktree], remote: &str) -> AppResult<Action> {
    if let Some(wt) = git::worktree_for_branch(worktrees, name) {
        return Ok(Action::CdToExisting(wt));
    }
    let locals = git::local_branches()?;
    if locals.iter().any(|b| b == name) {
        return Ok(Action::CreateForBranch(name.to_string()));
    }
    let remote_only = git::remote_only_branches(&locals, remote).unwrap_or_default();
    if remote_only.iter().any(|b| b == name) {
        return Ok(Action::CreateForBranch(name.to_string()));
    }
    Ok(Action::CreateNewBranch(name.to_string()))
}

/// Gate for a worktree named on the command line, where there is no picker row
/// to carry a marker. Nothing at risk means no prompt at all — `wt rm .` on a
/// clean, merged worktree is silent. Otherwise the risks are named and
/// confirmed, `--force` waives the prompt, and a run with no terminal to ask in
/// refuses rather than destroying anything unwarned.
fn confirm_removal(wt: &git::Worktree, risk: Risk, force: bool) -> AppResult<bool> {
    if force || !risk.any() {
        return Ok(true);
    }

    let subject = wt.branch.as_deref().unwrap_or("this worktree");

    if !super::is_interactive() {
        return Err(Error::Unconfirmed(format!(
            "{}; not removing. Rerun in a terminal to confirm, or pass --force.",
            reporting::describe(risk, subject, &wt.path).join(" and ")
        )));
    }

    for line in reporting::warnings(risk, subject, &wt.path) {
        eprintln!("{line}");
    }
    let question = match wt.branch.as_deref() {
        Some(branch) => format!("Remove the worktree and delete {branch} anyway?"),
        None => "Remove the worktree anyway?".to_string(),
    };
    confirm(&question, false)
}

/// Picker label for a removable worktree: its branch when it has one, else the
/// path (detached HEAD). Missing-on-disk worktrees are flagged so the user knows
/// the entry is a leftover registration; the worktree the cwd sits in is marked
/// `(current)` so it's identifiable even though it's labelled by branch.
fn rm_label(w: &git::Worktree, is_current: bool) -> String {
    let mut label = match &w.branch {
        Some(branch) => branch.clone(),
        None => display_path(&w.path),
    };
    if w.prunable {
        label.push_str(" (missing)");
    }
    if is_current {
        label.push_str(" (current)");
    }
    label
}

/// A `wt rm <name>` target matches a worktree by branch name or by the final
/// component of its path — the latter lets you name a detached/missing worktree.
fn rm_matches(w: &git::Worktree, name: &str) -> bool {
    w.branch.as_deref() == Some(name) || w.path.file_name().and_then(|n| n.to_str()) == Some(name)
}

fn main_of(worktrees: &[git::Worktree]) -> AppResult<&git::Worktree> {
    worktrees
        .iter()
        .find(|w| w.is_main)
        .ok_or_else(|| Error::Git {
            command: "worktree list".into(),
            message: "no main worktree found".into(),
        })
}

fn worktree_path_for(main_path: &Path, branch: &str) -> AppResult<PathBuf> {
    let parent = main_path.parent().ok_or_else(|| Error::Git {
        command: "worktree".into(),
        message: format!("main worktree has no parent: {}", main_path.display()),
    })?;
    let repo_name = main_path.file_name().ok_or_else(|| Error::Git {
        command: "worktree".into(),
        message: format!("cannot determine repo name from {}", main_path.display()),
    })?;
    Ok(parent.join("worktrees").join(repo_name).join(branch))
}

fn ensure_path_clear(path: &Path) -> AppResult<()> {
    if path.exists() {
        return Err(Error::Git {
            command: "worktree add".into(),
            message: format!(
                "{} exists but is not a registered worktree; remove it manually and retry.",
                path.display()
            ),
        });
    }
    Ok(())
}

fn ensure_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use console::style;
use indicatif::ProgressBar;

use super::{
    Availability, CursorGuard, Pick, PickKind, PickerOptions, Section, Selection, build_sections,
    fetch_and_ff, handoff_cd, interactive_term, multi_select, pick, prompt_delete_stale_branches,
    report_update,
};
use crate::{AppResult, Error, git};

enum Action {
    CdToExisting(git::Worktree),
    CreateForBranch(String),
    CreateNewBranch(String),
}

pub fn run(target: Option<&str>) -> AppResult<()> {
    let worktrees = git::worktree_list()?;
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

    let target_path = match action {
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
            wt.path
        }
        Action::CreateForBranch(branch) => {
            let branch_remote = git::current_remote(Some(branch.as_str()));
            create_worktree(&main.path, &branch, None, &branch_remote)?
        }
        Action::CreateNewBranch(branch) => {
            let default = git::default_branch(&remote).ok_or_else(|| Error::Git {
                command: "worktree add".into(),
                message: format!("no default branch on {remote}; cannot pick a base"),
            })?;
            let base = format!("{remote}/{default}");
            create_worktree(&main.path, &branch, Some(&base), &remote)?
        }
    };

    if let Err(e) = prompt_delete_stale_branches(None, &remote) {
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
    let max_branch = worktrees
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or("(detached)").len())
        .max()
        .unwrap_or(0);
    for w in worktrees {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        let main_mark = if w.is_main { "*" } else { " " };
        println!("{main_mark} {label:max_branch$}  {}", w.path.display());
    }
    Ok(())
}

pub fn run_rm(target: Option<&str>) -> AppResult<()> {
    let worktrees = git::worktree_list()?;
    let main = main_of(&worktrees)?.clone();
    let removable: Vec<git::Worktree> = worktrees
        .into_iter()
        .filter(|w| !w.is_main && w.branch.is_some())
        .collect();

    if removable.is_empty() {
        eprintln!("No worktrees to remove.");
        return Ok(());
    }

    let selected_indices: Vec<usize> = if let Some(name) = target {
        let i = removable
            .iter()
            .position(|w| w.branch.as_deref() == Some(name))
            .ok_or_else(|| Error::Git {
                command: "worktree remove".into(),
                message: format!("no worktree for branch '{name}'"),
            })?;
        vec![i]
    } else {
        // Non-interactive (piped/CI): we can't prompt, so remove nothing rather
        // than blocking on key input.
        let Some(mut term) = interactive_term() else {
            return Ok(());
        };
        let items: Vec<String> = removable
            .iter()
            .map(|w| w.branch.clone().unwrap_or_default())
            .collect();
        let defaults = vec![false; items.len()];
        multi_select(
            "Remove worktrees (space to toggle, →/← all/none)",
            &items,
            &defaults,
            &mut term,
        )?
    };

    if selected_indices.is_empty() {
        return Ok(());
    }

    // Canonicalize both sides: `env::current_dir()` and git's reported path can
    // disagree on symlinks (e.g. macOS /var vs /private/var), which would let a
    // plain `starts_with` miss a cwd that really is inside a doomed worktree.
    let cwd = env::current_dir().ok().and_then(|c| c.canonicalize().ok());
    let cwd_will_vanish = selected_indices.iter().any(|&i| {
        let wt_path = removable[i]
            .path
            .canonicalize()
            .unwrap_or_else(|_| removable[i].path.clone());
        cwd.as_ref().is_some_and(|c| c.starts_with(&wt_path))
    });

    if cwd_will_vanish {
        env::set_current_dir(&main.path)?;
    }

    for &i in &selected_indices {
        let wt = &removable[i];
        match git::worktree_remove(&wt.path)? {
            git::WorktreeRemoveOutcome::Removed => {
                let branch = wt.branch.as_deref().unwrap_or_default();
                match git::delete_branch_if_merged(branch)? {
                    git::BranchDeleteOutcome::Deleted => eprintln!(
                        "{} removed worktree {branch} (branch deleted)",
                        style("✓").green().bold(),
                    ),
                    git::BranchDeleteOutcome::NotMerged => eprintln!(
                        "{} removed worktree {branch}; kept branch with unmerged commits \
                         (run `git branch -D {branch}` to force-delete)",
                        style("!").yellow().bold(),
                    ),
                    git::BranchDeleteOutcome::Failed(detail) => eprintln!(
                        "{} removed worktree {branch} (branch delete failed: {detail})",
                        style("!").yellow().bold(),
                    ),
                }
            }
            git::WorktreeRemoveOutcome::Failed(detail) => {
                eprintln!(
                    "{} failed to remove {}:",
                    style("!").yellow().bold(),
                    wt.path.display()
                );
                for line in detail.lines() {
                    eprintln!("  {line}");
                }
            }
        }
    }

    if cwd_will_vanish {
        handoff_cd(&main.path);
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
    let Some(mut term) = interactive_term() else {
        return Ok(None);
    };
    let selection = pick(
        current_branch,
        &sections,
        PickerOptions {
            prompt: "Worktree",
            allow_create_from_filter: true,
        },
        &mut term,
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

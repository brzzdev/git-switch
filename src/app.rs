use console::{Key, Term, measure_text_width, style};
use dialoguer::FuzzySelect;
use indicatif::ProgressBar;

use crate::{AppResult, git};

struct CursorGuard(Term);

impl CursorGuard {
    fn hide() -> Self {
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

pub fn run(target: Option<&str>) -> AppResult<()> {
    let old_branch = git::current_branch()?;

    let target = match target {
        Some(name) => name.to_string(),
        None => match select_branch(old_branch.as_deref())? {
            Some(t) => t,
            None => return Ok(()),
        },
    };

    let stashed = if git::has_tracked_changes()? {
        git::stash_push()?;
        true
    } else {
        false
    };

    let result = switch_and_update(&target, old_branch.as_deref());

    if stashed {
        if result.is_err()
            && let Some(old) = old_branch.as_deref()
        {
            eprintln!("Switching back to {old} and restoring stashed changes.");
            let _ = git::checkout(old);
        }
        match git::stash_pop() {
            Ok(git::StashPopOutcome::Clean) => {}
            Ok(git::StashPopOutcome::Conflict) => {
                eprintln!(
                    "Conflicts detected restoring stashed changes. Resolve them, then run `git stash drop` to clean up the stash entry."
                );
            }
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!(
                    "Stash pop failed. Inspect `git status` and `git stash list` to recover manually."
                );
            }
        }
    }

    result
}

fn switch_and_update(target: &str, old_branch: Option<&str>) -> AppResult<()> {
    let already_on_target = old_branch.is_some_and(|b| b == target);

    if !already_on_target {
        git::checkout(target)?;
    }

    let (fetch_outcome, merge_result) = {
        let spinner = ProgressBar::new_spinner().with_message(format!("Updating {target}…"));
        let _cursor_guard = CursorGuard::hide();
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));

        let fetch_outcome =
            git::fetch().unwrap_or_else(|e| git::FetchOutcome::Failed(e.to_string()));
        let result = git::fast_forward_merge(target);

        spinner.finish_and_clear();
        (fetch_outcome, result)
    };

    if let git::FetchOutcome::Failed(detail) = &fetch_outcome {
        eprintln!(
            "{} fetch failed; results may be stale",
            style("!").yellow().bold()
        );
        for line in detail.lines() {
            eprintln!("  {line}");
        }
    }

    report_update(merge_result?)?;

    prompt_delete_stale_branches(if already_on_target { None } else { old_branch })?;

    Ok(())
}

fn report_update(result: git::MergeResult) -> AppResult<()> {
    match result {
        git::MergeResult::UpToDate => println!("Already up to date."),
        git::MergeResult::Pulled(1) => println!("Pulled 1 commit."),
        git::MergeResult::Pulled(n) => println!("Pulled {n} commits."),
        git::MergeResult::NoRemote => println!("No remote tracking branch."),
        git::MergeResult::Diverged(branch) => {
            eprintln!(
                "Local branch has diverged from origin/{branch}.\n\
                 Run `git rebase origin/{branch}` or `git merge origin/{branch}` to reconcile."
            );
            return Err("branch diverged from remote".into());
        }
    }
    Ok(())
}

struct BranchOption {
    display: String,
    checkout: String,
}

impl BranchOption {
    fn local(name: String) -> Self {
        Self {
            display: name.clone(),
            checkout: name,
        }
    }

    fn remote(name: String) -> Self {
        Self {
            display: format!("origin/{name}"),
            checkout: name,
        }
    }
}

fn select_branch(current: Option<&str>) -> AppResult<Option<String>> {
    let local = git::local_branches()?;
    let remote_only = git::remote_only_branches(&local).unwrap_or_default();

    if local.is_empty() && remote_only.is_empty() {
        return Err("no branches found".into());
    }

    let branches: Vec<BranchOption> = local
        .into_iter()
        .map(BranchOption::local)
        .chain(remote_only.into_iter().map(BranchOption::remote))
        .collect();

    let display: Vec<&str> = branches.iter().map(|b| b.display.as_str()).collect();

    let default = current
        .and_then(|c| branches.iter().position(|b| b.checkout == c))
        .unwrap_or(0);

    let selection = FuzzySelect::new()
        .with_prompt("Switch to")
        .items(&display)
        .default(default)
        .interact_opt()
        .map_err(std::io::Error::from)?;

    Ok(selection.map(|i| branches[i].checkout.clone()))
}

fn prompt_delete_stale_branches(old_branch: Option<&str>) -> AppResult<()> {
    let all_stale = git::stale_branches()?;
    if all_stale.is_empty() {
        return Ok(());
    }

    let held = git::worktree_branches().unwrap_or_default();
    let (locked, stale): (Vec<String>, Vec<String>) =
        all_stale.into_iter().partition(|b| held.contains(b));

    for branch in &locked {
        eprintln!(
            "{} stale but held by worktree, skipping: {}",
            style("!").yellow().bold(),
            branch
        );
    }

    if stale.is_empty() {
        return Ok(());
    }

    let defaults: Vec<bool> = stale
        .iter()
        .map(|b| old_branch.is_some_and(|old| old == b))
        .collect();

    let selections = multi_select(
        "Delete stale branches (space to toggle, →/← all/none)",
        &stale,
        &defaults,
    )?;

    let to_delete: Vec<&str> = selections.iter().map(|&i| stale[i].as_str()).collect();
    if !to_delete.is_empty() {
        git::delete_branches(&to_delete)?;
    }

    Ok(())
}

fn visual_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let w = measure_text_width(text);
    if w == 0 { 1 } else { w.div_ceil(width) }
}

fn multi_select(prompt: &str, items: &[String], defaults: &[bool]) -> AppResult<Vec<usize>> {
    let term = Term::stderr();
    let mut selected = defaults.to_vec();
    let mut cursor = 0usize;
    let header = format!("{} {}", style("?").green().bold(), style(prompt).bold());

    let _cursor_guard = CursorGuard::hide();

    let draw = |cursor: usize, selected: &[bool]| -> usize {
        let width = term.size().1 as usize;
        let mut rows = visual_rows(&header, width);
        eprintln!("{header}");
        for (i, item) in items.iter().enumerate() {
            let arrow = if i == cursor { ">" } else { " " };
            let check = if selected[i] { "[x]" } else { "[ ]" };
            let line = format!("  {arrow} {check} {item}");
            rows += visual_rows(&line, width);
            eprintln!("{line}");
        }
        rows
    };

    let clear = |n: usize| {
        if n > 0 {
            eprint!("\x1b[{n}F\x1b[J");
        }
    };

    let mut drawn = draw(cursor, &selected);

    loop {
        match term.read_key()? {
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

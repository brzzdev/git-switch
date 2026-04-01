use console::{Key, Term, style};
use dialoguer::Select;
use indicatif::ProgressBar;

use crate::{AppResult, git};

pub fn run(target: Option<&str>) -> AppResult<()> {
    let old_branch = git::current_branch()?;

    let target = match target {
        Some(name) => name.to_string(),
        None => select_branch(old_branch.as_deref())?,
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
        if let Err(e) = git::stash_pop() {
            eprintln!("error: {e}");
            eprintln!(
                "Your changes are still in the stash. Run `git stash pop` to restore them manually."
            );
        }
    }

    result
}

fn switch_and_update(target: &str, old_branch: Option<&str>) -> AppResult<()> {
    let already_on_target = old_branch.is_some_and(|b| b == target);

    if !already_on_target {
        git::checkout(target)?;
    }

    let spinner = ProgressBar::new_spinner().with_message(format!("Updating {target}…"));
    eprint!("\x1b[?25l"); // hide cursor
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let _ = git::fetch();
    let merge_result = git::fast_forward_merge(target)?;

    spinner.finish_and_clear();
    eprint!("\x1b[?25h"); // show cursor
    report_update(merge_result)?;

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

fn select_branch(current: Option<&str>) -> AppResult<String> {
    let branches = git::local_branches()?;
    if branches.is_empty() {
        return Err("no local branches found".into());
    }

    let default = current
        .and_then(|c| branches.iter().position(|b| b == c))
        .unwrap_or(0);

    let selection = Select::new()
        .with_prompt("Switch to")
        .items(&branches)
        .default(default)
        .interact()?;

    Ok(branches[selection].clone())
}

fn prompt_delete_stale_branches(old_branch: Option<&str>) -> AppResult<()> {
    let stale = git::stale_branches()?;
    if stale.is_empty() {
        return Ok(());
    }

    let mut selected: Vec<bool> = stale
        .iter()
        .map(|b| old_branch.is_some_and(|old| old == b))
        .collect();

    let selections = multi_select(
        "Delete stale branches (space to toggle, →/← all/none)",
        &stale,
        &mut selected,
    )?;

    let to_delete: Vec<&str> = selections.iter().map(|&i| stale[i].as_str()).collect();
    if !to_delete.is_empty() {
        git::delete_branches(&to_delete)?;
    }

    Ok(())
}

fn multi_select(prompt: &str, items: &[String], selected: &mut [bool]) -> AppResult<Vec<usize>> {
    let term = Term::stderr();
    let mut cursor = 0usize;
    let line_count = items.len() + 1; // prompt + items

    eprint!("\x1b[?25l"); // hide cursor

    let draw = |cursor: usize, selected: &[bool]| {
        eprintln!("{} {}", style("?").green().bold(), style(prompt).bold(),);
        for (i, item) in items.iter().enumerate() {
            let arrow = if i == cursor { ">" } else { " " };
            let check = if selected[i] { "[x]" } else { "[ ]" };
            eprintln!("  {arrow} {check} {item}");
        }
    };

    let clear = |n: usize| {
        for _ in 0..n {
            eprint!("\x1b[A\x1b[2K");
        }
    };

    draw(cursor, selected);

    loop {
        match term.read_key()? {
            Key::ArrowUp if cursor > 0 => cursor -= 1,
            Key::ArrowDown if cursor < items.len() - 1 => cursor += 1,
            Key::Char(' ') => selected[cursor] = !selected[cursor],
            Key::ArrowRight => selected.fill(true),
            Key::ArrowLeft => selected.fill(false),
            Key::Enter => break,
            Key::Escape => {
                clear(line_count);
                eprint!("\x1b[?25h"); // show cursor
                return Ok(vec![]);
            }
            _ => continue,
        }

        clear(line_count);
        draw(cursor, selected);
    }

    eprint!("\x1b[?25h"); // show cursor

    Ok(selected
        .iter()
        .enumerate()
        .filter(|(_, s)| **s)
        .map(|(i, _)| i)
        .collect())
}

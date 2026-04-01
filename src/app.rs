use indicatif::ProgressBar;
use inquire::{MultiSelect, Select};

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

    let selection = Select::new("Switch to", branches)
        .with_starting_cursor(default)
        .prompt()?;

    Ok(selection)
}

fn prompt_delete_stale_branches(old_branch: Option<&str>) -> AppResult<()> {
    let stale = git::stale_branches()?;
    if stale.is_empty() {
        return Ok(());
    }

    let defaults: Vec<usize> = stale
        .iter()
        .enumerate()
        .filter(|(_, b)| old_branch.is_some_and(|old| old == b.as_str()))
        .map(|(i, _)| i)
        .collect();

    let to_delete = MultiSelect::new(
        "Delete stale branches (space to toggle, →/← all/none)",
        stale,
    )
    .with_default(&defaults)
    .prompt()?;

    if !to_delete.is_empty() {
        let refs: Vec<&str> = to_delete.iter().map(String::as_str).collect();
        git::delete_branches(&refs)?;
    }

    Ok(())
}

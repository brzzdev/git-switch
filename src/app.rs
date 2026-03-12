use dialoguer::{MultiSelect, Select};
use indicatif::ProgressBar;

use crate::{AppResult, git};

pub fn run(target: Option<&str>) -> AppResult<()> {
    let target = match target {
        Some(name) => name.to_string(),
        None => select_branch()?,
    };

    let old_branch = git::current_branch()?;

    let stashed = if git::is_dirty()? {
        git::stash_push()?;
        true
    } else {
        false
    };

    let result = switch_and_update(&target, old_branch.as_deref());

    if stashed {
        git::stash_pop()?;
    }

    result
}

fn switch_and_update(target: &str, old_branch: Option<&str>) -> AppResult<()> {
    let already_on_target = old_branch.is_some_and(|b| b == target);

    if !already_on_target {
        git::checkout(target)?;
    }

    let spinner = ProgressBar::new_spinner()
        .with_message(format!("Updating {target}…"));
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let _ = git::fetch(target);
    let merge_result = git::fast_forward_merge(target)?;

    spinner.finish_and_clear();
    report_update(merge_result)?;

    prompt_delete_merged_branches(if already_on_target { None } else { old_branch })?;

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

fn select_branch() -> AppResult<String> {
    let branches = git::local_branches()?;
    if branches.is_empty() {
        return Err("no local branches found".into());
    }

    let current = git::current_branch()?;
    let default = current
        .as_ref()
        .and_then(|c| branches.iter().position(|b| b == c))
        .unwrap_or(0);

    let selection = Select::new()
        .with_prompt("Switch to")
        .items(&branches)
        .default(default)
        .interact()?;

    Ok(branches[selection].clone())
}

fn prompt_delete_merged_branches(old_branch: Option<&str>) -> AppResult<()> {
    let merged = git::merged_branches()?;
    if merged.is_empty() {
        return Ok(());
    }

    let defaults: Vec<bool> = merged
        .iter()
        .map(|b| old_branch.is_some_and(|old| old == b))
        .collect();

    let selections = MultiSelect::new()
        .with_prompt("Delete merged branches (space to toggle)")
        .items(&merged)
        .defaults(&defaults)
        .interact()?;

    for i in selections {
        git::delete_branch(&merged[i])?;
    }

    Ok(())
}

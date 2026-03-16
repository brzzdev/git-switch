use std::process::Command;

use crate::AppResult;

pub enum MergeResult {
    UpToDate,
    Pulled(u32),
    Diverged(String),
    NoRemote,
}

pub fn current_branch() -> AppResult<Option<String>> {
    let output = run(&["branch", "--show-current"])?;
    let name = output.trim().to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

pub fn local_branches() -> AppResult<Vec<String>> {
    let output = run(&["branch", "--format=%(refname:short)"])?;
    let branches = output.lines().map(String::from).collect();
    Ok(branches)
}

pub fn has_tracked_changes() -> AppResult<bool> {
    let output = run(&["status", "--porcelain", "--untracked-files=no"])?;
    Ok(!output.is_empty())
}

pub fn stash_push() -> AppResult<()> {
    run(&["stash", "push", "--quiet", "-m", "git-switch: auto-stash"])?;
    Ok(())
}

pub fn stash_pop() -> AppResult<()> {
    run(&["stash", "pop", "--quiet"])?;
    Ok(())
}

pub fn checkout(branch: &str) -> AppResult<()> {
    run(&["checkout", branch, "--quiet"])?;
    Ok(())
}

pub fn fetch(branch: &str) -> AppResult<bool> {
    let status = Command::new("git")
        .args(["fetch", "--quiet", "origin", branch])
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(status.success())
}

pub fn fast_forward_merge(branch: &str) -> AppResult<MergeResult> {
    let remote_ref = format!("origin/{branch}");

    let has_remote = Command::new("git")
        .args(["rev-parse", "--verify", &remote_ref])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success();

    if !has_remote {
        return Ok(MergeResult::NoRemote);
    }

    let before = rev_parse("HEAD")?;

    let status = Command::new("git")
        .args(["merge", "--ff-only", &remote_ref, "--quiet"])
        .status()?;

    if !status.success() {
        return Ok(MergeResult::Diverged(branch.to_string()));
    }

    let after = rev_parse("HEAD")?;

    if before == after {
        return Ok(MergeResult::UpToDate);
    }

    let output = run(&["rev-list", "--count", &format!("{before}..{after}")])?;
    let count: u32 = output.trim().parse()?;
    Ok(MergeResult::Pulled(count))
}

pub fn stale_branches() -> AppResult<Vec<String>> {
    let current = current_branch()?;
    let keep = kept_branches();

    let merged_output = run(&["branch", "--format=%(refname:short)", "--merged"])?;
    let gone_output = run(&[
        "for-each-ref",
        "--format=%(refname:short) %(upstream:track)",
        "refs/heads/",
    ])?;

    let gone: Vec<&str> = gone_output
        .lines()
        .filter_map(|line| {
            if line.ends_with("[gone]") {
                line.split_whitespace().next()
            } else {
                None
            }
        })
        .collect();

    let mut branches: Vec<String> = merged_output
        .lines()
        .chain(gone.into_iter())
        .filter(|b| current.as_deref() != Some(*b) && !keep.contains(&b.to_string()))
        .map(String::from)
        .collect();

    branches.sort();
    branches.dedup();
    Ok(branches)
}

fn kept_branches() -> Vec<String> {
    run(&["config", "--get-all", "git-switch.keep"])
        .map(|o| o.lines().map(String::from).collect())
        .unwrap_or_default()
}

pub fn delete_branches(branches: &[&str]) -> AppResult<()> {
    let mut args = vec!["branch", "-D", "--quiet"];
    args.extend(branches);
    run(&args)?;
    Ok(())
}

fn rev_parse(refname: &str) -> AppResult<String> {
    let output = run(&["rev-parse", refname])?;
    Ok(output.trim().to_string())
}

fn run(args: &[&str]) -> AppResult<String> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {}: {}", args.first().unwrap_or(&"<unknown>"), stderr.trim()).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

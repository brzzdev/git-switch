use std::process::Command;

use crate::AppResult;

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

fn parse_branch_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('*') || trimmed.starts_with('+') {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn is_dirty() -> AppResult<bool> {
    let unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .status()?
        .success();
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()?
        .success();
    Ok(!unstaged || !staged)
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

pub fn is_merged(branch: &str) -> AppResult<bool> {
    let output = run(&["branch", "--merged"])?;
    Ok(output.lines().any(|l| l.trim() == branch))
}

pub fn merged_branches() -> AppResult<Vec<String>> {
    let current = current_branch()?;
    let keep = kept_branches();
    let output = run(&["branch", "--merged"])?;
    let branches = output
        .lines()
        .filter_map(parse_branch_line)
        .filter(|b| current.as_deref() != Some(b.as_str()) && !keep.contains(b))
        .collect();
    Ok(branches)
}

fn kept_branches() -> Vec<String> {
    run(&["config", "--get-all", "git-switch.keep"])
        .map(|o| o.lines().map(String::from).collect())
        .unwrap_or_default()
}

pub fn delete_branch(branch: &str) -> AppResult<()> {
    run(&["branch", "-d", branch, "--quiet"])?;
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
        return Err(format!("git {}: {}", args[0], stderr.trim()).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub enum MergeResult {
    UpToDate,
    Pulled(u32),
    Diverged(String),
    NoRemote,
}

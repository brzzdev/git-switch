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
    let output = Command::new("git")
        .args(["stash", "pop", "--quiet"])
        .output()?;
    if !output.status.success() {
        // `git stash pop` writes conflict details to stdout, not stderr.
        let detail = String::from_utf8_lossy(&output.stdout);
        return Err(format!("git stash: {}", detail.trim()).into());
    }
    Ok(())
}

pub fn checkout(branch: &str) -> AppResult<()> {
    run(&["checkout", branch, "--quiet"])?;
    Ok(())
}

pub fn fetch() -> AppResult<bool> {
    let status = Command::new("git")
        .args(["fetch", "--quiet", "--prune", "origin"])
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
    let tracking_output = run(&[
        "for-each-ref",
        "--format=%(refname:short)|%(upstream:track)",
        "refs/heads/",
    ])?;

    let mut gone = Vec::new();
    let mut has_upstream = Vec::new();
    for line in tracking_output.lines() {
        let (name, track) = line.split_once('|').unwrap_or((line, ""));
        if track == "[gone]" {
            gone.push(name);
        } else if !track.is_empty() {
            has_upstream.push(name);
        }
    }

    let mut branches: Vec<String> = merged_output
        .lines()
        .filter(|b| has_upstream.contains(b))
        .chain(gone)
        .filter(|b| current.as_deref() != Some(*b) && !keep.iter().any(|k| k == b))
        .map(String::from)
        .collect();

    branches.sort();
    branches.dedup();
    Ok(branches)
}

fn default_branch() -> Option<String> {
    if let Ok(output) = run(&["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let trimmed = output.trim();
        if let Some(name) = trimmed.strip_prefix("refs/remotes/origin/") {
            return Some(name.to_string());
        }
    }
    None
}

fn kept_branches() -> Vec<String> {
    let mut kept: Vec<String> = run(&["config", "--get-all", "git-switch.keep"])
        .map(|o| o.lines().map(String::from).collect())
        .unwrap_or_default();
    if let Some(default) = default_branch()
        && !kept.contains(&default)
    {
        kept.push(default);
    }
    kept
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
        return Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&"<unknown>"),
            stderr.trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

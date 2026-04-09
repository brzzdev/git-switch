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
    let output = Command::new("git").args(["stash", "pop"]).output()?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stdout.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        return Err(format!("git stash pop: {detail}").into());
    }
    Ok(())
}

pub fn checkout(branch: &str) -> AppResult<()> {
    let output = Command::new("git")
        .args(["checkout", branch, "--quiet"])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains("Submodule") || !stderr.contains("could not be updated") {
        return Err(format!("git checkout: {}", stderr.trim()).into());
    }

    // Submodule checkout failed — likely missing objects. Fetch inside
    // each submodule and retry once.
    let _ = Command::new("git")
        .args(["submodule", "foreach", "--recursive", "git", "fetch"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

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
        "--format=%(refname:short) %(upstream) %(upstream:track)",
        "refs/heads/",
    ])?;

    let mut has_upstream = Vec::new();
    let mut gone = Vec::new();
    let mut local_only = Vec::new();
    for line in tracking_output.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        if parts.next().is_none() {
            local_only.push(name);
            continue;
        }
        if line.ends_with("[gone]") {
            gone.push(name);
        } else {
            has_upstream.push(name);
        }
    }

    let head = rev_parse("HEAD")?;

    let mut branches: Vec<String> = merged_output
        .lines()
        .filter(|b| {
            if has_upstream.contains(b) {
                has_unique_commits(b, &head)
            } else if local_only.contains(b) {
                is_behind_head(b, &head)
            } else {
                false
            }
        })
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

/// Returns true if the branch tip is strictly behind HEAD. For a merged
/// local-only branch this means its work has been incorporated into the main
/// line and HEAD has advanced past it.
fn is_behind_head(branch: &str, head: &str) -> bool {
    let Ok(tip) = rev_parse(branch) else {
        return false;
    };
    tip.trim() != head.trim()
}

/// Returns true if the branch had unique commits that were merged, not just a
/// pointer to a commit already on the main line that never diverged.
///
/// A branch created from main with no new commits has its tip equal to its
/// merge-base with HEAD and is strictly behind HEAD — this is not stale.
/// A fast-forward-merged branch also has tip == merge-base, but its tip equals
/// HEAD (or HEAD hasn't moved past it yet).
fn has_unique_commits(branch: &str, head: &str) -> bool {
    let Ok(merge_base) = run(&["merge-base", head, branch]) else {
        return false;
    };
    let Ok(tip) = rev_parse(branch) else {
        return false;
    };
    let (merge_base, tip) = (merge_base.trim(), tip.trim());
    // Branch diverged from main — it had unique work.
    if merge_base != tip {
        return true;
    }
    // merge-base == tip: branch never diverged. Only consider stale if HEAD
    // hasn't moved past it (fast-forward merge that just happened).
    tip == head.trim()
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

use std::collections::{HashMap, HashSet};
use std::process::Command;

use crate::AppResult;

pub enum MergeResult {
    UpToDate,
    Pulled(u32),
    Diverged(String),
    NoRemote,
}

pub enum StashPopOutcome {
    Clean,
    Conflict,
}

pub enum FetchOutcome {
    Ok,
    Failed(String),
}

pub fn current_branch() -> AppResult<Option<String>> {
    let output = run(&["branch", "--show-current"])?;
    let name = output.trim().to_string();
    Ok(if name.is_empty() { None } else { Some(name) })
}

/// Resolves the remote name to use for fetch, merge, and default-branch
/// detection. Prefers `branch.<current>.remote`, then the sole configured
/// remote, falling back to `origin`.
#[must_use]
pub fn current_remote(current: Option<&str>) -> String {
    if let Some(branch) = current
        && let Ok(output) = run(&["config", "--get", &format!("branch.{branch}.remote")])
    {
        let name = output.trim();
        // `.` means push to the local repo — useless for fetch/merge.
        if !name.is_empty() && name != "." {
            return name.to_string();
        }
    }

    if let Ok(output) = run(&["remote"]) {
        let mut remotes = output.lines().filter(|l| !l.is_empty());
        if let Some(only) = remotes.next()
            && remotes.next().is_none()
        {
            return only.to_string();
        }
    }

    "origin".to_string()
}

pub fn local_branches() -> AppResult<Vec<String>> {
    let output = run(&["branch", "--format=%(refname:short)"])?;
    let branches = output.lines().map(String::from).collect();
    Ok(branches)
}

pub fn remote_only_branches(local: &[String], remote: &str) -> AppResult<Vec<String>> {
    let prefix = format!("refs/remotes/{remote}/");
    let strip = format!("{remote}/");
    let output = run(&["for-each-ref", "--format=%(refname:short)", &prefix])?;
    let locals: HashSet<&str> = local.iter().map(String::as_str).collect();
    let branches = output
        .lines()
        .filter(|r| !r.ends_with("/HEAD"))
        .filter_map(|r| r.strip_prefix(&strip))
        .filter(|name| !locals.contains(name))
        .map(String::from)
        .collect();
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

pub fn stash_pop() -> AppResult<StashPopOutcome> {
    let output = Command::new("git").args(["stash", "pop"]).output()?;
    if output.status.success() {
        return Ok(StashPopOutcome::Clean);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("CONFLICT") || stderr.contains("CONFLICT") {
        return Ok(StashPopOutcome::Conflict);
    }

    let stdout_trimmed = stdout.trim();
    let detail = if stdout_trimmed.is_empty() {
        stderr.trim()
    } else {
        stdout_trimmed
    };
    Err(format!("git stash pop: {detail}").into())
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

pub fn fetch(remote: &str) -> AppResult<FetchOutcome> {
    let output = Command::new("git")
        .args(["fetch", "--quiet", "--prune", remote])
        .output()?;
    if output.status.success() {
        return Ok(FetchOutcome::Ok);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok(FetchOutcome::Failed(stderr))
}

pub fn fast_forward_merge(branch: &str, remote: &str) -> AppResult<MergeResult> {
    let remote_ref = format!("{remote}/{branch}");

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

pub fn stale_branches(remote: &str) -> AppResult<Vec<String>> {
    let current = current_branch()?;
    let keep = kept_branches(remote);

    let merged_output = run(&["branch", "--format=%(refname:short)", "--merged"])?;
    let refs_output = run(&[
        "for-each-ref",
        "--format=%(refname:short) %(objectname) %(upstream) %(upstream:track)",
        "refs/heads/",
    ])?;

    let mut tips: HashMap<&str, &str> = HashMap::new();
    let mut has_upstream: HashSet<&str> = HashSet::new();
    let mut gone: HashSet<&str> = HashSet::new();
    let mut local_only: HashSet<&str> = HashSet::new();
    for line in refs_output.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(sha) = parts.next() else { continue };
        tips.insert(name, sha);
        if parts.next().is_none() {
            local_only.insert(name);
            continue;
        }
        if line.ends_with("[gone]") {
            gone.insert(name);
        } else {
            has_upstream.insert(name);
        }
    }

    let head = rev_parse("HEAD")?;

    let mut branches: Vec<String> = merged_output
        .lines()
        .filter(|b| {
            let Some(tip) = tips.get(b) else { return false };
            if has_upstream.contains(b) {
                has_unique_commits(b, tip, &head)
            } else if local_only.contains(b) {
                *tip != head
            } else {
                false
            }
        })
        .chain(gone.iter().copied())
        .filter(|b| current.as_deref() != Some(*b) && !keep.contains(*b))
        .map(String::from)
        .collect();

    branches.sort();
    branches.dedup();
    Ok(branches)
}

fn default_branch(remote: &str) -> Option<String> {
    let head_ref = format!("refs/remotes/{remote}/HEAD");
    let prefix = format!("refs/remotes/{remote}/");
    if let Ok(output) = run(&["symbolic-ref", &head_ref]) {
        let trimmed = output.trim();
        if let Some(name) = trimmed.strip_prefix(prefix.as_str()) {
            return Some(name.to_string());
        }
    }
    None
}

fn kept_branches(remote: &str) -> HashSet<String> {
    let mut kept: HashSet<String> = run(&["config", "--get-all", "git-switch.keep"])
        .map(|o| o.lines().map(String::from).collect())
        .unwrap_or_default();
    if let Some(default) = default_branch(remote) {
        kept.insert(default);
    }
    kept
}

/// Branches currently checked out in any worktree (including the main one).
/// These cannot be deleted with `git branch -D`.
pub fn worktree_branches() -> AppResult<HashSet<String>> {
    let output = run(&["worktree", "list", "--porcelain"])?;
    let branches = output
        .lines()
        .filter_map(|l| l.strip_prefix("branch refs/heads/"))
        .map(String::from)
        .collect();
    Ok(branches)
}

pub fn delete_branches(branches: &[&str]) -> AppResult<()> {
    let mut args = vec!["branch", "-D", "--quiet"];
    args.extend(branches);
    run(&args)?;
    Ok(())
}

/// Returns true if the branch had unique commits that were merged, not just a
/// pointer to a commit already on the main line that never diverged.
///
/// A branch created from main with no new commits has its tip equal to its
/// merge-base with HEAD and is strictly behind HEAD — this is not stale.
/// A fast-forward-merged branch also has tip == merge-base, but its tip equals
/// HEAD (or HEAD hasn't moved past it yet).
fn has_unique_commits(branch: &str, tip: &str, head: &str) -> bool {
    let Ok(merge_base) = run(&["merge-base", head, branch]) else {
        return false;
    };
    if merge_base.trim() != tip {
        return true;
    }
    tip == head
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

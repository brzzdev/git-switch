use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{AppResult, Error};

pub enum MergeReport {
    UpToDate,
    Pulled(u32),
    NoRemote,
}

pub enum FastForwardResult {
    Merged(MergeReport),
    Diverged,
}

pub enum StashPopOutcome {
    Clean,
    Conflict,
}

pub enum FetchOutcome {
    Ok,
    Failed(String),
}

pub enum RebaseOutcome {
    Clean,
    Aborted,
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
        && let Some(name) = output.lines().next().map(str::trim)
        && !name.is_empty()
        // `.` means push to the local repo — useless for fetch/merge.
        && name != "."
    {
        return name.to_string();
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
    Err(Error::Git {
        command: "stash pop".to_string(),
        message: detail.to_string(),
    })
}

pub fn checkout(branch: &str) -> AppResult<()> {
    let output = Command::new("git")
        .args(["checkout", branch, "--quiet"])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The branch is held by a worktree whose directory is gone. Clear the dead
    // registration and retry; a live worktree survives prune, so this is a
    // no-op there and the original error still surfaces.
    if is_stale_worktree_error(&stderr) {
        let _ = worktree_prune();
        run(&["checkout", branch, "--quiet"])?;
        return Ok(());
    }

    if !stderr.contains("Submodule") || !stderr.contains("could not be updated") {
        return Err(Error::Git {
            command: "checkout".to_string(),
            message: stderr.trim().to_string(),
        });
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

pub fn fetch(dir: Option<&Path>, remote: &str) -> AppResult<FetchOutcome> {
    let output = git_cmd(dir)
        .args(["fetch", "--quiet", "--prune", remote])
        .output()?;
    if output.status.success() {
        return Ok(FetchOutcome::Ok);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok(FetchOutcome::Failed(stderr))
}

/// Rebase the current branch onto `onto` (e.g. `origin/main`). Git's stdout
/// and stderr stream directly to the terminal so users see progress and
/// conflict markers in real time. On failure the rebase is aborted, leaving
/// the working tree clean.
pub fn rebase(onto: &str) -> AppResult<RebaseOutcome> {
    let status = Command::new("git")
        .args(["-c", "advice.skippedCherryPicks=false", "rebase", onto])
        .status()?;
    if status.success() {
        return Ok(RebaseOutcome::Clean);
    }
    let _ = Command::new("git")
        .args(["rebase", "--abort"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(RebaseOutcome::Aborted)
}

pub fn fast_forward_merge(
    dir: Option<&Path>,
    branch: &str,
    remote: &str,
) -> AppResult<FastForwardResult> {
    let remote_ref = format!("{remote}/{branch}");

    let has_remote = git_cmd(dir)
        .args(["rev-parse", "--verify", &remote_ref])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success();

    if !has_remote {
        return Ok(FastForwardResult::Merged(MergeReport::NoRemote));
    }

    let before = rev_parse(dir, "HEAD")?;

    // Capture stderr so git's diverging hint and "fatal: Not possible to
    // fast-forward" don't leak — we surface a tailored message instead.
    let output = git_cmd(dir)
        .args([
            "-c",
            "advice.diverging=false",
            "merge",
            "--ff-only",
            &remote_ref,
            "--quiet",
        ])
        .output()?;

    if !output.status.success() {
        return Ok(FastForwardResult::Diverged);
    }

    let after = rev_parse(dir, "HEAD")?;

    if before == after {
        return Ok(FastForwardResult::Merged(MergeReport::UpToDate));
    }

    let output = run_in(dir, &["rev-list", "--count", &format!("{before}..{after}")])?;
    let count: u32 = output.trim().parse()?;
    Ok(FastForwardResult::Merged(MergeReport::Pulled(count)))
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

    let head = rev_parse(None, "HEAD")?;

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

/// Branches treated as "pinned" by the picker: the remote's default branch
/// first, then `git-switch.keep` entries in config order, deduplicated.
#[must_use]
pub fn pinned_branches(remote: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    if let Some(default) = default_branch(remote) {
        seen.insert(default.clone());
        out.push(default);
    }
    if let Ok(output) = run(&["config", "--get-all", "git-switch.keep"]) {
        for line in output.lines() {
            let name = line.trim();
            if !name.is_empty() && seen.insert(name.to_string()) {
                out.push(name.to_string());
            }
        }
    }
    out
}

#[must_use]
pub(crate) fn default_branch(remote: &str) -> Option<String> {
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

/// Detached worktrees have `branch: None`.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
    /// Registered but its directory is gone (`git worktree prune` clears it).
    /// Such an entry still blocks checkout/add of its branch, but cannot be
    /// entered.
    pub prunable: bool,
}

/// Lists all worktrees for the current repo. Per `git worktree list
/// --porcelain`, the first record is the main worktree.
pub fn worktree_list() -> AppResult<Vec<Worktree>> {
    let output = run(&["worktree", "list", "--porcelain"])?;
    let mut worktrees: Vec<Worktree> = Vec::new();
    // Each porcelain record starts with a `worktree <path>` line; the attributes
    // that follow apply to it until the next such line. Build the record in place
    // and push it when the next one begins (or the output ends).
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            worktrees.extend(current.take());
            current = Some(Worktree {
                path: PathBuf::from(p),
                branch: None,
                is_main: worktrees.is_empty(),
                prunable: false,
            });
        } else if let Some(wt) = current.as_mut() {
            if let Some(b) = line.strip_prefix("branch refs/heads/") {
                wt.branch = Some(b.to_string());
            } else if line == "prunable" || line.starts_with("prunable ") {
                wt.prunable = true;
            }
        }
    }
    worktrees.extend(current);
    Ok(worktrees)
}

/// Branches currently checked out in any worktree (including the main one).
/// These cannot be deleted with `git branch -D`.
pub fn worktree_branches() -> AppResult<HashSet<String>> {
    Ok(worktree_list()?
        .into_iter()
        .filter_map(|w| w.branch)
        .collect())
}

#[must_use]
pub fn worktree_for_branch(worktrees: &[Worktree], branch: &str) -> Option<Worktree> {
    worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .cloned()
}

/// Add a worktree at `path` for `branch`. If `base` is `Some`, create the
/// branch from `base` (e.g. `origin/main`); if `None`, check out an existing
/// local or remote-tracking branch.
pub fn worktree_add(path: &Path, branch: &str, base: Option<&str>) -> AppResult<()> {
    let path_str = path_to_str(path)?;
    let args: Vec<&str> = match base {
        Some(base) => vec!["worktree", "add", "-b", branch, path_str, base],
        None => vec!["worktree", "add", path_str, branch],
    };

    let output = Command::new("git").args(&args).output()?;
    if output.status.success() {
        return Ok(());
    }

    // A worktree whose directory was deleted by hand stays registered and
    // blocks re-adding ("missing but already registered"). Clear the dead
    // registrations and try once more.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_stale_worktree_error(&stderr) {
        let _ = worktree_prune();
        let retry = Command::new("git").args(&args).output()?;
        if retry.status.success() {
            return Ok(());
        }
        return Err(Error::Git {
            command: "worktree add".to_string(),
            message: String::from_utf8_lossy(&retry.stderr).trim().to_string(),
        });
    }

    Err(Error::Git {
        command: "worktree add".to_string(),
        message: stderr.trim().to_string(),
    })
}

/// Remove worktree registrations whose working directories no longer exist.
pub fn worktree_prune() -> AppResult<()> {
    run(&["worktree", "prune"])?;
    Ok(())
}

/// True when git refused because a branch/path is held by a worktree whose
/// directory is gone — a `git worktree prune` clears the stale registration.
fn is_stale_worktree_error(stderr: &str) -> bool {
    stderr.contains("already registered") || stderr.contains("used by worktree")
}

pub enum WorktreeRemoveOutcome {
    Removed,
    Failed(String),
}

pub fn worktree_remove(path: &Path) -> AppResult<WorktreeRemoveOutcome> {
    let path_str = path_to_str(path)?;
    let output = Command::new("git")
        .args(["worktree", "remove", path_str])
        .output()?;
    if output.status.success() {
        return Ok(WorktreeRemoveOutcome::Removed);
    }

    // The directory is already gone (deleted by hand): `git worktree remove` may
    // balk, but `prune` clears the dead registration. Only claim success if the
    // entry is actually gone afterwards — a locked worktree survives prune, and
    // reporting a phantom removal would then delete a branch still held by it.
    if !path.exists() {
        let _ = worktree_prune();
        let still_registered = worktree_list()?.iter().any(|w| w.path == path);
        if !still_registered {
            return Ok(WorktreeRemoveOutcome::Removed);
        }
    }

    Ok(WorktreeRemoveOutcome::Failed(
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn path_to_str(path: &Path) -> AppResult<&str> {
    path.to_str().ok_or_else(|| Error::Git {
        command: "worktree".to_string(),
        message: format!("non-utf8 path: {}", path.display()),
    })
}

pub fn delete_branches(branches: &[&str]) -> AppResult<()> {
    let mut args = vec!["branch", "-D", "--quiet"];
    args.extend(branches);
    run(&args)?;
    Ok(())
}

pub enum BranchDeleteOutcome {
    Deleted,
    /// Kept because it has commits not merged into its upstream or HEAD.
    NotMerged,
    Failed(String),
}

/// Delete `branch` only if git considers it fully merged (`git branch -d`).
/// Unlike [`delete_branches`] this never force-deletes, so unmerged work is
/// preserved rather than silently discarded.
pub fn delete_branch_if_merged(branch: &str) -> AppResult<BranchDeleteOutcome> {
    let output = git_cmd(None)
        .args(["branch", "-d", "--quiet", branch])
        .output()?;
    if output.status.success() {
        return Ok(BranchDeleteOutcome::Deleted);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not fully merged") {
        return Ok(BranchDeleteOutcome::NotMerged);
    }
    Ok(BranchDeleteOutcome::Failed(stderr.trim().to_string()))
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

fn rev_parse(dir: Option<&Path>, refname: &str) -> AppResult<String> {
    let output = run_in(dir, &["rev-parse", refname])?;
    Ok(output.trim().to_string())
}

/// A `git` command, optionally rooted in `dir` via `-C` so it operates on
/// another worktree without mutating the process's working directory.
fn git_cmd(dir: Option<&Path>) -> Command {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd
}

fn run(args: &[&str]) -> AppResult<String> {
    run_in(None, args)
}

fn run_in(dir: Option<&Path>, args: &[&str]) -> AppResult<String> {
    let output = git_cmd(dir).args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Git {
            command: args.first().copied().unwrap_or("<unknown>").to_string(),
            message: stderr.trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

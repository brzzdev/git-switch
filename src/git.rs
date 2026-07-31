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

/// True if `<remote>/<branch>` exists as a remote-tracking ref.
#[must_use]
pub fn remote_branch_exists(remote: &str, branch: &str) -> bool {
    let remote_ref = format!("{remote}/{branch}");
    git_cmd(None)
        .args(["rev-parse", "--verify", &remote_ref])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `(ahead, behind)` for `HEAD` relative to `<remote>/<branch>`: how many
/// commits are local-only (a hard reset would discard them) and how many the
/// remote has that aren't local. A rebase or amend gives commits new SHAs, so
/// rewritten-but-equivalent work still counts as ahead.
pub fn ahead_behind_remote(remote: &str, branch: &str) -> AppResult<(u32, u32)> {
    let range = format!("{remote}/{branch}...HEAD");
    let output = run(&["rev-list", "--left-right", "--count", &range])?;
    // `--left-right --count` prints "<left>\t<right>": left is reachable from
    // the remote only (behind), right from HEAD only (ahead).
    let mut counts = output.split_whitespace();
    let behind = counts.next().unwrap_or("0").parse()?;
    let ahead = counts.next().unwrap_or("0").parse()?;
    Ok((ahead, behind))
}

/// Hard-reset the current branch and working tree to `<remote>/<branch>`,
/// discarding local commits and tracked changes. Untracked files are left
/// alone, matching plain `git reset --hard`.
pub fn reset_hard(remote: &str, branch: &str) -> AppResult<()> {
    let remote_ref = format!("{remote}/{branch}");
    run(&["reset", "--hard", &remote_ref])?;
    Ok(())
}

/// One `refs/heads/` entry: where the branch points and what it tracks.
struct BranchRef<'a> {
    tip: &'a str,
    /// The full upstream ref (`refs/remotes/origin/foo`), empty when untracked.
    upstream: &'a str,
    /// Git's `upstream:track` summary: `ahead 2, behind 1`, `gone`, or empty.
    track: &'a str,
}

impl BranchRef<'_> {
    /// The upstream was configured but no longer exists on the remote.
    fn gone(&self) -> bool {
        self.track == "gone"
    }

    /// True when the upstream is this branch's own counterpart — on any remote,
    /// since a branch published to a second remote is no less published. A
    /// branch created off `origin/main` tracks *`main`*, which is how a
    /// never-pushed branch is told apart from one that was published.
    fn tracks_own_counterpart(&self, name: &str) -> bool {
        self.upstream
            .strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(_, counterpart)| counterpart == name)
    }

    /// True when what the branch tracks already shows its work has landed on the
    /// anchor: either it published its own counterpart and the anchor was
    /// fast-forwarded onto it, or it tracks nothing and the anchor has moved
    /// past it. Neither settles a branch merely cut from the anchor — see
    /// [`stale_branches`].
    fn landed_on(&self, name: &str, anchor_tip: &str) -> bool {
        let published_and_merged = self.tracks_own_counterpart(name) && self.tip == anchor_tip;
        let local_and_behind = self.upstream.is_empty() && self.tip != anchor_tip;
        published_and_merged || local_and_behind
    }
}

/// Every local branch's ref state. Refs are shared across worktrees, so this is
/// independent of which one the caller is standing in.
fn read_branch_refs() -> AppResult<String> {
    run(&[
        "for-each-ref",
        "--format=%(refname:short)%09%(objectname)%09%(upstream)%09%(upstream:track,nobracket)",
        "refs/heads/",
    ])
}

fn branch_refs(refs_output: &str) -> HashMap<&str, BranchRef<'_>> {
    refs_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let name = parts.next()?;
            let tip = parts.next()?;
            let upstream = parts.next()?;
            let track = parts.next().unwrap_or("");
            Some((
                name,
                BranchRef {
                    tip,
                    upstream,
                    track,
                },
            ))
        })
        .collect()
}

/// The ref that staleness is judged against — the default branch, not whatever
/// branch the current worktree happens to be on.
///
/// The local copy comes first: it is what you merge into, so work merged
/// locally but not yet pushed still counts. Its remote counterpart stands in
/// where there is no local copy. When neither resolves there is nothing to
/// judge "merged" against, and callers stand the merged rule down rather than
/// falling back to `HEAD`.
fn merged_anchor(remote: &str) -> Option<String> {
    let default = default_branch(remote)?;
    let local = format!("refs/heads/{default}");
    if rev_parse(None, &local).is_ok() {
        return Some(local);
    }
    let remote_ref = format!("refs/remotes/{remote}/{default}");
    rev_parse(None, &remote_ref).ok().map(|_| remote_ref)
}

/// The commits on `anchor`'s first-parent chain. A branch merged with a merge
/// commit hangs off a *second* parent, so its tip is absent here; a branch that
/// was only ever cut from the anchor sits squarely on it.
fn first_parent_chain(anchor: &str) -> AppResult<HashSet<String>> {
    let output = run(&["rev-list", "--first-parent", anchor])?;
    Ok(output.lines().map(String::from).collect())
}

/// The rule half of [`stale_branches`], split out so it can be tested against
/// fixed git output.
///
/// `merged` is the anchor's `--merged` list and `anchor_tip` its commit; both
/// are empty where no anchor resolved, which leaves only deleted upstreams
/// without needing a special case. `first_parent` is deferred so the walk is
/// only paid for when it can still change an answer.
fn stale_from(
    refs: &HashMap<&str, BranchRef<'_>>,
    merged: &str,
    anchor_tip: &str,
    first_parent: impl FnOnce() -> AppResult<HashSet<String>>,
) -> AppResult<Vec<String>> {
    let mut stale: Vec<String> = refs
        .iter()
        .filter(|(_, r)| r.gone())
        .map(|(name, _)| (*name).to_string())
        .collect();

    // Branches tracking alone can't settle, held back for the first-parent walk.
    let mut undecided: Vec<&str> = Vec::new();
    for name in merged.lines() {
        let Some(r) = refs.get(name) else { continue };
        if r.gone() {
            continue;
        }
        if r.landed_on(name, anchor_tip) {
            stale.push(name.to_string());
        } else {
            undecided.push(name);
        }
    }

    if !undecided.is_empty() {
        let chain = first_parent()?;
        stale.extend(
            undecided
                .into_iter()
                .filter(|name| refs.get(name).is_some_and(|r| !chain.contains(r.tip)))
                .map(String::from),
        );
    }

    Ok(stale)
}

/// Branches that have outlived their purpose: a deleted upstream, or work that
/// has landed on the anchor.
///
/// Merged-ness alone doesn't settle it. Every branch reachable from the anchor
/// has its tip as its own merge-base with it, so a branch freshly cut from the
/// anchor is topologically indistinguishable from one whose commits were
/// fast-forwarded onto it. Three clauses separate them, and any one suffices:
///
/// - the tip lies off the anchor's first-parent chain, so it was merged in;
/// - it tracks its own counterpart and its tip *is* the anchor's, so what it
///   published was fast-forwarded on;
/// - it tracks nothing and its tip is *behind* the anchor's, so it was merged
///   locally and left behind.
///
/// A branch cut from the anchor and never committed to satisfies none: it
/// tracks the *anchor's* counterpart, not its own, and sits on the first-parent
/// chain. Untracked ones are the exception — nothing distinguishes an empty
/// branch from a locally merged one once the anchor moves past both.
///
/// A branch merged by rebasing or squashing is no ancestor of the anchor at
/// all, so no clause here can see it; that case belongs to the deleted upstream
/// after a pruning fetch.
pub fn stale_branches(remote: &str) -> AppResult<Vec<String>> {
    let current = current_branch()?;
    let keep = kept_branches(remote);

    let refs_output = read_branch_refs()?;
    let refs = branch_refs(&refs_output);

    let anchor = merged_anchor(remote);
    let (merged, anchor_tip) = match &anchor {
        Some(anchor) => (
            run(&["branch", "--format=%(refname:short)", "--merged", anchor])?,
            rev_parse(None, anchor)?,
        ),
        None => (String::new(), String::new()),
    };

    let mut branches = stale_from(&refs, &merged, &anchor_tip, || {
        first_parent_chain(anchor.as_deref().unwrap_or_default())
    })?;

    branches.retain(|b| current.as_deref() != Some(b) && !keep.contains(b));
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

/// True if the worktree at `path` has uncommitted changes (tracked edits or
/// untracked, non-ignored files). A missing/unreadable path reports clean.
#[must_use]
pub fn worktree_dirty(path: &Path) -> bool {
    git_cmd(Some(path))
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

/// Maps each local branch to its (ahead, behind) commit counts versus its
/// upstream. Built from a single `for-each-ref` over the shared `refs/heads`,
/// so it costs one git call regardless of how many worktrees exist. Branches
/// with no upstream (or a `[gone]` one) are absent from the map.
#[must_use]
pub fn ahead_behind_map() -> HashMap<String, (u32, u32)> {
    let Ok(output) = run(&[
        "for-each-ref",
        "--format=%(refname:short)%09%(upstream:track,nobracket)",
        "refs/heads/",
    ]) else {
        return HashMap::new();
    };

    output
        .lines()
        .filter_map(|line| {
            let (name, track) = line.split_once('\t')?;
            let (ahead, behind) = parse_track(track);
            (ahead != 0 || behind != 0).then(|| (name.to_string(), (ahead, behind)))
        })
        .collect()
}

/// Parses git's `upstream:track,nobracket` field, e.g. `ahead 2, behind 1`.
fn parse_track(track: &str) -> (u32, u32) {
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

/// Branches currently checked out in any worktree (including the main one).
/// These cannot be deleted with `git branch -D`.
pub fn worktree_branches() -> AppResult<HashSet<String>> {
    Ok(worktree_list()?
        .into_iter()
        .filter_map(|w| w.branch)
        .collect())
}

/// A branch `git branch -d` would refuse to delete, and why there is or isn't a
/// commit count to show for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmerged {
    /// Ahead of a live upstream by this many commits.
    Ahead(u32),
    /// No upstream (or a `[gone]` one), so there is nothing to count against.
    NoUpstream,
}

/// Branches that `git branch -d` would refuse: those merged into neither the
/// HEAD of `dir` nor their own upstream. Callers use this both to mark rows and
/// to decide when a force-delete is licensed, so it must mirror `-d`'s rule
/// rather than a proxy like ahead-of-upstream — a purely local branch has no
/// upstream to be ahead of, yet is exactly the case worth warning about.
///
/// `dir` must be the worktree whose HEAD will be current when the delete runs.
/// `--merged` is relative to HEAD, and every branch is merged into itself, so
/// asking from inside the worktree being removed would report its own branch as
/// merged and skip the warning it exists to give.
pub fn unmerged_branches(dir: Option<&Path>) -> AppResult<HashMap<String, Unmerged>> {
    let merged_output = run_in(dir, &["branch", "--format=%(refname:short)", "--merged"])?;
    let refs_output = read_branch_refs()?;
    Ok(unmerged_from(&merged_output, &branch_refs(&refs_output)))
}

/// The rule half of [`unmerged_branches`], split out so it can be tested
/// against fixed git output.
fn unmerged_from(
    merged_output: &str,
    refs: &HashMap<&str, BranchRef<'_>>,
) -> HashMap<String, Unmerged> {
    let merged: HashSet<&str> = merged_output.lines().collect();

    refs.iter()
        .filter(|(name, _)| !merged.contains(*name))
        .filter_map(|(&name, r)| {
            // A `[gone]` upstream reports no ahead count, so it must be treated
            // as absent rather than as "zero ahead, therefore merged".
            if r.upstream.is_empty() || r.gone() {
                return Some((name.to_string(), Unmerged::NoUpstream));
            }
            let (ahead, _) = parse_track(r.track);
            (ahead > 0).then(|| (name.to_string(), Unmerged::Ahead(ahead)))
        })
        .collect()
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
    // `--track` rather than relying on git's default: `branch.autoSetupMerge`
    // can be off, and the upstream a new branch carries is what tells
    // `stale_branches` it was branched off the anchor rather than merged
    // into it.
    let args: Vec<&str> = match base {
        Some(base) => vec!["worktree", "add", "--track", "-b", branch, path_str, base],
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

/// Remove the worktree at `path`. With `force`, uncommitted and untracked
/// changes in it are discarded; without it, git refuses a dirty tree. A *locked*
/// worktree survives either way (git wants `--force --force`) and is reported as
/// a failure rather than escalated.
pub fn worktree_remove(path: &Path, force: bool) -> AppResult<WorktreeRemoveOutcome> {
    let path_str = path_to_str(path)?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str);
    let output = Command::new("git").args(&args).output()?;
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

/// Delete a branch with `git branch -D`, discarding unmerged commits. Only for
/// branches whose risk was shown to the user first — see [`unmerged_branches`].
///
/// `dir` must be the worktree whose HEAD the risk was judged from, so that the
/// markers shown and the deletion performed agree about what is merged.
pub fn force_delete_branch(dir: Option<&Path>, branch: &str) -> AppResult<BranchDeleteOutcome> {
    let output = git_cmd(dir)
        .args(["branch", "-D", "--quiet", branch])
        .output()?;
    if output.status.success() {
        return Ok(BranchDeleteOutcome::Deleted);
    }
    Ok(BranchDeleteOutcome::Failed(
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
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
///
/// `-d` judges merged-ness against the HEAD it runs under, so `dir` must be the
/// worktree the caller measured risk from — see [`unmerged_branches`].
pub fn delete_branch_if_merged(dir: Option<&Path>, branch: &str) -> AppResult<BranchDeleteOutcome> {
    let output = git_cmd(dir)
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

#[cfg(test)]
mod tests {
    use super::{Unmerged, branch_refs, parse_track, stale_from, unmerged_from};
    use std::collections::{HashMap, HashSet};

    /// One `for-each-ref` line per row: name, tip, upstream, track.
    fn head_refs(rows: &[(&str, &str, &str, &str)]) -> String {
        rows.iter()
            .map(|(name, tip, upstream, track)| format!("{name}\t{tip}\t{upstream}\t{track}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Run the `-d` mirror. Tips play no part in it, so rows name only the
    /// branch, its upstream, and its track summary.
    fn unmerged(merged: &str, rows: &[(&str, &str, &str)]) -> HashMap<String, Unmerged> {
        let rows: Vec<_> = rows
            .iter()
            .map(|(name, upstream, track)| (*name, "tip", *upstream, *track))
            .collect();
        let refs_output = head_refs(&rows);
        unmerged_from(merged, &branch_refs(&refs_output))
    }

    fn chain(shas: &[&str]) -> HashSet<String> {
        shas.iter().map(|s| (*s).to_string()).collect()
    }

    /// Run the rule. The anchor's own branch is left out of `merged` throughout:
    /// [`stale_branches`] filters it as kept, so it isn't this half's concern.
    fn stale(
        refs_output: &str,
        merged: &str,
        anchor_tip: &str,
        first_parent: &[&str],
    ) -> Vec<String> {
        let refs = branch_refs(refs_output);
        let mut got = stale_from(&refs, merged, anchor_tip, || Ok(chain(first_parent))).unwrap();
        got.sort();
        got
    }

    /// The reported bug: a branch cut from the anchor and never committed to
    /// tracks the *anchor's* counterpart, not its own, so it is not stale —
    /// whether or not the anchor has since moved past it.
    #[test]
    fn branch_cut_from_the_anchor_is_not_stale() {
        let refs_output = head_refs(&[
            ("main", "aaa", "refs/remotes/origin/main", ""),
            ("feature", "bbb", "refs/remotes/origin/main", "behind 1"),
        ]);
        let got = stale(&refs_output, "feature", "aaa", &["aaa", "bbb"]);
        assert!(
            got.is_empty(),
            "a freshly cut branch is not stale, got: {got:?}"
        );
    }

    /// Published its own counterpart, and the anchor was fast-forwarded onto it.
    #[test]
    fn published_branch_the_anchor_fast_forwarded_to_is_stale() {
        let refs_output = head_refs(&[
            ("main", "bbb", "refs/remotes/origin/main", ""),
            ("feature", "bbb", "refs/remotes/origin/feature", ""),
        ]);
        let got = stale(&refs_output, "feature", "bbb", &["aaa", "bbb"]);
        assert_eq!(got, vec!["feature".to_string()]);
    }

    /// Tracks nothing and the anchor has moved past it: merged locally, then
    /// left behind.
    #[test]
    fn untracked_branch_behind_the_anchor_is_stale() {
        let refs_output = head_refs(&[
            ("main", "ccc", "refs/remotes/origin/main", ""),
            ("scratch", "bbb", "", ""),
        ]);
        let got = stale(&refs_output, "scratch", "ccc", &["aaa", "bbb", "ccc"]);
        assert_eq!(got, vec!["scratch".to_string()]);
    }

    /// Merged with a merge commit, so its tip hangs off a second parent. Neither
    /// tracking clause applies — only the first-parent walk sees it.
    #[test]
    fn branch_merged_off_the_first_parent_chain_is_stale() {
        let refs_output = head_refs(&[
            ("main", "ccc", "refs/remotes/origin/main", ""),
            ("feature", "bbb", "refs/remotes/origin/feature", ""),
        ]);
        // `bbb` is reachable from main but not on its first-parent chain.
        let got = stale(&refs_output, "feature", "ccc", &["aaa", "ccc"]);
        assert_eq!(got, vec!["feature".to_string()]);
    }

    /// A deleted upstream speaks for itself, with no anchor to measure against.
    #[test]
    fn gone_upstream_is_stale_without_an_anchor() {
        let refs_output = head_refs(&[
            ("main", "aaa", "refs/remotes/origin/main", ""),
            ("merged-work", "bbb", "", ""),
            ("abandoned", "ccc", "refs/remotes/origin/abandoned", "gone"),
        ]);
        let got = stale(&refs_output, "", "", &[]);
        assert_eq!(
            got,
            vec!["abandoned".to_string()],
            "without an anchor the merged rule stands down"
        );
    }

    /// The first-parent walk is the expensive half, so it must not run when
    /// tracking has already settled every candidate.
    #[test]
    fn first_parent_walk_is_skipped_when_tracking_settles_everything() {
        let refs_output = head_refs(&[
            ("main", "bbb", "refs/remotes/origin/main", ""),
            ("feature", "bbb", "refs/remotes/origin/feature", ""),
        ]);
        let refs = branch_refs(&refs_output);
        let got = stale_from(&refs, "feature", "bbb", || {
            panic!("first-parent walk should not have been needed")
        })
        .unwrap();
        assert_eq!(got, vec!["feature".to_string()]);
    }

    #[test]
    fn merged_into_head_is_not_unmerged() {
        let got = unmerged(
            "main\nfeature",
            &[("feature", "refs/remotes/origin/feature", "ahead 2")],
        );
        assert!(
            got.is_empty(),
            "merged into HEAD wins regardless of upstream, got: {got:?}"
        );
    }

    #[test]
    fn in_sync_with_upstream_is_not_unmerged() {
        let got = unmerged("main", &[("feature", "refs/remotes/origin/feature", "")]);
        assert!(got.is_empty(), "merged into upstream, got: {got:?}");
    }

    #[test]
    fn ahead_of_upstream_is_unmerged_with_a_count() {
        let got = unmerged(
            "main",
            &[(
                "feature",
                "refs/remotes/origin/feature",
                "ahead 3, behind 1",
            )],
        );
        assert_eq!(got.get("feature"), Some(&Unmerged::Ahead(3)));
    }

    /// The case a plain ahead-of-upstream check misses: no upstream means
    /// nothing to be ahead of, yet `git branch -d` still refuses.
    #[test]
    fn local_only_branch_is_unmerged_without_a_count() {
        let got = unmerged("main", &[("scratch", "", "")]);
        assert_eq!(got.get("scratch"), Some(&Unmerged::NoUpstream));
    }

    /// A `[gone]` upstream reports no ahead count, which must not be read as
    /// "zero ahead, therefore merged".
    #[test]
    fn gone_upstream_is_unmerged_without_a_count() {
        let got = unmerged("main", &[("orphan", "refs/remotes/origin/orphan", "gone")]);
        assert_eq!(got.get("orphan"), Some(&Unmerged::NoUpstream));
    }

    #[test]
    fn parse_track_reads_ahead_behind() {
        assert_eq!(parse_track("ahead 2"), (2, 0));
        assert_eq!(parse_track("behind 1"), (0, 1));
        assert_eq!(parse_track("ahead 2, behind 1"), (2, 1));
    }

    #[test]
    fn parse_track_in_sync_or_gone_is_zero() {
        assert_eq!(parse_track(""), (0, 0));
        assert_eq!(parse_track("gone"), (0, 0));
    }
}

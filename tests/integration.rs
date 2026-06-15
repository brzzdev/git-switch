use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tempfile::TempDir;

/// Serializes tests that mutate process cwd while calling library functions.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Locks `CWD_LOCK`, switches process cwd, and restores the previous cwd on
/// drop — even on panic. Without this, a panicking test would leave cwd at a
/// deleted `TempDir` and cascade failures into unrelated tests.
struct CwdGuard {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

fn cwd_at(path: &Path) -> CwdGuard {
    // Poisoning is safe to recover from: the guard always restores cwd, so
    // the mutex's protected state is in fact consistent.
    let lock = CWD_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(path).unwrap();
    CwdGuard {
        _lock: lock,
        original,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_switch_args(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-switch"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git-switch")
}

fn git_switch(dir: &Path, branch: &str) -> Output {
    git_switch_args(dir, &[branch])
}

/// Like `setup`, but places the working clone inside a parent `TempDir` so
/// worktrees created at `<parent>/worktrees/<repo>/...` land in cleanable
/// space. Returns `(bare, parent, work_path)`.
fn setup_with_parent() -> (TempDir, TempDir, PathBuf) {
    let bare = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let work = parent.path().join("repo");
    fs::create_dir(&work).unwrap();

    git(bare.path(), &["init", "--bare"]);

    git(&work, &["init", "-b", "main"]);
    git(&work, &["config", "user.name", "test"]);
    git(&work, &["config", "user.email", "test@example.com"]);
    git(
        &work,
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );

    fs::write(work.join("file.txt"), "hello\n").unwrap();
    git(&work, &["add", "file.txt"]);
    git(&work, &["commit", "-m", "initial"]);
    git(&work, &["push", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);

    (bare, parent, work)
}

/// Create a bare "remote" and a working clone with one commit on `main`.
fn setup() -> (TempDir, TempDir) {
    setup_with_remote("origin")
}

fn setup_with_remote(remote: &str) -> (TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();

    git(bare.path(), &["init", "--bare"]);

    git(work.path(), &["init", "-b", "main"]);
    // Library code under test spawns its own `git` subprocesses without our
    // helper's GIT_*_NAME/EMAIL env, so commits it makes (e.g. during rebase)
    // need identity from .git/config — otherwise CI hosts without a global
    // gitconfig fail with "empty ident name".
    git(work.path(), &["config", "user.name", "test"]);
    git(work.path(), &["config", "user.email", "test@example.com"]);
    git(
        work.path(),
        &["remote", "add", remote, bare.path().to_str().unwrap()],
    );

    fs::write(work.path().join("file.txt"), "hello\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "initial"]);
    git(work.path(), &["push", "-u", remote, "main"]);

    (bare, work)
}

/// Push a commit to `<remote>/main` that the working tree doesn't have.
/// Works by committing locally, pushing, then rewinding.
fn push_upstream_change(work: &Path, file: &str, content: &str, msg: &str) {
    push_upstream_change_to(work, "origin", file, content, msg);
}

fn push_upstream_change_to(work: &Path, remote: &str, file: &str, content: &str, msg: &str) {
    fs::write(work.join(file), content).unwrap();
    git(work, &["add", file]);
    git(work, &["commit", "-m", msg]);
    git(work, &["push", remote, "main"]);
    git(work, &["reset", "--hard", "HEAD~1"]);
}

fn clone_bare(bare: &Path) -> TempDir {
    let dir = TempDir::new().unwrap();
    Command::new("git")
        .args(["clone", bare.to_str().unwrap(), "."])
        .current_dir(dir.path())
        .output()
        .expect("failed to clone");
    dir
}

fn stdout_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_str(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fast_forward_pull() {
    let (_bare, work) = setup();

    push_upstream_change(work.path(), "file.txt", "updated\n", "upstream change");

    let output = git_switch(work.path(), "main");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("Pulled 1 commit"),
        "stderr: {}",
        stderr_str(&output)
    );

    let content = fs::read_to_string(work.path().join("file.txt")).unwrap();
    assert_eq!(content, "updated\n");
}

#[test]
fn auto_stash_and_restore() {
    let (_bare, work) = setup();

    // Track a second file so we can dirty it without conflicting with the pull.
    fs::write(work.path().join("other.txt"), "original\n").unwrap();
    git(work.path(), &["add", "other.txt"]);
    git(work.path(), &["commit", "-m", "add other"]);
    git(work.path(), &["push", "origin", "main"]);

    push_upstream_change(work.path(), "file.txt", "updated\n", "upstream change");

    // Dirty a tracked file.
    fs::write(work.path().join("other.txt"), "local work in progress\n").unwrap();

    let output = git_switch(work.path(), "main");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("Pulled 1 commit"),
        "stderr: {}",
        stderr_str(&output)
    );

    // Local modification must survive the round-trip.
    let content = fs::read_to_string(work.path().join("other.txt")).unwrap();
    assert_eq!(content, "local work in progress\n");
}

#[test]
fn stash_pop_conflict_shows_guidance() {
    let (_bare, work) = setup();

    push_upstream_change(work.path(), "file.txt", "upstream version\n", "upstream");

    // Create a conflicting local modification to the same file.
    fs::write(work.path().join("file.txt"), "local version\n").unwrap();

    let output = git_switch(work.path(), "main");

    // The pull itself succeeds; only the stash pop conflicts.
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let stderr = stderr_str(&output);
    assert!(
        stderr.contains("Conflicts detected"),
        "expected conflict-specific guidance in stderr, got: {stderr}"
    );

    // The stash should still be present for manual recovery.
    let stash_list = git(work.path(), &["stash", "list"]);
    assert!(
        !stdout_str(&stash_list).is_empty(),
        "stash should not be empty after a failed pop"
    );
}

#[test]
fn diverged_branch_reports_error() {
    let (bare, work) = setup();

    // Create and push a feature branch.
    git(work.path(), &["checkout", "-b", "feature"]);
    fs::write(work.path().join("feature.txt"), "v1\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature v1"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);

    // Make a local-only commit so local is ahead.
    fs::write(work.path().join("feature.txt"), "local diverge\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "local diverge"]);

    // From a second clone, force-push a different commit to origin/feature.
    let second = clone_bare(bare.path());
    git(second.path(), &["checkout", "feature"]);
    fs::write(second.path().join("feature.txt"), "remote diverge\n").unwrap();
    git(second.path(), &["add", "feature.txt"]);
    git(second.path(), &["commit", "-m", "remote diverge"]);
    git(second.path(), &["push", "--force", "origin", "feature"]);

    let output = git_switch(work.path(), "feature");

    assert!(!output.status.success());

    let combined = format!("{}{}", stdout_str(&output), stderr_str(&output));
    assert!(
        combined.contains("diverged"),
        "expected divergence message, got: {combined}"
    );
}

#[test]
fn local_only_branch_not_stale_right_after_merge() {
    let (_bare, work) = setup();

    // Create a local-only branch (never pushed) and merge it into main.
    // Right after the merge HEAD == branch tip, so it's not stale yet.
    git(work.path(), &["checkout", "-b", "local-experiment"]);
    fs::write(work.path().join("experiment.txt"), "try something\n").unwrap();
    git(work.path(), &["add", "experiment.txt"]);
    git(work.path(), &["commit", "-m", "experiment"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "local-experiment"]);

    let _cwd = cwd_at(work.path());
    let stale = git_switch::git::stale_branches("origin").unwrap();

    assert!(
        !stale.contains(&"local-experiment".to_string()),
        "local-only branch should not be stale right after merge, got: {stale:?}"
    );
}

#[test]
fn local_only_branch_stale_after_main_advances() {
    let (_bare, work) = setup();

    // Create a local-only branch, merge it, then advance main past it.
    git(work.path(), &["checkout", "-b", "local-merged"]);
    fs::write(work.path().join("local.txt"), "work\n").unwrap();
    git(work.path(), &["add", "local.txt"]);
    git(work.path(), &["commit", "-m", "local work"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "local-merged"]);

    // Advance main past the branch.
    push_upstream_change(work.path(), "advance.txt", "new\n", "advance main");
    git(work.path(), &["pull", "origin", "main"]);

    let _cwd = cwd_at(work.path());
    let stale = git_switch::git::stale_branches("origin").unwrap();

    assert!(
        stale.contains(&"local-merged".to_string()),
        "merged local branch behind HEAD should be stale, got: {stale:?}"
    );
}

#[test]
fn merged_tracked_branch_is_stale() {
    let (_bare, work) = setup();

    // Create a branch, push it, then merge into main.
    // The upstream is in sync (not gone), but the branch is fully merged.
    git(work.path(), &["checkout", "-b", "feature-done"]);
    fs::write(work.path().join("feature.txt"), "done\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature"]);
    git(work.path(), &["push", "-u", "origin", "feature-done"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "feature-done"]);
    git(work.path(), &["push", "origin", "main"]);

    let _cwd = cwd_at(work.path());
    let stale = git_switch::git::stale_branches("origin").unwrap();

    assert!(
        stale.contains(&"feature-done".to_string()),
        "merged branch with upstream should be stale, got: {stale:?}"
    );
}

#[test]
fn tracked_branch_without_unique_commits_not_stale() {
    let (_bare, work) = setup();

    // Create and push a branch from main without adding any commits.
    git(work.path(), &["checkout", "-b", "new-feature"]);
    git(work.path(), &["push", "-u", "origin", "new-feature"]);
    git(work.path(), &["checkout", "main"]);

    // Simulate a pull that moves main ahead (branch is now behind HEAD).
    push_upstream_change(work.path(), "ahead.txt", "new\n", "advance main");
    git(work.path(), &["pull", "origin", "main"]);

    let _cwd = cwd_at(work.path());
    let stale = git_switch::git::stale_branches("origin").unwrap();

    assert!(
        !stale.contains(&"new-feature".to_string()),
        "branch with no unique commits should not be stale, got: {stale:?}"
    );
}

#[test]
fn delete_branches_removes_listed_branches() {
    let (_bare, work) = setup();

    for name in ["feat-a", "feat-b"] {
        git(work.path(), &["checkout", "-b", name]);
        fs::write(work.path().join(format!("{name}.txt")), "x\n").unwrap();
        git(work.path(), &["add", "."]);
        git(work.path(), &["commit", "-m", name]);
        git(work.path(), &["checkout", "main"]);
        git(
            work.path(),
            &["merge", "--no-ff", name, "-m", &format!("merge {name}")],
        );
    }

    let _cwd = cwd_at(work.path());
    let result = git_switch::git::delete_branches(&["feat-a", "feat-b"]);

    result.expect("delete_branches should succeed");

    let listing = git(work.path(), &["branch", "--format=%(refname:short)"]);
    let names = stdout_str(&listing);
    for name in ["feat-a", "feat-b"] {
        assert!(
            !names.lines().any(|l| l == name),
            "{name} should be deleted, got: {names}"
        );
    }
}

#[test]
fn worktree_held_stale_branch_is_skipped_with_warning() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "-b", "wip"]);
    fs::write(work.path().join("wip.txt"), "x\n").unwrap();
    git(work.path(), &["add", "wip.txt"]);
    git(work.path(), &["commit", "-m", "wip"]);
    git(work.path(), &["push", "-u", "origin", "wip"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "wip"]);
    git(work.path(), &["push", "origin", "main"]);

    let parent = TempDir::new().unwrap();
    let worktree_path = parent.path().join("wt");
    git(
        work.path(),
        &["worktree", "add", worktree_path.to_str().unwrap(), "wip"],
    );

    let output = git_switch(work.path(), "main");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let stderr = stderr_str(&output);
    assert!(
        stderr.contains("stale but held by worktree, skipping: wip"),
        "expected worktree-held warning, got: {stderr}"
    );

    let branches = git(
        work.path(),
        &["branch", "--list", "--format=%(refname:short)"],
    );
    assert!(
        stdout_str(&branches).lines().any(|l| l == "wip"),
        "wip should still exist, got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn help_flag_prints_usage() {
    let dir = TempDir::new().unwrap();
    let output = git_switch(dir.path(), "--help");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    let out = stdout_str(&output);
    assert!(
        out.contains("Usage: git-switch"),
        "expected usage line, got: {out}"
    );
    assert!(
        out.contains("git-switch wt"),
        "expected worktree usage in help, got: {out}"
    );
}

#[test]
fn version_flag_prints_version() {
    let dir = TempDir::new().unwrap();
    let output = git_switch(dir.path(), "--version");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert_eq!(
        stdout_str(&output).trim(),
        format!("git-switch {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn non_origin_remote_pulls_via_branch_config() {
    let (_bare, work) = setup_with_remote("upstream");

    push_upstream_change_to(
        work.path(),
        "upstream",
        "file.txt",
        "updated\n",
        "upstream change",
    );

    let output = git_switch(work.path(), "main");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("Pulled 1 commit"),
        "stderr: {}",
        stderr_str(&output)
    );

    let content = fs::read_to_string(work.path().join("file.txt")).unwrap();
    assert_eq!(content, "updated\n");
}

#[test]
fn non_origin_remote_detects_stale_branch() {
    let (_bare, work) = setup_with_remote("upstream");

    git(work.path(), &["checkout", "-b", "feature-done"]);
    fs::write(work.path().join("feature.txt"), "done\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature"]);
    git(work.path(), &["push", "-u", "upstream", "feature-done"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "feature-done"]);
    git(work.path(), &["push", "upstream", "main"]);

    let _cwd = cwd_at(work.path());
    let stale = git_switch::git::stale_branches("upstream").unwrap();

    assert!(
        stale.contains(&"feature-done".to_string()),
        "merged branch with upstream remote should be stale, got: {stale:?}"
    );
}

#[test]
fn current_remote_handles_multiline_config_value() {
    let (_bare, work) = setup_with_remote("upstream");

    git(
        work.path(),
        &["config", "branch.main.remote", "upstream\nstray"],
    );

    let _cwd = cwd_at(work.path());
    let remote = git_switch::git::current_remote(Some("main"));

    assert_eq!(remote, "upstream");
}

#[test]
fn rebase_replays_local_commits_onto_remote() {
    let (bare, work) = setup();

    git(work.path(), &["checkout", "-b", "feature"]);
    fs::write(work.path().join("feature.txt"), "base\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature base"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);

    // Local-only commit on a unique file (no conflict).
    fs::write(work.path().join("local.txt"), "local\n").unwrap();
    git(work.path(), &["add", "local.txt"]);
    git(work.path(), &["commit", "-m", "local commit"]);

    // From a second clone, push a different commit on a different file.
    let second = clone_bare(bare.path());
    git(second.path(), &["checkout", "feature"]);
    fs::write(second.path().join("remote.txt"), "remote\n").unwrap();
    git(second.path(), &["add", "remote.txt"]);
    git(second.path(), &["commit", "-m", "remote commit"]);
    git(second.path(), &["push", "origin", "feature"]);

    git(work.path(), &["fetch", "origin"]);

    let _cwd = cwd_at(work.path());
    let outcome = git_switch::git::rebase("origin/feature").expect("rebase call failed");

    assert!(
        matches!(outcome, git_switch::git::RebaseOutcome::Clean),
        "expected Clean rebase outcome"
    );
    assert!(
        work.path().join("local.txt").exists(),
        "local.txt should survive the rebase"
    );
    assert!(
        work.path().join("remote.txt").exists(),
        "remote.txt should be present after rebase"
    );
}

#[test]
fn rebase_aborts_on_conflict_and_leaves_clean_tree() {
    let (bare, work) = setup();

    git(work.path(), &["checkout", "-b", "feature"]);
    fs::write(work.path().join("file.txt"), "base\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "feature base"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);

    // Conflicting local change.
    fs::write(work.path().join("file.txt"), "local\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "local"]);

    // Conflicting remote change (force-pushed from a second clone).
    let second = clone_bare(bare.path());
    git(second.path(), &["checkout", "feature"]);
    fs::write(second.path().join("file.txt"), "remote\n").unwrap();
    git(second.path(), &["add", "file.txt"]);
    git(second.path(), &["commit", "-m", "remote"]);
    git(second.path(), &["push", "--force", "origin", "feature"]);

    git(work.path(), &["fetch", "origin"]);

    let _cwd = cwd_at(work.path());
    let outcome = git_switch::git::rebase("origin/feature").expect("rebase call failed");

    assert!(
        matches!(outcome, git_switch::git::RebaseOutcome::Aborted),
        "expected Aborted rebase outcome"
    );

    let git_dir = work.path().join(".git");
    assert!(
        !git_dir.join("rebase-merge").exists(),
        "rebase-merge directory should not exist after abort"
    );
    assert!(
        !git_dir.join("rebase-apply").exists(),
        "rebase-apply directory should not exist after abort"
    );
}

#[test]
fn pinned_branches_includes_default_first() {
    let (_bare, work) = setup();

    git(work.path(), &["remote", "set-head", "origin", "main"]);

    let _cwd = cwd_at(work.path());
    let pinned = git_switch::git::pinned_branches("origin");

    assert_eq!(
        pinned.first().map(String::as_str),
        Some("main"),
        "expected main first, got: {pinned:?}"
    );
}

#[test]
fn pinned_branches_appends_keep_config_in_order_and_dedups() {
    let (_bare, work) = setup();

    git(work.path(), &["remote", "set-head", "origin", "main"]);
    // Repo-local keep entries: deliberately duplicate "main" (the default)
    // and reorder release branches to verify config order is preserved.
    git(
        work.path(),
        &["config", "--add", "git-switch.keep", "release/v2"],
    );
    git(work.path(), &["config", "--add", "git-switch.keep", "main"]);
    git(
        work.path(),
        &["config", "--add", "git-switch.keep", "release/v1"],
    );

    let _cwd = cwd_at(work.path());
    let pinned = git_switch::git::pinned_branches("origin");

    let position = |name: &str| pinned.iter().position(|p| p == name);
    let main = position("main").expect("main should be present");
    let v2 = position("release/v2").expect("release/v2 should be present");
    let v1 = position("release/v1").expect("release/v1 should be present");

    assert_eq!(main, 0, "default branch must be first, got: {pinned:?}");
    assert!(v2 > main, "release/v2 after main, got: {pinned:?}");
    assert!(v1 > v2, "release/v1 after release/v2, got: {pinned:?}");
    assert_eq!(
        pinned.iter().filter(|p| *p == "main").count(),
        1,
        "main should be deduped, got: {pinned:?}"
    );
}

#[test]
fn detached_head_can_switch_to_branch() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "--detach", "HEAD"]);

    let output = git_switch(work.path(), "main");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let head = git(work.path(), &["branch", "--show-current"]);
    assert_eq!(stdout_str(&head).trim(), "main");
}

#[test]
fn wt_creates_worktree_for_existing_local_branch() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);

    let output = git_switch_args(&work, &["wt", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let expected = parent.path().join("worktrees").join("repo").join("feature");
    assert!(
        expected.exists(),
        "worktree should exist at {}",
        expected.display()
    );
    assert!(
        stdout_str(&output).trim().ends_with("repo/feature"),
        "stdout should be the worktree path; got: {}",
        stdout_str(&output)
    );

    let list = git(&work, &["worktree", "list", "--porcelain"]);
    let s = stdout_str(&list);
    assert!(
        s.contains("branch refs/heads/feature"),
        "expected `feature` worktree; got: {s}"
    );
}

#[test]
fn wt_creates_new_branch_from_default_when_branch_absent() {
    let (_bare, parent, work) = setup_with_parent();

    let output = git_switch_args(&work, &["wt", "brand-new"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let expected = parent
        .path()
        .join("worktrees")
        .join("repo")
        .join("brand-new");
    assert!(
        expected.exists(),
        "worktree should exist at {}",
        expected.display()
    );

    // The new branch should have a base commit (from origin/main).
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&expected)
        .output()
        .unwrap();
    assert!(head.status.success(), "stderr: {}", stderr_str(&head));
}

#[test]
fn wt_preserves_slashes_as_subdirs() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature/nested"]);

    let output = git_switch_args(&work, &["wt", "feature/nested"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let expected = parent
        .path()
        .join("worktrees")
        .join("repo")
        .join("feature")
        .join("nested");
    assert!(
        expected.exists(),
        "nested worktree should exist at {}",
        expected.display()
    );
}

#[test]
fn wt_cd_to_existing_worktree_prints_path() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    let output = git_switch_args(&work, &["wt", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let printed = stdout_str(&output).trim().to_string();
    assert!(
        printed.ends_with("worktrees/repo/feature") && Path::new(&printed).is_dir(),
        "stdout should be the existing worktree path; got: {printed}"
    );
    // A worktree branch without an upstream must not emit "No remote…" noise.
    assert!(
        !stderr_str(&output).contains("No remote"),
        "cd to a worktree should stay quiet about missing upstream; got: {}",
        stderr_str(&output)
    );
}

#[test]
fn wt_refuses_when_target_path_is_stale_non_worktree_directory() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let stale = parent.path().join("worktrees").join("repo").join("feature");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("leftover.txt"), "junk").unwrap();

    let output = git_switch_args(&work, &["wt", "feature"]);
    assert!(!output.status.success());

    let combined = format!("{}{}", stdout_str(&output), stderr_str(&output));
    assert!(
        combined.contains("exists but is not a registered worktree"),
        "expected stale-dir error; got: {combined}"
    );
}

#[test]
fn wt_recreates_worktree_whose_directory_was_deleted_by_hand() {
    let (_bare, parent, work) = setup_with_parent();

    // Create a worktree, then delete its directory without telling git. The
    // registration lingers as "missing but already registered" and would block
    // `git worktree add`; git-switch should prune it and recreate cleanly.
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "-b", "feature"],
    );
    fs::remove_dir_all(&path).unwrap();

    let output = git_switch_args(&work, &["wt", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        path.is_dir(),
        "worktree should be recreated at {}",
        path.display()
    );

    let list = stdout_str(&git(&work, &["worktree", "list", "--porcelain"]));
    assert!(
        !list.contains("prunable"),
        "stale registration should be pruned; got: {list}"
    );
}

#[test]
fn wt_ls_lists_all_worktrees() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    let output = git_switch_args(&work, &["wt", "ls"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let out = stdout_str(&output);
    assert!(out.contains("main"), "ls should mention main; got: {out}");
    assert!(
        out.contains("feature"),
        "ls should mention feature; got: {out}"
    );
}

#[test]
fn wt_rm_removes_worktree_and_deletes_branch() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    let output = git_switch_args(&work, &["wt", "rm", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    assert!(
        !path.exists(),
        "worktree dir should be removed: {}",
        path.display()
    );

    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        !stdout_str(&branches).lines().any(|l| l == "feature"),
        "branch should be deleted; got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn in_place_switch_hands_off_when_branch_is_held_by_worktree() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    // Plain `git-switch feature` from main worktree: branch is held → handoff.
    let output = git_switch(&work, "feature");
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let printed = stdout_str(&output).trim().to_string();
    assert!(
        printed.ends_with("worktrees/repo/feature") && Path::new(&printed).is_dir(),
        "stdout should be the worktree path for the shell wrapper; got: {printed}"
    );

    // Original worktree's HEAD must NOT have changed (no checkout happened).
    let head = git(&work, &["branch", "--show-current"]);
    assert_eq!(stdout_str(&head).trim(), "main");
}

#[test]
fn wt_rm_keeps_branch_with_unmerged_commits() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    // A commit on `feature` that never lands on main → not fully merged.
    fs::write(path.join("new.txt"), "unmerged\n").unwrap();
    git(&path, &["add", "new.txt"]);
    git(&path, &["commit", "-m", "unmerged work"]);

    let output = git_switch_args(&work, &["wt", "rm", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    assert!(
        !path.exists(),
        "worktree dir should be removed: {}",
        path.display()
    );

    // Branch must survive, since -d refuses to drop unmerged commits.
    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        stdout_str(&branches).lines().any(|l| l == "feature"),
        "branch should be kept; got: {}",
        stdout_str(&branches)
    );
    assert!(
        stderr_str(&output).contains("unmerged"),
        "should warn about unmerged commits; got: {}",
        stderr_str(&output)
    );
}

#[test]
fn double_dash_switches_to_branch_named_like_subcommand() {
    let (_bare, work) = setup();

    git(work.path(), &["branch", "wt"]);

    let output = git_switch_args(work.path(), &["--", "wt"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let head = git(work.path(), &["branch", "--show-current"]);
    assert_eq!(stdout_str(&head).trim(), "wt");

    // Switching off `main` makes it a stale merged branch, which triggers the
    // delete prompt. Non-interactively that must neither block nor act: `main`
    // must survive (regression guard for the multi_select TTY check).
    let branches = git(work.path(), &["branch", "--format=%(refname:short)"]);
    assert!(
        stdout_str(&branches).lines().any(|l| l == "main"),
        "main must not be auto-deleted in a non-interactive run; got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn wt_rm_from_inside_doomed_worktree_hands_off_to_main() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    // Run `wt rm feature` *from inside* the worktree being removed: cwd would
    // vanish, so it must chdir to main and hand that path off for the wrapper.
    let output = git_switch_args(&path, &["wt", "rm", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(!path.exists(), "worktree dir should be removed");

    let printed = stdout_str(&output).trim().to_string();
    assert!(
        Path::new(&printed).is_dir() && printed.ends_with("repo"),
        "stdout should be the main worktree path; got: {printed}"
    );
}

#[test]
fn handoff_fast_forwards_held_worktree_from_its_own_remote() {
    let (_bare, parent, work) = setup_with_parent();

    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            path.to_str().unwrap(),
            "main",
        ],
    );

    // Publish a commit on `feature`, record it, then rewind the worktree so it
    // sits one commit behind its upstream.
    fs::write(path.join("f.txt"), "v1\n").unwrap();
    git(&path, &["add", "f.txt"]);
    git(&path, &["commit", "-m", "remote work"]);
    git(&path, &["push", "-u", "origin", "feature"]);
    let upstream = stdout_str(&git(&path, &["rev-parse", "HEAD"]))
        .trim()
        .to_string();
    git(&path, &["reset", "--hard", "HEAD~1"]);

    // Plain `git-switch feature` from main: hands off and updates the worktree.
    let output = git_switch(&work, "feature");
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("Pulled 1 commit"),
        "should fast-forward the held worktree; stderr: {}",
        stderr_str(&output)
    );

    // The worktree (not the main checkout) must now be at the upstream commit.
    let head = stdout_str(&git(&path, &["rev-parse", "HEAD"]))
        .trim()
        .to_string();
    assert_eq!(head, upstream, "worktree HEAD should be fast-forwarded");
}

#[test]
fn wt_rm_reports_failure_and_keeps_branch_when_worktree_is_dirty() {
    let (_bare, parent, work) = setup_with_parent();

    git(&work, &["branch", "feature"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    git(
        &work,
        &["worktree", "add", path.to_str().unwrap(), "feature"],
    );

    // An untracked file makes `git worktree remove` refuse without --force.
    fs::write(path.join("dirty.txt"), "uncommitted\n").unwrap();

    let output = git_switch_args(&work, &["wt", "rm", "feature"]);
    // The command itself succeeds (per-worktree failures are reported, not fatal).
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("failed to remove"),
        "should report the removal failure; stderr: {}",
        stderr_str(&output)
    );

    // Nothing was destroyed: the worktree dir survives and the branch remains.
    assert!(path.exists(), "worktree dir should still exist on failure");
    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        stdout_str(&branches).lines().any(|l| l == "feature"),
        "branch must not be deleted when removal failed; got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn wt_rm_clears_missing_detached_worktree_by_dir_name() {
    let (_bare, parent, work) = setup_with_parent();

    // A detached worktree whose directory was deleted by hand: it reports no
    // branch and lingers as a "prunable" registration. `wt rm` must still be
    // able to target it (by directory name) and clear the dead entry.
    let path = parent.path().join("worktrees").join("repo").join("scratch");
    git(
        &work,
        &["worktree", "add", "--detach", path.to_str().unwrap()],
    );
    fs::remove_dir_all(&path).unwrap();

    let output = git_switch_args(&work, &["wt", "rm", "scratch"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let list = stdout_str(&git(&work, &["worktree", "list", "--porcelain"]));
    assert!(
        !list.contains("prunable") && !list.contains("scratch"),
        "stale registration should be cleared; got: {list}"
    );
}

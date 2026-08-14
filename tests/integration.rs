use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
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

/// Hooks come from git config, which layers in the developer's global file, so
/// suppress them everywhere: a machine with `git-switch.hook.created` set would
/// otherwise run it throughout the suite. The hook tests use
/// [`git_switch_hooked`].
fn git_switch_args(dir: &Path, args: &[&str]) -> Output {
    git_switch_command(dir, args)
        .env("GIT_SWITCH_NO_HOOKS", "1")
        .output()
        .expect("failed to run git-switch")
}

/// Like [`git_switch_args`], but with hooks left on — for the tests that
/// configure one in the repo under test.
fn git_switch_hooked(dir: &Path, args: &[&str]) -> Output {
    git_switch_command(dir, args)
        .env_remove("GIT_SWITCH_NO_HOOKS")
        .output()
        .expect("failed to run git-switch")
}

fn git_switch_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_git-switch"));
    cmd.args(args).current_dir(dir);
    cmd
}

fn git_switch(dir: &Path, branch: &str) -> Output {
    git_switch_args(dir, &[branch])
}

/// Point a bare repo's HEAD at `main`.
///
/// `git init --bare` derives HEAD from the host's `init.defaultBranch`, so on a
/// machine that still defaults to `master` the bare ends up with a HEAD that
/// names a ref the tests never create. Cloning it then checks out nothing —
/// "remote HEAD refers to nonexistent ref" — and a later `push origin main`
/// fails with "src refspec main does not match any". Pin it so the tests don't
/// depend on the developer's git config.
fn pin_default_branch(bare: &Path) {
    git(bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
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
    pin_default_branch(bare.path());

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
    pin_default_branch(bare.path());

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
    // `git clone` writes `refs/remotes/<remote>/HEAD`, but `init` + `remote add`
    // + `push` does not. Staleness is judged against the default branch, so a
    // setup without it wouldn't resemble any real clone.
    git(work.path(), &["remote", "set-head", remote, "main"]);

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

/// Just the names of the stale branches. Which ground each is stale on is
/// covered by the unit tests in `git`, against fixed refs rather than a repo.
fn stale_names(remote: &str) -> Vec<String> {
    git_switch::git::stale_branches(remote)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect()
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
fn refresh_dot_fast_forwards_clean_branch() {
    let (_bare, work) = setup();

    push_upstream_change(work.path(), "file.txt", "updated\n", "upstream change");

    let output = git_switch(work.path(), ".");

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
fn refresh_dot_already_up_to_date_reports_so() {
    let (_bare, work) = setup();

    let output = git_switch(work.path(), ".");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert!(
        stderr_str(&output).contains("Already up to date"),
        "stderr: {}",
        stderr_str(&output)
    );
}

#[test]
fn refresh_dot_clean_diverge_rebases_onto_remote() {
    let (bare, work) = setup();

    // A local commit on a different file than the one the remote advances, so
    // the branch diverges but the rebase replays cleanly.
    fs::write(work.path().join("other.txt"), "local work\n").unwrap();
    git(work.path(), &["add", "other.txt"]);
    git(work.path(), &["commit", "-m", "local work"]);

    let second = clone_bare(bare.path());
    push_upstream_change(
        second.path(),
        "file.txt",
        "remote change\n",
        "remote change",
    );
    git(work.path(), &["fetch", "origin"]);

    let output = git_switch(work.path(), ".");

    // Clean tree: the local commit is rebased on top of the remote commit with
    // no prompt. Both changes are present and origin/main is now in HEAD.
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    assert_eq!(
        fs::read_to_string(work.path().join("other.txt")).unwrap(),
        "local work\n"
    );
    assert_eq!(
        fs::read_to_string(work.path().join("file.txt")).unwrap(),
        "remote change\n"
    );
    let behind = git(work.path(), &["rev-list", "--count", "HEAD..origin/main"]);
    assert_eq!(
        stdout_str(&behind).trim(),
        "0",
        "HEAD should contain origin/main after the rebase"
    );
}

#[test]
fn refresh_dot_clean_diverge_with_conflict_aborts() {
    let (bare, work) = setup();

    // Local commit and a rewritten origin commit that both touch file.txt, so
    // rebasing the local commit onto origin conflicts.
    fs::write(work.path().join("file.txt"), "local diverge\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "local diverge"]);
    let local_head = stdout_str(&git(work.path(), &["rev-parse", "HEAD"]));

    let second = clone_bare(bare.path());
    push_upstream_change(
        second.path(),
        "file.txt",
        "remote rebase\n",
        "remote rebase",
    );
    git(work.path(), &["fetch", "origin"]);

    let output = git_switch(work.path(), ".");

    // The rebase conflicts, aborts, and restores the original HEAD.
    assert!(!output.status.success());
    let combined = format!("{}{}", stdout_str(&output), stderr_str(&output));
    assert!(
        combined.contains("Rebase aborted"),
        "expected rebase-aborted message, got: {combined}"
    );
    let head_after = stdout_str(&git(work.path(), &["rev-parse", "HEAD"]));
    assert_eq!(
        head_after, local_head,
        "abort must restore the original HEAD"
    );
}

#[test]
fn refresh_dot_dirty_with_incoming_is_left_unchanged_non_interactively() {
    let (_bare, work) = setup();

    // Remote advances, then dirty a tracked file: there's work to integrate but
    // the tree is dirty, so a non-interactive run can't prompt and does nothing.
    push_upstream_change(work.path(), "file.txt", "remote change\n", "remote change");
    fs::write(work.path().join("file.txt"), "uncommitted edit\n").unwrap();
    let head_before = stdout_str(&git(work.path(), &["rev-parse", "HEAD"]));

    let output = git_switch(work.path(), ".");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    let stderr = stderr_str(&output);
    assert!(
        stderr.contains("uncommitted changes") && stderr.contains("Left main unchanged"),
        "expected dirty-tree notice and no-op, got: {stderr}"
    );
    assert_eq!(
        stdout_str(&git(work.path(), &["rev-parse", "HEAD"])),
        head_before,
        "HEAD must not move without a prompt"
    );
    assert_eq!(
        fs::read_to_string(work.path().join("file.txt")).unwrap(),
        "uncommitted edit\n",
        "uncommitted changes must be preserved"
    );
}

#[test]
fn refresh_dot_with_unpushed_commit_reports_ahead_only() {
    let (_bare, work) = setup();

    // A local commit the remote doesn't have, but the remote hasn't moved on:
    // ahead, not diverged.
    fs::write(work.path().join("file.txt"), "local only\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "local only"]);
    let local_head = stdout_str(&git(work.path(), &["rev-parse", "HEAD"]));

    let output = git_switch(work.path(), ".");

    assert!(output.status.success(), "stderr: {}", stderr_str(&output));
    let stderr = stderr_str(&output);
    assert!(
        stderr.contains("1 commit ahead of origin/main") && stderr.contains("nothing to pull"),
        "expected ahead-only notice, got: {stderr}"
    );
    let head_after = stdout_str(&git(work.path(), &["rev-parse", "HEAD"]));
    assert_eq!(
        head_after, local_head,
        "HEAD must not move when nothing to pull"
    );
}

#[test]
fn refresh_dot_on_detached_head_errors() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "--detach"]);

    let output = git_switch(work.path(), ".");

    assert!(!output.status.success());
    assert!(
        stderr_str(&output).contains("not on a branch"),
        "stderr: {}",
        stderr_str(&output)
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
    let stale = stale_names("origin");

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
    let stale = stale_names("origin");

    assert!(
        stale.contains(&"local-merged".to_string()),
        "merged local branch behind HEAD should be stale, got: {stale:?}"
    );
}

/// A branch that published its own counterpart hands the question over to the
/// remote: once both are pushed, a branch whose commits main fast-forwarded
/// over is byte-identical to one pushed without any commits at all. Deleting
/// the remote branch is the signal that settles it.
#[test]
fn merged_tracked_branch_waits_for_its_upstream_to_go() {
    let (bare, work) = setup();

    git(work.path(), &["checkout", "-b", "feature-done"]);
    fs::write(work.path().join("feature.txt"), "done\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature"]);
    git(work.path(), &["push", "-u", "origin", "feature-done"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "feature-done"]);
    git(work.path(), &["push", "origin", "main"]);

    {
        let _cwd = cwd_at(work.path());
        let stale = stale_names("origin");
        assert!(
            !stale.contains(&"feature-done".to_string()),
            "a live upstream is indistinguishable from an unstarted branch, got: {stale:?}"
        );
    }

    git(bare.path(), &["branch", "-D", "feature-done"]);
    git(work.path(), &["fetch", "--prune", "origin"]);

    let _cwd = cwd_at(work.path());
    let stale = stale_names("origin");
    assert!(
        stale.contains(&"feature-done".to_string()),
        "a deleted upstream settles it, got: {stale:?}"
    );
}

/// The other half of the same ambiguity: a branch pushed before any work was
/// done on it. Judging it by its tip would offer it the moment you switched
/// away — and, being the branch just left, pre-tick it.
#[test]
fn empty_published_branch_is_not_stale_from_a_branch_past_main() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "-b", "feature"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);

    // Somewhere further along than main, so the old ambient-HEAD rule and the
    // anchor rule disagree about `feature`.
    git(work.path(), &["checkout", "-b", "other"]);
    fs::write(work.path().join("other.txt"), "x\n").unwrap();
    git(work.path(), &["add", "other.txt"]);
    git(work.path(), &["commit", "-m", "other advances"]);

    let _cwd = cwd_at(work.path());
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"feature".to_string()),
        "a branch pushed without commits must not be offered, got: {stale:?}"
    );
}

/// History is no more telling than the tip. An empty branch pushed at a merged
/// topic's tip presents the same refs as the topic itself, so no shape of
/// history can tell them apart.
#[test]
fn empty_published_branch_at_a_merged_tip_is_not_stale() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "-b", "topic"]);
    fs::write(work.path().join("topic.txt"), "work\n").unwrap();
    git(work.path(), &["add", "topic.txt"]);
    git(work.path(), &["commit", "-m", "topic work"]);
    git(work.path(), &["checkout", "main"]);
    git(
        work.path(),
        &["merge", "--no-ff", "-m", "merge topic", "topic"],
    );

    // Branch off the merged topic without adding anything, and publish it.
    git(work.path(), &["checkout", "-b", "feature", "topic"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);
    git(work.path(), &["checkout", "main"]);

    let _cwd = cwd_at(work.path());
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"feature".to_string()),
        "a branch pushed without commits must not be offered, got: {stale:?}"
    );
    assert!(
        stale.contains(&"topic".to_string()),
        "the untracked topic that did the work should still be offered, got: {stale:?}"
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
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"new-feature".to_string()),
        "branch with no unique commits should not be stale, got: {stale:?}"
    );
}

/// Adds a worktree the way `git-switch wt` does: a new branch off
/// `origin/main`, tracking it.
fn add_worktree_branch(work: &Path, parent: &Path, branch: &str) -> PathBuf {
    let path = parent.join("worktrees").join("repo").join(branch);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        work,
        &[
            "worktree",
            "add",
            "--track",
            "-b",
            branch,
            path.to_str().unwrap(),
            "origin/main",
        ],
    );
    path
}

/// A branch created off `origin/main` and never committed to has its tip *equal*
/// to main's, which the old `tip == HEAD` rule read as "fast-forward merged".
#[test]
fn fresh_worktree_branch_is_not_stale_from_the_main_worktree() {
    let (_bare, parent, work) = setup_with_parent();
    add_worktree_branch(&work, parent.path(), "feature-a");

    let _cwd = cwd_at(&work);
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"feature-a".to_string()),
        "an empty worktree branch should not be stale, got: {stale:?}"
    );
}

/// The reported bug: staleness used to be judged against ambient HEAD, so
/// committing in one worktree made every *sibling* worktree's branch look
/// merged-and-behind.
#[test]
fn fresh_worktree_branch_is_not_stale_from_a_sibling_worktree() {
    let (_bare, parent, work) = setup_with_parent();
    add_worktree_branch(&work, parent.path(), "feature-a");
    let b = add_worktree_branch(&work, parent.path(), "feature-b");

    fs::write(b.join("work.txt"), "work\n").unwrap();
    git(&b, &["add", "work.txt"]);
    git(&b, &["commit", "-m", "sibling work"]);

    let _cwd = cwd_at(&b);
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"feature-a".to_string()),
        "a sibling's commits must not make feature-a stale, got: {stale:?}"
    );
}

/// `git-switch wt`'s own merge-locally workflow: a worktree branch that did real
/// work, fast-forwarded into main. It tracks `origin/main` rather than its own
/// counterpart rather than its own, so only being *ahead* of what it tracks
/// separates it from a branch that never held a commit.
#[test]
fn worktree_branch_fast_forwarded_into_main_is_stale() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree_branch(&work, parent.path(), "feature-a");

    fs::write(path.join("work.txt"), "real work\n").unwrap();
    git(&path, &["add", "work.txt"]);
    git(&path, &["commit", "-m", "real work"]);
    git(&work, &["merge", "--ff-only", "feature-a"]);

    let _cwd = cwd_at(&work);
    let stale = stale_names("origin");

    assert!(
        stale.contains(&"feature-a".to_string()),
        "a worktree branch merged into main should be stale, got: {stale:?}"
    );
}

/// How a merge commit reshapes history is not evidence either way, so a `wt`
/// branch merged with `--no-ff` rests on the same ahead count as any other:
/// offered while main still holds commits its upstream doesn't, and silent once
/// main is pushed and the count falls back to zero.
#[test]
fn no_ff_merged_branch_is_stale_until_main_is_pushed() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree_branch(&work, parent.path(), "feature-noff");

    fs::write(path.join("noff.txt"), "work\n").unwrap();
    git(&path, &["add", "noff.txt"]);
    git(&path, &["commit", "-m", "work"]);
    git(
        &work,
        &["merge", "--no-ff", "-m", "merge feature", "feature-noff"],
    );

    {
        let _cwd = cwd_at(&work);
        let stale = stale_names("origin");
        assert!(
            stale.contains(&"feature-noff".to_string()),
            "work main holds and origin/main doesn't should be offered, got: {stale:?}"
        );
    }

    git(&work, &["push", "origin", "main"]);

    let _cwd = cwd_at(&work);
    let stale = stale_names("origin");
    assert!(
        !stale.contains(&"feature-noff".to_string()),
        "once pushed it cannot be told from an untouched branch, got: {stale:?}"
    );
}

/// The same shape reached from the other side: an empty branch pointed at a
/// merged topic's tip and set to track `origin/main`. Nothing separates it from
/// the merged worktree branch above once main is pushed, so neither is offered.
#[test]
fn empty_anchor_tracking_branch_at_a_merged_tip_is_not_stale() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "-b", "topic"]);
    fs::write(work.path().join("topic.txt"), "work\n").unwrap();
    git(work.path(), &["add", "topic.txt"]);
    git(work.path(), &["commit", "-m", "topic work"]);
    git(work.path(), &["checkout", "main"]);
    git(
        work.path(),
        &["merge", "--no-ff", "-m", "merge topic", "topic"],
    );
    git(work.path(), &["push", "origin", "main"]);

    git(work.path(), &["branch", "feature", "topic"]);
    git(
        work.path(),
        &["branch", "--set-upstream-to=origin/main", "feature"],
    );

    let _cwd = cwd_at(work.path());
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"feature".to_string()),
        "a branch with no commits of its own must not be offered, got: {stale:?}"
    );
    assert!(
        stale.contains(&"topic".to_string()),
        "the untracked topic that did the work should still be offered, got: {stale:?}"
    );
}

/// Without a default branch there is nothing to judge "merged" against, so
/// the merged rule stands down rather than falling back to ambient HEAD. A
/// deleted upstream still speaks for itself.
#[test]
fn without_a_default_branch_only_gone_upstreams_are_stale() {
    let (bare, work) = setup();

    // A merged branch that would otherwise qualify.
    git(work.path(), &["checkout", "-b", "merged-work"]);
    fs::write(work.path().join("merged.txt"), "work\n").unwrap();
    git(work.path(), &["add", "merged.txt"]);
    git(work.path(), &["commit", "-m", "work"]);
    git(work.path(), &["checkout", "main"]);
    git(
        work.path(),
        &["merge", "--no-ff", "-m", "merge", "merged-work"],
    );

    // A branch whose upstream is deleted on the remote.
    git(work.path(), &["checkout", "-b", "abandoned"]);
    git(work.path(), &["push", "-u", "origin", "abandoned"]);
    git(work.path(), &["checkout", "main"]);
    git(bare.path(), &["branch", "-D", "abandoned"]);
    git(work.path(), &["fetch", "--prune", "origin"]);

    git(work.path(), &["remote", "set-head", "origin", "--delete"]);

    let _cwd = cwd_at(work.path());
    let stale = stale_names("origin");

    assert!(
        !stale.contains(&"merged-work".to_string()),
        "no anchor means no merged rule, got: {stale:?}"
    );
    assert!(
        stale.contains(&"abandoned".to_string()),
        "a gone upstream is stale with or without an anchor, got: {stale:?}"
    );
}

/// The upstream a new worktree branch carries is load-bearing for the staleness
/// rules, so it must not depend on the user's `branch.autoSetupMerge`.
#[test]
fn worktree_add_sets_upstream_with_auto_setup_merge_off() {
    let (_bare, parent, work) = setup_with_parent();
    git(&work, &["config", "branch.autoSetupMerge", "false"]);
    let path = parent.path().join("worktrees").join("repo").join("feature");
    fs::create_dir_all(path.parent().unwrap()).unwrap();

    let _cwd = cwd_at(&work);
    git_switch::git::worktree_add(&path, "feature", Some("origin/main")).unwrap();

    let upstream = git(
        &work,
        &["for-each-ref", "--format=%(upstream)", "refs/heads/feature"],
    );
    assert_eq!(
        stdout_str(&upstream).trim(),
        "refs/remotes/origin/main",
        "worktree branches must track what they were created from, whatever the config"
    );
}

#[test]
fn force_delete_branch_removes_branch() {
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
    for name in ["feat-a", "feat-b"] {
        let outcome = git_switch::git::force_delete_branch(None, name)
            .expect("force_delete_branch should not error");
        assert!(
            matches!(outcome, git_switch::git::BranchDeleteOutcome::Deleted),
            "{name} should report as deleted"
        );
    }

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
fn worktree_held_stale_branch_is_no_longer_reported_as_skipped() {
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

    // A held stale branch is now offered in the prompt alongside its worktree
    // rather than dismissed as unactionable.
    let stderr = stderr_str(&output);
    assert!(
        !stderr.contains("skipping"),
        "the dead-end skip message should be gone, got: {stderr}"
    );

    // Non-interactively there's no prompt, so nothing is destroyed: the branch
    // and its worktree both survive.
    let branches = git(
        work.path(),
        &["branch", "--list", "--format=%(refname:short)"],
    );
    assert!(
        stdout_str(&branches).lines().any(|l| l == "wip"),
        "wip should still exist, got: {}",
        stdout_str(&branches)
    );
    assert!(
        worktree_path.is_dir(),
        "worktree should survive a non-interactive run: {}",
        worktree_path.display()
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
    let (bare, work) = setup_with_remote("upstream");

    git(work.path(), &["checkout", "-b", "feature-done"]);
    fs::write(work.path().join("feature.txt"), "done\n").unwrap();
    git(work.path(), &["add", "feature.txt"]);
    git(work.path(), &["commit", "-m", "feature"]);
    git(work.path(), &["push", "-u", "upstream", "feature-done"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "feature-done"]);
    git(work.path(), &["push", "upstream", "main"]);
    git(bare.path(), &["branch", "-D", "feature-done"]);
    git(work.path(), &["fetch", "--prune", "upstream"]);

    let _cwd = cwd_at(work.path());
    let stale = stale_names("upstream");

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

/// Risk is judged from the main worktree, so the delete must run there too.
/// `git branch -d` consults HEAD only where no upstream is set, so an untracked
/// branch is where the difference shows: removing it while standing in an
/// unrelated worktree used to ask `-d` from that unrelated HEAD, which refuses.
/// The row was marked safe and the branch survived anyway.
#[test]
fn wt_rm_deletes_an_untracked_merged_branch_from_an_unrelated_worktree() {
    let (_bare, parent, work) = setup_with_parent();

    let done = parent
        .path()
        .join("worktrees")
        .join("repo")
        .join("feature-done");
    fs::create_dir_all(done.parent().unwrap()).unwrap();
    git(
        &work,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            "feature-done",
            done.to_str().unwrap(),
            "main",
        ],
    );
    fs::write(done.join("done.txt"), "work\n").unwrap();
    git(&done, &["add", "done.txt"]);
    git(&done, &["commit", "-m", "done"]);
    git(&work, &["merge", "--ff-only", "feature-done"]);

    // Diverge the worktree we run from, so `feature-done` is merged into main
    // but not into this HEAD.
    let elsewhere = add_worktree_branch(&work, parent.path(), "feature-elsewhere");
    fs::write(elsewhere.join("other.txt"), "other\n").unwrap();
    git(&elsewhere, &["add", "other.txt"]);
    git(&elsewhere, &["commit", "-m", "other"]);

    let output = git_switch_args(&elsewhere, &["wt", "rm", "feature-done"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        !stdout_str(&branches).lines().any(|l| l == "feature-done"),
        "branch merged into main should be deleted; got: {}",
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

/// Creates a worktree for a new branch and returns its path.
fn add_worktree(work: &Path, parent: &TempDir, branch: &str) -> PathBuf {
    git(work, &["branch", branch]);
    let path = parent.path().join("worktrees").join("repo").join(branch);
    git(work, &["worktree", "add", path.to_str().unwrap(), branch]);
    path
}

fn commit_in(path: &Path, file: &str, msg: &str) {
    fs::write(path.join(file), "x\n").unwrap();
    git(path, &["add", file]);
    git(path, &["commit", "-m", msg]);
}

/// A named target carries risk but has no picker row to warn on, and a piped
/// run can neither show a warning nor ask — so it must refuse outright rather
/// than destroy something unwarned.
#[test]
fn wt_rm_refuses_unmerged_branch_non_interactively() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    commit_in(&path, "new.txt", "unmerged work");

    let output = git_switch_args(&work, &["wt", "rm", "feature"]);

    assert!(
        !output.status.success(),
        "should exit non-zero; stderr: {}",
        stderr_str(&output)
    );
    assert!(
        path.exists(),
        "worktree must survive a refusal: {}",
        path.display()
    );
    let stderr = stderr_str(&output);
    assert!(
        stderr.contains("--force"),
        "refusal should point at the escape hatch; got: {stderr}"
    );

    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        stdout_str(&branches).lines().any(|l| l == "feature"),
        "branch must survive a refusal; got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn wt_rm_refuses_dirty_worktree_non_interactively() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    fs::write(path.join("scratch.txt"), "uncommitted\n").unwrap();

    let output = git_switch_args(&work, &["wt", "rm", "feature"]);

    assert!(
        !output.status.success(),
        "should exit non-zero; stderr: {}",
        stderr_str(&output)
    );
    assert!(
        stderr_str(&output).contains("uncommitted"),
        "should name the risk; got: {}",
        stderr_str(&output)
    );
    assert!(path.exists(), "worktree must survive: {}", path.display());
}

/// `--force` waives the confirmation, discarding uncommitted changes and the
/// unmerged branch alike.
#[test]
fn wt_rm_force_removes_dirty_worktree_and_unmerged_branch() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    commit_in(&path, "new.txt", "unmerged work");
    fs::write(path.join("scratch.txt"), "uncommitted\n").unwrap();

    let output = git_switch_args(&work, &["wt", "rm", "feature", "--force"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    assert!(
        !path.exists(),
        "worktree should be removed: {}",
        path.display()
    );
    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        !stdout_str(&branches).lines().any(|l| l == "feature"),
        "unmerged branch should be force-deleted; got: {}",
        stdout_str(&branches)
    );
}

/// A clean, merged worktree has nothing to lose, so `.` needs no confirmation
/// even though it names a target.
#[test]
fn wt_rm_dot_removes_the_current_worktree() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");

    let output = git_switch_args(&path, &["wt", "rm", "."]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    assert!(
        !path.exists(),
        "the worktree we stood in should be removed: {}",
        path.display()
    );

    // The cwd just vanished, so the shell wrapper is handed the main worktree.
    let printed = stdout_str(&output).trim().to_string();
    assert_eq!(
        Path::new(&printed).canonicalize().ok(),
        work.canonicalize().ok(),
        "stdout should hand the main worktree to the shell wrapper; got: {printed}"
    );
}

/// Regression: `git branch --merged` is relative to HEAD, and every branch is
/// merged into itself — so judging risk from inside the worktree being removed
/// reported its own branch as merged, skipped the warning, and left the branch
/// behind after the worktree went. Risk must be judged from the main worktree.
#[test]
fn wt_rm_dot_sees_its_own_branch_as_unmerged() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    commit_in(&path, "new.txt", "unmerged work");

    let output = git_switch_args(&path, &["wt", "rm", "."]);

    assert!(
        !output.status.success(),
        "unmerged work should be flagged, not silently skipped; stderr: {}",
        stderr_str(&output)
    );
    assert!(
        stderr_str(&output).contains("unmerged"),
        "should name the unmerged commits; got: {}",
        stderr_str(&output)
    );
    assert!(path.exists(), "worktree must survive: {}", path.display());
}

/// `--force` on `.` must finish the job: no worktree *and* no leftover branch.
#[test]
fn wt_rm_dot_force_leaves_no_branch_behind() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    commit_in(&path, "new.txt", "unmerged work");

    let output = git_switch_args(&path, &["wt", "rm", ".", "--force"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    assert!(
        !path.exists(),
        "worktree should be gone: {}",
        path.display()
    );
    let branches = git(&work, &["branch", "--format=%(refname:short)"]);
    assert!(
        !stdout_str(&branches).lines().any(|l| l == "feature"),
        "no leftover branch; got: {}",
        stdout_str(&branches)
    );
}

#[test]
fn wt_rm_dot_in_the_main_worktree_errors() {
    let (_bare, parent, work) = setup_with_parent();
    // A removable worktree exists, so `.` fails on its own merits rather than
    // on there being nothing to remove at all.
    add_worktree(&work, &parent, "feature");

    let output = git_switch_args(&work, &["wt", "rm", "."]);

    assert!(
        !output.status.success(),
        "should exit non-zero; stderr: {}",
        stderr_str(&output)
    );
    assert!(
        stderr_str(&output).contains("main worktree cannot be removed"),
        "should explain why; got: {}",
        stderr_str(&output)
    );
}

#[test]
fn wt_rm_dot_refuses_dirty_worktree_non_interactively() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");
    fs::write(path.join("scratch.txt"), "uncommitted\n").unwrap();

    let output = git_switch_args(&path, &["wt", "rm", "."]);

    assert!(
        !output.status.success(),
        "should exit non-zero; stderr: {}",
        stderr_str(&output)
    );
    assert!(path.exists(), "worktree must survive: {}", path.display());
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
fn wt_rm_reports_failure_and_keeps_branch_when_worktree_is_locked() {
    let (_bare, parent, work) = setup_with_parent();
    let path = add_worktree(&work, &parent, "feature");

    // A locked worktree survives even `--force` (git wants `--force --force`),
    // which we deliberately don't escalate to.
    git(&work, &["worktree", "lock", path.to_str().unwrap()]);

    let output = git_switch_args(&work, &["wt", "rm", "feature", "--force"]);
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

/// How long the pty test is willing to wait on its child. Generous enough that
/// a loaded CI host redrawing a pty is never mistaken for a hang, but finite so
/// a child that wedges fails the test instead of stalling the whole run.
const PATIENCE: Duration = Duration::from_secs(30);
/// Gap between polls: short enough to add no perceptible delay to a run that
/// behaves, long enough not to spin a core while waiting.
const POLL: Duration = Duration::from_millis(10);

/// Waits for `ready` to hold, reporting whether it did before `PATIENCE` ran
/// out. Polling what the child has actually done keeps the pty test off a fixed
/// sleep, so a slow CI host waits longer instead of racing ahead.
fn poll_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// Kills the child when the test ends, however it ends. An assertion that fires
/// mid-session — a `wait_for` timeout, say — unwinds with the picker still on
/// screen, and an orphaned pty-attached git-switch would outlive the test.
struct ChildGuard(Box<dyn portable_pty::Child + Send + Sync>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl ChildGuard {
    /// Blocks until the child exits, giving up after `PATIENCE`. Polling rather
    /// than a plain `wait()` keeps a child that never exits from hanging the
    /// test binary indefinitely.
    fn wait_bounded(&mut self) {
        let exited = poll_until(|| self.0.try_wait().expect("failed to poll child").is_some());
        assert!(exited, "child did not exit within {PATIENCE:?}");
    }
}

/// Blocks until the child has written `needle` to the pty, so keys are only
/// sent once the prompt they answer is on screen.
fn wait_for(seen: &Mutex<Vec<u8>>, needle: &str) {
    let drawn = poll_until(|| {
        let buf = seen.lock().unwrap();
        buf.windows(needle.len()).any(|w| w == needle.as_bytes())
    });
    assert!(
        drawn,
        "timed out waiting for {needle:?}; got: {}",
        String::from_utf8_lossy(&seen.lock().unwrap())
    );
}

/// Drives the post-switch cleanup prompt over a real pty — the only way to see
/// the rows it draws and the deletions it performs, since a piped run declines
/// to prompt at all. Waits for `row` to be drawn, ticks every row with `→`, runs
/// `before_confirm`, confirms, and returns every byte the child wrote.
///
/// `hooks` leaves worktree hooks on for the tests that configure one in the repo
/// under test; every other caller wants them off, as [`git_switch_args`] does.
fn drive_cleanup_prompt(
    work: &Path,
    target: &str,
    row: &str,
    hooks: bool,
    before_confirm: impl FnOnce(),
) -> Vec<u8> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    use std::sync::Arc;

    let pty = native_pty_system()
        .openpty(PtySize::default())
        .expect("failed to open pty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_git-switch"));
    cmd.arg(target);
    cmd.cwd(work);
    if !hooks {
        cmd.env("GIT_SWITCH_NO_HOOKS", "1");
    }
    let mut child = ChildGuard(pty.slave.spawn_command(cmd).expect("failed to spawn"));
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    // Read on a thread into a buffer the test can watch: the pty must keep
    // draining or the child blocks on a full buffer while we wait to send keys.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&seen);
    let output = std::thread::spawn(move || {
        // The loop reassembles the stream whatever the chunk size, so 1 KiB is
        // simply enough to swallow a picker redraw in a read or two.
        let mut chunk = [0u8; 1024];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            collected.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    });

    // Drive the picker off what it has drawn rather than off a clock: `→` ticks
    // every row, Enter confirms, and each key waits for the redraw that proves
    // the last one landed.
    wait_for(&seen, &format!("[ ] {row}"));
    writer.write_all(b"\x1b[C").unwrap();
    writer.flush().unwrap();
    wait_for(&seen, &format!("[x] {row}"));
    before_confirm();
    writer.write_all(b"\r").unwrap();
    writer.flush().unwrap();

    child.wait_bounded();
    drop(writer);
    drop(pty.master);
    output.join().unwrap();
    Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
}

/// [`drive_cleanup_prompt`] with hooks off and nothing to do between ticking and
/// confirming, read back as text — what most callers want.
fn cleanup_prompt(work: &Path, target: &str, row: &str) -> String {
    String::from_utf8_lossy(&drive_cleanup_prompt(work, target, row, false, || {})).into_owned()
}

/// The stale-branch picker holds the terminal in raw mode, where a bare `\n`
/// drops a line without returning to column 0. Printing the deletion outcomes
/// before that mode is released staircases them across the screen, so this
/// drives the picker over a real pty and insists every newline is a CRLF.
#[test]
fn stale_deletion_outcomes_are_not_printed_in_raw_mode() {
    let (_bare, work) = setup();

    // Three stale branches — each published, then its upstream deleted — plus a
    // destination to switch to, so the post-switch cleanup prompt fires.
    for branch in ["aaa", "bbb", "ccc"] {
        git(work.path(), &["checkout", "-b", branch]);
        git(work.path(), &["push", "-u", "origin", branch]);
        git(work.path(), &["push", "origin", "--delete", branch]);
    }
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["fetch", "--prune", "origin"]);
    git(work.path(), &["branch", "dest", "main"]);

    let raw = drive_cleanup_prompt(work.path(), "dest", "ccc", false, || {});
    let text = String::from_utf8_lossy(&raw);

    let deletions = text.matches(" deleted ").count();
    assert_eq!(
        deletions, 3,
        "expected all three ticked branches to report a deletion; got: {text}"
    );

    let staircased = raw
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == b'\n' && (i == 0 || raw[i - 1] != b'\r'))
        .count();
    assert_eq!(
        staircased, 0,
        "every newline written to a tty must be CRLF; got: {text}"
    );
}

// ---------------------------------------------------------------------------
// Equivalence
// ---------------------------------------------------------------------------

/// A branch with a commit of its own, published under its own name — what a
/// topic branch looks like the moment before it is merged on the forge.
fn push_topic_branch(work: &Path, branch: &str) {
    git(work, &["checkout", "-b", branch]);
    commit_in(work, &format!("{branch}.txt"), "topic work");
    git(work, &["push", "-u", "origin", branch]);
}

/// Land `branch`'s work on the remote the way a forge squash-merge does: one new
/// commit on `main` carrying the whole diff under a hash of its own, then the
/// branch's upstream deleted. What is left locally is a branch that is stale on
/// the *Gone* ground and unmerged by every test git offers.
fn squash_merge_upstream(work: &Path, branch: &str) {
    git(work, &["checkout", "main"]);
    git(work, &["merge", "--squash", branch]);
    git(work, &["commit", "-m", &format!("squash {branch}")]);
    git(work, &["push", "origin", "main"]);
    git(work, &["push", "origin", "--delete", branch]);
    git(work, &["fetch", "--prune", "origin"]);
}

/// Land `branch`'s two commits on the remote the way a forge *rebase*-merge
/// does: each replayed onto `main` under a hash of its own, so no single commit
/// there carries the branch's whole diff. Then the upstream is deleted, as after
/// a squash merge.
///
/// `-x` is what makes it a replay rather than a fast-forward: it rewords each
/// commit, so the ones landing on `main` are new objects. Without it git may
/// produce byte-identical commits, which share the branch's hashes and move the
/// merge-base — a different scenario entirely, and one this test isn't about.
fn rebase_merge_upstream(work: &Path, branch: &str) {
    git(work, &["checkout", "main"]);
    git(work, &["cherry-pick", "-x", &format!("{branch}~1"), branch]);
    git(work, &["push", "origin", "main"]);
    git(work, &["push", "origin", "--delete", branch]);
    git(work, &["fetch", "--prune", "origin"]);
}

/// The local branch names, as one blob to search — enough to answer "did this
/// branch survive?".
fn branch_listing(work: &Path) -> String {
    stdout_str(&git(work, &["branch", "--format=%(refname:short)"]))
}

/// The whole point of *Equivalent*: a branch whose diff the anchor already
/// holds under another hash warns of nothing, so it draws no marker, earns no
/// legend, and goes — even though `git branch -d` would refuse it.
#[test]
fn a_squash_merged_branch_is_deleted_without_a_warning() {
    let (_bare, work) = setup();

    push_topic_branch(work.path(), "feature");
    squash_merge_upstream(work.path(), "feature");
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        !text.contains('↑'),
        "a proven branch destroys nothing, so no marker: {text}"
    );
    assert!(
        !text.contains("unmerged commits"),
        "and nothing for a legend to gloss: {text}"
    );
    assert!(
        !branch_listing(work.path()).contains("feature"),
        "the branch should be gone: {}",
        branch_listing(work.path())
    );
}

/// The proof is about content, and a commit on top is content the anchor has
/// never seen. Landing the rest of the branch buys it nothing: the warning —
/// and the license it carries — stand.
#[test]
fn a_commit_on_top_of_a_squash_merge_keeps_its_warning() {
    let (_bare, work) = setup();

    push_topic_branch(work.path(), "feature");
    squash_merge_upstream(work.path(), "feature");
    git(work.path(), &["checkout", "feature"]);
    commit_in(work.path(), "later.txt", "work done after the merge");
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        text.contains('↑'),
        "unique work is still at risk, so the marker stands: {text}"
    );
    assert!(
        text.contains("unmerged commits"),
        "and the legend still glosses it: {text}"
    );
    // ADR 0001 from there on: the marker was shown, so ticking the row discards
    // the commits it warned about. Equivalence changed nothing here.
    assert!(
        !branch_listing(work.path()).contains("feature"),
        "a warned row is still deleted when ticked: {}",
        branch_listing(work.path())
    );
}

/// A rebase-merge replays each commit separately, so no commit on the anchor
/// carries the branch's whole diff and the patch-id route finds nothing. The
/// content route answers it: the files the branch touched read identically on
/// the anchor, however they got there.
#[test]
fn a_rebase_merged_branch_is_deleted_without_a_warning() {
    let (_bare, work) = setup();

    git(work.path(), &["checkout", "-b", "feature"]);
    commit_in(work.path(), "one.txt", "first");
    commit_in(work.path(), "two.txt", "second");
    git(work.path(), &["push", "-u", "origin", "feature"]);
    rebase_merge_upstream(work.path(), "feature");
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        !text.contains('↑'),
        "replayed commit by commit is still landed: {text}"
    );
    assert!(
        !branch_listing(work.path()).contains("feature"),
        "the branch should be gone: {}",
        branch_listing(work.path())
    );
}

/// `git cherry` compares patch ids, which are normalised: they ignore
/// whitespace, so a branch differing from what landed by whitespace alone would
/// pass. That is fine for `git rebase`, which leaves the branch behind; it is
/// not fine for a force-delete, so the match is confirmed verbatim.
#[test]
fn a_branch_differing_only_in_whitespace_is_not_proven() {
    let (_bare, work) = setup();

    fs::write(work.path().join("a.txt"), "foo\n").unwrap();
    git(work.path(), &["add", "."]);
    git(
        work.path(),
        &["commit", "-m", "the line before either edit"],
    );
    git(work.path(), &["push", "origin", "main"]);

    git(work.path(), &["checkout", "-b", "feature"]);
    fs::write(work.path().join("a.txt"), "foo bar\n").unwrap();
    git(work.path(), &["commit", "-am", "spaced"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);

    // What landed says `foobar`, not `foo bar` — the same patch to git's
    // normalised reckoning, a different file to anyone reading it.
    git(work.path(), &["checkout", "main"]);
    fs::write(work.path().join("a.txt"), "foobar\n").unwrap();
    git(work.path(), &["commit", "-am", "unspaced"]);
    git(work.path(), &["push", "origin", "main"]);
    git(work.path(), &["push", "origin", "--delete", "feature"]);
    git(work.path(), &["fetch", "--prune", "origin"]);
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        text.contains('↑'),
        "the whitespace is the branch's own unique work: {text}"
    );
}

/// The point of keeping the patch route at all: it answers a branch whose work
/// landed even after the anchor has moved on over the same file. Confirming the
/// match verbatim must not cost that — patch ids ignore line numbers, so a later
/// edit shifting every hunk header leaves the proof standing.
#[test]
fn a_squash_merged_branch_is_still_proven_once_the_anchor_moves_on() {
    let (_bare, work) = setup();

    push_topic_branch(work.path(), "feature");
    squash_merge_upstream(work.path(), "feature");
    // An unrelated edit to the same file, above the branch's own change.
    let landed = fs::read_to_string(work.path().join("feature.txt")).unwrap();
    fs::write(
        work.path().join("feature.txt"),
        format!("a line added later\n{landed}"),
    )
    .unwrap();
    git(work.path(), &["commit", "-am", "later work above it"]);
    git(work.path(), &["push", "origin", "main"]);
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        !text.contains('↑'),
        "the branch's patch is still on the anchor, wherever it now sits: {text}"
    );
    assert!(
        !branch_listing(work.path()).contains("feature"),
        "the branch should be gone: {}",
        branch_listing(work.path())
    );
}

/// The content route compares the paths a branch touched, and git reports a
/// rename as its destination alone — which would leave the deletion of its
/// source uncompared, and prove a branch whose deletion never landed.
#[test]
fn a_rename_whose_deletion_never_landed_is_not_proven() {
    let (_bare, work) = setup();

    // The file predates the branch, so removing it is work of the branch's own.
    commit_in(work.path(), "old.txt", "a file to be renamed");
    git(work.path(), &["push", "origin", "main"]);
    git(work.path(), &["checkout", "-b", "feature"]);
    git(work.path(), &["push", "-u", "origin", "feature"]);
    git(work.path(), &["mv", "old.txt", "new.txt"]);
    git(work.path(), &["commit", "-m", "rename it"]);

    // Only the arrival lands on main; `old.txt` stays, so the branch still holds
    // a deletion the anchor has never seen.
    git(work.path(), &["checkout", "main"]);
    let moved = stdout_str(&git(work.path(), &["show", "feature:new.txt"]));
    fs::write(work.path().join("new.txt"), moved).unwrap();
    git(work.path(), &["add", "new.txt"]);
    git(work.path(), &["commit", "-m", "add the new name only"]);
    git(work.path(), &["push", "origin", "main"]);
    git(work.path(), &["push", "origin", "--delete", "feature"]);
    git(work.path(), &["fetch", "--prune", "origin"]);
    git(work.path(), &["branch", "dest", "main"]);

    let text = cleanup_prompt(work.path(), "dest", "feature");

    assert!(
        text.contains('↑'),
        "the deletion is unique work, so the warning stands: {text}"
    );
}

/// A license covers the commit it was established at. Move the branch after the
/// proof and the delete falls back to `git branch -d`, which refuses it — the
/// same guard an unmarked worktree meets.
#[test]
fn a_branch_that_moves_after_the_proof_is_no_longer_covered_by_it() {
    let (_bare, work) = setup();

    push_topic_branch(work.path(), "feature");
    squash_merge_upstream(work.path(), "feature");
    // A commit for the branch to be moved onto, parked out of the way on a
    // branch of its own so nothing else notices it.
    git(work.path(), &["checkout", "-b", "parked"]);
    commit_in(work.path(), "later.txt", "work done after the proof");
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["branch", "dest", "main"]);

    // The rows — and the proof — are built before the picker draws, so moving
    // the branch now is exactly the race the pin exists for.
    let raw = drive_cleanup_prompt(work.path(), "dest", "feature", false, || {
        git(work.path(), &["branch", "--force", "feature", "parked"]);
    });
    let text = String::from_utf8_lossy(&raw);

    assert!(
        branch_listing(work.path()).contains("feature"),
        "the proof no longer covers where the branch points, so git refuses: {text}"
    );
}

/// The pinned delete is plumbing, and `git update-ref -d` will happily remove a
/// branch some worktree has checked out — which `git branch -D` refuses, leaving
/// that worktree pointing at nothing. A worktree that appears while the picker is
/// open is the "became risky after the warning" case, and it must still meet a
/// guard.
#[test]
fn a_worktree_taken_on_the_proven_branch_mid_prompt_saves_it() {
    let (_bare, parent, work) = setup_with_parent();

    push_topic_branch(&work, "feature");
    squash_merge_upstream(&work, "feature");
    git(&work, &["branch", "dest", "main"]);
    let held = parent.path().join("held");

    let raw = drive_cleanup_prompt(&work, "dest", "feature", false, || {
        git(
            &work,
            &["worktree", "add", held.to_str().unwrap(), "feature"],
        );
    });
    let text = String::from_utf8_lossy(&raw);

    assert!(
        branch_listing(&work).contains("feature"),
        "a branch a worktree now holds must survive: {text}"
    );
}

/// Equivalence only ever subtracts. A branch cut from the anchor and never
/// committed to has nothing the anchor lacks, and reading that as "landed"
/// would offer it for deletion the moment it was created.
#[test]
fn an_untouched_branch_cut_from_the_anchor_is_not_read_as_landed() {
    let (_bare, work) = setup();

    git(work.path(), &["branch", "fresh", "main"]);

    let _cwd = cwd_at(work.path());
    assert!(
        !stale_names("origin").contains(&"fresh".to_string()),
        "an untouched branch is not stale, got: {:?}",
        stale_names("origin")
    );
    assert!(
        git_switch::git::equivalent_branches(None, "origin", &["fresh"]).is_empty(),
        "and an empty diff proves nothing, so equivalence cannot offer it either"
    );
}

// ---------------------------------------------------------------------------
// Worktree hooks
// ---------------------------------------------------------------------------

/// The payload a hook is handed, from both creation arms and from a removal,
/// each firing exactly once for the worktree it describes.
#[test]
fn wt_hooks_report_each_creation_and_removal_once() {
    let (_bare, parent, work) = setup_with_parent();

    let log = parent.path().join("hook.log");
    let script = format!(
        "printf '%s|%s|%s|%s|%s\\n' \"$GIT_SWITCH_EVENT\" \"$GIT_SWITCH_BRANCH\" \
         \"$GIT_SWITCH_MAIN\" \"$GIT_SWITCH_WORKTREE\" \"$(pwd -P)\" >> '{}'",
        log.display()
    );
    git(&work, &["config", "git-switch.hook.created", &script]);
    git(&work, &["config", "git-switch.hook.removed", &script]);

    // An existing branch and a new one take different creation arms; both are
    // creations as far as a hook is concerned.
    git(&work, &["branch", "feature"]);
    for args in [
        ["wt", "feature"].as_slice(),
        ["wt", "brand-new"].as_slice(),
        ["wt", "rm", "feature", "--force"].as_slice(),
    ] {
        let output = git_switch_hooked(&work, args);
        assert!(
            output.status.success(),
            "`{}` failed: {}",
            args.join(" "),
            stderr_str(&output)
        );
    }

    // Git reports resolved paths, so compare against resolved ones — on macOS
    // a TempDir under /var is really /private/var.
    let main = work.canonicalize().unwrap();
    let worktrees = main.parent().unwrap().join("worktrees").join("repo");
    let line = |event: &str, branch: &str| {
        format!(
            "{event}|{branch}|{}|{}|{}",
            main.display(),
            worktrees.join(branch).display(),
            main.display()
        )
    };

    let logged = fs::read_to_string(&log).unwrap();
    assert_eq!(
        logged.lines().collect::<Vec<_>>(),
        vec![
            line("created", "feature"),
            line("created", "brand-new"),
            line("removed", "feature"),
        ],
        "each event fires once, from the main worktree, with the full payload"
    );
}

/// A hook is told, never asked: one that fails is warned about and otherwise
/// ignored, and its stderr reaches the user untouched.
#[test]
fn a_failing_wt_hook_leaves_the_worktree_and_the_handoff_intact() {
    let (_bare, parent, work) = setup_with_parent();

    git(
        &work,
        &[
            "config",
            "git-switch.hook.created",
            "echo 'hook says no' >&2; exit 3",
        ],
    );

    let output = git_switch_hooked(&work, &["wt", "feature"]);
    assert!(
        output.status.success(),
        "a failing hook must not fail the command; stderr: {}",
        stderr_str(&output)
    );

    let expected = parent.path().join("worktrees").join("repo").join("feature");
    assert!(
        expected.exists(),
        "worktree should exist at {}",
        expected.display()
    );
    assert!(
        stdout_str(&output).trim().ends_with("repo/feature"),
        "stdout should still be the worktree path; got: {}",
        stdout_str(&output)
    );
    assert!(
        stderr_str(&output).contains("hook says no"),
        "hook stderr should pass through; got: {}",
        stderr_str(&output)
    );
    assert!(
        stderr_str(&output).contains("created hook exited 3"),
        "a non-zero exit should be warned about; got: {}",
        stderr_str(&output)
    );
}

/// The shell wrapper reads the destination path off stdout, so a hook that
/// talks is diverted to stderr rather than being allowed to send the user
/// somewhere absurd.
#[test]
fn a_chatty_wt_hook_cannot_corrupt_the_handoff() {
    let (_bare, _parent, work) = setup_with_parent();

    git(
        &work,
        &["config", "git-switch.hook.created", "echo /somewhere/else"],
    );

    let output = git_switch_hooked(&work, &["wt", "feature"]);
    assert!(output.status.success(), "stderr: {}", stderr_str(&output));

    let stdout = stdout_str(&output);
    let printed: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        printed.len(),
        1,
        "stdout should carry the handoff path alone; got: {stdout}"
    );
    assert!(
        printed[0].ends_with("repo/feature"),
        "stdout should be the worktree path; got: {}",
        printed[0]
    );
    assert!(
        stderr_str(&output).contains("/somewhere/else"),
        "hook stdout should be re-emitted on stderr; got: {}",
        stderr_str(&output)
    );
}

/// A stale branch held by a worktree takes that worktree with it, which is as
/// much a removal as `wt rm` is — so the hook fires there too. Without it,
/// `git-switch wt <branch>` could announce a creation and then silently destroy
/// a different worktree in the same breath. The prompt is interactive, so this
/// drives it over a real pty.
#[test]
fn a_stale_branch_taking_its_worktree_fires_the_removal_hook() {
    let (_bare, work) = setup();

    // Published, then its upstream deleted: stale, and holding a commit of its
    // own so the row carries a marker that licenses deleting it.
    git(work.path(), &["checkout", "-b", "wip"]);
    fs::write(work.path().join("wip.txt"), "x\n").unwrap();
    git(work.path(), &["add", "wip.txt"]);
    git(work.path(), &["commit", "-m", "wip"]);
    git(work.path(), &["push", "-u", "origin", "wip"]);
    git(work.path(), &["push", "origin", "--delete", "wip"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["fetch", "--prune", "origin"]);
    git(work.path(), &["branch", "dest", "main"]);

    let parent = TempDir::new().unwrap();
    let worktree = parent.path().join("wt");
    git(
        work.path(),
        &["worktree", "add", worktree.to_str().unwrap(), "wip"],
    );

    let log = parent.path().join("hook.log");
    let script = format!(
        "printf '%s|%s|%s\\n' \"$GIT_SWITCH_EVENT\" \"$GIT_SWITCH_BRANCH\" \
         \"$GIT_SWITCH_WORKTREE\" >> '{}'",
        log.display()
    );
    git(work.path(), &["config", "git-switch.hook.removed", &script]);

    drive_cleanup_prompt(work.path(), "dest", "wip", true, || {});

    assert!(
        !worktree.exists(),
        "the held worktree should be gone: {}",
        worktree.display()
    );
    // Resolve through the parent: the worktree itself is gone by now, and git
    // reports the path it resolved (on macOS, /private/var for a /var TempDir).
    let resolved = parent.path().canonicalize().unwrap().join("wt");
    let logged = fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        logged.trim(),
        format!("removed|wip|{}", resolved.display()),
        "the stale prompt should report the worktree it removed; got: {logged}"
    );
}

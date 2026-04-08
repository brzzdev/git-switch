use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Mutex;
use tempfile::TempDir;

/// Guards tests that call library functions relying on process cwd.
static CWD_LOCK: Mutex<()> = Mutex::new(());

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

fn git_switch(dir: &Path, branch: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-switch"))
        .arg(branch)
        .current_dir(dir)
        .output()
        .expect("failed to run git-switch")
}

/// Create a bare "remote" and a working clone with one commit on `main`.
fn setup() -> (TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();

    git(bare.path(), &["init", "--bare"]);

    git(work.path(), &["init", "-b", "main"]);
    git(
        work.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    );

    fs::write(work.path().join("file.txt"), "hello\n").unwrap();
    git(work.path(), &["add", "file.txt"]);
    git(work.path(), &["commit", "-m", "initial"]);
    git(work.path(), &["push", "-u", "origin", "main"]);

    (bare, work)
}

/// Push a commit to `origin/main` that the working tree doesn't have.
/// Works by committing locally, pushing, then rewinding.
fn push_upstream_change(work: &Path, file: &str, content: &str, msg: &str) {
    fs::write(work.join(file), content).unwrap();
    git(work, &["add", file]);
    git(work, &["commit", "-m", msg]);
    git(work, &["push", "origin", "main"]);
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
        stdout_str(&output).contains("Pulled 1 commit"),
        "stdout: {}",
        stdout_str(&output)
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
        stdout_str(&output).contains("Pulled 1 commit"),
        "stdout: {}",
        stdout_str(&output)
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
        stderr.contains("git stash pop"),
        "expected stash guidance in stderr, got: {stderr}"
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
    let _lock = CWD_LOCK.lock().unwrap();
    let (_bare, work) = setup();

    // Create a local-only branch (never pushed) and merge it into main.
    // Right after the merge HEAD == branch tip, so it's not stale yet.
    git(work.path(), &["checkout", "-b", "local-experiment"]);
    fs::write(work.path().join("experiment.txt"), "try something\n").unwrap();
    git(work.path(), &["add", "experiment.txt"]);
    git(work.path(), &["commit", "-m", "experiment"]);
    git(work.path(), &["checkout", "main"]);
    git(work.path(), &["merge", "local-experiment"]);

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(work.path()).unwrap();
    let stale = git_switch::git::stale_branches().unwrap();
    std::env::set_current_dir(&original).unwrap();

    assert!(
        !stale.contains(&"local-experiment".to_string()),
        "local-only branch should not be stale right after merge, got: {stale:?}"
    );
}

#[test]
fn local_only_branch_stale_after_main_advances() {
    let _lock = CWD_LOCK.lock().unwrap();
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

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(work.path()).unwrap();
    let stale = git_switch::git::stale_branches().unwrap();
    std::env::set_current_dir(&original).unwrap();

    assert!(
        stale.contains(&"local-merged".to_string()),
        "merged local branch behind HEAD should be stale, got: {stale:?}"
    );
}

#[test]
fn merged_tracked_branch_is_stale() {
    let _lock = CWD_LOCK.lock().unwrap();
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

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(work.path()).unwrap();
    let stale = git_switch::git::stale_branches().unwrap();
    std::env::set_current_dir(&original).unwrap();

    assert!(
        stale.contains(&"feature-done".to_string()),
        "merged branch with upstream should be stale, got: {stale:?}"
    );
}

#[test]
fn tracked_branch_without_unique_commits_not_stale() {
    let _lock = CWD_LOCK.lock().unwrap();
    let (_bare, work) = setup();

    // Create and push a branch from main without adding any commits.
    git(work.path(), &["checkout", "-b", "new-feature"]);
    git(work.path(), &["push", "-u", "origin", "new-feature"]);
    git(work.path(), &["checkout", "main"]);

    // Simulate a pull that moves main ahead (branch is now behind HEAD).
    push_upstream_change(work.path(), "ahead.txt", "new\n", "advance main");
    git(work.path(), &["pull", "origin", "main"]);

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(work.path()).unwrap();
    let stale = git_switch::git::stale_branches().unwrap();
    std::env::set_current_dir(&original).unwrap();

    assert!(
        !stale.contains(&"new-feature".to_string()),
        "branch with no unique commits should not be stale, got: {stale:?}"
    );
}

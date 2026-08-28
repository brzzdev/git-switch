//! Durable, detached reclamation of removed worktree directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::{AppResult, Error};

const CONFIG_KEY: &str = "perch.cleanup.worktree";
const TRASH_PREFIX: &str = ".perch-trash.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Staged {
    pub(super) original: PathBuf,
    pub(super) trash: PathBuf,
}

/// Retry every exact trash path left by an earlier Removal. Reading config and
/// starting workers are both best-effort: recovery must not delay or block the
/// `wt` command the user asked for.
pub(super) fn retry() {
    let Ok(config) = repository_config() else {
        return;
    };
    let Ok(paths) = configured_paths(&config) else {
        return;
    };
    for path in paths {
        if valid_trash_path(&path) {
            let _ = spawn_deleter(&config, &path);
        }
    }
}

/// Record and move `original` to a hidden sibling on the same volume.
pub(super) fn stage(original: &Path) -> AppResult<Staged> {
    let config = repository_config()?;
    let trash = unused_trash_path(original)?;
    record(&config, &trash)?;
    fs::rename(original, &trash).map_err(|error| cleanup_error("move worktree", &trash, &error))?;
    Ok(Staged {
        original: original.to_path_buf(),
        trash,
    })
}

/// Put a staged worktree back after Git refused to remove its registration.
/// The durable record is deliberately left until a later retry. It names the
/// now-absent trash path, so that retry can only clear the record.
pub(super) fn restore(staged: &Staged) -> AppResult<()> {
    fs::rename(&staged.trash, &staged.original)
        .map_err(|error| cleanup_error("restore worktree", &staged.trash, &error))
}

/// Start deletion without waiting for it, so unlinking cannot hold up
/// reporting, hooks, or handoff.
pub(super) fn start(staged: &Staged) -> AppResult<()> {
    let config = repository_config()?;
    spawn_deleter(&config, &staged.trash)
        .map_err(|error| cleanup_error("start background cleanup", &staged.trash, &error))
}

fn repository_config() -> AppResult<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()?;
    if !output.status.success() {
        return Err(Error::Git {
            command: "rev-parse --git-common-dir".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()).join("config"))
}

fn configured_paths(config: &Path) -> AppResult<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("config")
        .arg("--file")
        .arg(config)
        .args(["--null", "--get-all", CONFIG_KEY])
        .output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect());
    }
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    Err(Error::Git {
        command: "config --get-all".to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn record(config: &Path, trash: &Path) -> AppResult<()> {
    let output = Command::new("git")
        .arg("config")
        .arg("--file")
        .arg(config)
        .args(["--add", CONFIG_KEY])
        .arg(trash)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Git {
        command: "config --add".to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn unused_trash_path(original: &Path) -> AppResult<PathBuf> {
    let parent = original.parent().ok_or_else(|| Error::Git {
        command: "worktree cleanup".to_string(),
        message: format!("{} has no parent directory", original.display()),
    })?;
    let name = original
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Git {
            command: "worktree cleanup".to_string(),
            message: format!("non-utf8 worktree path: {}", original.display()),
        })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    for attempt in 0..100_u8 {
        let candidate = parent.join(format!(
            "{TRASH_PREFIX}{name}.{}.{nonce}.{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::Git {
        command: "worktree cleanup".to_string(),
        message: format!(
            "could not choose a trash path beside {}",
            original.display()
        ),
    })
}

fn valid_trash_path(path: &Path) -> bool {
    path.is_absolute()
        && path.parent().is_some()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(TRASH_PREFIX) && name.len() > TRASH_PREFIX.len())
}

fn spawn_deleter(config: &Path, trash: &Path) -> std::io::Result<()> {
    if !valid_trash_path(trash) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing cleanup path {}", trash.display()),
        ));
    }
    let script = format!(
        "rm -rf -- \"$1\" || exit 0\n\
         git config --file \"$2\" --fixed-value --unset-all {CONFIG_KEY} \"$1\" >/dev/null 2>&1 || :"
    );
    let mut command = Command::new("sh");
    command
        .args(["-c", &script, "perch-cleanup"])
        .arg(trash)
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map(|_| ())
}

fn cleanup_error(action: &str, trash: &Path, error: &std::io::Error) -> Error {
    Error::Git {
        command: "worktree cleanup".to_string(),
        message: format!(
            "could not {action}: {error}; files remain at {}",
            trash.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_accepts_only_absolute_hidden_siblings() {
        assert!(valid_trash_path(Path::new(
            "/tmp/worktrees/repo/.perch-trash.feature.1"
        )));
        assert!(!valid_trash_path(Path::new(".perch-trash.feature.1")));
        assert!(!valid_trash_path(Path::new("/tmp/worktrees/repo/feature")));
        assert!(!valid_trash_path(Path::new("/")));
    }
}

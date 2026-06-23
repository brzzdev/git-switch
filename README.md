# git-switch

A fast, interactive Git branch switcher. Pick a branch, fetch & fast-forward it, and clean up merged branches — all in one step.

## Features

- **Interactive branch picker** — fuzzy-select from local branches (or pass a name directly)
- **Auto-stash** — dirty working tree? Changes are stashed before switching and restored after
- **Fast-forward pull** — fetches from origin and fast-forward merges, warns if the branch has diverged
- **Merged branch cleanup** — prompts to delete local branches that have been merged into the current branch
- **Worktree support** — create, switch into, list, and remove worktrees with `git-switch wt`

## Install

Requires [Rust](https://rustup.rs) and [just](https://github.com/casey/just).

```sh
git clone https://github.com/brzzdev/git-switch.git
cd git-switch
just install
```

This builds a release binary and copies it to `~/.local/bin/git-switch`. Make sure `~/.local/bin` is on your `PATH`.

## Usage

```sh
# Interactive — pick a branch from a list
git-switch

# Direct — switch to a specific branch
git-switch main

# Refresh the current branch from its remote
git-switch .
```

`git-switch .` fetches and fast-forwards the branch you're on. If you've rebased it through a web UI (so local has diverged), or have uncommitted changes, it offers to **keep** that local work (rebase it onto the remote, restoring stashed edits) or **discard** it (hard reset to the remote).

## Worktrees

```sh
# Interactive picker: existing worktrees + branches without one; "Create new: <typed>" when filter matches nothing
git-switch wt

# DWIM — switch into the worktree for `feature`, or create one if it doesn't exist.
# If `feature` doesn't exist as a branch, a new one is created from the remote's default branch.
git-switch wt feature

# List all worktrees
git-switch wt ls

# Remove one or more worktrees (also deletes the branch when it's fully merged;
# a branch with unmerged commits is kept and reported)
git-switch wt rm           # multi-select picker
git-switch wt rm feature   # specific
```

Worktrees land at `../worktrees/<repo>/<branch>` relative to the main checkout. Branch names with slashes (`feature/foo`) preserve their structure as subdirectories.

Plain `git-switch <branch>` also knows about worktrees: if the picked branch is already checked out in another worktree, it hands off to that worktree rather than failing with git's "already checked out" error.

### Shell integration (required for `cd`)

A child process can't change its parent shell's directory. Worktree commands print their target path on stdout; a small shell function reads that and runs `cd` for you.

```sh
just install-shell-integration
# Then add the printed line to your shell rc (e.g. ~/.zshrc):
#   source ~/.config/git-switch/git-switch.sh
```

Without the wrapper, `git-switch wt foo` still creates / finds the worktree and prints its path — you'd just `cd` there manually.

## Shell Completions

Tab completions are available for zsh, bash, and fish.

```sh
just install-completions
```

This installs the appropriate completion script for your current shell. For zsh, make sure `~/.zsh/completions` is in your `fpath` before `compinit`:

```sh
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit && compinit
```

## Configuration

Protect branches from the "delete merged branches" prompt by adding them to your Git config:

```sh
git config --add git-switch.keep develop
git config --add git-switch.keep staging
```

## License

MIT

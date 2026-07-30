# git-switch

A fast, interactive Git branch switcher. Pick a branch, fetch & fast-forward it, and clean up merged branches — all in one step.

## Features

- **Interactive branch picker** — fuzzy-select from local branches (or pass a name directly)
- **Auto-stash** — dirty working tree? Changes are stashed before switching and restored after
- **Fast-forward pull** — fetches from origin and fast-forward merges, warns if the branch has diverged
- **Merged branch cleanup** — prompts to delete local branches that have been merged into the current branch, including ones held by a worktree (the worktree goes too)
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

`git-switch .` fetches and brings the branch you're on up to date with its remote. A clean branch integrates with no prompt — fast-forwarding, or (when it has diverged, e.g. after rebasing through a web UI) rebasing your local commits onto the remote, which drops any already upstream and replays genuine new work. Only when the working tree is dirty does it stop to ask: **keep** the uncommitted work (stash, rebase, restore) or **discard** it (hard reset to the remote).

## Worktrees

```sh
# Interactive picker: existing worktrees + branches without one; "Create new: <typed>" when filter matches nothing
git-switch wt

# DWIM — switch into the worktree for `feature`, or create one if it doesn't exist.
# If `feature` doesn't exist as a branch, a new one is created from the remote's default branch.
git-switch wt feature

# List all worktrees
git-switch wt ls

# Remove one or more worktrees, deleting the branch along with them
git-switch wt rm           # multi-select picker
git-switch wt rm feature   # specific
git-switch wt rm .         # the worktree you're standing in
git-switch wt rm . --force # …without being asked about uncommitted work
```

Worktrees land at `../worktrees/<repo>/<branch>` relative to the main checkout. Branch names with slashes (`feature/foo`) preserve their structure as subdirectories.

`wt rm .` removes the worktree you're in and `cd`s you back to the main checkout. If your cwd is the main checkout there's nothing for `.` to name, and it says so.

Removing a worktree destroys two things — a directory and a branch — so anything irreversible is shown before it happens. In the picker, rows carry markers: `●` for uncommitted changes, `↑3` for commits that aren't merged anywhere (a bare `↑` when the branch has no upstream to count against). A marked row is fair warning, so ticking it removes the worktree and deletes the branch outright. An unmarked row has nothing to lose, so it keeps git's own guards — no `--force` on the worktree, a plain `git branch -d` on the branch. If such a worktree turns out to be dirty after all (you changed it while the picker was open), git refuses and says so rather than discarding the work.

A named target like `wt rm .` has no row to carry a marker, so the same information arrives as a confirmation instead:

```
! ~/dev/worktrees/repo/spike has uncommitted changes
! spike has unmerged commits and no upstream
? Remove the worktree and delete spike anyway? [y/N]
```

Nothing at risk means no prompt at all. `--force` (`-f`) skips it. In a pipe or CI run there's no way to show the warning or ask, so a risky removal refuses and exits non-zero unless you pass `--force`.

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

## Cleaning up stale branches

After a switch, git-switch offers to delete branches that have outlived their purpose — merged into your current branch, or whose upstream has been deleted. A branch checked out in another worktree appears here too, annotated with the worktree that holds it; deleting it removes that worktree as well:

```
? Delete stale branches (space to toggle, →/← all/none)
  > [ ] chore/deps       (+ worktree ●)
    [ ] fix/typo
    [ ] spike/abandoned  ↑
```

The markers mean the same thing as in `wt rm`: `●` for a worktree with uncommitted changes, `↑` for commits that aren't merged anywhere — the case where a branch's remote was deleted while it still held unpushed work. `(+ worktree, missing)` marks a leftover registration whose directory is already gone. Every deletion reports itself, naming the path of any worktree that was removed.

## Configuration

Protect branches from the "delete merged branches" prompt by adding them to your Git config:

```sh
git config --add git-switch.keep develop
git config --add git-switch.keep staging
```

## License

MIT

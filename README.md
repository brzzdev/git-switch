# git-switch

A fast, interactive Git branch switcher. Pick a branch, fetch & fast-forward it, and clean up merged branches — all in one step.

## Features

- **Interactive branch picker** — fuzzy-select from local branches (or pass a name directly)
- **Auto-stash** — dirty working tree? Changes are stashed before switching and restored after
- **Fast-forward pull** — fetches from origin and fast-forward merges, warns if the branch has diverged
- **Merged branch cleanup** — prompts to delete local branches that have been merged into the current branch

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
```

## Configuration

Protect branches from the "delete merged branches" prompt by adding them to your Git config:

```sh
git config --add git-switch.keep develop
git config --add git-switch.keep staging
```

## License

MIT

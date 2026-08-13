# git-switch

An interactive Git branch and worktree switcher. Its domain is the small set of judgements it makes on the user's behalf: which branches have outlived their purpose, which worktrees can go, and what may be destroyed without asking first.

## Language

### Branches

**Anchor**:
The ref staleness is judged against: the local default branch, or its remote counterpart where there is no local copy. Never the current branch — with worktrees in play, "current" is an accident of which directory you are standing in. Where no anchor resolves, the merged rule stands down and only a deleted upstream marks a branch stale. See [ADR 0002](./docs/adr/0002-staleness-is-anchored-to-the-default-branch.md).
_Avoid_: Mainline, main line, HEAD, trunk

**Stale**:
A branch that has outlived its purpose — its work has landed on the anchor, or its upstream has been deleted. Topology cannot show landing, since a branch cut from the anchor looks exactly like one the anchor absorbed, so it is read from what the branch *tracks*: either it tracks the anchor's counterpart while *ahead* of it, or it tracks nothing and its tip is *behind* the anchor. Neither applies to a branch published under a name of its own, which waits for its upstream to be deleted. Both are proxies: a branch cut from an anchor that was already ahead or behind borrows that position and is offered though it holds nothing. Staleness is what qualifies a branch for the cleanup prompt; it says nothing about whether deleting it is safe.
_Avoid_: Dead, old, obsolete

**Unmerged**:
Holding commits that `git branch -d` would refuse to discard. Git's rule is *"fully merged in its upstream branch, or in HEAD if no upstream was set"* — alternatives, not a pair: where an upstream exists it alone decides, so a branch merged into HEAD but ahead of its upstream is still unmerged. A branch can be both stale and unmerged: work merged into the anchor locally, before the anchor was pushed.
_Avoid_: Unpushed, ahead, dirty

**Kept**:
Pinned out of the cleanup prompt, via `git-switch.keep` config or by being the remote's default branch.
_Avoid_: Protected, ignored, excluded

### Worktrees

**Held**:
Of a branch, checked out in some worktree. Git forbids the same branch in two worktrees, so a held stale branch is always held by a worktree other than the one you're in.
_Avoid_: Locked, checked out, in use

**Missing**:
A worktree still registered in `.git/worktrees` whose directory is gone — git calls this *prunable*. It cannot be entered, but still blocks its branch from being checked out or deleted.
_Avoid_: Stale (reserved for branches), dead, orphaned

**Dirty**:
Of a worktree, holding uncommitted changes: tracked edits or untracked, non-ignored files.
_Avoid_: Modified, unclean

**Main worktree**:
The original checkout, which git will not let you remove. Every other worktree is *removable*.
_Avoid_: Root, primary, parent

### Destruction

**Risk**:
What removing something would irreversibly destroy — a dirty worktree's files, an unmerged branch's commits, or both. Something with no risk can be removed without asking.
_Avoid_: Danger, safety, hazard

**Marker**:
The rendering of a risk in a picker row: `●` for dirty, `↑N` for unmerged. A marker is a warning, and per [ADR 0001](./docs/adr/0001-warned-means-forceable.md) a shown warning is what licenses forcing. `wt ls` draws from the same vocabulary, so a glyph looks the same wherever it appears — but `↑N` there counts commits the upstream lacks, which is not the same judgement as *Unmerged* and licenses nothing. Sharing the glyphs is not sharing the facts.
_Avoid_: Flag, badge, indicator

**Forcing**:
Destroying something git would otherwise protect — `worktree remove --force`, `branch -D`. Only ever licensed by a warning the user has already seen.
_Avoid_: Overriding, ignoring

**License**:
What permits forcing: the markers already shown to the user, or an explicit `--force`. It covers what was warned about and nothing else, so anything that became risky after its row was drawn meets git's own guard instead. See [ADR 0001](./docs/adr/0001-warned-means-forceable.md).
_Avoid_: Permission, approval, consent

**Removal**:
Destroying a branch, a worktree, or a branch together with the worktree holding it. The worktree goes first, and one that refuses to go leaves its branch alone — git will not delete a branch something still holds.
_Avoid_: Deletion (reserved for branches), cleanup, teardown

### Targets

**`.`**:
The one you're in. `git-switch .` refreshes the current branch; `git-switch wt rm .` removes the current worktree.
_Avoid_: Here, current, self

**Handoff**:
Printing a directory to stdout for the shell wrapper to `cd` into, since a process cannot change its parent's working directory.
_Avoid_: Jump, teleport, redirect

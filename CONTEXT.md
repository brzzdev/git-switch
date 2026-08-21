# perch

An interactive Git branch and worktree switcher. Its domain is the small set of judgements it makes on the user's behalf: which branches have outlived their purpose, which worktrees can go, and what may be destroyed without asking first.

## Language

### Branches

**Anchor**:
The ref staleness is judged against: the local default branch, or its remote counterpart where there is no local copy. Never the current branch — with worktrees in play, "current" is an accident of which directory you are standing in. Where no anchor resolves, the merged rule stands down and only a deleted upstream marks a branch stale. See [ADR 0002](./docs/adr/0002-staleness-is-anchored-to-the-default-branch.md).
_Avoid_: Mainline, main line, HEAD, trunk

**Stale**:
A branch that has outlived its purpose — its work has landed on the anchor, or its upstream has been deleted. Topology cannot show landing, since a branch cut from the anchor looks exactly like one the anchor absorbed, so it is read from what the branch *tracks*: either it tracks the anchor's counterpart while *ahead* of it, or it tracks nothing and its tip is *behind* the anchor. Neither applies to a branch published under a name of its own, which waits for its upstream to be deleted. Both are proxies: a branch cut from an anchor that was already ahead or behind borrows that position and is offered though it holds nothing. Staleness is what qualifies a branch for the cleanup prompt; which clause did it is the branch's *Ground*. It says nothing about whether deleting it is safe.
_Avoid_: Dead, old, obsolete

**Ground**:
Which of *Stale*'s two clauses put a branch on the cleanup prompt: *Landed*, its work absorbed by the anchor, or *Gone*, its upstream deleted. The two never overlap — a deleted upstream is read first, and a branch whose upstream is gone is never asked whether it landed. A ground is not a *Risk*: it says why a branch is offered, never what deleting it would destroy, so it is written as a word and never drawn as a *Marker*. See [ADR 0004](./docs/adr/0004-a-ground-is-not-a-marker.md).
_Avoid_: Reason, cause, merged (reserved for topology, which staleness deliberately does not read)

**Unmerged**:
Holding commits that `git branch -d` would refuse to discard. Git's rule is *"fully merged in its upstream branch, or in HEAD if no upstream was set"* — alternatives, not a pair: where an upstream exists it alone decides, so a branch merged into HEAD but ahead of its upstream is still unmerged. A branch can be both stale and unmerged: work merged into the anchor locally, before the anchor was pushed. Being *Equivalent* does not make a branch merged — git would still refuse it — it only means refusing costs nothing.
_Avoid_: Unpushed, ahead, dirty

**Equivalent**:
A branch whose whole diff against the anchor is already in the anchor, under some other commit — squash-merged, rebase-merged, or cherry-picked. It is the one judgement read from *content* rather than from what a branch tracks or where its commits sit, and it is positive evidence: where it cannot be established, the branch is treated as holding unique work. Equivalence only ever subtracts a *Risk* from a branch already on the cleanup prompt, never adds a branch to it. See [ADR 0005](./docs/adr/0005-proof-of-equivalence-is-a-license.md).
_Avoid_: Squashed, duplicate, redundant

**Kept**:
Pinned out of the cleanup prompt, via `perch.keep` config or by being the remote's default branch.
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
What removing something would irreversibly destroy — a dirty worktree's files, an unmerged branch's commits, or both. Something with no risk can be removed without asking. An unmerged branch that is *Equivalent* destroys nothing, so it carries no risk and draws no *Marker*.
_Avoid_: Danger, safety, hazard

**Marker**:
The rendering of a risk in a picker row: `●` for dirty, `↑N` for unmerged. A marker is a warning, and per [ADR 0001](./docs/adr/0001-warned-means-forceable.md) a shown warning is what licenses forcing. `wt ls` draws from the same vocabulary, so a glyph looks the same wherever it appears — but `↑N` there counts commits the upstream lacks, which is not the same judgement as *Unmerged* and licenses nothing. Sharing the glyphs is not sharing the facts.
_Avoid_: Flag, badge, indicator

**Forcing**:
Destroying something git would otherwise protect — `worktree remove --force`, `branch -D`. Only ever licensed: by a warning the user has already seen, or — for a branch alone — by proof that it is *Equivalent*.
_Avoid_: Overriding, ignoring

**License**:
What permits forcing: the markers already shown to the user, an explicit `--force`, or proof that a branch is *Equivalent*. It covers what was warned about or proven and nothing else, so anything that became risky after its row was drawn meets git's own guard instead — as does a proof whose ground has shifted, since equivalence is established on a pair of commits and lapses when either the branch or the *Anchor* moves off it. See [ADR 0001](./docs/adr/0001-warned-means-forceable.md) and [ADR 0005](./docs/adr/0005-proof-of-equivalence-is-a-license.md).
_Avoid_: Permission, approval, consent

**Removal**:
Destroying a branch, a worktree, or a branch together with the worktree holding it. The worktree goes first, and one that refuses to go leaves its branch alone — git will not delete a branch something still holds.
_Avoid_: Deletion (reserved for branches), cleanup, teardown

### Targets

**Verb**:
Which of three intents a command carries: bare `perch` goes to the branch wherever it lives, `br` checks it out in the worktree you're in, `wt` gives it one of its own. The verb decides what happens to a *Held* branch and nothing else, since git leaves exactly one move legal in every other case. See [ADR 0007](./docs/adr/0007-three-verbs-one-per-intent.md).
_Avoid_: Mode, action

**Subverb**:
A word `wt` reads before it reads a branch name: `ls` and `rm`, plus the retired `list` and `remove`, which are refused rather than taken for branches. Only `wt` has any — `br` reads everything after it as a branch. A branch spelled like a subverb is reachable only through `--`, as is one spelled like a *Verb*.
_Avoid_: Subcommand (reserved for the *Verb*), flag, option

**`.`**:
The one you're in. `perch .` refreshes the current branch; `perch wt rm .` removes the current worktree.
_Avoid_: Here, current, self

**Handoff**:
Printing a directory to stdout for the shell wrapper to `cd` into, since a process cannot change its parent's working directory.
_Avoid_: Jump, teleport, redirect

### Hooks

**Hook**:
A user command run after a worktree is created or removed. A hook is told what happened; it is never asked. It cannot refuse a *Removal*, grant a *License*, or change anything `perch` does — one that fails is reported and ignored. See [ADR 0003](./docs/adr/0003-hooks-are-told-never-asked.md).
_Avoid_: Callback, plugin, trigger, integration

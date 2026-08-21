# Three verbs, one per intent

Branches and worktrees are two modes of one job — get me onto that code — but the command surface privileged the first and buried the second under a subcommand. The fix is not to promote worktrees to a peer subcommand and leave the user choosing between them, but to name the three things a user can actually mean: `perch <name>` goes to the branch wherever it lives, `perch br <name>` checks it out in the worktree they are standing in, and `perch wt <name>` insists the branch get a worktree of its own. Each verb is a different intent, not a different implementation of one.

The bare verb needs no mode prompt, because git already refuses the ambiguity:

```
fatal: 'feat' is already used by worktree at '/tmp/…/wt-feat'
```

A branch held by another worktree cannot be checked out here, and a branch held by nothing cannot be `cd`'d to. Whichever state `perch <name>` finds, exactly one move is legal, so asking the user to choose would offer them an option that does not exist.

## Consequences

- **`br` refuses rather than redirects.** Asked for a branch another worktree holds, `br` errors with the path and points at `perch <name>`. Silently going there would make the verb a liar — the whole content of `br` is *here* — and the error is where the verb gets taught, so its wording is the feature. [ADR 0001](./0001-warned-means-forceable.md) leaves room for a `br --force` that removes the other worktree, if that ever proves to be a thing anyone wants.
- **The long spellings are gone.** `worktree`, `list` and `remove` were aliases for `wt`, `ls` and `rm`, and a surface with two names per verb teaches neither. Breaking them is free at 2.0.0, where the binary is being renamed anyway.
- **`br` gets no `ls` or `rm`.** Worktrees need them because they are invisible from inside a repo and awkward to remove by hand; branches are neither, `git branch` already does both well, and branch deletion is already offered by the stale-cleanup prompt. Adding them to fill out a symmetry in a table would mean reimplementing git.
- **`.` still means "here" everywhere it appears.** `perch .` refreshes the current branch and `wt rm .` removes the worktree you are in, both unchanged.

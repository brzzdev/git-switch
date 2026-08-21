# Three verbs, one per intent

Branches and worktrees are two modes of one job — get me onto that code — but the command surface privileged the first and buried the second under a subcommand. Promoting worktrees to a peer subcommand only moves the problem, leaving the user to choose a mode where they had an intent. So name the three things a user can mean: `perch <name>` goes to the branch wherever it lives, `perch br <name>` checks it out in the worktree they are standing in, and `perch wt <name>` insists the branch get a worktree of its own.

The bare verb needs no mode prompt, because git already refuses the ambiguity:

```
fatal: 'feature' is already used by worktree at '/private/tmp/dev/worktrees/repo/feature'
```

A branch held by another worktree cannot be checked out here, and a branch held by nothing cannot be `cd`'d to. Whichever state `perch <name>` finds, exactly one move is legal, so asking the user to choose would offer them an option that does not exist.

## Consequences

- **`br` refuses rather than redirects.** Silently going where the branch lives would make the verb a liar, since the whole content of `br` is *here*. The error is also the only place that teaches the difference between the two verbs, so its wording — the path, and `perch <name>` named as what reaches it — is the feature. [ADR 0001](./0001-warned-means-forceable.md) leaves room for a `br --force` that removes the other worktree, if it is ever wanted.
- **The long spellings are gone, and refused by name.** `worktree`, `list` and `remove` were aliases for `wt`, `ls` and `rm`, and a surface with two names per verb teaches neither. Breaking them costs nothing in the 2.0.0 the rename is already heading for — but `wt <name>` builds a worktree for any word it does not recognise, so dropping `list` and `remove` without turning them away would make old muscle memory create a branch called `list`. A retired spelling errors and names its replacement. Top-level `worktree` needs no such guard: it falls through to a checkout, which creates nothing, and guarding it would put a branch genuinely named `worktree` out of reach.
- **`br` gets no `ls` or `rm`.** Worktrees need them because they are invisible from inside a repo and awkward to remove by hand; branches are neither, `git branch` already does both well, and the stale-cleanup prompt already offers branch deletion. Adding them to fill out a symmetry in a table would mean reimplementing git.
- **`.` still means "here" everywhere it appears.** `perch .` refreshes the current branch and `wt rm .` removes the worktree you are in, both unchanged.

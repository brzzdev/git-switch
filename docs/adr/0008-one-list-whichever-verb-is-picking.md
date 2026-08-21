# One list, whichever verb is picking

[ADR 0007](./0007-three-verbs-one-per-intent.md) gave each intent a verb, but only for a branch named on the command line. With no name, the three verbs drew three different lists: bare `perch` showed branches and said nothing about worktrees, and `perch wt` showed a *Worktrees* section above the branches that weren't in one. "Which of my branches has a worktree?" needed a second command to answer.

The tempting fix — asking "branch or worktree?" on a bare invocation — is a tax on the most-used path. It adds a step every single time to collect something the user knew before they started typing, and it only ever fires for the people who didn't reach for a verb, i.e. the people least able to answer it.

But the mode is not a question the user should answer. It is a property of the thing they are picking, and the tool already knows it. So all three verbs draw **one list of every branch**, in one rendering, with the worktree-backed ones marked by their path. What the verb changes is what selecting a row *does* — and, in the one case where that is nothing, whether the row can be selected at all.

## Consequences

- **The *Worktrees* section is gone.** A branch appears once, under the heading it belongs to, carrying its worktree path as a marker. Two sections listing the same branch under different rules is the mode question again, asked in the layout instead of in a prompt.
- **A row a verb can't act on is greyed, never hidden.** `perch br` cannot check out a branch a second worktree already holds, so that row is dimmed and says where the branch went and which verb reaches it. Dropping the row instead would turn "why isn't `main` in this list?" into a support question, and the inline reason is the same thing the `br` error teaches — which ADR 0007 calls the feature.
- **The prompt carries the verb.** With the lists identical, the prompt line is the only thing on screen saying whether Enter will `cd`, check out here, or build a worktree. It is wording, not decoration.
- **The branch we're standing on is marked `*`, never pathed.** Git forbids the same branch in two worktrees and the one we're in holds the current branch, so annotating it would put a path on every list saying only "you are here" — and under `br` would grey out the one row that is always checkoutable.
- **`perch wt` with no argument subsumes the browsing `wt ls` did.** `wt ls` stays: it is the scriptable, non-interactive listing, and it draws worktree *Marker*s the picker does not.
- **The list is a pure function.** Given the branches and the worktrees, it produces the rows, the markers, and the disabled set with no repo on disk — which is where the rule above is tested, rather than through a terminal.

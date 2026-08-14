# Proof of equivalence is a license

A squash-merged branch is unmerged by every test git offers: its commits are not ancestors of the
anchor, so `git branch -d` refuses it and the picker draws `↑N` over work that already landed. The
warning is true and useless — the commits it names are irrecoverable only in the sense that their
*hashes* are, and their content sits in the anchor under another name. So git-switch proves the
content case for itself: a branch whose whole diff against the anchor is already present there as
some other commit is *Equivalent*, carries no *Risk*, draws no *Marker*, and is deleted without
being asked about. That makes proof a third source of *License* alongside the markers shown and an
explicit `--force`, which [ADR 0001](./0001-warned-means-forceable.md) did not anticipate.

## Considered options

- **A distinct glyph** — mark equivalent branches with their own symbol rather than nothing. Rejected:
  it would be the only marker in the vocabulary that warns of nothing, and the interruption the
  feature exists to remove would survive in a new costume.
- **Redefining *Unmerged*** — teach it that equivalent branches aren't unmerged. Rejected: *Unmerged*'s
  worth is that it predicts `git branch -d` exactly, and the safe-delete path is built on that
  prediction. A branch git will refuse must keep being called unmerged.
- **Asking the forge** — query `gh`/`glab` for the merged pull request. Rejected: it needs a network,
  credentials, and a forge we recognise, to answer a question local git can answer alone.

## Consequences

- **Equivalence only ever subtracts.** It removes a warning from a branch already on the cleanup
  prompt; it never puts one there. *Stale* keeps reading what a branch tracks, never its content —
  a branch cut from the anchor and never committed to is trivially equivalent, and would otherwise
  be offered for deletion the moment it was created.
- **The proof is pinned to a commit.** *License* covers what was established and nothing more, so the
  tip proven equivalent is re-checked before the force-delete. A branch that moved in the meantime
  falls to `-d` and meets git's own guard, exactly as an unmarked worktree does.
- **An inconclusive proof means unmerged.** Equivalence is positive evidence that defeats a warning;
  where it cannot be established — no anchor, a failing `merge-base` — the warning stands, silently
  and indistinguishably from a branch that genuinely holds unique work.
- **How a branch landed stops being visible.** With no marker, a squash merge and a real merge look
  alike in the picker. The picker answers "may this go?", not "how did it get in?".
- **Answering costs a write.** The proof synthesises the branch's diff as one commit via
  `commit-tree`, which puts a dangling object in the repository — the only mutation git-switch makes
  in order to observe something. It is unreachable and gc reaps it.

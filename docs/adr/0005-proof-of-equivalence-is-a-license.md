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

## How it is proven

Two routes, and either proves it, because work lands in two shapes and neither test sees both:

- **The patch already landed.** The branch's whole diff over its merge-base is synthesised as one
  commit and handed to `git cherry`, which answers by patch id. This survives the anchor moving on
  over the same files, and it is the only route that answers a squash merge — but it reads the diff
  as a single patch, so a rebase-merge that replayed several commits individually defeats it.
  `cherry` is not the whole answer, because the patch ids it compares are normalised: they ignore
  whitespace, and a branch differing from what landed by whitespace alone would pass. That is the
  right trade for `git rebase`, which drops such a commit but leaves the branch behind to recover
  it from; it is the wrong one for a force-delete. So `cherry` — asked the other way round — is used
  to *name* the commit carrying the patch, and the two are then compared with `patch-id --verbatim`,
  which weighs whitespace but still ignores line numbers, so an anchor that has drifted does not
  cost the proof. `--verbatim` arrived in git 2.39; an older git fails that step, which is a failed
  step like any other and leaves this route proving nothing at all.
- **The content is present.** Every path the branch touched since the merge-base reads
  byte-identically on the anchor — every path as git records them, renames left undetected, since a
  rename reported as its destination alone would hide the deletion of its source. Blind to how the
  work arrived, so a rebase-merge or a scattered cherry-pick answers it; broken by any later edit to
  those files, which is why it cannot stand alone. A branch that touched nothing has no paths to
  compare and is proven by neither route.

## Consequences

- **Equivalence only ever subtracts.** It removes a warning from a branch already on the cleanup
  prompt; it never puts one there. *Stale* keeps reading what a branch tracks, never its content —
  a branch cut from the anchor and never committed to is trivially equivalent, and would otherwise
  be offered for deletion the moment it was created.
- **The proof is pinned to both commits it was made from.** *License* covers what was established
  and nothing more, and equivalence is established on a pair: where the branch stood, and what the
  anchor held. Both are re-checked before the force-delete. A branch that grew a commit holds work
  nobody proved; an anchor rewound in the meantime — by a *Hook* fired for an earlier row, say — no
  longer holds the content that made the branch safe to discard. Either lapse drops the delete to
  `-d`, to meet git's own guard exactly as an unmarked worktree does. The branch half is checked
  *by* the delete rather than before it — `update-ref -d <ref> <oid>` compares and deletes as one
  operation, so a commit made in the gap between the two cannot be discarded unwarned. The anchor
  half has no such gap to close, since no single git command speaks for two refs; that window stays
  open and is accepted, being a rewind of the anchor within the seconds a prompt is on screen.
- **An inconclusive proof means unmerged.** Equivalence is positive evidence that defeats a warning;
  where it cannot be established — no anchor, a failing `merge-base` — the warning stands, silently
  and indistinguishably from a branch that genuinely holds unique work.
- **How a branch landed stops being visible.** With no marker, a squash merge and a real merge look
  alike in the picker. The picker answers "may this go?", not "how did it get in?".
- **Answering costs a write.** The proof synthesises the branch's diff as one commit via
  `commit-tree`, which puts a dangling object in the repository — the only mutation git-switch makes
  in order to observe something. It is unreachable and gc reaps it.

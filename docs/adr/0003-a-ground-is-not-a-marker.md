# A ground is not a marker

A branch reaches the cleanup prompt on one of two *Grounds* — *Landed* or *Gone* — and the prompt now says which, because the row previously showed only the branch name and left the user to guess why it was being offered. The obvious rendering is a glyph, alongside the `●` and `↑N` already in the column. We render it as a word instead: per [ADR 0001](./0001-warned-means-forceable.md) a shown *Marker* is what licenses forcing, and a ground destroys nothing, so admitting one into the glyph vocabulary would grant force permission on the strength of a fact that warns of no loss.

## Consequences

- **The grounds are typed at the git boundary, not re-derived.** `stale_branches` carries the ground out with each branch rather than returning bare names, so the one place that decides what makes a branch stale stays the one place that knows why. The alternative — recomputing it in the app layer from refs it already holds — is a second judgement free to drift from the first, which is the drift `marker.rs` exists to prevent.
- **The words are the glossary's words.** Rows read `landed` and `gone`, not `merged`: staleness is read from what a branch *tracks*, never from topology, and borrowing topology's word in the UI would undo a distinction the domain works hard to hold. `gone` is also what `git branch -vv` prints.
- **The ground changes what is shown, not what is ticked.** *Gone* is the firmer evidence — a deleted upstream is a fact, where *Landed* rests on a tracking proxy that can misfire on a branch cut from an already-ahead anchor. That is an argument for pre-ticking riskless *Gone* rows, and it was deliberately not taken: a destructive default should be changed on evidence of use, not on the day the ground first becomes visible.

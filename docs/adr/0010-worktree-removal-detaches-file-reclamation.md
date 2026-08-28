# Worktree removal detaches file reclamation

`git worktree remove` deregisters a worktree and unlinks its directory in one synchronous command. The unlink dominates runtime for large ignored trees. A measured 260,000-file worktree takes about 15 seconds even though Perch has finished every repository decision before that wait.

Perch now moves a removable worktree to a collision-resistant hidden sibling on the same volume, asks Git to prune and verify the missing registration, then starts deletion in a separate process group. `wt rm` returns after the worker starts. Reclaiming disk space may finish later.

## Consequences

- **Background reclamation is the default for `wt rm`.** The original path and Git registration are gone before Perch deletes the branch, reports success, fires the removed hook, or hands the shell back to the main worktree. Stale Removal keeps its synchronous behavior.
- **A fresh safety read preserves ADR 0001.** Perch checks dirtiness immediately before moving the directory. A clean reading may take the fast path. A dirty reading may take it only when the License covers discarding files. An unlicensed dirty reading, a failed read, or a missing directory goes through ordinary unforced `git worktree remove`, leaving Git's guard in charge.
- **Deregistration gates deletion.** Durable records distinguish a directory that is merely staged from one that is ready for reclamation. An advisory repository lock keeps concurrent retries from acting on a staged directory while Git decides whether to remove its registration. A refusal or Git process error restores the directory to its original path. If the detached worker cannot start after deregistration, the ready record and hidden directory remain for the next `wt` command.
- **Recovery is durable and exact.** Before moving anything, Perch records the full trash path in the repository's local Git config. The worker clears that entry only after deleting that exact path. Every later `wt` command retries recorded paths without waiting. This covers the last linked worktree and worktrees created outside Perch's usual directory layout.
- **Cleanup targets stay narrow.** A retry accepts only an absolute path whose final component starts with `.perch-trash.`. Failure leaves that path and its config entry intact. Perch never falls back to a parent directory or pattern.
- **The worker is detached from terminal interruption.** It has its own process group and no inherited standard streams. A Ctrl-C sent to the invoking shell's foreground group does not kill reclamation.

There is no synchronous configuration switch. Failed background deletion stays silent because no caller is listening by then; its durable entry makes the next `wt` command the recovery path.

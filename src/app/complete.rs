//! Repository facts for shell completion.
//!
//! Grammar decides which facts a completion needs, then filters and renders the
//! result. This module only performs the Git reads requested by that query.

use crate::grammar::{Completion, CompletionSource};
use crate::{AppResult, git};

pub(crate) fn run(query: &Completion) -> AppResult<()> {
    let candidates = match query.source() {
        CompletionSource::ReachableBranches => {
            let remote = git::current_remote(git::current_branch()?.as_deref());
            let (mut local, remote_only) = super::reachable_branches(&remote)?;
            local.extend(remote_only);
            local
        }
        CompletionSource::LocalBranches => git::local_branches()?,
        CompletionSource::Worktrees => super::wt::removal_candidates()?,
    };
    print!("{}", query.render(&candidates));
    Ok(())
}

function __git_switch_branches
    git branch --format="%(refname:short)" 2>/dev/null
end

# Top-level: branches + subcommands.
complete -c git-switch -f -n '__fish_is_nth_token 1' -a '(__git_switch_branches)'
complete -c git-switch -f -n '__fish_is_nth_token 1' -a 'wt worktree' -d 'Worktree commands'

# After `wt`/`worktree`: subverbs + branches.
complete -c git-switch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_is_nth_token 2' -a 'ls list rm remove'
complete -c git-switch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_is_nth_token 2' -a '(__git_switch_branches)'

# After `wt rm`: branches.
complete -c git-switch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_seen_subcommand_from rm remove' -a '(__git_switch_branches)'

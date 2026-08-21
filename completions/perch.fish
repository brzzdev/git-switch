function __perch_branches
    git branch --format="%(refname:short)" 2>/dev/null
end

# Top-level: branches + subcommands.
complete -c perch -f -n '__fish_is_nth_token 1' -a '(__perch_branches)'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'wt worktree' -d 'Worktree commands'

# After `wt`/`worktree`: subverbs + branches.
complete -c perch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_is_nth_token 2' -a 'ls list rm remove'
complete -c perch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_is_nth_token 2' -a '(__perch_branches)'

# After `wt rm`: branches.
complete -c perch -f -n '__fish_seen_subcommand_from wt worktree; and __fish_seen_subcommand_from rm remove' -a '(__perch_branches)'

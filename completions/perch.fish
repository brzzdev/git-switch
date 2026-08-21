function __perch_branches
    git branch --format="%(refname:short)" 2>/dev/null
end

# Top-level: branches + subcommands.
complete -c perch -f -n '__fish_is_nth_token 1' -a '(__perch_branches)'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'br' -d 'Check a branch out here'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'wt' -d 'Worktree commands'

# After `wt`: subverbs + branches.
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2' -a 'ls rm'
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2' -a '(__perch_branches)'

# After `wt rm`: branches.
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_seen_subcommand_from rm' -a '(__perch_branches)'

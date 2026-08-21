function __perch_branches
    git branch --format="%(refname:short)" 2>/dev/null
end

# The dispatcher reads some words as commands before it reads them as branch
# names, and `--` is the only way to reach a branch spelled like one. Offering
# such a name where it would be eaten completes into a command that misfires,
# so drop it there. Keep these patterns in step with `dispatch`/`dispatch_wt`.
function __perch_branches_except
    __perch_branches | string match --invert --regex "^($argv[1])\$"
end

# True once `--` has been typed, whichever verb it followed: everything after it
# is a branch name, so every escaped route completes the same way.
function __perch_after_double_dash
    set -l tokens (commandline -opc)
    test (count $tokens) -ge 2; and test "$tokens[-1]" = "--"
end

# Top-level: subcommands + the branches reachable without `--`.
complete -c perch -f -n '__fish_is_nth_token 1' -a '(__perch_branches_except "br|wt")'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'br' -d 'Check a branch out here'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'wt' -d 'Worktree commands'

# After any `--`: branches, unfiltered. One rule covers every escaped route,
# because the position rules below are all false once `--` has been typed.
complete -c perch -f -n '__perch_after_double_dash' -a '(__perch_branches)'

# After `br`: branches. Unlike bash and zsh, fish has no fall-through case, so
# every verb needs its own rule or the second token completes to nothing. `br`
# has no subverbs, so nothing is filtered — `perch br wt` reaches a branch `wt`.
complete -c perch -f -n '__fish_seen_subcommand_from br; and __fish_is_nth_token 2' -a '(__perch_branches)'

# After `wt`: subverbs + the branches reachable without `--`.
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2' -a 'ls rm'
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2' -a '(__perch_branches_except "ls|rm|list|remove")'

# After `wt rm`: branches.
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_seen_subcommand_from rm' -a '(__perch_branches)'

# What the command will accept at the position given — asked of the binary,
# which answers for the position it is handed: worktree names after `wt rm`,
# and everywhere else the branches, minus the words its own match arm eats
# first. It sees remote-only branches, which the `git branch` this replaced
# could not. `command` skips the shell wrapper: it is a function here, and it
# would `cd` the interactive shell on a single-line answer.
function __perch_offers
    command perch $argv --complete 2>/dev/null
end

# True where `--` has just been typed and the dispatcher still has a branch left
# to read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
# branch the dispatcher has already taken, the words after `--` go nowhere.
function __perch_after_double_dash
    set -l tokens (commandline -opc)
    test "$tokens[-1]" = "--"; or return 1
    test (count $tokens) -eq 2; and return 0
    test (count $tokens) -eq 3; and contains -- $tokens[2] br wt
    test (count $tokens) -eq 4; and test "$tokens[2]" = wt; and test "$tokens[3]" = --no-switch
end

# True while `wt rm` still wants a target. It reads that target as the first word
# after `rm` that isn't an option, and takes its `--force` in either order, so a
# flag or a `--` leaves the slot open while a bare word closes it.
function __perch_wt_rm_wants_target
    set -l tokens (commandline -opc)
    test (count $tokens) -ge 3; or return 1
    test "$tokens[2]" = wt; and test "$tokens[3]" = rm; or return 1
    if test (count $tokens) -gt 3
        for token in $tokens[4..-1]
            string match --quiet -- '-*' $token; or return 1
        end
    end
    return 0
end

# Top-level: subcommands + the branches reachable without `--`.
complete -c perch -f -n '__fish_is_nth_token 1' -a '(__perch_offers)'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'br' -d 'Check a branch out here'
complete -c perch -f -n '__fish_is_nth_token 1' -a 'wt' -d 'Worktree commands'

# After a `--` that still has a branch to escape: branches, unfiltered. The `--`
# is the position, and it eats nothing at any of the three levels, so one
# question answers for all of them. `wt rm --` is the one escaped route this
# misses, and its own rule below takes it.
complete -c perch -f -n '__perch_after_double_dash' -a '(__perch_offers --)'

# After `br`: branches. Unlike bash and zsh, fish has no fall-through case, so
# every verb needs its own rule or the second token completes to nothing. `br`
# has no subverbs, so nothing is filtered — `perch br wt` reaches a branch `wt`.
complete -c perch -f -n '__fish_seen_subcommand_from br; and __fish_is_nth_token 2' -a '(__perch_offers br)'

# After `wt`: subverbs + the branches reachable without `--`. `__fish_is_nth_token`
# looks past a `--`, so without the guard `perch wt -- ` would offer the subverbs
# on top of the branches the rule above already gave it — and `--` is precisely
# how you say you meant the branch.
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2; and not __perch_after_double_dash' -a 'ls rm'
complete -c perch -f -n '__fish_seen_subcommand_from wt; and __fish_is_nth_token 2; and not __perch_after_double_dash' -a '(__perch_offers wt)'
complete -c perch -l no-switch -d 'Create or find the worktree without switching to it' -n '__fish_seen_subcommand_from wt; and not __fish_seen_subcommand_from ls rm'

# After `wt rm`: the worktrees, until one has been taken.
complete -c perch -f -n '__perch_wt_rm_wants_target' -a '(__perch_offers wt rm)'

# `br` and `wt` are the shell wrapper's shorthand for `perch br` and `perch wt`.
# `--wraps` takes a command prefix, so every rule above applies to them at the
# right offset without being restated.
#
# Claim a shortcut name only while it is still ours. Nothing about perch's own
# state can answer that: the completions install without the wrapper, and a `br`
# the wrapper did define can be replaced afterwards by anything sourced later —
# broot ships one. So ask fish what the name resolves to *now*. This file is
# autoloaded on first use, well after config has finished, so the answer here is
# the current one. The body line is matched whole: a stale `--wraps` annotation
# left on someone else's function mentions `perch br` in its signature.
function __perch_owns
    functions $argv[1] 2>/dev/null | string match -qr "^\s*perch $argv[1] \\\$argv\s*\$"
end

if __perch_owns br
    complete -c br --wraps 'perch br'
end
if __perch_owns wt
    complete -c wt --wraps 'perch wt'
end

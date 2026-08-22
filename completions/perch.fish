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

# Targets `wt rm` will accept: every worktree but the main one, named the way
# `rm_matches` reads it — by branch, or by the final path component where there
# is no branch, which is how a detached or missing worktree is reached.
function __perch_wt_targets
    git worktree list --porcelain 2>/dev/null | awk '
      function flush() {
        if (!have) return
        if (main) main = 0
        else if (branch != "") print branch
        else { n = split(path, parts, "/"); print parts[n] }
        have = 0
      }
      BEGIN { main = 1 }
      /^worktree / { flush(); path = substr($0, 10); branch = ""; have = 1 }
      /^branch refs\/heads\// { branch = substr($0, 19) }
      END { flush() }
    '
end

# The command line normalised to `perch <verb> …`, whichever of the three names
# was typed — `br` and `wt` are the shell wrapper's shorthand for `perch br` and
# `perch wt`. Every predicate below reads this rather than the raw tokens, so one
# offset is all that separates the three.
function __perch_tokens
    set -l tokens (commandline -opc)
    set -l name (string replace --regex '^.*/' '' -- $tokens[1])
    switch $name
        case br wt
            echo perch
            echo $name
            test (count $tokens) -gt 1; and printf '%s\n' $tokens[2..-1]
        case '*'
            printf '%s\n' $tokens
    end
end

# True when the token being completed sits at normalised position $argv[1] —
# `perch` itself is position 0, so `perch wt <cursor>` is position 2. The tokens
# before the cursor are exactly those `commandline -opc` returns.
function __perch_nth_token
    test (count (__perch_tokens)) -eq $argv[1]
end

# True when the normalised verb is $argv[1]. Unlike `__fish_seen_subcommand_from`
# this is positional, so a branch named `wt` further along the line can't fake it.
function __perch_verb_is
    set -l tokens (__perch_tokens)
    test (count $tokens) -ge 2; and test "$tokens[2]" = "$argv[1]"
end

# True where `--` has just been typed and the dispatcher still has a branch left
# to read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
# branch the dispatcher has already taken, the words after `--` go nowhere.
function __perch_after_double_dash
    set -l tokens (__perch_tokens)
    test "$tokens[-1]" = "--"; or return 1
    test (count $tokens) -eq 2; and return 0
    test (count $tokens) -eq 3; and contains -- $tokens[2] br wt
end

# True while `wt rm` still wants a target. It reads that target as the first word
# after `rm` that isn't an option, and takes its `--force` in either order, so a
# flag or a `--` leaves the slot open while a bare word closes it.
function __perch_wt_rm_wants_target
    set -l tokens (__perch_tokens)
    test (count $tokens) -ge 3; or return 1
    test "$tokens[2]" = wt; and test "$tokens[3]" = rm; or return 1
    if test (count $tokens) -gt 3
        for token in $tokens[4..-1]
            string match --quiet -- '-*' $token; or return 1
        end
    end
    return 0
end

# All three names take the same rules, and fish has no way to alias one command's
# completions onto another, so register each rule against each name.
for __perch_cmd in perch br wt
    # Top-level: subcommands + the branches reachable without `--`. Only bare
    # `perch` ever completes here — `br` and `wt` start at position 2.
    complete -c $__perch_cmd -f -n '__perch_nth_token 1' -a '(__perch_branches_except "br|wt")'
    complete -c $__perch_cmd -f -n '__perch_nth_token 1' -a br -d 'Check a branch out here'
    complete -c $__perch_cmd -f -n '__perch_nth_token 1' -a wt -d 'Worktree commands'

    # After a `--` that still has a branch to escape: branches, unfiltered. The
    # position rules below are all false once `--` has been typed; `wt rm --` is
    # the one escaped route this misses, and its own rule below takes it.
    complete -c $__perch_cmd -f -n __perch_after_double_dash -a '(__perch_branches)'

    # After `br`: branches. Unlike bash and zsh, fish has no fall-through case, so
    # every verb needs its own rule or the next token completes to nothing. `br`
    # has no subverbs, so nothing is filtered — `perch br wt` reaches a branch `wt`.
    complete -c $__perch_cmd -f -n '__perch_verb_is br; and __perch_nth_token 2' -a '(__perch_branches)'

    # After `wt`: subverbs + the branches reachable without `--`.
    complete -c $__perch_cmd -f -n '__perch_verb_is wt; and __perch_nth_token 2' -a 'ls rm'
    complete -c $__perch_cmd -f -n '__perch_verb_is wt; and __perch_nth_token 2' -a '(__perch_branches_except "ls|rm|list|remove")'

    # After `wt rm`: worktrees, until one has been taken.
    complete -c $__perch_cmd -f -n __perch_wt_rm_wants_target -a '(__perch_wt_targets)'
end
set --erase __perch_cmd

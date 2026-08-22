# perch shell integration (fish)
#
# Source from ~/.config/fish/conf.d/:
#   source ~/.config/perch/perch.fish
#
# Behaviour mirrors the bash/zsh wrapper: the binary prints the target path on
# stdout only when a cd hand-off is wanted; everything else is on stderr.
#
# `br` and `wt` are defined alongside `perch` as shorthand for `perch br` and
# `perch wt`. Set PERCH_NO_SHORTCUTS to any non-empty value before sourcing to
# leave both names alone — broot defines its own `br`, and only one can win:
#   set -gx PERCH_NO_SHORTCUTS 1
#   source ~/.config/perch/perch.fish

function perch
    set -l out (command perch $argv)
    set -l rc $status
    if test -z "$out"
        return $rc
    end
    if test (count $out) -gt 1
        printf '%s\n' $out
        return $rc
    end
    if test -d "$out"
        cd -- "$out"
        or return $status
    else
        printf '%s\n' "$out"
    end
    return $rc
end

# The shortcuts call perch rather than the binary, so the cd hand-off above
# covers them without being spelled out again.
if test -z "$PERCH_NO_SHORTCUTS"
    function br
        perch br $argv
    end

    function wt
        perch wt $argv
    end
end

# perch shell integration (fish)
#
# Source from ~/.config/fish/conf.d/:
#   source ~/.config/perch/perch.fish
#
# Behaviour mirrors the bash/zsh wrapper: the binary prints the target path on
# stdout only when a cd hand-off is wanted; everything else is on stderr.

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

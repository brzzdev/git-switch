_perch_branches() {
  git branch --format='%(refname:short)' 2>/dev/null
}

# The dispatcher reads some words as commands before it reads them as branch
# names, and `--` is the only way to reach a branch spelled like one. Offering
# such a name where it would be eaten completes into a command that misfires,
# so drop it there. Keep these patterns in step with `dispatch`/`dispatch_wt`.
_perch_branches_except() {
  _perch_branches | grep -vxE "$1"
}

_perch_completions() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  local prev="${COMP_WORDS[COMP_CWORD-1]}"
  local verb="${COMP_WORDS[1]}"
  local subverb="${COMP_WORDS[2]}"

  # `--` ends parsing wherever it sits, so whatever precedes it, the next word
  # is a branch name. Checked before position, since the routes it opens are at
  # four different depths.
  if [[ "$prev" == "--" ]]; then
    COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
    return
  fi

  case "$COMP_CWORD" in
    1)
      COMPREPLY=($(compgen -W "br wt $(_perch_branches_except 'br|wt')" -- "$cur"))
      ;;
    2)
      if [[ "$verb" == "wt" ]]; then
        COMPREPLY=($(compgen -W "ls rm $(_perch_branches_except 'ls|rm|list|remove')" -- "$cur"))
      else
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      fi
      ;;
    # `wt rm` takes its branch and its `--force` in either order, so keying off
    # the words rather than the depth is what keeps the branch offered after a
    # flag. Every other route has taken its argument by here.
    *)
      if [[ "$verb" == "wt" && "$subverb" == "rm" ]]; then
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      fi
      ;;
  esac
}

complete -F _perch_completions perch

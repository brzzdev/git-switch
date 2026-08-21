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

# `wt rm` reads its target as the first word after `rm` that isn't an option,
# and takes its `--force` in either order, so a flag or a `--` leaves the slot
# open while a bare word closes it. Words typed after a target are ignored.
_perch_wt_rm_wants_target() {
  local i
  for (( i = 3; i < COMP_CWORD; i++ )); do
    [[ "${COMP_WORDS[i]}" == -* ]] || return 1
  done
  return 0
}

_perch_completions() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  local prev="${COMP_WORDS[COMP_CWORD-1]}"
  local verb="${COMP_WORDS[1]}"
  local subverb="${COMP_WORDS[2]}"

  if [[ "$verb" == "wt" && "$subverb" == "rm" ]] && (( COMP_CWORD >= 3 )); then
    if _perch_wt_rm_wants_target; then
      COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
    fi
    return
  fi

  # `--` ends parsing, but only where the dispatcher still has a branch left to
  # read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
  # branch the dispatcher has already taken, the words after `--` go nowhere.
  if [[ "$prev" == "--" ]]; then
    if (( COMP_CWORD == 2 )) ||
      { (( COMP_CWORD == 3 )) && [[ "$verb" == "br" || "$verb" == "wt" ]]; }; then
      COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
    fi
    return
  fi

  case "$COMP_CWORD" in
    1)
      COMPREPLY=($(compgen -W "br wt $(_perch_branches_except 'br|wt')" -- "$cur"))
      ;;
    # Only the two verbs read a second word. `perch <branch>` has taken its
    # target by here, and the dispatcher ignores whatever follows it.
    2)
      if [[ "$verb" == "wt" ]]; then
        COMPREPLY=($(compgen -W "ls rm $(_perch_branches_except 'ls|rm|list|remove')" -- "$cur"))
      elif [[ "$verb" == "br" ]]; then
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      fi
      ;;
  esac
}

complete -F _perch_completions perch

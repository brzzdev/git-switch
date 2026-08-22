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

# Targets `wt rm` will accept: every worktree but the main one, named the way
# `rm_matches` reads it — by branch, or by the final path component where there
# is no branch, which is how a detached or missing worktree is reached.
_perch_wt_targets() {
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
}

# `wt rm` reads its target as the first word after `rm` that isn't an option,
# and takes its `--force` in either order, so a flag or a `--` leaves the slot
# open while a bare word closes it. Words typed after a target are ignored.
# Reads the normalised words that _perch_completions publishes below.
_perch_wt_rm_wants_target() {
  local i
  for (( i = 3; i < _perch_cword; i++ )); do
    [[ "${_perch_words[i]}" == -* ]] || return 1
  done
  return 0
}

# The normalised command line, in `perch <verb> …` form whichever of the three
# names was typed. Published as globals because bash 3.2 — still what macOS
# ships — has no namerefs to pass an array through.
_perch_words=()
_perch_cword=0

_perch_completions() {
  # `br` and `wt` are the shell wrapper's shorthand for `perch br` and `perch wt`,
  # so every rule below reads the verb spelled out. One offset separates them.
  case "${COMP_WORDS[0]##*/}" in
    br | wt)
      _perch_words=(perch "${COMP_WORDS[0]##*/}" "${COMP_WORDS[@]:1}")
      _perch_cword=$(( COMP_CWORD + 1 ))
      ;;
    *)
      _perch_words=("${COMP_WORDS[@]}")
      _perch_cword=$COMP_CWORD
      ;;
  esac

  local cur="${_perch_words[_perch_cword]}"
  local prev="${_perch_words[_perch_cword-1]}"
  local verb="${_perch_words[1]}"
  local subverb="${_perch_words[2]}"

  if [[ "$verb" == "wt" && "$subverb" == "rm" ]] && (( _perch_cword >= 3 )); then
    if _perch_wt_rm_wants_target; then
      COMPREPLY=($(compgen -W "$(_perch_wt_targets)" -- "$cur"))
    fi
    return
  fi

  # `--` ends parsing, but only where the dispatcher still has a branch left to
  # read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
  # branch the dispatcher has already taken, the words after `--` go nowhere.
  if [[ "$prev" == "--" ]]; then
    if (( _perch_cword == 2 )) ||
      { (( _perch_cword == 3 )) && [[ "$verb" == "br" || "$verb" == "wt" ]]; }; then
      COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
    fi
    return
  fi

  case "$_perch_cword" in
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

complete -F _perch_completions perch br wt

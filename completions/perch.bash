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

# Targets `wt rm` will accept, spelled the way `rm_matches` in src/app/wt.rs
# reads them. Porcelain records end with a blank line, so counting terminators
# is what lets the first record — always the main worktree — go by unprinted.
_perch_wt_targets() {
  git worktree list --porcelain 2>/dev/null | awk '
    /^worktree /            { path = substr($0, 10); branch = "" }
    /^branch refs\/heads\// { branch = substr($0, 19) }
    /^$/ && ++seen > 1      { if (branch != "") print branch
                              else { n = split(path, parts, "/"); print parts[n] } }
  '
}

# `wt rm` reads its target as the first word after `rm` that isn't an option,
# and takes its `--force` in either order, so a flag or a `--` leaves the slot
# open while a bare word closes it. Words typed after a target are ignored.
# Takes those in-between words as its arguments.
_perch_wt_rm_wants_target() {
  local word
  for word in "$@"; do
    [[ "$word" == -* ]] || return 1
  done
  return 0
}

_perch_completions() {
  # `br` and `wt` are the shell wrapper's shorthand for `perch br` and `perch wt`,
  # so every rule below reads a word list with the verb spelled out. One offset
  # is all that separates the three names.
  local -a cmdline
  local pos name="${COMP_WORDS[0]##*/}"
  case "$name" in
    br | wt)
      cmdline=(perch "$name" "${COMP_WORDS[@]:1}")
      pos=$(( COMP_CWORD + 1 ))
      ;;
    *)
      cmdline=("${COMP_WORDS[@]}")
      pos=$COMP_CWORD
      ;;
  esac

  local cur="${cmdline[pos]}"
  local prev="${cmdline[pos-1]}"
  local verb="${cmdline[1]}"
  local subverb="${cmdline[2]}"

  if [[ "$verb" == "wt" && "$subverb" == "rm" ]] && (( pos >= 3 )); then
    if _perch_wt_rm_wants_target "${cmdline[@]:3:pos-3}"; then
      COMPREPLY=($(compgen -W "$(_perch_wt_targets)" -- "$cur"))
    fi
    return
  fi

  # `--` ends parsing, but only where the dispatcher still has a branch left to
  # read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
  # branch the dispatcher has already taken, the words after `--` go nowhere.
  if [[ "$prev" == "--" ]]; then
    if (( pos == 2 )) ||
      { (( pos == 3 )) && [[ "$verb" == "br" || "$verb" == "wt" ]]; }; then
      COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
    fi
    return
  fi

  case "$pos" in
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

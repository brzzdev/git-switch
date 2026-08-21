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
  local sub="${COMP_WORDS[1]}"

  case "$COMP_CWORD" in
    1)
      COMPREPLY=($(compgen -W "br wt $(_perch_branches_except 'br|wt')" -- "$cur"))
      ;;
    2)
      if [[ "$prev" == "--" ]]; then
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      elif [[ "$sub" == "wt" ]]; then
        COMPREPLY=($(compgen -W "ls rm $(_perch_branches_except 'ls|rm|list|remove')" -- "$cur"))
      else
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      fi
      ;;
    3)
      if [[ "$prev" == "--" ]] || [[ "$sub" == "wt" && "$prev" == "rm" ]]; then
        COMPREPLY=($(compgen -W "$(_perch_branches)" -- "$cur"))
      fi
      ;;
  esac
}

complete -F _perch_completions perch

_perch_completions() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  local prev="${COMP_WORDS[COMP_CWORD-1]}"
  local sub="${COMP_WORDS[1]}"
  local branches
  branches=$(git branch --format='%(refname:short)' 2>/dev/null)

  case "$COMP_CWORD" in
    1)
      COMPREPLY=($(compgen -W "wt worktree $branches" -- "$cur"))
      ;;
    2)
      if [[ "$sub" == "wt" || "$sub" == "worktree" ]]; then
        COMPREPLY=($(compgen -W "ls list rm remove $branches" -- "$cur"))
      else
        COMPREPLY=($(compgen -W "$branches" -- "$cur"))
      fi
      ;;
    3)
      if [[ "$sub" == "wt" || "$sub" == "worktree" ]] && [[ "$prev" == "rm" || "$prev" == "remove" ]]; then
        COMPREPLY=($(compgen -W "$branches" -- "$cur"))
      fi
      ;;
  esac
}

complete -F _perch_completions perch

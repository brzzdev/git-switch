_git_switch_completions() {
  local branches
  branches=$(git branch --format='%(refname:short)' 2>/dev/null)
  COMPREPLY=($(compgen -W "$branches" -- "${COMP_WORDS[COMP_CWORD]}"))
}

complete -F _git_switch_completions git-switch

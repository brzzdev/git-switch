# git-switch shell integration (bash/zsh)
#
# Wraps the `git-switch` binary so that worktree create / switch operations
# `cd` the parent shell into the target directory. The binary prints the
# target path on stdout only when a `cd` hand-off is wanted; everything else
# (prompts, status, errors) is written to stderr.
#
# Source from your shell rc:
#   source ~/.config/git-switch/git-switch.sh
#
# Behaviour:
#   - empty stdout              → nothing to do
#   - single-line existing dir  → cd there
#   - anything else             → print stdout through unchanged

git-switch() {
  local out
  out="$(command git-switch "$@")"
  local rc=$?
  if [ -z "$out" ]; then
    return $rc
  fi
  case "$out" in
    *$'\n'*)
      printf '%s\n' "$out"
      ;;
    *)
      if [ -d "$out" ]; then
        cd -- "$out" || return $?
      else
        printf '%s\n' "$out"
      fi
      ;;
  esac
  return $rc
}

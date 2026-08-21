# perch shell integration (bash/zsh)
#
# Wraps the `perch` binary so that worktree create / switch operations
# `cd` the parent shell into the target directory. The binary prints the
# target path on stdout only when a `cd` hand-off is wanted; everything else
# (prompts, status, errors) is written to stderr.
#
# Source from your shell rc:
#   source ~/.config/perch/perch.sh
#
# Behaviour:
#   - empty stdout              → nothing to do
#   - single-line existing dir  → cd there
#   - anything else             → print stdout through unchanged

perch() {
  local out
  out="$(command perch "$@")"
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

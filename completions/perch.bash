# What the command will accept at the position given — asked of the binary,
# which answers for the position it is handed: worktree names after `wt rm`,
# and everywhere else the branches, minus the words its own match arm eats
# first. It sees remote-only branches, which the `git branch` this replaced
# could not. `command` skips the shell wrapper: it is a function here, and it
# would `cd` the interactive shell on a single-line answer.
_perch_offers() {
  command perch "$@" --complete 2>/dev/null
}

# Fills COMPREPLY from the newline-separated candidates on stdin, keeping the
# ones $cur is a prefix of.
#
# Git permits `$`, backticks and `${IFS}` in a ref name, so a branch can be
# named `$(…)`. Whoever can push to a repo you fetch chooses that name, and it
# reaches here as an ordinary candidate, so this has to stay literal text at two
# separate moments:
#
# Reading it. Deliberately not `compgen -W`, which expands its word list —
# command substitution included — before matching against it, so merely
# offering such a branch ran it. Reading line by line evaluates nothing. Lines
# are the unit rather than words because a worktree directory may hold spaces
# where a branch never can, and `IFS=` keeps them intact.
#
# Inserting it. `printf %q` because bash puts a match on the command line
# exactly as given, leaving the quoting to whoever wrote the completion — so an
# unescaped candidate is a command substitution again the moment Enter follows
# TAB. zsh and fish escape on insertion themselves, which is why only this file
# has to. Ordinary names come back unchanged.
#
# What escaping costs is that the word on the command line stops being the name.
# Where several candidates share a prefix, bash inserts that prefix and TAB
# again arrives with `$cur` in escaped spelling — `feat\&` for `feat&one` and
# `feat&two` — which no raw name starts with, so a second TAB would answer
# nothing and completion would dead-end where it should narrow. `$cur` is
# therefore matched against both spellings: the raw one the user types, and the
# escaped one the previous TAB left behind. Both are comparisons, evaluating
# nothing.
_perch_reply() {
  local cur=$1 candidate escaped
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    escaped=$(printf '%q' "$candidate")
    if [[ "$candidate" == "$cur"* || "$escaped" == "$cur"* ]]; then
      COMPREPLY+=("$escaped")
    fi
  done
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
      # The registration below was made when this file loaded, and a name can be
      # taken back after that. Re-read it here, where completion actually runs.
      _perch_owns "$name" || return
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
      COMPREPLY=()
      _perch_reply "$cur" < <(_perch_offers wt rm)
    fi
    return
  fi

  # `--` ends parsing, but only where the dispatcher still has a branch left to
  # read: `perch --`, `perch br --`, `perch wt --`. Past `perch wt ls`, or a
  # branch the dispatcher has already taken, the words after `--` go nowhere.
  if [[ "$prev" == "--" ]]; then
    if (( pos == 2 )) ||
      { (( pos == 3 )) && [[ "$verb" == "br" || "$verb" == "wt" ]]; }; then
      # The `--` is the position: it eats nothing at any of the three levels,
      # so one question answers for all of them.
      COMPREPLY=()
      _perch_reply "$cur" < <(_perch_offers --)
    fi
    return
  fi

  case "$pos" in
    1)
      COMPREPLY=()
      _perch_reply "$cur" < <(printf '%s\n' br wt; _perch_offers)
      ;;
    # Only the two verbs read a second word. `perch <branch>` has taken its
    # target by here, and the dispatcher ignores whatever follows it.
    2)
      COMPREPLY=()
      if [[ "$verb" == "wt" ]]; then
        _perch_reply "$cur" < <(printf '%s\n' ls rm; _perch_offers wt)
      elif [[ "$verb" == "br" ]]; then
        _perch_reply "$cur" < <(_perch_offers br)
      fi
      ;;
  esac
}

complete -F _perch_completions perch

# Claim a shortcut name only while it is still ours. Nothing about perch's own
# state can answer that: the completions install without the wrapper, and a
# `br` the wrapper did define can be replaced afterwards by anything sourced
# later — broot ships one. So ask the shell what the name resolves to *now*.
# This file is autoloaded on first use, well after any rc has finished, so the
# answer here is the current one.
_perch_owns() {
  case "$(declare -f "$1" 2>/dev/null)" in
    *"perch $1 \"\$@\""*) return 0 ;;
    *) return 1 ;;
  esac
}

for _perch_shortcut in br wt; do
  if _perch_owns "$_perch_shortcut"; then
    complete -F _perch_completions "$_perch_shortcut"
  fi
done
unset _perch_shortcut

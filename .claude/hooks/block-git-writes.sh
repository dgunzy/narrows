#!/usr/bin/env bash
# PreToolUse hook (matcher: Bash). Blocks git and gh write operations.
#
# Policy: agents in this repo may only READ git state. Anything that changes
# the index, HEAD, refs, config, remotes, or talks to a remote is blocked.
#
# Contract: the hook reads the tool call as JSON on stdin. Exit 0 allows the
# call. Exit 2 blocks it, and stderr is shown to the agent.
#
# This is best-effort defence in depth, not a sandbox. It cannot see through
# git aliases, scripts written to disk and then executed, or eval'd strings.
# AGENTS.md forbids those workarounds.

set -u
set -f  # no glob expansion when tokenising commands

input="$(cat)"

# Extract the command. Prefer jq; otherwise grep the raw JSON, which still
# contains the command text (JSON escapes are normalised below).
if command -v jq >/dev/null 2>&1; then
  cmd="$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null)"
else
  cmd="$(printf '%s' "$input" | sed -e 's/\\[nrt]/ /g' -e 's/\\"/"/g')"
fi
[ -z "$cmd" ] && exit 0

block() {
  echo "BLOCKED by .claude/hooks/block-git-writes.sh: $1" >&2
  echo "Git/gh write operations are not allowed for agents in this repo (see AGENTS.md, Hard rule 1)." >&2
  echo "Leave changes uncommitted and tell the human what to run instead. Do not try to work around this." >&2
  exit 2
}

# Split into segments on shell separators and subshell or quote characters,
# so that "cd x && git commit" and sh -c "git push" are both inspected.
segments="$(printf '%s' "$cmd" | tr ';&|()`"'"'"'{}\n' '\n\n\n\n\n\n\n\n\n\n')"

# Read-only uses of otherwise-mutating subcommands, matched against the
# arguments after the subcommand.
#
# None of these regexes may start with an empty alternative, e.g. '^(|-v)$'.
# macOS ships bash 3.2 (frozen there for licensing reasons; confirm with
# `bash --version`), and its =~ has a real bug: a leading empty branch in a
# group breaks matching for every branch, not just the empty one — so
# '^(|-v)$' fails to match even "-v". Where empty (the bare subcommand) is
# meant to be read-only, the call site checks `-z "$rest"` itself instead,
# the way `tag` and `worktree` below already did before this was found.
readonly_branch='^(--show-current|--list|-l|-a|--all|-r|--remotes|-v|-vv|--verbose|--contains.*|--merged.*|--no-merged.*|--points-at.*|--format.*|--sort.*)( .*)?$'
readonly_tag='^(-l|--list|--contains.*|--points-at.*|--sort.*|--format.*|-n[0-9]*)( .*)?$'
readonly_remote='^(-v|--verbose|show( .*)?|get-url( .*)?)$'
readonly_config='^(--get|--get-all|--get-regexp|--list|-l|--show-origin|--show-scope|get|list)( .*)?$'
readonly_stash='^(list|show)( .*)?$'
readonly_reflog='^(show( .*)?|--.*)$'
readonly_worktree='^list( .*)?$'
readonly_submodule='^(status|summary)( .*)?$'
readonly_notes='^(list|show)( .*)?$'

always_write=' add am apply bisect checkout checkout-index cherry-pick clean clone commit commit-tree fast-import fetch filter-branch filter-repo gc hash-object init maintenance merge merge-file mktag mktree mv pack-refs prune pull push read-tree rebase repack replace request-pull reset restore revert rm send-email sparse-checkout switch symbolic-ref update-index update-ref update-server-info write-tree lfs '

check_git() {
  # $@ holds the tokens after "git".
  local sub="" rest=""
  while [ $# -gt 0 ]; do
    case "$1" in
      -C|-c|--git-dir|--work-tree|--namespace|--exec-path|--super-prefix|--config-env)
        shift; [ $# -gt 0 ] && shift ;;
      -*) shift ;;
      *) sub="$1"; shift; rest="$*"; break ;;
    esac
  done
  [ -z "$sub" ] && return 0

  case "$always_write" in
    *" $sub "*) block "git $sub" ;;
  esac

  case "$sub" in
    branch)    [[ -z "$rest" || "$rest" =~ $readonly_branch ]] || block "git branch $rest" ;;
    tag)       [[ -z "$rest" || "$rest" =~ $readonly_tag ]]    || block "git tag $rest" ;;
    remote)    [[ -z "$rest" || "$rest" =~ $readonly_remote ]] || block "git remote $rest" ;;
    config)    [[ "$rest" =~ $readonly_config ]]    || block "git config $rest" ;;
    stash)     [[ "$rest" =~ $readonly_stash ]]     || block "git stash $rest" ;;
    reflog)    [[ -z "$rest" || "$rest" =~ $readonly_reflog ]] || block "git reflog $rest" ;;
    worktree)  [[ "$rest" =~ $readonly_worktree ]]  || block "git worktree $rest" ;;
    submodule) [[ "$rest" =~ $readonly_submodule ]] || block "git submodule $rest" ;;
    notes)     [[ "$rest" =~ $readonly_notes ]]     || block "git notes $rest" ;;
  esac
  return 0
}

gh_write_verbs='^(create|merge|close|reopen|edit|comment|review|delete|ready|lock|unlock|transfer|pin|unpin|develop|upload|fork|sync|archive|unarchive|rename|rerun|cancel|enable|disable|run|set|remove|add|clone|checkout)$'

check_gh() {
  local group="${1:-}" verb="${2:-}"
  case "$group" in
    pr|issue|release|repo|gist|workflow|run|label|secret|variable|cache|ruleset|project)
      [[ "$verb" =~ $gh_write_verbs ]] && block "gh $group $verb" ;;
    api)
      # Plain GETs are fine; explicit methods other than GET, or fields
      # (which imply POST), are not.
      if [[ " $* " =~ \ (-X|--method)[\ =]?(POST|PUT|PATCH|DELETE) ]] \
        || [[ " $* " =~ \ (-f|-F|--field|--raw-field|--input)[\ =] ]]; then
        block "gh $*"
      fi ;;
  esac
  return 0
}

while IFS= read -r seg; do
  # shellcheck disable=SC2206  # deliberate word splitting into tokens
  tokens=($seg)
  n=${#tokens[@]}
  for ((i = 0; i < n; i++)); do
    tok="${tokens[i]##*/}"   # handles /usr/bin/git
    case "$tok" in
      git) check_git "${tokens[@]:i+1}"; break ;;
      gh)  check_gh "${tokens[@]:i+1}"; break ;;
    esac
  done
done <<< "$segments"

exit 0

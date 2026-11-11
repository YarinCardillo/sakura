#!/bin/bash
# New tasks use the product namespace; existing task branches remain their actual branches.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
"$BOX" project add app --repo "$SOURCE" --subdir sub --branch-prefix retained/ >/dev/null
"$BOX" new existing app >/dev/null
"$BOX" project set app --branch-prefix '' >/dev/null
"$BOX" new current app >/dev/null
assert_eq 'new native tasks use the Sakura prefix' sakura/current "$(git -C "$AGENTBOX_HOME/work/current" branch --show-current)"
assert_eq 'an existing task keeps its original branch' retained/existing "$("$BOX" facts existing --json | jq -r .branch)"
COMMAND='printf "%s\n" "$SAKURA_TASK" "$SAKURA_WORKTREE" "$SAKURA_REPO" "$SAKURA_BASE"; env | awk -F= '\''$1 ~ /_LAVORO$/ {n++} END {print n+0}'\'''
"$BOX" project set app --setup "$COMMAND" >/dev/null
"$BOX" new setup app >/dev/null
LOG="$("$BOX" log setup --setup)"
assert_contains 'setup receives the task under the Sakura name' 'setup' "$LOG"
assert_contains 'setup receives the worktree under the Sakura name' "$AGENTBOX_HOME/work/setup" "$LOG"
assert_contains 'setup receives the repository under the Sakura name' "$SOURCE" "$LOG"
assert_contains 'setup receives the base under the Sakura name' 'main' "$LOG"
assert_eq 'no previous task alias is exported' 0 "$(tail -1 <<<"$LOG")"
"$BOX" project set app --run 'printf "%s %s %s\n" "$SAKURA_TASK" "$SAKURA_PORT" "$SAKURA_PORTS"; sleep 90' --ports 3 >/dev/null
"$BOX" new preview app >/dev/null
assert_status 'Play starts with the Sakura environment' 0 "$BOX" play preview start
assert_contains 'Play receives its Sakura task and port range' 'preview 56000 3' "$("$BOX" play preview log)"
"$BOX" play preview stop >/dev/null
finish

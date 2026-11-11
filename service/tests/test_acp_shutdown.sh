#!/bin/bash
# The end of a conversation has a deadline the agent cannot move: it starts when the client
# closes its side or a signal arrives, whatever the agent still sends or reads.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_ACCOUNTS="personal"
"$BOX" new t1 sandbox >/dev/null 2>&1
readonly CHATTER='{"jsonrpc":"2.0","id":21,"method":"test/chatter"}'
readonly STALLED_EXIT_CODE=66
readonly PATIENCE_SECONDS=40
readonly CHATTER_SECONDS=4

# descendants <pid>: the process and everything under it.
descendants() {
  local child
  echo "$1"
  for child in $(pgrep -P "$1"); do descendants "$child"; done
}

# wait_bounded <pid>: its exit code in ENDED, or "never" after killing it and what it started.
wait_bounded() {
  local second
  for second in $(seq 1 "$PATIENCE_SECONDS"); do
    if ! kill -0 "$1" 2>/dev/null; then wait "$1"; ENDED=$?; return; fi
    sleep 1
  done
  # shellcheck disable=SC2046
  kill -KILL $(descendants "$1") 2>/dev/null
  wait "$1" 2>/dev/null
  ENDED="never"
}

echo " an agent that keeps sending refused requests after the client left is ended at the deadline"
STARTED=$SECONDS
{ printf '%s\n' "$CHATTER"; sleep "$CHATTER_SECONDS"; } \
  | FAKE_ACP_QUIET_REPLIES=1 BOX_ACP_END_SECONDS=2 "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err" &
wait_bounded $!
assert_eq "the deadline's exit code (after $((SECONDS - STARTED)) s)" "$STALLED_EXIT_CODE" "$ENDED"
assert_contains "the last line says why" "box: the agent of task 't1' did not end" "$(tail -n 1 "$TMP/acp.err")"
assert_eq "nothing of it reached the client" "" "$(cat "$TMP/acp.out")"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"

echo " a signal starts the same deadline for an agent that ignores it and keeps talking"
mkfifo "$TMP/stdin"
FAKE_ACP_QUIET_REPLIES=1 FAKE_ACP_IGNORE_TERM=1 BOX_ACP_END_SECONDS=2 "$BOX" acp t1 personal <"$TMP/stdin" >"$TMP/acp.out" 2>"$TMP/acp.err" &
SESSION=$!
exec 5>"$TMP/stdin"
printf '%s\n' "$CHATTER" >&5
sleep "$CHATTER_SECONDS"
kill -TERM "$SESSION"
wait_bounded "$SESSION"
exec 5>&-
assert_eq "the deadline's exit code" "$STALLED_EXIT_CODE" "$ENDED"
assert_contains "the last line says why" "box: the agent of task 't1' did not end" "$(tail -n 1 "$TMP/acp.err")"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"
finish

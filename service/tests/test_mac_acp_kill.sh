#!/bin/bash
# When the box acp process of a lavoro on the Mac is killed outright (SIGKILL: no trap, no
# cleanup) while the client still holds its input open, the conversation does not go on without
# an owner: the process that stays (the relay between client and adapter) holds the lock of the
# front end, sees through a lifetime pipe that box acp is gone, ends the adapter and everything
# it started, and only then lets go of the lock. A second conversation never runs beside it.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log" BOX_ACP_END_SECONDS=2
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$TMP/login"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'

echo " box acp is killed while the client keeps its input open"
mkfifo "$TMP/stdin"
FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" "$BOX" acp m1 personal <"$TMP/stdin" >"$TMP/first.out" 2>/dev/null &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 50); do [ -s "$TMP/child.pid" ] && break; sleep 0.2; done
CHILD="$(cat "$TMP/child.pid")"
ADAPTER_GROUP="$(sed -n 1p "$AGENTBOX_HOME/acp/m1.pid")"
assert_eq "(the conversation is open)" "1 1" "$(ps -p "$CHILD" -o pid= | wc -l | tr -d ' ') $(ps -p "$ADAPTER_GROUP" -o pid= | wc -l | tr -d ' ')"
kill -KILL "$SESSION"
wait "$SESSION" 2>/dev/null

echo " a second conversation, requested right away and again until it opens, and then kept open"
# Each try is a box acp that stays for as long as the test holds its input: one that is refused
# ends by itself, one that opens answers the first message and goes on waiting for the next.
mkfifo "$TMP/stdin2"
lives() {
  ps -p "$1" -o pid= | wc -l | tr -d ' '
}
OPENED=0
for _ in $(seq 1 40); do
  : >"$TMP/second.out"
  "$BOX" acp m1 personal <"$TMP/stdin2" >"$TMP/second.out" 2>"$TMP/second.err" &
  SECOND=$!
  exec 6>"$TMP/stdin2"
  printf '%s\n' "$INITIALIZE" >&6
  for _ in $(seq 1 150); do
    if [ -s "$TMP/second.out" ]; then OPENED=1; break; fi
    kill -0 "$SECOND" 2>/dev/null || break
    sleep 0.1
  done
  [ "$OPENED" -eq 0 ] || break
  exec 6>&-
  wait "$SECOND" 2>/dev/null
  sleep 0.5
done
# Looked at once, at the moment the second answered: no waiting for the first to go.
FIRST_LEFT="$(lives "$ADAPTER_GROUP") $(lives "$CHILD")"
assert_eq "it opens in the end" "1 1" "$OPENED $(jq -r '.result.protocolVersion' "$TMP/second.out")"
assert_eq "at the moment it answered, the adapter of the first was gone, with what it had started" "0 0" "$FIRST_LEFT"
assert_eq "(the second conversation is still open: the two were never open together)" "1" "$(lives "$SECOND")"
exec 6>&-
wait "$SECOND" 2>/dev/null
exec 5>&-
for _ in $(seq 1 50); do [ -z "$(pgrep -f "$AGENTBOX_HOME/tools/")" ] && break; sleep 0.2; done
assert_eq "no adapter of this test is left" "0" "$(pgrep -f "$AGENTBOX_HOME/tools/" | wc -l | tr -d ' ')"
finish

#!/bin/bash
# A command of a project that leaves its process group on purpose (a new session, a double
# fork) is outside what box ends: on the Mac the commands are the person's own. But box never
# waits for it: the reader of a log stops a moment after the command it serves has ended, even
# when a detached child still holds the pipe. Setup, checks, Play and the conversation all end.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log" FAKE_DOCKER_DOWN=1
readonly PATIENCE_SECONDS=60
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$TMP/login"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
# Starts a child in a session of its own that keeps the output of the command open, and ends.
DETACH="perl -MPOSIX -e 'exit if fork; setsid(); open(my \$f, q(>), \$ARGV[0]); print \$f qq(\$\$\\n); close \$f; sleep 300'"

bounded() {
  perl -e 'alarm shift; exec @ARGV' "$PATIENCE_SECONDS" "$@"
}
# detached <file>: the pid in the file, once it is there.
detached() {
  local tick
  for tick in $(seq 1 50); do [ -s "$1" ] && break; sleep 0.2; done
  cat "$1"
}
end_detached() {
  kill "$(cat "$1")" 2>/dev/null
}

echo " a setup command that leaves a detached child with its output open"
"$BOX" project add app --repo "$REPO" --subdir sub --setup "echo before; $DETACH \"$TMP/setup.pid\"; echo after" \
  --check "test=echo checking; $DETACH \"$TMP/check.pid\"; echo checked" --run "$DETACH \"$TMP/play.pid\"; echo playing; sleep 300" >/dev/null
STARTED=$SECONDS
assert_status "box new ends" 0 bounded "$BOX" new m1 app
assert_eq "without waiting for the child (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 40 ] && echo 1 || echo 0)"
assert_contains "the output of the command itself is in the log" "after" "$("$BOX" log m1 --setup)"
assert_eq "the detached child is the person's own and still runs: box does not look for it" "1" "$(ps -p "$(detached "$TMP/setup.pid")" -o pid= | wc -l | tr -d ' ')"
end_detached "$TMP/setup.pid"

echo " a check"
"$BOX" verify m1 test --no-wait >/dev/null
STARTED=$SECONDS
assert_status "the runner ends" 0 bounded "$BOX" runner --once
assert_eq "soon (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 40 ] && echo 1 || echo 0)"
RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-test.json | head -1)"
assert_eq "the result is the command's" "0" "$(jq -r .exit_code "$RESULT")"
assert_contains "with its output" "checked" "$(cat "${RESULT%.json}.log")"
end_detached "$TMP/check.pid"

echo " Play"
assert_status "start" 0 bounded "$BOX" play m1 start
detached "$TMP/play.pid" >/dev/null
STARTED=$SECONDS
assert_status "stop ends" 0 bounded "$BOX" play m1 stop
assert_eq "the Play does not run" "false" "$("$BOX" play m1 status --json | jq -r .running)"
for _ in $(seq 1 50); do [ -z "$(pgrep -f "bounded_log.pl.*play/m1")" ] && break; sleep 0.2; done
assert_eq "and the reader of its log is gone too" "0" "$(pgrep -f "bounded_log.pl.*play/m1" | wc -l | tr -d ' ')"
end_detached "$TMP/play.pid"

echo " the conversation: an adapter that leaves a detached child with its output open"
STARTED=$SECONDS
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}' \
  | FAKE_ACP_DETACHED_PID_FILE="$TMP/acp.pid" bounded "$BOX" acp m1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "box acp ends with the adapter's exit code" "0" "$?"
assert_eq "soon (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 40 ] && echo 1 || echo 0)"
assert_eq "what the adapter answered was delivered" "1" "$(jq -r '.result.protocolVersion' "$TMP/acp.out")"
assert_eq "a second conversation can start: nothing holds the first" "1" "$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}' | bounded "$BOX" acp m1 personal 2>/dev/null | jq -r '.result.protocolVersion')"
end_detached "$TMP/acp.pid"
finish

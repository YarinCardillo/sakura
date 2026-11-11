#!/bin/bash
# verify never waits forever: it ends with one line and a distinct exit code when the runner is
# not running, when the request was quarantined, and when the overall deadline passes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/t1"
RESULTS="$AGENTBOX_HOME/results/t1"
DROPS="$AGENTBOX_HOME/drop/t1"
export BOX_VERIFY_POLL=1
readonly EXIT_NO_RUNNER=75 EXIT_REJECTED=76 EXIT_DEADLINE=77

# run_verify <verb>: the same script an agent runs, with the host variables.
run_verify() {
  BOX_TASK=t1 BOX_WORKSPACE="$WORKSPACE" BOX_REQUESTS_DIR="$DROPS" BOX_RESULTS_DIR="$RESULTS" \
    "$BOX_ROOT/image/verify" "$@"
}

# keep_alive: refreshes the heartbeat like a live runner, until killed.
keep_alive() {
  while true; do heartbeat t1; sleep 1; done
}

echo " no runner heartbeat: verify ends at once and says how to start it"
STARTED="$(date +%s)"
OUTPUT="$(run_verify test 2>&1)"; STATUS=$?
assert_eq "exit code for a missing runner" "$EXIT_NO_RUNNER" "$STATUS"
assert_contains "the message names the cause" "runner is not running" "$OUTPUT"
assert_contains "the message says how to start it" "box runner --daemon" "$OUTPUT"
assert_eq "one line" "1" "$(printf '%s\n' "$OUTPUT" | wc -l | tr -d ' ')"
assert_no_file "no request was left behind" "$DROPS/test"
assert_eq "it did not wait" "1" "$([ $(( $(date +%s) - STARTED )) -lt 5 ] && echo 1 || echo 0)"

echo " a stale heartbeat counts as no runner"
heartbeat t1 600
assert_status "stale heartbeat" "$EXIT_NO_RUNNER" run_verify test

echo " the runner dies while verify waits"
heartbeat t1
OUTPUT="$(BOX_VERIFY_STALE=3 run_verify test 2>&1)"; STATUS=$?
assert_eq "exit code when the heartbeat stops" "$EXIT_NO_RUNNER" "$STATUS"
assert_contains "the message says the runner stopped" "runner stopped" "$OUTPUT"
assert_contains "the message says how to start it" "box runner --daemon" "$OUTPUT"
rm -f "$DROPS/test"

echo " a drop the runner rejected ends verify with the reason"
keep_alive & ALIVE=$!
(run_verify deploy >"$TMP/rej.out" 2>&1; echo $? >"$TMP/rej.status") &
WAITER=$!
sleep 2
"$BOX" runner --once >/dev/null 2>&1
wait "$WAITER"
kill "$ALIVE" 2>/dev/null
assert_eq "exit code for a rejected request" "$EXIT_REJECTED" "$(cat "$TMP/rej.status")"
assert_contains "the reason is printed" "not in the profile" "$(cat "$TMP/rej.out")"
assert_eq "one summary line" "1" "$(grep -c '^verify: rejected' "$TMP/rej.out")"

echo " a rejection older than the request does not count"
sleep 2
keep_alive & ALIVE=$!
BOX_VERIFY_WAIT=3 run_verify deploy >"$TMP/old.out" 2>&1; OLD_STATUS=$?
kill "$ALIVE" 2>/dev/null
assert_eq "the old rejection is ignored, the new one is reported" "$EXIT_DEADLINE" "$OLD_STATUS"
rm -f "$DROPS"/*

echo " a queued request the runner quarantined ends verify with the reason"
commit_in_task t1 quarantine
SHA="$(workspace_head t1)"
queue_directly t1 test 20260201T000000 "$SHA"
cp "$AGENTBOX_HOME/tasks/t1.env" "$TMP/t1.env"
rm -f "$AGENTBOX_HOME/tasks/t1.env"
heartbeat t1
keep_alive & ALIVE=$!
(run_verify test >"$TMP/q.out" 2>&1; echo $? >"$TMP/q.status") &
WAITER=$!
sleep 2
"$BOX" runner --once >/dev/null 2>&1
wait "$WAITER"
kill "$ALIVE" 2>/dev/null
assert_eq "exit code for a quarantined request" "$EXIT_REJECTED" "$(cat "$TMP/q.status")"
assert_contains "the reason is printed" "task has no profile record" "$(cat "$TMP/q.out")"
cp "$TMP/t1.env" "$AGENTBOX_HOME/tasks/t1.env"


echo " the overall deadline ends a wait nobody answers"
rm -f "$DROPS"/* "$RESULTS"/*
keep_alive & ALIVE=$!
STARTED="$(date +%s)"
OUTPUT="$(BOX_VERIFY_WAIT=3 run_verify test 2>&1)"; STATUS=$?
kill "$ALIVE" 2>/dev/null
assert_eq "exit code for the deadline" "$EXIT_DEADLINE" "$STATUS"
assert_contains "the message says what was waited for" "no result after 3s" "$OUTPUT"
assert_eq "it ended near the deadline" "1" "$([ $(( $(date +%s) - STARTED )) -lt 10 ] && echo 1 || echo 0)"

echo " the default deadline is the verb timeout plus a margin"
assert_eq "default wait" "4500" "$(grep -o 'BOX_VERIFY_WAIT:-[0-9]*' "$BOX_ROOT/image/verify" | grep -o '[0-9]*$')"

echo " the runner writes the heartbeat for every task and stops it on exit"
rm -f "$RESULTS/.runner-alive"
"$BOX" runner --once >/dev/null 2>&1
assert_file "heartbeat written" "$RESULTS/.runner-alive"
NOW="$(date +%s)"; BEAT="$(cat "$RESULTS/.runner-alive")"
assert_eq "heartbeat is a current epoch" "1" "$([ $(( NOW - BEAT )) -lt 10 ] && echo 1 || echo 0)"
sleep 3
assert_eq "no ticker outlives the runner" "$BEAT" "$(cat "$RESULTS/.runner-alive")"

echo " box verify on the host refuses at once without a runner"
rm -f "$RESULTS/.runner-alive"
assert_status "host verify without a runner" "$EXIT_NO_RUNNER" "$BOX" verify t1 test
assert_contains "same message" "box runner --daemon" "$OUTPUT"
assert_no_file "no request was left behind by the host" "$DROPS/test"
finish

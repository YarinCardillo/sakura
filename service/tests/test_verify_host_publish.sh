#!/bin/bash
# box verify on the host never writes into the drop directory of a task: an agent writes that
# directory, and a link there would have the host truncate what it leads to, a FIFO would have
# it wait for ever. The waiting box verify asks through the queue of box, as --no-wait does,
# for the commit and the incarnation of the task it read, then waits for the result.
. "$(dirname "$0")/lib.sh"
BOX_TEST_HOME_NAME="Application Support (test)"
setup_sandbox
export BOX_VERIFY_POLL=1
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 work
DROPS="$AGENTBOX_HOME/drop/t1"
HEAD_SHA="$(workspace_head t1)"
echo "keep me" >"$TMP/sentinel"

# A runner that is there and does nothing: its lock is held, its heartbeat is fresh. What box
# verify does before a result can come is then seen without the intake of a real runner.
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 120' "$AGENTBOX_HOME/locks/runner.lock" &
IDLE_RUNNER=$!
sleep 1
heartbeat t1

echo " a link stands where the request would be dropped"
ln -s "$TMP/sentinel" "$DROPS/test"
BOX_VERIFY_WAIT=2 "$BOX" verify t1 test >/dev/null 2>&1
assert_eq "box verify gives up at its deadline (nobody runs the request)" "77" "$?"
assert_eq "what the link leads to is untouched" "keep me" "$(cat "$TMP/sentinel")"
assert_eq "the link is as the agent left it: the host did not touch the drop directory" "$TMP/sentinel" "$(readlink "$DROPS/test")"
assert_contains "the request is in the queue of box, for the commit that was read" "sha=$HEAD_SHA" "$(cat "$AGENTBOX_HOME"/queue/*-test.t1.req 2>/dev/null)"
rm -f "$DROPS/test" "$AGENTBOX_HOME"/queue/*.req

echo " a FIFO stands there"
mkfifo "$DROPS/test"
(BOX_VERIFY_WAIT=2 "$BOX" verify t1 test >/dev/null 2>&1; echo $? >"$TMP/fifo.status") &
WAITER=$!
assert_eq "box verify does not wait on it: it ends" "0" "$(still_alive "$WAITER")"
assert_eq "at its deadline" "77" "$(cat "$TMP/fifo.status" 2>/dev/null)"
assert_eq "the request is in the queue of box" "1" "$(/bin/ls "$AGENTBOX_HOME"/queue/*-test.t1.req 2>/dev/null | wc -l | tr -d ' ')"
kill -KILL "$WAITER" 2>/dev/null
# A box verify that was left waiting on the FIFO has a writer of its own in there: let it go.
(exec 3<>"$DROPS/test") 2>/dev/null
rm -f "$DROPS/test" "$AGENTBOX_HOME"/queue/*.req
kill "$IDLE_RUNNER" 2>/dev/null
wait "$IDLE_RUNNER" 2>/dev/null

echo " with a runner: the result of the check, and nothing in the drop directory"
"$BOX" runner >"$TMP/runner.out" 2>&1 &
RUNNER=$!
for _ in $(seq 1 50); do [ -f "$AGENTBOX_HOME/locks/runner.lock" ] && break; sleep 0.2; done
sleep 1
OUT="$(BOX_VERIFY_WAIT=10 "$BOX" verify t1 test 2>"$TMP/verify.err")"
assert_eq "the exit code of the check" "0" "$?"
assert_contains "the line that sums it up" "verb=test sha=$HEAD_SHA exit=0" "$OUT"
assert_contains "it said what it waits for" "requested 'test' for ${HEAD_SHA:0:12}, waiting for the runner" "$(cat "$TMP/verify.err")"
assert_eq "the drop directory was never written" "0" "$(/bin/ls -A "$DROPS" | wc -l | tr -d ' ')"
kill "$RUNNER" 2>/dev/null
wait "$RUNNER" 2>/dev/null

echo " the script an agent runs in its container still drops its request there, and only there"
assert_eq "image/verify is not started by box on the host" "0" "$(grep -c 'image/verify' "$BOX_ROOT"/lib/*.sh "$BOX_ROOT/bin/box" | awk -F: '{ total += $2 } END { print total }')"

echo " a request the runner quarantined ends the waiting verify as rejected"
RESULTS="$AGENTBOX_HOME/results/t1"
printf '{\n  "verb": "test",\n  "sha": "%s",\n  "exit_code": 70,\n  "seconds": 0\n}\n' "$HEAD_SHA" >"$RESULTS/x-quarantined.json"
echo "[box] request quarantined: the job could not be prepared" >"$RESULTS/x-quarantined.log"
BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report; do . "$BOX_ROOT/lib/$lib.sh"; done
  print_verify_result "$1"' _ "$RESULTS/x-quarantined.json" >"$TMP/rejected.out" 2>&1
assert_eq "exit code 76" "76" "$?"
assert_contains "with the reason" "the runner quarantined the request: the job could not be prepared" "$(cat "$TMP/rejected.out")"
finish

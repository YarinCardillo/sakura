#!/bin/bash
# A task is removed together with its requests: box rm refuses while a check of the task is
# queued or running, --force ends them first, and nothing of a removed task comes back, also
# when a task of the same name is created again.
. "$(dirname "$0")/lib.sh"
setup_sandbox
QUEUE="$AGENTBOX_HOME/queue"
RESULTS="$AGENTBOX_HOME/results"
readonly REMOVAL_SECONDS=90
queued() {
  /bin/ls "$QUEUE" 2>/dev/null | grep -c '\.req$'
}

echo " a box that keeps asking for checks cannot keep its task from being removed"
"$BOX" new t1 sandbox >/dev/null
(while [ -d "$AGENTBOX_HOME/drop/t1" ]; do : >"$AGENTBOX_HOME/drop/t1/build" 2>/dev/null; sleep 0.1; done) &
DROPPER=$!
"$BOX" runner >/dev/null 2>&1 &
RUNNER=$!
for _ in $(seq 1 30); do [ "$(queued)" -gt 0 ] && break; sleep 0.5; done
assert_eq "(the box's requests are being queued)" "1" "$([ "$(queued)" -gt 0 ] && echo 1 || echo 0)"
perl -e 'alarm shift; exec @ARGV' "$REMOVAL_SECONDS" "$BOX" rm t1 --force >"$TMP/rm.out" 2>&1
assert_eq "box rm --force ends within $REMOVAL_SECONDS s with exit 0" "0" "$?"
assert_no_file "the task is gone" "$AGENTBOX_HOME/tasks/t1.env"
assert_no_file "and its drop directory" "$AGENTBOX_HOME/drop/t1"
sleep 4
assert_eq "no request of the task reappears" "0" "$(grep -l '^task=t1$' "$QUEUE"/*.req 2>/dev/null | wc -l | tr -d ' ')"
assert_no_file "nor its results" "$RESULTS/t1"
kill "$RUNNER" "$DROPPER" 2>/dev/null
wait "$RUNNER" "$DROPPER" 2>/dev/null

echo " without --force, taken and clean work is removed too, whatever is queued"
"$BOX" new t1 sandbox >/dev/null
"$BOX" verify t1 build --no-wait 2>/dev/null
assert_status "box rm" 0 "$BOX" rm t1
assert_eq "its request was cancelled, not left" "0" "$(queued)"
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 "not taken"
"$BOX" verify t1 build --no-wait 2>/dev/null
assert_status "work that was not taken still keeps the task" 1 "$BOX" rm t1
assert_contains "for that reason" "not taken" "$OUTPUT"
assert_eq "and nothing was cancelled" "1" "$(queued)"
assert_status "a check can still be queued for it" 0 "$BOX" verify t1 test --no-wait
assert_eq "(two requests now)" "2" "$(queued)"

echo " --force ends the requests, then removes the task"
assert_status "box rm --force" 0 "$BOX" rm t1 --force
assert_eq "no request is left" "0" "$(queued)"
assert_no_file "the results are gone" "$RESULTS/t1"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
assert_no_file "a later runner brings nothing back" "$RESULTS/t1"
assert_eq "and runs nothing" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"

echo " a task that is being removed takes no new request"
"$BOX" new t5 sandbox >/dev/null
touch "$AGENTBOX_HOME/tasks/t5.retiring"
assert_status "box verify says so" 1 "$BOX" verify t5 build --no-wait
assert_contains "in words" "being removed" "$OUTPUT"
drop_request t5 build
"$BOX" runner --once 2>/dev/null
assert_eq "a drop of its box is not queued" "0" "$(queued)"
assert_status "a second box rm finishes the removal" 0 "$BOX" rm t5 --force
assert_no_file "the mark is gone with the task" "$AGENTBOX_HOME/tasks/t5.retiring"

echo " a running check is stopped first"
"$BOX" new t2 sandbox >/dev/null
"$BOX" verify t2 build --no-wait 2>/dev/null
(FAKE_JOB_DELAY=20 "$BOX" runner --once >/dev/null 2>&1) &
RUNNER=$!
for _ in $(seq 1 20); do grep -q '^state=running' "$QUEUE"/*.req 2>/dev/null && break; sleep 1; done
assert_status "box rm --force while the check runs" 0 "$BOX" rm t2 --force
wait "$RUNNER"
assert_eq "no request is left" "0" "$(queued)"
assert_no_file "the runner did not bring the results back" "$RESULTS/t2"
assert_no_file "the workspace is gone" "$AGENTBOX_HOME/work/t2"

echo " a request left in the queue by an older box does not come back to life"
"$BOX" new t3 sandbox >/dev/null
OLD_HEAD="$(workspace_head t3)"
"$BOX" rm t3 --force >/dev/null 2>&1
queue_directly t3 build 20260101T000000 "$OLD_HEAD"
assert_status "a task of that name cannot be created while the request is there" 1 "$BOX" new t3 sandbox
assert_contains "it says why" "request" "$OUTPUT"
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/t3"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
assert_eq "the runner threw the request out" "0" "$(queued)"
assert_no_file "without writing a result for a task that does not exist" "$RESULTS/t3"
assert_eq "and without running it" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_status "now the name can be used again" 0 "$BOX" new t3 sandbox
"$BOX" runner --once 2>/dev/null
assert_eq "and the new task has no result it did not ask for" "0" "$(/bin/ls "$RESULTS/t3" 2>/dev/null | grep -c '\.json$')"

echo " a request cannot be queued for a task that is being removed"
"$BOX" new t4 sandbox >/dev/null
rm -f "$AGENTBOX_HOME/tasks/t4.env"
"$BOX" verify t4 build --no-wait >/dev/null 2>&1
assert_eq "nothing is queued for a task without a record" "0" "$(queued)"
finish

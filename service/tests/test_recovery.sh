#!/bin/bash
# A result that was published is final: a runner that restarts after a crash recognises it and
# only cleans up, and no later run of the same request replaces it.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
RESULTS="$AGENTBOX_HOME/results/t1"
QUEUE="$AGENTBOX_HOME/queue"

commit_in_task t1 one
SHA="$(workspace_head t1)"
"$BOX" verify t1 test --no-wait
KEY="$(basename "$(/bin/ls "$QUEUE"/*.req)" .req)"
ID="${KEY%.t1}"
"$BOX" runner --once 2>/dev/null
assert_contains "the request ran and passed" '"exit_code": 0,' "$(cat "$RESULTS/$ID.json")"
cp "$RESULTS/$ID.json" "$TMP/result.before"
cp "$RESULTS/$ID.log" "$TMP/log.before"

# requeue_as <state>: the queue file of the finished request is back, as after a crash.
requeue_as() {
  printf 'task=t1\nverb=test\nsha=%s\nrequested=now\nstate=%s\npid=1\nstarted=2026-01-01T00:00:00Z\n' "$SHA" "$1" >"$QUEUE/$KEY.req"
  : >"$FAKE_DOCKER_LOG"
}

echo " a crash after the result was published and before the request left the queue"
requeue_as running
FAKE_JOB_EXIT=9 "$BOX" runner --once 2>"$TMP/runner.err"
assert_eq "no job ran again" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "the result is the one that was delivered" "" "$(diff "$TMP/result.before" "$RESULTS/$ID.json")"
assert_eq "the log was not truncated" "" "$(diff "$TMP/log.before" "$RESULTS/$ID.log")"
assert_no_file "the request left the queue" "$QUEUE/$KEY.req"
assert_contains "the runner says why" "already has its result" "$(cat "$TMP/runner.err")"

echo " the same request queued again does not replace the result either"
requeue_as queued
FAKE_JOB_EXIT=9 "$BOX" runner --once 2>/dev/null
assert_eq "no job ran" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "the result is unchanged" "" "$(diff "$TMP/result.before" "$RESULTS/$ID.json")"
assert_no_file "the request left the queue" "$QUEUE/$KEY.req"

echo " cancelling a request that already has its result leaves the result alone"
requeue_as queued
"$BOX" cancel t1 test >/dev/null 2>&1
assert_eq "the result is unchanged" "" "$(diff "$TMP/result.before" "$RESULTS/$ID.json")"
assert_no_file "the request left the queue" "$QUEUE/$KEY.req"

echo " writing a result never replaces one"
STATUS="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project sandbox journal task request checks; do . "$BOX_ROOT/lib/$lib.sh"; done
  write_result t1 "'"$ID"'" test "'"$SHA"'" 9 1 0 2026-01-01T00:00:00Z false false false \
    "{\"changesChecks\":false,\"checksTouched\":[],\"testsTouched\":null}" 2>/dev/null; echo $?')"
assert_eq "the second write is refused" "1" "$STATUS"
assert_eq "the result is unchanged" "" "$(diff "$TMP/result.before" "$RESULTS/$ID.json")"
assert_eq "no temporary file is left" "0" "$(/bin/ls -A "$RESULTS" | grep -c -e '\.tmp')"

echo " an interrupted request without a result is still requeued and run"
commit_in_task t1 two
queue_directly t1 test 20260103T000000 "$(workspace_head t1)"
edit_in_place 's/state=queued/state=running/' "$QUEUE/$QUEUED_KEY.req"
"$BOX" runner --once 2>/dev/null
assert_file "it got its result" "$RESULTS/$QUEUED_ID.json"
finish

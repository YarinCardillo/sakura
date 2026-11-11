#!/bin/bash
# box cancel <task> <verb>: queued or running requests end as a cancelled result (exit code 130),
# never run, never requeued, also across a runner restart. Exit 0 cancelled, 78 nothing to cancel.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
RESULTS="$AGENTBOX_HOME/results/t1"
QUEUE="$AGENTBOX_HOME/queue"
readonly EXIT_NOTHING_TO_CANCEL=78

echo " nothing to cancel"
assert_status "no request: exit 78" "$EXIT_NOTHING_TO_CANCEL" "$BOX" cancel t1 test
assert_contains "the message says why" "nothing to cancel" "$OUTPUT"
assert_status "a verb that is not valid is refused" 1 "$BOX" cancel t1 "Bad Verb"

echo " a queued request is removed and recorded as cancelled"
"$BOX" verify t1 test --no-wait
SHA="$(workspace_head t1)"
assert_status "cancel a queued request" 0 "$BOX" cancel t1 test
assert_eq "the queue is empty" "0" "$(/bin/ls "$QUEUE" | wc -l | tr -d ' ')"
RESULT="$(/bin/ls "$RESULTS"/*-test.json)"
assert_contains "result is cancelled" '"cancelled": true' "$(cat "$RESULT")"
assert_contains "exit code 130" '"exit_code": 130,' "$(cat "$RESULT")"
assert_contains "result names the commit" "\"sha\": \"$SHA\"" "$(cat "$RESULT")"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
assert_eq "the runner ran nothing" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "still one result" "1" "$(/bin/ls "$RESULTS"/*.json | wc -l | tr -d ' ')"

echo " status and facts show the cancelled result like any other"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "status: cancelled" "true" "$(jq -r '.tasks[0].results.test.cancelled' <<<"$OUT")"
assert_eq "status: exit code" "130" "$(jq -r '.tasks[0].results.test.exitCode' <<<"$OUT")"
assert_eq "status: for the head" "true" "$(jq -r '.tasks[0].results.test.isHead' <<<"$OUT")"
assert_eq "facts shows the same" "true" "$("$BOX" facts t1 --json 2>/dev/null | jq -r '.results.test.cancelled')"
assert_eq "journal has a cancel event" "cancel" "$("$BOX" journal --json | jq -r '[.[] | select(.event == "cancel")][0].event')"
assert_eq "the cancel event names the verb" "test" "$("$BOX" journal --json | jq -r '[.[] | select(.event == "cancel")][0].verb')"

echo " a running request: the job container is stopped and the result is cancelled"
commit_in_task t1 two
"$BOX" verify t1 build --no-wait
(FAKE_JOB_DELAY=30 "$BOX" runner --once >/dev/null 2>&1) &
RUNNER=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q '^state=running' "$QUEUE"/*.req 2>/dev/null && break; sleep 1; done
assert_contains "the request is running" "state=running" "$(cat "$QUEUE"/*.req)"
assert_status "cancel a running request" 0 "$BOX" cancel t1 build
wait "$RUNNER"
assert_contains "the job container was removed" "rm -f agentbox-job-" "$(cat "$FAKE_DOCKER_LOG")"
RESULT="$(grep -l '"verb": "build"' "$RESULTS"/*.json | head -1)"
assert_contains "running result is cancelled" '"cancelled": true' "$(cat "$RESULT")"
assert_contains "running result exit code 130" '"exit_code": 130,' "$(cat "$RESULT")"
assert_eq "the queue is empty again" "0" "$(/bin/ls "$QUEUE" | wc -l | tr -d ' ')"

echo " a cancelled request left running by a crashed runner is not requeued"
commit_in_task t1 three
queue_directly t1 test 20260105T000000 "$(workspace_head t1)"
edit_in_place 's/state=queued/state=running/' "$QUEUE/$QUEUED_KEY.req"
echo "cancelled=1" >>"$QUEUE/$QUEUED_KEY.req"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
assert_eq "no job ran" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_contains "result is cancelled" '"cancelled": true' "$(cat "$RESULTS/$QUEUED_ID.json")"
assert_eq "the queue is empty" "0" "$(/bin/ls "$QUEUE" | wc -l | tr -d ' ')"

echo " a cancelled request that was still queued is not run either"
commit_in_task t1 four
queue_directly t1 test 20260106T000000 "$(workspace_head t1)"
echo "cancelled=1" >>"$QUEUE/$QUEUED_KEY.req"
"$BOX" runner --once 2>/dev/null
assert_eq "no job ran" "0" "$(grep -c -e '--name agentbox-job-.*20260106' "$FAKE_DOCKER_LOG")"
assert_contains "result is cancelled" '"cancelled": true' "$(cat "$RESULTS/$QUEUED_ID.json")"

echo " cancel while the dependencies are still being prepared: it ends cancelled and no job runs"
commit_in_task t1 six
rm -f "$AGENTBOX_HOME"/deps/*.ok
"$BOX" verify t1 build --no-wait
SLOW_KEY="$(basename "$(/bin/ls "$QUEUE"/*.req)" .req)"
: >"$FAKE_DOCKER_LOG"
(FAKE_INSTALL_DELAY=40 "$BOX" runner --once >/dev/null 2>&1) &
RUNNER=$!
for _ in $(seq 1 20); do grep -q -e '--name agentbox-install-' "$FAKE_DOCKER_LOG" && break; sleep 1; done
assert_contains "the dependencies are being prepared" "--name agentbox-install-" "$(cat "$FAKE_DOCKER_LOG")"
assert_status "cancel during the preparation" 0 "$BOX" cancel t1 build
wait "$RUNNER"
RESULT="$RESULTS/${SLOW_KEY%.t1}.json"
assert_contains "the result is cancelled" '"cancelled": true' "$(cat "$RESULT")"
assert_contains "exit code 130" '"exit_code": 130,' "$(cat "$RESULT")"
assert_eq "no job container ever ran" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "the queue is empty" "0" "$(/bin/ls "$QUEUE" | wc -l | tr -d ' ')"
assert_eq "the interrupted dependencies are not marked ready" "0" "$(/bin/ls "$AGENTBOX_HOME"/deps/*.ok 2>/dev/null | wc -l | tr -d ' ')"

echo " a cancel recorded between two phases stops the request before the next phase"
commit_in_task t1 seven
rm -f "$AGENTBOX_HOME"/deps/*.ok
"$BOX" verify t1 build --no-wait
PHASE_KEY="$(basename "$(/bin/ls "$QUEUE"/*.req)" .req)"
: >"$FAKE_DOCKER_LOG"
(FAKE_INSTALL_DELAY=3 "$BOX" runner --once >/dev/null 2>&1) &
RUNNER=$!
for _ in $(seq 1 20); do grep -q -e '--name agentbox-install-' "$FAKE_DOCKER_LOG" && break; sleep 1; done
assert_file "the request is still being prepared" "$QUEUE/$PHASE_KEY.req"
[ ! -f "$QUEUE/$PHASE_KEY.req" ] || echo "cancelled=1" >>"$QUEUE/$PHASE_KEY.req"
wait "$RUNNER"
assert_contains "the result is cancelled" '"cancelled": true' "$(cat "$RESULTS/${PHASE_KEY%.t1}.json")"
assert_eq "the checkout did not start" "0" "$(grep -c -e '/box/checkout.sh' "$FAKE_DOCKER_LOG")"
assert_eq "the job did not start" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"

echo " whatever the moment of the cancel, a cancelled request never ends as an ordinary result"
for delay in 0 0.4 0.9 1.5 2.5; do
  commit_in_task t1 "race $delay"
  "$BOX" verify t1 build --no-wait
  RACE_KEY="$(basename "$(/bin/ls "$QUEUE"/*.req)" .req)"
  (FAKE_JOB_DELAY=2 "$BOX" runner --once >/dev/null 2>&1) &
  RUNNER=$!
  sleep "$delay"
  "$BOX" cancel t1 build >/dev/null 2>&1
  CANCEL_STATUS=$?
  wait "$RUNNER"
  EXPECTED=false
  [ "$CANCEL_STATUS" -eq 0 ] && EXPECTED=true
  assert_eq "cancel after ${delay}s exits $CANCEL_STATUS and the result agrees" "$EXPECTED" "$(jq -r .cancelled "$RESULTS/${RACE_KEY%.t1}.json")"
  assert_eq "  the queue is empty" "0" "$(/bin/ls "$QUEUE" | wc -l | tr -d ' ')"
done

echo " an ordinary result says it was not cancelled"
commit_in_task t1 five
"$BOX" verify t1 test --no-wait
"$BOX" runner --once 2>/dev/null
assert_eq "cancelled is false" "false" "$("$BOX" facts t1 --json | jq -r '.results.test.cancelled')"
finish

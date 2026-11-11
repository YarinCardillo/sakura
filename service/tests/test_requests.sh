#!/bin/bash
# Drop requests, queue order, results and the runner loop (fake docker, real sandbox scripts).
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/t1"
RESULTS="$AGENTBOX_HOME/results/t1"
DROPS="$AGENTBOX_HOME/drop/t1"

echo " verify drops an empty file named like the verb and returns the verb's exit code"
export BOX_VERIFY_POLL=1
heartbeat t1
(BOX_TASK=t1 BOX_WORKSPACE="$WORKSPACE" BOX_REQUESTS_DIR="$DROPS" BOX_RESULTS_DIR="$RESULTS" \
  "$BOX_ROOT/image/verify" test >"$TMP/verify.out" 2>&1; echo $? >"$TMP/verify.status") &
VERIFY_PID=$!
sleep 2
assert_file "drop file named like the verb" "$DROPS/test"
assert_eq "drop file is empty" "0" "$(wc -c <"$DROPS/test" | tr -d ' ')"
"$BOX" runner --once 2>/dev/null
wait "$VERIFY_PID"
SHA="$(workspace_head t1)"
assert_no_file "drop file is consumed" "$DROPS/test"
assert_eq "verify exit code is the verb's" "0" "$(cat "$TMP/verify.status")"
RESULT="$(/bin/ls "$RESULTS"/*.json)"
assert_eq "result id is <time>-<sha>-<verb>, with the whole sha" "-$SHA-test.json" "$(basename "$RESULT" | sed 's/^[0-9]*T[0-9]*//')"
assert_contains "result has the commit the host read" "\"sha\": \"$SHA\"" "$(cat "$RESULT")"
assert_contains "result has the verb" "\"verb\": \"test\"" "$(cat "$RESULT")"
assert_contains "result has the exit code" "\"exit_code\": 0," "$(cat "$RESULT")"
assert_eq "result has the seconds the runner measured for the container run" "1" "$([ "$(jq -r .seconds "$RESULT")" -le 60 ] && echo 1 || echo 0)"
assert_contains "the peak memory of a job that ended before a reading is unknown" "\"peak_memory_bytes\": 0," "$(cat "$RESULT")"
assert_eq "queue is empty" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | wc -l | tr -d ' ')"
assert_file "log is written" "${RESULT%.json}.log"
assert_eq "result is valid JSON" "0" "$(jq -e . "$RESULT" >/dev/null 2>&1; echo $?)"
assert_no_file "nothing is written into the workspace" "$WORKSPACE/.box"

echo " an old result does not satisfy a new request for the same commit"
heartbeat t1
(BOX_TASK=t1 BOX_WORKSPACE="$WORKSPACE" BOX_REQUESTS_DIR="$DROPS" BOX_RESULTS_DIR="$RESULTS" \
  "$BOX_ROOT/image/verify" test >"$TMP/verify2.out" 2>&1; echo $? >"$TMP/verify2.status") &
VERIFY_PID=$!
sleep 2
assert_file "verify is still waiting (no status yet)" "$DROPS/test"
assert_no_file "no status written" "$TMP/verify2.status"
FAKE_JOB_EXIT=5 "$BOX" runner --once 2>/dev/null
wait "$VERIFY_PID"
assert_eq "the new result's exit code is returned" "5" "$(cat "$TMP/verify2.status")"

echo " the same commit and verb is not queued twice"
commit_in_task t1 dup
queue_directly t1 test 20260101T000000 "$(workspace_head t1)"
drop_request t1 test
FAKE_JOB_DELAY=0 "$BOX" runner --once 2>/dev/null
assert_eq "one job for the pair" "1" "$(grep -c "^run .*--name agentbox-job-.*-$(workspace_head t1)-test" "$FAKE_DOCKER_LOG")"

echo " a failing verb gives a failing result"
commit_in_task t1 two
drop_request t1 build
FAKE_JOB_EXIT=3 "$BOX" runner --once 2>/dev/null
FAILED="$(grep -l '"verb": "build"' "$RESULTS"/*.json | head -1)"
assert_contains "exit code recorded" "\"exit_code\": 3," "$(cat "$FAILED")"

echo " requests run oldest first"
commit_in_task t1 three
HEAD3="$(workspace_head t1)"
queue_directly t1 build 20260102T000002 "$HEAD3"; SECOND="$QUEUED_KEY"
queue_directly t1 test 20260102T000001 "$HEAD3"; FIRST="$QUEUED_KEY"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
ORDER="$(grep '^run ' "$FAKE_DOCKER_LOG" | grep -o -e '--name agentbox-job-[0-9T]*-[0-9a-f]*-[a-z0-9.-]*' | cut -d' ' -f2 | tr '\n' ' ')"
assert_eq "older request first" "agentbox-job-$FIRST agentbox-job-$SECOND " "$ORDER"

echo " a verb that is not in the profile is quarantined, never queued"
drop_request t1 deploy
"$BOX" runner --once 2>/dev/null
assert_no_file "drop entry removed" "$DROPS/deploy"
assert_eq "nothing queued" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | wc -l | tr -d ' ')"

echo " a queue file with a bad sha is quarantined with a result-less rejection"
printf 'task=t1\nverb=test\nsha=not-a-sha\nstate=queued\n' >"$AGENTBOX_HOME/queue/20260101T000000-aaaaaaaaaaaa-test.req"
"$BOX" runner --once 2>/dev/null
assert_no_file "queue file removed" "$AGENTBOX_HOME/queue/20260101T000000-aaaaaaaaaaaa-test.req"

echo " an interrupted request is requeued and run"
commit_in_task t1 five
queue_directly t1 test 20260103T000000 "$(workspace_head t1)"
edit_in_place 's/state=queued/state=running/' "$AGENTBOX_HOME/queue/$QUEUED_KEY.req"
"$BOX" runner --once 2>/dev/null
assert_file "interrupted request got a result" "$RESULTS/$QUEUED_ID.json"

echo " a runner error quarantines the request and the loop goes on"
commit_in_task t1 six
HEAD6="$(workspace_head t1)"
queue_directly t1 test 20260104T000000 "$HEAD6"; BROKEN="$QUEUED_KEY"
queue_directly t1 build 20260104T000001 "$HEAD6"; HEALTHY="$QUEUED_ID"
chmod 000 "$AGENTBOX_HOME/queue/$BROKEN.req"
"$BOX" runner --once 2>/dev/null
assert_file "the later request still ran" "$RESULTS/$HEALTHY.json"
chmod 644 "$AGENTBOX_HOME/queue/$BROKEN.req" 2>/dev/null

echo " two tasks at the same commit, verb and second keep separate requests"
"$BOX" new t2 sandbox >/dev/null
"$BOX" new t3 sandbox >/dev/null
RESULTS2="$AGENTBOX_HOME/results/t2"
RESULTS3="$AGENTBOX_HOME/results/t3"
SAME_HEAD="$(workspace_head t2)"
FROZEN_RESULT_ID="20261110T120000-$SAME_HEAD-build"
assert_eq "both workspaces are at the same commit" "$SAME_HEAD" "$(workspace_head t3)"
export BOX_REQUEST_TIMESTAMP=20261110T120000
"$BOX" verify t2 build --no-wait
"$BOX" verify t3 build --no-wait
assert_eq "one queue file per task" "2" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c -e "^$FROZEN_RESULT_ID\.t[23]\.req$")"
"$BOX" verify t2 build --no-wait
assert_eq "a duplicate for one task is still deduplicated" "2" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c -e "^$FROZEN_RESULT_ID\.t[23]\.req$")"
assert_eq "no temporary file is left in the queue" "0" "$(/bin/ls -A "$AGENTBOX_HOME/queue" | grep -c -e '^\.' -e '\.tmp$')"
"$BOX" runner --once 2>/dev/null
unset BOX_REQUEST_TIMESTAMP
assert_contains "task t2 got its own result" '"task": "t2"' "$(cat "$RESULTS2/$FROZEN_RESULT_ID.json" 2>/dev/null)"
assert_contains "task t3 got its own result" '"task": "t3"' "$(cat "$RESULTS3/$FROZEN_RESULT_ID.json" 2>/dev/null)"
assert_contains "t3 result names the shared commit" "\"sha\": \"$SAME_HEAD\"" "$(cat "$RESULTS3/$FROZEN_RESULT_ID.json" 2>/dev/null)"
assert_eq "two job containers ran, one per task" "2" "$(grep -c "^run .*--name agentbox-job-$FROZEN_RESULT_ID\.t[23] " "$FAKE_DOCKER_LOG")"
finish

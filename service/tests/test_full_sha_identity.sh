#!/bin/bash
# A request and its result are for one commit, named by its whole sha. Two commits whose shas
# begin with the same twelve characters (an agent can make such a pair) are two requests with
# two results, and whoever waits for one never takes the result of the other: the name of a
# result holds the whole sha, and what the file says inside (task, verb, sha) must be what was
# requested.
. "$(dirname "$0")/lib.sh"
BOX_TEST_HOME_NAME="Application Support (test)"
setup_sandbox
export BOX_VERIFY_POLL=1
"$BOX" new t1 sandbox >/dev/null
readonly SHA_A="aaaaaaaaaaaa$(printf 'b%.0s' {1..28})" SHA_B="aaaaaaaaaaaa$(printf 'c%.0s' {1..28})"
RESULTS="$AGENTBOX_HOME/results/t1"
in_box() {
  BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report; do . "$BOX_ROOT/lib/$lib.sh"; done
    '"$1" 2>/dev/null
}

echo " two commits whose shas begin with the same twelve characters"
in_box "load_task_project t1; G=\"\$(task_generation t1)\"; queue_commit t1 test $SHA_A \"\$G\"; queue_commit t1 test $SHA_B \"\$G\""
assert_eq "are two requests" "2" "$(/bin/ls "$AGENTBOX_HOME/queue"/*.req 2>/dev/null | wc -l | tr -d ' ')"
assert_eq "each named by its whole sha" "1 1" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c -e "-$SHA_A-test\.t1\.req$") $(/bin/ls "$AGENTBOX_HOME/queue" | grep -c -e "-$SHA_B-test\.t1\.req$")"
in_box "load_task_project t1; queue_commit t1 test $SHA_A \"\$(task_generation t1)\""
assert_eq "the same commit asked again stays one request" "2" "$(/bin/ls "$AGENTBOX_HOME/queue"/*.req | wc -l | tr -d ' ')"
assert_status "an id with twelve characters is not the id of a request" 1 in_box 'is_valid_request_id 20260101T000000-aaaaaaaaaaaa-test'
assert_status "one with the whole sha is" 0 in_box "is_valid_request_id 20260101T000000-$SHA_A-test"
rm -f "$AGENTBOX_HOME/queue"/*.req

echo " a result of the first commit"
in_box "load_task_project t1; write_result t1 20260101T000000-$SHA_A-test test $SHA_A 0 1 0 2026-01-01T00:00:00Z false false"
RESULT_A="$RESULTS/20260101T000000-$SHA_A-test.json"
assert_file "is named by its whole sha" "$RESULT_A"
assert_eq "who waits for the first commit finds it" "$RESULT_A" "$(in_box "matching_results t1 $SHA_A test")"
assert_eq "who waits for the second finds nothing" "" "$(in_box "matching_results t1 $SHA_B test")"

echo " a file that has the right name and says something else inside"
sed "s/$SHA_A/$SHA_B/" "$RESULT_A" >"$RESULTS/20260101T000001-$SHA_A-test.json"
sed 's/"verb": "test"/"verb": "build"/' "$RESULT_A" >"$RESULTS/20260101T000002-$SHA_A-test.json"
sed 's/"task": "t1"/"task": "t2"/' "$RESULT_A" >"$RESULTS/20260101T000003-$SHA_A-test.json"
assert_eq "only the file that is for this task, verb and commit counts" "$RESULT_A" "$(in_box "matching_results t1 $SHA_A test")"
rm -f "$RESULTS"/2026010*T00000[123]-*.json

echo " the verify an agent runs in its container"
agent_verify() {
  BOX_TASK=t1 BOX_HEAD_SHA="$1" BOX_WORKSPACE="$AGENTBOX_HOME/work/t1" BOX_REQUESTS_DIR="$AGENTBOX_HOME/drop/t1" BOX_RESULTS_DIR="$RESULTS" \
    BOX_VERIFY_WAIT=2 "$BOX_ROOT/image/verify" test >/dev/null 2>&1
}
heartbeat t1
# The result of the first commit appears while each of them waits (it is not there before).
mv "$RESULT_A" "$TMP/held.json"
(sleep 1; cp "$TMP/held.json" "$RESULT_A") &
agent_verify "$SHA_B"
assert_eq "waiting for the second commit: the result of the first is not taken (deadline, 77)" "77" "$?"
wait
rm -f "$RESULT_A" "$AGENTBOX_HOME/drop/t1/test"
(sleep 1; cp "$TMP/held.json" "$RESULT_A") &
agent_verify "$SHA_A"
assert_eq "waiting for the first commit: its result ends the wait with its exit code" "0" "$?"
wait
assert_contains "status reads the result beneath a data home with spaces" "result    test" "$("$BOX" status 2>/dev/null)"
assert_contains "status preserves result paths with spaces" "exit 0 1s sha ${SHA_A:0:12} (stale)" "$("$BOX" status 2>/dev/null)"
finish

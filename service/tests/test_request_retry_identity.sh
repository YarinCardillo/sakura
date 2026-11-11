#!/bin/bash
# Completed attempts at the same commit need distinct identities, even in the same second.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export BOX_REQUEST_TIMESTAMP=20261111T000000
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 retry
SHA="$(workspace_head t1)"
GENERATION="$(sed -n 's/^GENERATION=//p' "$AGENTBOX_HOME/tasks/t1.env")"
export BOX_ROOT SHA GENERATION TMP
publish() {
  bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report; do . "$BOX_ROOT/lib/$lib.sh"; done
    eval "$(declare -f find_queued | sed "1s/find_queued/original_find_queued/")"
    find_queued() {
      local found
      found="$(original_find_queued "$@")"
      sleep 0.2
      printf "%s" "$found"
    }
    : >"$TMP/ready-$1"
    for tick in $(seq 1 100); do [ ! -f "$TMP/publish-go" ] || break; sleep 0.05; done
    [ -f "$TMP/publish-go" ] || exit 1
    queue_commit t1 test "$SHA" "$GENERATION"
  ' _ "$1"
}
publish one >"$TMP/publish-one.out" 2>&1 &
ONE=$!
publish two >"$TMP/publish-two.out" 2>&1 &
TWO=$!
for tick in $(seq 1 100); do
  [ ! -f "$TMP/ready-one" ] || [ ! -f "$TMP/ready-two" ] || break
  sleep 0.05
done
assert_file 'first publisher reaches the barrier' "$TMP/ready-one"
assert_file 'second publisher reaches the barrier' "$TMP/ready-two"
: >"$TMP/publish-go"
wait "$ONE"
assert_eq 'first concurrent publisher succeeds' 0 "$?"
wait "$TWO"
assert_eq 'second concurrent publisher succeeds' 0 "$?"
assert_eq 'concurrent duplicate requests remain one request' 1 "$(find "$AGENTBOX_HOME/queue" -name '*.req' | wc -l | tr -d ' ')"
"$BOX" runner --once >/dev/null 2>&1
assert_eq 'the first attempt succeeds' 0 "$("$BOX" facts t1 --json | jq -r '.results.test.exitCode')"
"$BOX" verify t1 test --no-wait >/dev/null
FAKE_JOB_EXIT=17 "$BOX" runner --once >/dev/null 2>&1
assert_eq 'a completed attempt is never reused' 2 "$(find "$AGENTBOX_HOME/results/t1" -name '*.json' | wc -l | tr -d ' ')"
assert_eq 'the second check really ran' 2 "$(grep -c -e 'agentbox.kind=job' "$FAKE_DOCKER_LOG")"
assert_eq 'the new result is the second command exit' 17 "$("$BOX" facts t1 --json | jq -r '.results.test.exitCode')"
finish

#!/bin/bash
# A request belongs to one incarnation of a task. An intake that was reading the workspace while
# the task was removed and a task of the same name created must not publish its request against
# the new task, and a request of an earlier incarnation is never run.
. "$(dirname "$0")/lib.sh"
setup_sandbox
QUEUE="$AGENTBOX_HOME/queue"
generation_of() {
  sed -n 's/^GENERATION=//p' "$AGENTBOX_HOME/tasks/$1.env"
}
# publish <task> <sha> <generation>: what the intake does at its last step.
publish() {
  AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project project_detect project_admin sandbox journal task request; do . "$BOX_ROOT/lib/$lib.sh"; done
    with_queue_lock publish_request "20260101T000000-$2-build.$1" "$1" build "$2" "$3"; echo $?' _ "$@" 2>/dev/null
}

"$BOX" new t1 sandbox >/dev/null
OLD_GENERATION="$(generation_of t1)"
OLD_HEAD="$(workspace_head t1)"
assert_eq "a task has a generation" "1" "$([ -n "$OLD_GENERATION" ] && echo 1 || echo 0)"

echo " the task is removed and created again while an intake holds its old commit"
"$BOX" rm t1 --force >/dev/null 2>&1
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 "the new task moved on"
assert_eq "the new task is another incarnation" "1" "$([ "$(generation_of t1)" != "$OLD_GENERATION" ] && echo 1 || echo 0)"
assert_eq "the old intake cannot publish" "1" "$(publish t1 "$OLD_HEAD" "$OLD_GENERATION")"
assert_eq "nothing is queued" "0" "$(/bin/ls "$QUEUE" 2>/dev/null | grep -c '\.req$')"
assert_eq "an intake of the task as it is can" "0" "$(publish t1 "$(workspace_head t1)" "$(generation_of t1)")"
assert_contains "the request names its incarnation" "generation=$(generation_of t1)" "$(cat "$QUEUE"/*.req)"
"$BOX" runner --once 2>/dev/null
assert_eq "and it runs" "1" "$(/bin/ls "$AGENTBOX_HOME/results/t1" | grep -c '\.json$')"

echo " a request of an earlier incarnation that is in the queue is dropped"
printf 'task=t1\nverb=build\nsha=%s\nrequested=now\ngeneration=%s\nstate=queued\n' "$OLD_HEAD" "$OLD_GENERATION" >"$QUEUE/20260102T000000-$OLD_HEAD-build.t1.req"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>"$TMP/runner.err"
assert_eq "it left the queue" "0" "$(/bin/ls "$QUEUE" | grep -c '\.req$')"
assert_eq "no job ran" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "no result was written for it" "1" "$(/bin/ls "$AGENTBOX_HOME/results/t1" | grep -c '\.json$')"
assert_contains "the runner says why" "earlier task" "$(cat "$TMP/runner.err")"

echo " the ordinary ways still work"
commit_in_task t1 more
assert_status "box verify --no-wait" 0 "$BOX" verify t1 build --no-wait
drop_request t1 test
"$BOX" runner --once 2>/dev/null
assert_eq "both ran" "3" "$(/bin/ls "$AGENTBOX_HOME/results/t1" | grep -c '\.json$')"
finish

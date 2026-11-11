#!/bin/bash
# A task that fills its drop directory does not keep the runner from the work of the others.
# Each time round, the runner looks at no more than a fixed number of entries of a task's drop
# directory (every entry counts, whatever it is), never lists the directory whole, and then
# serves the queue before it looks again.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export BOX_DROP_INTAKE_BUDGET=10
"$BOX" new a1 sandbox >/dev/null
"$BOX" new b1 sandbox >/dev/null
commit_in_task b1 work
FLOOD="$AGENTBOX_HOME/drop/a1"
QUARANTINE="$AGENTBOX_HOME/quarantine"
for i in $(seq 1 45); do : >"$FLOOD/Not-a-verb.$i"; done
for i in $(seq 1 15); do : >"$FLOOD/verb$i"; done
drop_request b1 test
in_box() {
  BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report; do . "$BOX_ROOT/lib/$lib.sh"; done
    '"$1" 2>/dev/null
}
left() {
  /bin/ls -A "$FLOOD" | wc -l | tr -d ' '
}

echo " one time round, with 60 entries in the drop directory of one task"
: >"$FAKE_DOCKER_LOG"
NEXT="$(in_box 'ingest_drops; next_runnable_request')"
assert_eq "ten of its entries were looked at, the rest wait" "50" "$(left)"
assert_eq "ten were put aside" "10" "$(/bin/ls "$QUARANTINE" | grep -c '^drop-a1\.')"
assert_contains "the request of the other task is the next thing the runner does" "-test.b1.req" "$NEXT"
ASKED="$(grep -e '/box/drops.py' "$FAKE_DOCKER_LOG" | grep -e "drop/a1:" | sed 's|.*/box/drops.py||' | wc -w | tr -d ' ')"
assert_eq "the reader of drops was asked about no more names than that" "1" "$([ "$ASKED" -le 10 ] && echo 1 || echo 0)"
assert_eq "and never to list the directory itself" "0" "$(grep -e '/box/drops.py$' "$FAKE_DOCKER_LOG" | wc -l | tr -d ' ')"

echo " the listing stops at the budget"
assert_eq "ten names, however many there are" "10" "$(in_box 'drop_names a1' | tr -cd '\0' | wc -c | tr -d ' ')"

echo " a flood of names that are not verbs asks nothing of a container"
"$BOX" new c1 sandbox >/dev/null
for i in $(seq 1 30); do : >"$AGENTBOX_HOME/drop/c1/BAD $i"; done
: >"$FAKE_DOCKER_LOG"
in_box 'ingest_task_drops c1' >/dev/null
assert_eq "no container was started for them" "0" "$(grep -c -e '/box/drops.py' "$FAKE_DOCKER_LOG")"
assert_eq "ten were put aside" "10" "$(/bin/ls "$QUARANTINE" | grep -c '^drop-c1\.')"

echo " the runner goes on until nothing is left"
"$BOX" runner --once >/dev/null 2>&1
assert_eq "the check of the other task ran" "1" "$(/bin/ls "$AGENTBOX_HOME/results/b1"/*-test.json 2>/dev/null | wc -l | tr -d ' ')"
assert_eq "the flooded directories are empty" "0 0" "$(left) $(/bin/ls -A "$AGENTBOX_HOME/drop/c1" | wc -l | tr -d ' ')"
assert_eq "every entry was put aside, one folder each" "60 30" "$(/bin/ls "$QUARANTINE" | grep -c '^drop-a1\.') $(/bin/ls "$QUARANTINE" | grep -c '^drop-c1\.')"
finish

#!/bin/bash
# Native Play uses the person's current preview settings while task execution stays frozen.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
"$BOX" project add app --repo "$SOURCE" --subdir sub >/dev/null
"$BOX" new early app >/dev/null
"$BOX" new other app >/dev/null
SNAPSHOT_HASH="$(shasum <"$AGENTBOX_HOME/tasks/early.project")"
MISSING="box: the project of task 'early' has no run command (box project set app --run '<command>')"
assert_status 'a project without a command refuses Play' 1 "$BOX" play early start
assert_eq 'the refusal is one plain actionable line' "$MISSING" "$OUTPUT"
COMMAND='printf "current %s %s %s\n" "$SAKURA_TASK" "$SAKURA_PORTS" "$PWD"; sleep 90'
"$BOX" project set app --run "$COMMAND" --ports 3 --run-mode many >/dev/null
assert_status 'a task created before the run command now starts' 0 "$BOX" play early start
assert_eq 'the current port count is used' 3 "$("$BOX" play early status --json | jq '.ports | length')"
assert_contains 'the frozen task folder is retained' "current early 3 $AGENTBOX_HOME/work/early/sub" "$("$BOX" play early log)"
assert_status 'current many mode allows another old task to start' 0 "$BOX" play other start
assert_eq 'many mode leaves both old tasks running' 'true true' "$("$BOX" play early status --json | jq -r .running) $("$BOX" play other status --json | jq -r .running)"
"$BOX" play early stop >/dev/null
UPDATED='printf "updated %s %s\n" "$SAKURA_TASK" "$SAKURA_PORTS"; sleep 90'
"$BOX" project set app --run "$UPDATED" --ports 5 >/dev/null
assert_status 'a restart reads the updated command and port count' 0 "$BOX" play early start
assert_eq 'the prior reservation is resized' 5 "$("$BOX" play early status --json | jq '.ports | length')"
assert_contains 'the new command actually ran' 'updated early 5' "$("$BOX" play early log)"
EARLY_PORTS="$("$BOX" play early status --json | jq -c .ports)"
OTHER_PORTS="$("$BOX" play other status --json | jq -c .ports)"
assert_eq 'resized reservations do not overlap' 0 "$(jq -n --argjson a "$EARLY_PORTS" --argjson b "$OTHER_PORTS" '[$a[] as $p | $b[] | select(. == $p)] | length')"
"$BOX" play other stop >/dev/null
"$BOX" project set app --run-mode one >/dev/null
assert_status 'a start reads the current one mode' 0 "$BOX" play other start
assert_eq 'one mode stops the other preview' 'false true' "$("$BOX" play early status --json | jq -r .running) $("$BOX" play other status --json | jq -r .running)"
"$BOX" new configured app >/dev/null
"$BOX" project set app --run '' >/dev/null
assert_status 'removing the command refuses an old task with a nonempty snapshot command' 1 "$BOX" play configured start
assert_eq 'removed-command refusal is one plain actionable line' "box: the project of task 'configured' has no run command (box project set app --run '<command>')" "$OUTPUT"
"$BOX" play configured stop >/dev/null
"$BOX" play other stop >/dev/null
assert_eq 'Play did not rewrite the task snapshot' "$SNAPSHOT_HASH" "$(shasum <"$AGENTBOX_HOME/tasks/early.project")"
"$BOX" new boxed app --where box >/dev/null
assert_status 'box Play keeps its existing refusal' 81 "$BOX" play boxed start
finish

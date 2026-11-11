#!/bin/bash
# A task keeps the project values it was created with: changing a project afterwards changes
# what new tasks get, never the checks, the protected paths or the commands of existing ones.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
SNAPSHOT="$AGENTBOX_HOME/tasks/t1.project"
assert_file "the task has its own copy of the project values" "$SNAPSHOT"

echo " the project changes after the task was created"
assert_status "box project set" 0 "$BOX" project set sandbox --check "test=true" --check "build=" --protected unrelated --test-files unrelated --install "echo other"
assert_eq "the project itself changed" '["test"]' "$("$BOX" projects --json | jq -c '.[0].checks | map(.verb)')"
assert_eq "the task still has the checks it was created with" '["build","test"]' "$("$BOX" facts t1 --json 2>/dev/null | jq -c .checks)"
echo '{"x":1}' >"$AGENTBOX_HOME/work/t1/sub/package.json"
commit_in_task t1 "touch a protected file"
"$BOX" verify t1 build --no-wait 2>/dev/null
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/t1"/*-build.json | head -1)"
assert_contains "its check runs the command it was created with" "BOX_VERB_CMD=echo build " "$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "against the protected paths it was created with" '["sub/package.json"]' "$(jq -c .checksTouched "$RESULT")"
assert_eq "its results are still shown" "0" "$("$BOX" facts t1 --json 2>/dev/null | jq -r .results.build.exitCode)"

echo " a new task gets the project as it is now"
"$BOX" new t2 sandbox >/dev/null
assert_eq "its checks" '["test"]' "$("$BOX" facts t2 --json 2>/dev/null | jq -c .checks)"
assert_status "the removed verb cannot be asked of it" 1 "$BOX" verify t2 build --no-wait

echo " the copy is read as data and fails closed"
cp "$SNAPSHOT" "$TMP/snapshot.good"
echo "PATH=/tmp" >>"$SNAPSHOT"
assert_status "a copy with an unknown key is refused" 1 "$BOX" facts t1 --json
assert_contains "one line that says so" "are refused: unknown key 'PATH'" "$OUTPUT"
cp "$TMP/snapshot.good" "$SNAPSHOT"

echo " a task of an older box gets its copy the first time it is used"
rm -f "$AGENTBOX_HOME/tasks/t2.project"
"$BOX" facts t2 --json >/dev/null 2>&1
assert_file "the copy is written" "$AGENTBOX_HOME/tasks/t2.project"

echo " removing the task removes its copy"
"$BOX" rm t2 --force >/dev/null 2>&1
assert_no_file "gone" "$AGENTBOX_HOME/tasks/t2.project"
finish

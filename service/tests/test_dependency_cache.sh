#!/bin/bash
# The dependencies the runner prepares for jobs belong to one task and to the inputs they were
# built from (package.json, the lockfile, the install and rebuild commands). No task ever runs
# on dependencies another task built, or on ones built another way.
. "$(dirname "$0")/lib.sh"
setup_sandbox
{ grep -v '^INSTALL=' "$PROJECTS_DIR/sandbox.env"; echo "INSTALL=npm ci"; } >"$PROJECTS_DIR/installs.env"

# run_build <task>: queue and run the build verb; the docker calls of that run are kept in $TMP/run.log.
run_build() {
  "$BOX" verify "$1" build --no-wait 2>/dev/null
  : >"$FAKE_DOCKER_LOG"
  "$BOX" runner --once 2>/dev/null
  cp "$FAKE_DOCKER_LOG" "$TMP/run.log"
}
# The volume the job of the last run mounted as its dependencies, and how many install containers ran.
job_dependencies() {
  grep -e '--name agentbox-job-' "$TMP/run.log" | grep -o -e '-v [^ ]*:/ws/sub/node_modules:ro' | sed 's/^-v //; s/:.*//'
}
installs() {
  grep -c -e '--name agentbox-install-' "$TMP/run.log"
}

echo " two tasks with the same lockfile and another install command"
"$BOX" new first sandbox >/dev/null
"$BOX" new second installs >/dev/null
run_build first
FIRST="$(job_dependencies)"
assert_eq "the first task's dependencies were built" "1" "$(installs)"
run_build second
SECOND="$(job_dependencies)"
assert_eq "the second task's are built too, with its own command" "1" "$(grep -e '--name agentbox-install-' "$TMP/run.log" | grep -c -e 'BOX_INSTALL=npm ci')"
if [ -n "$SECOND" ] && [ "$FIRST" != "$SECOND" ]; then pass "the two jobs mount different volumes"; else fail "both jobs mount '$FIRST'"; fi
assert_contains "a volume is named after its task" "first" "$FIRST"

echo " two tasks of one project do not share either"
"$BOX" new third sandbox >/dev/null
run_build third
assert_eq "the third task builds its own" "1" "$(installs)"
if [ "$(job_dependencies)" != "$FIRST" ]; then pass "and mounts its own"; else fail "the third task mounts the first task's volume"; fi

echo " one task, the same inputs: built once"
commit_in_task first again
run_build first
assert_eq "no second build" "0" "$(installs)"
assert_eq "the same volume" "$FIRST" "$(job_dependencies)"

echo " one task, another package.json under the same lockfile: built again"
echo '{"scripts":{"postinstall":"true"}}' >"$AGENTBOX_HOME/work/first/sub/package.json"
commit_in_task first package
run_build first
assert_eq "a new build" "1" "$(installs)"
if [ "$(job_dependencies)" != "$FIRST" ]; then pass "in another volume"; else fail "the old volume was reused"; fi

echo " removing a task removes its dependencies"
: >"$FAKE_DOCKER_LOG"
"$BOX" rm first --force >/dev/null 2>&1
assert_eq "no marker of the task is left" "0" "$(/bin/ls "$AGENTBOX_HOME/deps" | grep -c '^first\.')"
assert_contains "its volumes are found by its label" "volume ls -q --filter label=agentbox.task=first" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "the other tasks keep theirs" "2" "$(/bin/ls "$AGENTBOX_HOME/deps" | grep -c -e '^second\.' -e '^third\.')"
finish

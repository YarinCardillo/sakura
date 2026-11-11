#!/bin/bash
# No phase of a request waits without a limit, also when Docker itself stops answering: the
# deadline ends the docker client the runner waits for, whether or not the container could be
# removed, and the copy of the dependencies has a deadline of its own.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
readonly PATIENCE_SECONDS=150
export FAKE_HANG_SECONDS="$PATIENCE_SECONDS"

# run_build <task>: commit, queue, run the runner once with a hard limit; prints the newest
# build result file. The runner's own exit status is kept in $TMP/runner.status.
run_build() {
  commit_in_task "$1" "change $RANDOM"
  "$BOX" verify "$1" build --no-wait 2>/dev/null
  : >"$FAKE_DOCKER_LOG"
  perl -e 'alarm shift; exec @ARGV' "$PATIENCE_SECONDS" "$BOX" runner --once >/dev/null 2>&1
  echo "$?" >"$TMP/runner.status"
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-build.json 2>/dev/null | head -1
}

echo " a job past its deadline while docker rm does not answer"
STARTED=$SECONDS
R="$(FAKE_JOB_DELAY="$PATIENCE_SECONDS" FAKE_HANG_RM=1 BOX_VERB_TIMEOUT=1 BOX_DEADLINE_GRACE=1 BOX_DOCKER_TIMEOUT=3 run_build t1)"
assert_eq "the runner came back by itself (after $((SECONDS - STARTED)) s)" "0" "$(cat "$TMP/runner.status")"
assert_eq "the request ended as a deadline" "124 true" "$(jq -r '"\(.exit_code) \(.timed_out)"' "$R" 2>/dev/null)"
assert_eq "and left the queue" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c '\.req$')"

echo " a copy of the dependencies that never ends"
STARTED=$SECONDS
R="$(BOX_DEPS_MODE=copy FAKE_COPY_HANG=1 BOX_COPY_TIMEOUT=2 run_build t1)"
assert_eq "the runner came back by itself (after $((SECONDS - STARTED)) s)" "0" "$(cat "$TMP/runner.status")"
assert_eq "the request failed" "1" "$([ "$(jq -r .exit_code "$R" 2>/dev/null)" != "0" ] && echo 1 || echo 0)"
assert_eq "no job ran on dependencies that were not copied" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"

echo " without a hang the copy mode still works"
R="$(BOX_DEPS_MODE=copy run_build t1)"
assert_eq "exit 0" "0" "$(jq -r .exit_code "$R")"
assert_contains "the job mounted its private copy" "agentbox-nmjob-" "$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
finish

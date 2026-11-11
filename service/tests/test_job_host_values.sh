#!/bin/bash
# Nothing the runner records about a job is produced inside the job container, where the code
# of the commit runs as the same user as everything else: the seconds are measured by the
# runner around the container run, the peak memory is sampled through Docker from outside, the
# exit code is the container's exit status as Docker reports it. Only the verb's exit code and
# the log come from the repository's own code.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
export BOX_MEMORY_SAMPLE_SECONDS=1
readonly JOB_SECONDS=3
readonly SLOW_MACHINE_SECONDS=60
readonly SAMPLED_BYTES=54321
readonly FORGED='BOX_VERB_SECONDS=999999\nBOX_PEAK_BYTES=1\nseconds=888888\npeak_bytes=2'

# run_test <task>: commit, queue, run, print the newest test result file of the task.
run_test() {
  commit_in_task "$1" "change $RANDOM"
  "$BOX" verify "$1" test --no-wait 2>/dev/null
  : >"$FAKE_DOCKER_LOG"
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-test.json | head -1
}

# assert_host_values <name> <result file> <exit code>: seconds and memory are the runner's own.
assert_host_values() {
  local seconds
  seconds="$(jq -r .seconds "$2")"
  assert_eq "$1: seconds are the runner's measure of the container run (got $seconds)" "1" \
    "$([ "$seconds" -ge "$JOB_SECONDS" ] && [ "$seconds" -le "$SLOW_MACHINE_SECONDS" ] && echo 1 || echo 0)"
  assert_eq "$1: peak memory is the reading sampled through Docker" "$SAMPLED_BYTES" "$(jq -r .peak_memory_bytes "$2")"
  assert_eq "$1: the exit code is the container's" "$3" "$(jq -r .exit_code "$2")"
  assert_eq "$1: nothing was copied out of the container" "0" "$(grep -c -e '^cp ' "$FAKE_DOCKER_LOG")"
}

echo " a job that prints look-alike marker lines"
export FAKE_JOB_DELAY="$JOB_SECONDS" FAKE_JOB_MEMORY="${SAMPLED_BYTES}B / 6GiB"
export FAKE_JOB_SUMMARY="$FORGED"'\n      Tests  12 passed (12)'
R="$(run_test t1)"
assert_host_values "forged lines" "$R" 0
assert_eq "the test counts are still read from the log" "12" "$(jq -r .tests.ran "$R")"

echo " a job that prints them and then overflows the log limit"
export FAKE_JOB_SUMMARY="$FORGED"'\n'"$(head -c 20000 /dev/zero | tr '\0' 'x')"
R="$(BOX_MAX_LOG_BYTES=4000 run_test t1)"
assert_contains "(the log was cut)" "[box] log cut at" "$(tail -n 1 "${R%.json}.log")"
assert_host_values "forged lines and a cut log" "$R" 0

echo " a job that leaves a forged record file in its container before exiting 0"
export FAKE_JOB_SUMMARY="$FORGED"
R="$(FAKE_JOB_RECORD='seconds=888888\npeak_bytes=2\nexit_code=0' run_test t1)"
assert_host_values "a forged record file" "$R" 0

echo " a file with that name in the checkout changes nothing"
printf 'seconds=888888\npeak_bytes=2\n' >"$AGENTBOX_HOME/work/t1/sub/.box-job-meta"
R="$(run_test t1)"
assert_host_values "a record file in the commit" "$R" 0

echo " a job that kills the wrapper: the container's exit status is recorded, by signal"
R="$(FAKE_JOB_EXIT=137 FAKE_JOB_RECORD='seconds=1\npeak_bytes=2\nexit_code=0' run_test t1)"
assert_host_values "a killed wrapper" "$R" 137
assert_eq "it is not a deadline" "false" "$(jq -r .timed_out "$R")"

echo " the container's own exit status counts, not what the docker client returned"
R="$(FAKE_JOB_EXIT=5 FAKE_JOB_CLIENT_EXIT=0 run_test t1)"
assert_host_values "a client that returned 0 for a failed container" "$R" 5

echo " a deadline is the runner's own measure, not an exit code the job can choose"
R="$(FAKE_JOB_EXIT=124 run_test t1)"
assert_eq "a job that exits 124 by itself did not time out" "124 false" "$(jq -r '"\(.exit_code) \(.timed_out)"' "$R")"
R="$(FAKE_JOB_EXIT=124 BOX_VERB_TIMEOUT=2 run_test t1)"
assert_eq "exit 124 after the verb's time limit is a deadline" "124 true" "$(jq -r '"\(.exit_code) \(.timed_out)"' "$R")"

echo " without a reading the peak memory is unknown, never a number from the job"
export FAKE_JOB_DELAY=0
R="$(run_test t1)"
assert_eq "a job that ended before the first sample: 0" "0" "$(jq -r .peak_memory_bytes "$R")"
assert_eq "its seconds are still the runner's" "1" "$([ "$(jq -r .seconds "$R")" -le "$SLOW_MACHINE_SECONDS" ] && echo 1 || echo 0)"
unset FAKE_JOB_DELAY FAKE_JOB_MEMORY FAKE_JOB_SUMMARY

echo " marker look-alikes and the test summary are not taken for one another"
export FAKE_JOB_SUMMARY='BOX_PEAK_BYTES=Tests  9 passed (9)\nseconds=Tests  8 passed (8)\n      Tests  12 passed (12)'
R="$(run_test t1)"
assert_eq "one summary line, the real one" "12" "$(jq -r .tests.ran "$R")"
assert_eq "and no number of theirs is a second or a byte" "1 0" \
  "$([ "$(jq -r .seconds "$R")" -le "$SLOW_MACHINE_SECONDS" ] && echo 1 || echo 0) $(jq -r .peak_memory_bytes "$R")"

echo " the wrapper has no channel back and the runner reads none"
assert_eq "no record or marker is named in the runner or the wrapper" "0" \
  "$(cat "$BOX_ROOT"/lib/*.sh "$BOX_ROOT"/image/runner/job.sh | grep -c -e 'BOX_VERB_SECONDS' -e 'BOX_PEAK_BYTES' -e 'log_marker' -e 'box-job-meta' -e 'memory.peak')"
finish

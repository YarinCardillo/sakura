#!/bin/bash
# The drop directory is hostile input: only empty-or-not regular files named like profile
# verbs are accepted; everything else is moved aside by rename and never followed or read
# (each into a folder of quarantine reserved for it, see test_quarantine_place.sh).
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
"$BOX" new t2 sandbox >/dev/null
DROPS="$AGENTBOX_HOME/drop/t1"
RESULTS="$AGENTBOX_HOME/results/t1"
VICTIM="$TMP/victim.txt"
echo "keep me" >"$VICTIM"

echo " poisoned drop directory"
ln -s "$VICTIM" "$DROPS/test"
mkdir "$DROPS/build"
ln -s "$VICTIM" "$DROPS/build/inner"
: >"$DROPS/te st"
: >"$DROPS/$(printf 'x\ty')"
: >"$DROPS/$(printf 'x\033[31my')"
: >"$DROPS/$(printf 'new\nline')"
: >"$DROPS/.hidden"
: >"$DROPS/UPPER"
: >"$DROPS/-rf"
: >"$DROPS/deploy"
ln -s /nonexistent "$DROPS/dangling"
OUTPUT="$("$BOX" runner --once 2>&1)"
assert_eq "runner survives and exits cleanly" "0" "$?"
assert_eq "the file behind the symlink is untouched" "keep me" "$(cat "$VICTIM")"
assert_eq "nothing was queued from the poison" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | wc -l | tr -d ' ')"
assert_eq "no result for the poison" "0" "$(/bin/ls "$RESULTS" 2>/dev/null | wc -l | tr -d ' ')"
assert_eq "the drop directory is empty again" "0" "$(/bin/ls -A "$DROPS" | wc -l | tr -d ' ')"
assert_eq "everything was moved aside" "11" "$(/bin/ls "$AGENTBOX_HOME/quarantine" | wc -l | tr -d ' ')"
assert_eq "no control character reached the log" "0" "$(printf '%s' "$OUTPUT" | LC_ALL=C tr -d '\n[:print:]' | wc -c | tr -d ' ')"

echo " a good request next to poison still runs"
ln -s "$VICTIM" "$DROPS/build"
drop_request t1 test
"$BOX" runner --once >/dev/null 2>&1
assert_eq "one result, for the regular file only" "1" "$(/bin/ls "$RESULTS"/*.json | wc -l | tr -d ' ')"
assert_contains "it is the test verb" '"verb": "test"' "$(cat "$RESULTS"/*.json)"
assert_eq "the file behind the symlink is still untouched" "keep me" "$(cat "$VICTIM")"

echo " a drop in a task directory without a task record is ignored"
mkdir -p "$AGENTBOX_HOME/drop/ghost"
: >"$AGENTBOX_HOME/drop/ghost/test"
"$BOX" runner --once >/dev/null 2>&1
assert_file "ghost drop left alone" "$AGENTBOX_HOME/drop/ghost/test"

echo " a runner error in one request does not stop the next one"
commit_in_task t1 more
drop_request t1 test
commit_in_task t2 other
queue_directly t2 test 20270101T000000 "$(workspace_head t2)"
mkdir -p "$AGENTBOX_HOME/results/t2"
chmod 555 "$AGENTBOX_HOME/results/t2"
"$BOX" runner --once >/dev/null 2>&1
chmod 755 "$AGENTBOX_HOME/results/t2"
assert_no_file "the failing request left the queue" "$AGENTBOX_HOME/queue/$QUEUED_KEY.req"
assert_eq "the failing request is in quarantine, in a folder reserved for it" "1" "$(/bin/ls "$AGENTBOX_HOME/quarantine"/request-"$QUEUED_KEY".*/request 2>/dev/null | wc -l | tr -d ' ')"
assert_eq "the other task's request ran" "2" "$(/bin/ls "$RESULTS"/*.json | wc -l | tr -d ' ')"

echo " a job that outlives its deadline has its container removed by name"
commit_in_task t1 slow
drop_request t1 test
STARTED="$(date +%s)"
BOX_VERB_TIMEOUT=1 BOX_DEADLINE_GRACE=1 FAKE_JOB_DELAY=30 "$BOX" runner --once >/dev/null 2>&1
ELAPSED=$(( $(date +%s) - STARTED ))
assert_eq "the runner did not wait for the job" "1" "$([ "$ELAPSED" -lt 20 ] && echo 1 || echo 0)"
assert_contains "the job container was force-removed" "rm -f agentbox-job-" "$(cat "$FAKE_DOCKER_LOG")"
LATEST="$(/bin/ls -t "$RESULTS"/*.json | head -1)"
assert_contains "the result says the deadline was hit" '"timed_out": true' "$(cat "$LATEST")"

echo " a request is an empty file: one with content is refused"
commit_in_task t1 content
: >"$FAKE_DOCKER_LOG"
printf '%s' "--watch; touch pwned-content" >"$AGENTBOX_HOME/drop/t1/test"
"$BOX" runner --once >"$TMP/content.out" 2>&1
assert_eq "no job ran for it" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "nothing was queued" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c '\.req$')"
assert_no_file "it was moved aside" "$AGENTBOX_HOME/drop/t1/test"
assert_contains "the runner says why" "the drop is not empty" "$(cat "$TMP/content.out")"
assert_contains "and a waiting verify is told" "the drop is not empty" "$(cat "$AGENTBOX_HOME/results/t1/.rejected-test")"
assert_contains "its size was read in a container that mounts only the drop directory" ":/drops:ro" "$(grep -e '/box/drops.py' "$FAKE_DOCKER_LOG")"
assert_eq "which has neither the workspace nor the verified store" "0" "$(grep -e '/box/drops.py' "$FAKE_DOCKER_LOG" | grep -c -e ':/src' -e '/verified')"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once >/dev/null 2>&1
assert_eq "an empty drop directory starts no container" "0" "$(grep -c -e '/box/drops.py' "$FAKE_DOCKER_LOG")"

echo " a request carries no arguments: the command always comes from the profile"
commit_in_task t1 args
: >"$FAKE_DOCKER_LOG"
: >"$AGENTBOX_HOME/drop/t1/test"
: >"$AGENTBOX_HOME/drop/t1/test --watch=true"
: >"$AGENTBOX_HOME/drop/t1/test;touch pwned-name"
: >"$AGENTBOX_HOME/drop/t1/test\$(touch pwned-sub)"
: >"$AGENTBOX_HOME/drop/t1/test-extra"
"$BOX" runner --once >/dev/null 2>&1
assert_eq "exactly one job ran" "1" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "its command is the profile's, whatever the file held" "1" "$(grep -c -e '-e BOX_VERB_CMD=echo test ' "$FAKE_DOCKER_LOG")"
assert_eq "no job command mentions an argument" "0" "$(grep -c -e 'BOX_VERB_CMD=.*\(watch\|pwned\|extra\)' "$FAKE_DOCKER_LOG")"
assert_no_file "nothing from the content ran" "$PWD/pwned-content"
assert_no_file "nothing from the name ran" "$PWD/pwned-name"
assert_no_file "nothing from a substitution in the name ran" "$PWD/pwned-sub"
assert_eq "the odd names were all moved aside" "0" "$(/bin/ls -A "$AGENTBOX_HOME/drop/t1" | wc -l | tr -d ' ')"
assert_status "the host form refuses a verb with arguments" 1 "$BOX" verify t1 "test --watch" --no-wait
finish

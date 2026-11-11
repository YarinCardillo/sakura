#!/bin/bash
# logged runs a command with its output through the reader that keeps the log within its byte
# limit. Its exit code is the command's; but a command that succeeded while the reader of its
# log failed did not succeed for box (what it printed was not kept): then the reader's status
# is returned, as the runner did before the reader got a helper of its own.
. "$(dirname "$0")/lib.sh"
setup_sandbox

# outcome <command text>: the exit code of logged for that command (bash -c), with the done
# file and the limit this test names, so that the command can find the reader of its own log.
outcome() {
  BOX_ROOT="$BOX_ROOT" DONE="$TMP/job.done" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner; do . "$BOX_ROOT/lib/$lib.sh"; done
    code=0
    logged "$DONE" 4096 bash -c "$1" >"$2" || code=$?
    echo "$code"' _ "$1" "$TMP/job.log"
}
readonly KILL_READER='kill -KILL "$(pgrep -f "bounded_log.pl 4096 $DONE")"; sleep 0.5'

echo " the command and the reader both end well"
assert_eq "exit code 0" "0" "$(outcome 'echo a line')"
assert_eq "the output is in the log" "a line" "$(cat "$TMP/job.log")"

echo " the command fails"
assert_eq "its exit code is returned" "3" "$(outcome 'echo a line; exit 3')"

echo " the reader of the log is killed while a quiet command runs and succeeds"
assert_eq "the result is not a success: the status of the reader (killed, 137)" "137" "$(outcome "$KILL_READER; exit 0")"

echo " the reader is killed and the command fails too"
assert_eq "the exit code of the command is kept" "3" "$(outcome "$KILL_READER; exit 3")"
assert_no_file "the done file is removed each time" "$TMP/job.done"
finish

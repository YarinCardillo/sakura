#!/bin/bash
# The reader of a conversation's log is started before the relay that writes to it. It must not
# hold the locks of box while it waits for that writer (were box acp killed in between, the
# locks would stay taken with nobody left to end the session), and it must not wait for a
# writer for ever.
. "$(dirname "$0")/lib.sh"
setup_sandbox
mkdir -p "$AGENTBOX_HOME/locks" "$AGENTBOX_HOME/acp"
TASK_LOCK="$AGENTBOX_HOME/locks/task-t1.lock"
FRONT_LOCK="$AGENTBOX_HOME/locks/frontend-t1.lock"

lock_is_free() {
  perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 1; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$1"
}

echo " box acp is killed after it started the reader of the log and before it started the relay"
AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" BOX_LOG_RENDEZVOUS_SECONDS=3 bash -c '
  for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks journal task request checks factscache cancel launchd runner status report session agent acp; do . "$BOX_ROOT/lib/$lib.sh"; done
  exec 5>>"$1" 6>>"$2"
  perl -MFcntl=:flock -e "for my \$fd (5, 6) { open(my \$f, \">&=\", \$fd) or die; flock(\$f, LOCK_EX) or die }"
  mkfifo "$ACP_LOG_DIR/.t1.stderr"
  start_log_reader "$ACP_LOG_DIR/.t1.stderr" "$ACP_LOG_DIR/t1.log" "$ACP_LOG_DIR/.t1.ended"
  echo "$LOG_READER_PID" >"$3"
  kill -KILL $$' _ "$TASK_LOCK" "$FRONT_LOCK" "$TMP/reader.pid" 2>/dev/null
READER="$(cat "$TMP/reader.pid" 2>/dev/null)"
assert_eq "(a reader was started)" "1" "$([ -n "$READER" ] && echo 1 || echo 0)"
sleep 1
assert_eq "the lifecycle lock is free at once" "0" "$(lock_is_free "$TASK_LOCK"; echo $?)"
assert_eq "and the lock of the front end" "0" "$(lock_is_free "$FRONT_LOCK"; echo $?)"
assert_eq "the reader gives up waiting for a writer by itself" "0" "$(still_alive "$READER")"

echo " a writer that comes in time is read as before"
mkfifo "$TMP/pipe"
AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks journal task request checks factscache cancel launchd runner status report session agent acp; do . "$BOX_ROOT/lib/$lib.sh"; done
  start_log_reader "$1/pipe" "$1/log" "$1/ended"
  sleep 1
  echo "a line of the adapter" >"$1/pipe"
  : >"$1/ended"
  wait "$LOG_READER_PID"' _ "$TMP"
assert_eq "the log has what was written" "a line of the adapter" "$(cat "$TMP/log")"

echo " both conversations, in a box and on the Mac, start their reader this way"
assert_eq "the container launch" "1" "$(grep -c '^  start_log_reader "\$pipe" "\$log" "\$ended"$' "$BOX_ROOT/lib/acp.sh")"
assert_eq "the native launch" "1" "$(grep -c '^  start_log_reader "\$pipe" "\$log" "\$ended"$' "$BOX_ROOT/lib/native_acp.sh")"
assert_eq "no launch opens the pipe for reading in the shell" "0" "$(cat "$BOX_ROOT/lib/acp.sh" "$BOX_ROOT/lib/native_acp.sh" | grep -c '<"\$pipe"')"
finish

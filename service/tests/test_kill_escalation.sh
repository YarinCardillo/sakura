#!/bin/bash
# When box ends a process group it asks first (TERM), waits, and kills what stays. Between the
# asking and the killing the group can end and its number be given to another process. So the
# record is compared with the machine again immediately before the kill: when it no longer
# names its process, nothing is killed. A group that simply ignores the asking is still killed.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --run 'sleep 300' --check "test=true" >/dev/null
"$BOX" new m1 app >/dev/null
BOOT="$(record_boot)"
export LIBS="common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report session agent acp native_acp play"

# stubborn [<record>]: a process that leads a group of its own and does not end when asked
# (TERM). Its pid is in STUBBORN. With a record: the moment it is asked, the record stops naming
# it (its start time becomes another one), as if the group had ended and its number had gone to
# another process between the asking and the kill.
stubborn() {
  perl -MPOSIX -e 'setpgid(0, 0); my $record = shift;
    $SIG{TERM} = sub {
      return unless defined $record;
      open(my $in, "<", $record) or return; my @lines = <$in>; close $in;
      $lines[2] = "Thu Jan  1 00:00:00 1970\n";
      open(my $out, ">", $record) or return; print $out @lines; close $out;
    };
    for (1 .. 300) { sleep 1 }' ${1+"$1"} &
  STUBBORN=$!
  sleep 1
}
# record_for <file> <pid>: a record that names the process, as native_run.pl writes one.
record_for() {
  mkdir -p "$(dirname "$1")"
  printf '%s\n%s\n%s\n' "$2" "$BOOT" "$(record_start "$2")" >"$1"
}
lives() {
  ps -p "$1" -o pid= | wc -l | tr -d ' '
}
# in_box <shell text>: runs functions of box with the project of m1 loaded.
in_box() {
  BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in $LIBS; do . "$BOX_ROOT/lib/$lib.sh"; done
    load_task_project m1
    '"$1" >/dev/null 2>&1
}

echo " Play: a group that ignores the asking is killed"
stubborn
record_for "$AGENTBOX_HOME/play/m1.pid" "$STUBBORN"
assert_status "box play stop" 0 "$BOX" play m1 stop
assert_eq "it is gone" "0" "$(still_alive "$STUBBORN")"

echo " Play: a record that stops naming its process between the asking and the kill"
stubborn "$AGENTBOX_HOME/play/m1.pid"
record_for "$AGENTBOX_HOME/play/m1.pid" "$STUBBORN"
assert_status "box play stop" 0 "$BOX" play m1 stop
assert_eq "nothing was killed" "1" "$(lives "$STUBBORN")"
kill -KILL "$STUBBORN" 2>/dev/null

echo " a native check"
stubborn
record_for "$AGENTBOX_HOME/stage/key.m1.host/group" "$STUBBORN"
in_box 'end_native_job key.m1'
assert_eq "a job that ignores the asking is killed" "0" "$(still_alive "$STUBBORN")"
stubborn "$AGENTBOX_HOME/stage/key.m1.host/group"
record_for "$AGENTBOX_HOME/stage/key.m1.host/group" "$STUBBORN"
in_box 'end_native_job key.m1'
assert_eq "a record that stops naming its process: nothing was killed" "1" "$(lives "$STUBBORN")"
kill -KILL "$STUBBORN" 2>/dev/null

echo " a conversation that box rm ends"
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 40' "$AGENTBOX_HOME/locks/frontend-m1.lock" &
HOLDER=$!
sleep 1
stubborn
record_for "$AGENTBOX_HOME/acp/m1.pid" "$STUBBORN"
in_box 'end_conversation m1'
assert_eq "one that ignores the asking is killed" "0" "$(still_alive "$STUBBORN")"
stubborn "$AGENTBOX_HOME/acp/m1.pid"
record_for "$AGENTBOX_HOME/acp/m1.pid" "$STUBBORN"
in_box 'end_conversation m1'
assert_eq "a record that stops naming its process: nothing was killed" "1" "$(lives "$STUBBORN")"
kill -KILL "$STUBBORN" "$HOLDER" 2>/dev/null
wait "$HOLDER" 2>/dev/null
finish

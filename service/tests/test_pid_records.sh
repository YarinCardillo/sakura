#!/bin/bash
# A record of a process group that box keeps (Play, the conversation, a native check) names the
# group, the boot of the machine and the start of the process. Before box signals a group it
# checks all three against the machine as it is; a record that does not match (left by a crash
# or a reboot, its number given to another process since) is discarded without a signal.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --run 'sleep 300' --check "test=true" >/dev/null
"$BOX" new m1 app >/dev/null
BOOT="$(record_boot)"

# An unrelated program of the person, leading a process group of its own.
perl -MPOSIX -e 'setpgid(0, 0); sleep 300' &
VICTIM=$!
sleep 1
victim_lives() {
  ps -p "$VICTIM" -o pid= | wc -l | tr -d ' '
}
# forge <file> <kind>: a record that names the unrelated program.
forge() {
  mkdir -p "$(dirname "$1")"
  case "$2" in
    number-only) printf '%s\n' "$VICTIM" >"$1" ;;
    other-start) printf '%s\n%s\n%s\n' "$VICTIM" "$BOOT" "Thu Jan  1 00:00:00 1970" >"$1" ;;
    other-boot) printf '%s\n%s\n%s\n' "$VICTIM" "1" "$(record_start "$VICTIM")" >"$1" ;;
  esac
}

echo " what a record holds"
"$BOX" play m1 start >/dev/null
RECORD="$AGENTBOX_HOME/play/m1.pid"
GROUP="$(sed -n 1p "$RECORD")"
assert_eq "three lines: the group, the boot of the machine, the start of the process" "3 $BOOT" "$(wc -l <"$RECORD" | tr -d ' ') $(sed -n 2p "$RECORD")"
assert_eq "the start is the one the system reports for the group's leader" "$(record_start "$GROUP")" "$(sed -n 3p "$RECORD")"
"$BOX" play m1 stop >/dev/null
assert_eq "(a real record is honoured: the Play was stopped)" "0" "$(still_alive "$GROUP")"

echo " a forged or stale record of a Play"
for kind in number-only other-start other-boot; do
  forge "$RECORD" "$kind"
  assert_eq "$kind: the Play is not taken to run" "false" "$("$BOX" play m1 status --json | jq -r .running)"
  forge "$RECORD" "$kind"
  assert_status "$kind: box play stop" 0 "$BOX" play m1 stop
  assert_eq "  the unrelated program was not signalled" "1" "$(victim_lives)"
  assert_no_file "  the record was discarded" "$RECORD"
done

echo " of a conversation"
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 6' "$AGENTBOX_HOME/locks/frontend-m1.lock" &
HOLDER=$!
sleep 1
forge "$AGENTBOX_HOME/acp/m1.pid" other-start
assert_status "box stop does not take it for the conversation" 1 "$BOX" stop m1
assert_eq "the unrelated program was not signalled" "1" "$(victim_lives)"
assert_no_file "the record was discarded" "$AGENTBOX_HOME/acp/m1.pid"
wait "$HOLDER"

echo " of a native check that was running when the runner died"
"$BOX" verify m1 test --no-wait >/dev/null
REQUEST="$(/bin/ls "$AGENTBOX_HOME/queue"/*.req)"
KEY="$(basename "$REQUEST" .req)"
edit_in_place 's/^state=queued$/state=running/' "$REQUEST"
forge "$AGENTBOX_HOME/stage/$KEY.host/group" other-start
assert_status "the next runner takes the request up again" 0 "$BOX" runner --once
assert_eq "the unrelated program was not signalled" "1" "$(victim_lives)"
assert_eq "and the check ran" "0" "$(jq -r .exit_code "$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-test.json | head -1)")"

echo " box rm with stale records of everything"
forge "$RECORD" other-start
forge "$AGENTBOX_HOME/acp/m1.pid" number-only
assert_status "box rm --force" 0 "$BOX" rm m1 --force
assert_eq "the unrelated program was not signalled" "1" "$(victim_lives)"
kill "$VICTIM" 2>/dev/null
finish

#!/bin/bash
# The record of a process group needs the boot time of the machine and the start time of the
# process. Both are read with programs named by their absolute paths, so a short PATH (the app
# starts box with one that has no /usr/sbin) does not leave them empty. And when a record
# cannot be made, the command that was started for it is ended and collected at once: nothing
# runs that box could not find again.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --run "echo \$\$ >\"$TMP/play.pid\"; exec sleep 300" >/dev/null
"$BOX" new m1 app >/dev/null
SHORT_PATH="$TESTS_DIR/fake-bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"

echo " a PATH without /usr/sbin, as the app has it"
assert_eq "(sysctl is not in that PATH)" "0" "$(PATH="$SHORT_PATH" /bin/sh -c 'command -v sysctl' | wc -l | tr -d ' ')"
OUT="$(PATH="$SHORT_PATH" "$BOX" play m1 start --json 2>"$TMP/start.err")"
assert_eq "box play start works" "0 true" "$? $(jq -r .running <<<"$OUT")"
RECORD="$AGENTBOX_HOME/play/m1.pid"
assert_eq "the record has the group, the boot time and the start time, none empty" "3 3" "$(wc -l <"$RECORD" | tr -d ' ') $(grep -c . "$RECORD")"
assert_eq "the boot time is a number" "1" "$(sed -n 2p "$RECORD" | grep -c -E '^[0-9a-f-]+$')"
PID="$(cat "$TMP/play.pid")"
assert_status "box play stop, with the same PATH" 0 env PATH="$SHORT_PATH" "$BOX" play m1 stop
assert_eq "the command is gone" "0" "$(still_alive "$PID")"

echo " a record that cannot be written"
rm -f "$TMP/child.pid"
perl "$BOX_ROOT/lib/native_run.pl" 0 "$TMP/no-such-folder/record" /bin/sh -c "echo \$\$ >$TMP/child.pid; exec sleep 300" 2>"$TMP/run.err"
STATUS=$?
assert_eq "the start fails" "1" "$([ "$STATUS" -ne 0 ] && echo 1 || echo 0)"
assert_contains "and says why" "record" "$(cat "$TMP/run.err")"
for _ in $(seq 1 25); do [ -s "$TMP/child.pid" ] && break; sleep 0.2; done
if [ -s "$TMP/child.pid" ]; then
  assert_eq "the command it had started is ended and collected" "0" "$(still_alive "$(cat "$TMP/child.pid")")"
else
  pass "the command was never started"
fi
finish

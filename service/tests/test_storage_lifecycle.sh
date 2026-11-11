#!/bin/bash
# A storage read must not mount a task that was removed or replaced while Docker starts.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
: >"$FAKE_DOCKER_LOG"
FAKE_SLOW_CALL=/box/storage.py FAKE_SLOW_SECONDS=6 "$BOX" facts t1 --json >"$TMP/facts.json" 2>"$TMP/facts.err" &
READER=$!
for tick in $(seq 1 100); do
  grep -q '/box/storage.py' "$FAKE_DOCKER_LOG" && break
  sleep 0.1
done
assert_contains 'the storage client is pending' /box/storage.py "$(cat "$FAKE_DOCKER_LOG")"
exclusive_lock() {
  perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 2; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$AGENTBOX_HOME/locks/task-t1.lock"
}
assert_status 'removal or recreation cannot acquire the task while storage is read' 1 exclusive_lock
wait "$READER"
assert_eq 'the storage read succeeded' 0 "$?"
assert_eq 'facts include the completed measure' detection "$(jq -r '.disk.enforcement' "$TMP/facts.json")"
assert_status 'the task lock is released after the read' 0 exclusive_lock
finish

#!/bin/bash
# Creating and removing a task are one at a time per task, and the host never runs git in a
# workspace that already exists: after a second box new of the same name lost the race, the
# folder there may be one an agent has written.
. "$(dirname "$0")/lib.sh"
setup_sandbox
MARKER="$TMP/ran-on-the-host"
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
BEFORE="$(git -C "$W" for-each-ref --format='%(refname) %(objectname)'; git -C "$W" remote; git -C "$W" config --get core.sparseCheckout)"

echo " the clone step refuses a folder that exists and touches nothing in it"
git -C "$W" config filter.evil.smudge "sh -c 'touch $MARKER-smudge; cat'"
git -C "$W" config filter.evil.clean "sh -c 'touch $MARKER-clean; cat'"
echo '* filter=evil' >"$W/.git/info/attributes"
git -C "$W" remote add origin "$SOURCE"
BEFORE="$(git -C "$W" for-each-ref --format='%(refname) %(objectname)'; git -C "$W" remote)"
STATUS="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project project_detect project_admin sandbox journal task; do . "$BOX_ROOT/lib/$lib.sh"; done
  load_project sandbox
  sha="$(clone_workspace t1 main)"; echo "status=$? sha=$sha"' 2>/dev/null)"
assert_eq "it fails and prints no commit" "status=1 sha=" "$STATUS"
assert_eq "the workspace's refs and remotes are as the agent left them" "$BEFORE" "$(git -C "$W" for-each-ref --format='%(refname) %(objectname)'; git -C "$W" remote)"
assert_eq "no filter of the workspace ran on the host" "0" "$(/bin/ls "$TMP" | grep -c '^ran-on-the-host')"
assert_status "box new of an existing task" 1 "$BOX" new t1 sandbox
assert_contains "says so" "already exists" "$OUTPUT"

echo " box new and box rm of one task never run at the same time"
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 8' "$AGENTBOX_HOME/locks/task-t2.lock" &
HOLDER=$!
sleep 1
assert_status "box new while another is at work on that task" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" new t2 sandbox
assert_contains "one line that says so" "another box new or box rm" "$OUTPUT"
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/t2"
assert_status "box rm too" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm t2 --force
assert_status "another task is not held up" 0 "$BOX" new t3 sandbox
kill "$HOLDER" 2>/dev/null; wait "$HOLDER" 2>/dev/null
assert_status "once the other is done it works" 0 "$BOX" new t2 sandbox
assert_status "two box new of one name at the same moment: one wins" 0 true
"$BOX" new t4 sandbox >"$TMP/a.out" 2>&1 &
FIRST=$!
"$BOX" new t4 sandbox >"$TMP/b.out" 2>&1 &
SECOND=$!
wait "$FIRST"; A=$?
wait "$SECOND"; B=$?
assert_eq "exit codes" "0 1" "$(printf '%s\n' "$A" "$B" | sort | tr '\n' ' ' | sed 's/ $//')"
assert_eq "the task is whole" "box/t4" "$(git -C "$AGENTBOX_HOME/work/t4" rev-parse --abbrev-ref HEAD)"
finish

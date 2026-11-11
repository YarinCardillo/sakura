#!/bin/bash
# A command that starts something for a task (a conversation, Play, an agent in a box) holds the
# lifecycle lock of the task from before it looks at the task until what it starts is running,
# and checks again, just before the start, that the task is still the one it looked at (same
# place, same generation). box rm and box new wait a short while for that lock
# (BOX_TASK_LOCK_PATIENCE) and are then refused. So box rm and box new cannot swap a task for one of the other place
# under a start that is pending: a native start never lands in a box workspace, and the mounts
# prepared for a box never land on a worktree of the Mac.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_ACCOUNTS="personal" FAKE_NPM_LOG="$TMP/npm.log"
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$TMP/login"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
"$BOX" project add app --repo "$REPO" --subdir sub --run 'sleep 300' >/dev/null
"$BOX" new m1 app >/dev/null
"$BOX" new b1 app --where box >/dev/null
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'

echo " a native conversation that is still starting (the adapter is being installed)"
mkfifo "$TMP/stdin"
FAKE_NPM_DELAY=6 "$BOX" acp m1 personal <"$TMP/stdin" >"$TMP/acp.out" 2>"$TMP/acp.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 50); do [ -s "$FAKE_NPM_LOG" ] && break; sleep 0.2; done
assert_status "box rm --force of that lavoro is refused meanwhile" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm m1 --force
assert_contains "it says that the task is busy" "another box new or box rm" "$OUTPUT"
assert_status "and so is box new of that name in a box" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" new m1 app --where box
assert_contains "the record still says mac" "WHERE=mac" "$(cat "$AGENTBOX_HOME/tasks/m1.env")"
printf '%s\n' "$INITIALIZE" >&5
exec 5>&-
wait "$SESSION"
assert_eq "the conversation then starts, natively, in the worktree it was requested" "0 1" "$? $(grep -c "fake adapter started in $AGENTBOX_HOME/work/m1" "$AGENTBOX_HOME/acp/m1.log")"
assert_eq "no container was started for it" "0" "$(grep -c -e 'agentbox-acp-m1' "$FAKE_DOCKER_LOG")"

echo " once the conversation runs the lock is free again: box rm ends it and removes the lavoro"
rm -f "$TMP/child.pid"
FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" "$BOX" acp m1 personal <"$TMP/stdin" >/dev/null 2>&1 &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 50); do [ -s "$TMP/child.pid" ] && break; sleep 0.2; done
# The adapter runs; box acp lets go of the lifecycle lock a moment after it has seen that.
wait_for_started m1
assert_status "box rm --force while a conversation is open" 0 "$BOX" rm m1 --force
exec 5>&-
wait "$SESSION" 2>/dev/null
assert_eq "what the conversation had started is gone" "0" "$(still_alive "$(cat "$TMP/child.pid")")"
assert_no_file "and so is the worktree" "$AGENTBOX_HOME/work/m1"

echo " a Play that is waiting for its turn to start"
"$BOX" new m2 app >/dev/null
mkdir -p "$AGENTBOX_HOME/locks"
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 5' "$AGENTBOX_HOME/locks/play.lock" &
HOLDER=$!
sleep 1
"$BOX" play m2 start >"$TMP/play.out" 2>&1 &
PLAY=$!
sleep 1
assert_status "box rm --force is refused meanwhile" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm m2 --force
wait "$HOLDER" "$PLAY"
assert_eq "the Play then runs, for the lavoro it was requested" "true" "$("$BOX" play m2 status --json | jq -r .running)"
"$BOX" play m2 stop >/dev/null

echo " the reverse: a conversation of a box that is still preparing its container"
: >"$FAKE_DOCKER_LOG"
FAKE_SLOW_CALL="rm -f agentbox-acp-b1" FAKE_SLOW_SECONDS=5 "$BOX" acp b1 personal <"$TMP/stdin" >/dev/null 2>"$TMP/box.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 50); do grep -q 'rm -f agentbox-acp-b1' "$FAKE_DOCKER_LOG" && break; sleep 0.2; done
assert_status "box rm --force of that task is refused meanwhile" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm b1 --force
assert_status "and so is box new of that name on the Mac" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" new b1 app --where mac
assert_contains "the record still says box" "WHERE=box" "$(cat "$AGENTBOX_HOME/tasks/b1.env")"
printf '%s\n' "$INITIALIZE" >&5
exec 5>&-
wait "$SESSION"
assert_eq "the container then starts, on the workspace of the box" "1" "$(grep -e '--name agentbox-acp-b1' "$FAKE_DOCKER_LOG" | grep -c "work/b1:/work/b1")"
assert_eq "it is a clone, not a worktree of the person's repository" "1" "$([ -d "$AGENTBOX_HOME/work/b1/.git" ] && echo 1 || echo 0)"

echo " a docker client that hangs before it has created the container (run, agent, shell, acp)"
# suspended <verb name> <task> <start of the container's name> <box arguments...>: the client of that verb sleeps before it does
# anything; meanwhile the task cannot be replaced; when the wait for the container is over the
# pending client is ended, and only then is the task free. No container ever comes of it.
suspended() {
  local verb="$1" task="$2" container="$3" pid status
  shift 3
  "$BOX" new "$task" app --where box >/dev/null 2>&1
  : >"$FAKE_DOCKER_LOG"
  FAKE_SLOW_CALL="--name $container" FAKE_SLOW_SECONDS=40 BOX_START_TIMEOUT=6 "$BOX" "$@" </dev/null >/dev/null 2>"$TMP/suspended.err" &
  pid=$!
  for _ in $(seq 1 50); do grep -q -e "^run .*work/$task:/work/$task" "$FAKE_DOCKER_LOG" && break; sleep 0.2; done
  assert_eq "$verb: (its docker client was started with the workspace of the box)" "1" "$(grep -c -e "^run .*work/$task:/work/$task" "$FAKE_DOCKER_LOG")"
  assert_status "$verb: box rm --force of the task is refused while the client is pending" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm "$task" --force
  assert_status "$verb: and box new of its name on the Mac" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" new "$task" app --where mac
  wait "$pid"; status=$?
  assert_eq "$verb: the start is given up with an error" "1" "$([ "$status" -ne 0 ] && echo 1 || echo 0)"
  assert_contains "$verb: one line says so" "did not start within" "$(tail -n 1 "$TMP/suspended.err")"
  assert_eq "$verb: the pending client is gone" "0" "$(pgrep -f "fake-bin/docker run .*work/$task:/work/$task" | wc -l | tr -d ' ')"
  assert_status "$verb: now the task can be removed" 0 "$BOX" rm "$task" --force
  assert_status "$verb: and its name taken by a lavoro on the Mac" 0 "$BOX" new "$task" app --where mac
  sleep 1
  assert_eq "$verb: no container exists for it" "0" "$(/bin/ls "$FAKE_DOCKER_LOG".* 2>/dev/null | grep -c -E "\.log\.(ctr\.)?agentbox-(agent|acp|ws)-$task")"
  "$BOX" rm "$task" --force >/dev/null 2>&1
}
suspended run s-run agentbox-ws-s-run- run s-run -- true
suspended agent s-agent agentbox-agent-s-agent agent s-agent personal
suspended shell s-shell agentbox-ws-s-shell- shell s-shell
suspended acp s-acp agentbox-acp-s-acp acp s-acp personal

echo " a read of a workspace in a container that is still running"
"$BOX" new s-read app --where box >/dev/null 2>&1
: >"$FAKE_DOCKER_LOG"
FAKE_SLOW_CALL="/box/head.py" FAKE_SLOW_SECONDS=8 "$BOX" log s-read --json >/dev/null 2>&1 &
READ=$!
for _ in $(seq 1 50); do grep -q -e "/box/head.py" "$FAKE_DOCKER_LOG" && break; sleep 0.2; done
assert_status "box rm --force of the task is refused while the read is under way" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" rm s-read --force
assert_status "and box new of its name on the Mac" 1 env BOX_TASK_LOCK_PATIENCE=1 "$BOX" new s-read app --where mac
assert_eq "two reads at once are fine" "box" "$("$BOX" facts s-read --json | jq -r .where)"
wait "$READ"
assert_status "once the read is over the task can be removed" 0 "$BOX" rm s-read --force
"$BOX" new s-read app --where box >/dev/null 2>&1
perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or die; flock($f, LOCK_EX) or die; sleep 4' "$AGENTBOX_HOME/locks/task-s-read.lock" &
HOLDER=$!
sleep 1
STARTED=$SECONDS
assert_eq "a read waits for a start or a removal that is under way, then answers" "box" "$("$BOX" facts s-read --json | jq -r .where)"
assert_eq "(it waited: $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -ge 2 ] && echo 1 || echo 0)"
wait "$HOLDER"

echo " the start is bound to the task it looked at: another generation or place refuses"
for change in 's/^GENERATION=.*/GENERATION=another/' 's/^WHERE=.*/WHERE=mac/'; do
  cp "$AGENTBOX_HOME/tasks/b1.env" "$TMP/record"
  OUTPUT="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" CHANGE="$change" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks journal task; do . "$BOX_ROOT/lib/$lib.sh"; done
    begin_startup b1 box
    . "$BOX_ROOT/tests/lib.sh"
    edit_in_place "$CHANGE" "$AGENTBOX_HOME/tasks/b1.env"
    confirm_startup b1; echo "confirmed"' 2>&1)"
  assert_eq "a record that changed under the start ($change): nothing is started" "1 0" "$? $(grep -c confirmed <<<"$OUTPUT")"
  assert_contains "  one line says so" "box: task 'b1' was removed or created again while this command was starting" "$OUTPUT"
  cp "$TMP/record" "$AGENTBOX_HOME/tasks/b1.env"
done
finish

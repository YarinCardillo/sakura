#!/bin/bash
# Verified readers, imports and take retain the task binding until their host effects end.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export BOX_TASK_LOCK_PATIENCE=1
exclusive_lock() {
  perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 2; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$AGENTBOX_HOME/locks/task-t1.lock"
}
for step in /box/verify_import.py /box/bundle.sh; do
  "$BOX" rm t1 --force >/dev/null 2>&1
  git -C "$SOURCE" branch -D box/t1 >/dev/null 2>&1
  "$BOX" new t1 sandbox >/dev/null
  commit_in_task t1 work
  rm -rf "$AGENTBOX_HOME/verified/t1.store"
  : >"$FAKE_DOCKER_LOG"
  FAKE_SLOW_CALL="$step" FAKE_SLOW_SECONDS=6 "$BOX" take t1 >"$TMP/take.out" 2>"$TMP/take.err" &
  TAKER=$!
  for tick in $(seq 1 100); do grep -q "$step" "$FAKE_DOCKER_LOG" && break; sleep 0.1; done
  assert_contains 'the selected helper is paused' "$step" "$(cat "$FAKE_DOCKER_LOG")"
  assert_status 'the lifecycle lock excludes removal and recreation' 1 exclusive_lock
  assert_status 'force removal refuses while the helper is pending' 1 "$BOX" rm t1 --force
  assert_status 'same-name native creation refuses' 1 "$BOX" new t1 sandbox --where mac
  wait "$TAKER"
  assert_eq 'take completes in the original repository' 0 "$?"
  assert_eq 'the fetched branch is the original task commit' "$(workspace_head t1)" "$(git -C "$SOURCE" rev-parse refs/heads/box/t1)"
  assert_status 'the lock is released after the final fetch' 0 exclusive_lock
  assert_eq 'every task sandbox has its task label' 0 "$(grep 'agentbox.kind=sandbox' "$FAKE_DOCKER_LOG" | grep -vc 'agentbox.task=t1')"
done
# Pause the host-side fetch after the verified bundle container has already ended.
export BOX_ROOT TMP
bash -c '
  for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner status report; do . "$BOX_ROOT/lib/$lib.sh"; done
  eval "$(declare -f person_git | sed "1s/person_git/original_person_git/")"
  person_git() {
    case " $* " in *" fetch "*) : >"$TMP/fetch-pending"; sleep 4 ;; esac
    original_person_git "$@"
  }
  cmd_take t1
' >"$TMP/final-fetch.out" 2>"$TMP/final-fetch.err" &
TAKER=$!
for tick in $(seq 1 100); do [ ! -f "$TMP/fetch-pending" ] || break; sleep 0.1; done
assert_file 'the host fetch is pending after the bundle completed' "$TMP/fetch-pending"
assert_status 'the lifecycle lock remains held during the final host fetch' 1 exclusive_lock
assert_status 'force removal refuses during the host fetch' 1 "$BOX" rm t1 --force
wait "$TAKER"
assert_eq 'the final fetch completes' 0 "$?"
assert_status 'the lifecycle lock is released after the host fetch completes' 0 exclusive_lock
finish

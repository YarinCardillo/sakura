#!/bin/bash
# Checks for a lavoro on the Mac: box verify queues as for a box, and the runner runs the
# project's check command natively in the worktree, one at a time across all lavori, only on a
# worktree without uncommitted changes, with the same result fields (computed with plain git
# against the base), the same deadline, cancel and finality. Docker is never asked.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
readonly SUMMARY='echo " Tests  1 failed | 4 passed | 2 skipped (7)"'
"$BOX" project add app --repo "$REPO" --subdir sub --protected "package.json scripts/**" --test-files "**/*.spec.ts" \
  --check "test=echo \"test of \$SAKURA_TASK in \$PWD\"; $SUMMARY" --check "build=echo building; exit 3" \
  --check "slow=echo \$\$ >\"$TMP/slow.pid\"; sh -c 'echo \$\$ >\"$TMP/slow.child\"; exec sleep 300' & wait" \
  --check "mark=echo start \$SAKURA_TASK >>\"$TMP/order\"; sleep 2; echo end \$SAKURA_TASK >>\"$TMP/order\"" >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"

commit_in() {
  git -C "$1" add -A && git -C "$1" -c user.name=t -c user.email=t@example.com commit -q -m "$2"
}
last_result() {
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-"$2".json 2>/dev/null | head -1
}
is_alive() {
  ps -p "$1" -o pid= 2>/dev/null | wc -l | tr -d ' '
}

echo " box verify queues the commit of the worktree; the runner runs the command natively"
echo "work" >"$W/sub/work.txt"
commit_in "$W" "work"
HEAD_SHA="$(git -C "$W" rev-parse HEAD)"
: >"$FAKE_DOCKER_LOG"
assert_status "verify --no-wait" 0 "$BOX" verify m1 test --no-wait
assert_eq "one request for the head, queued" "$HEAD_SHA queued" "$(sed -n 's/^sha=//p; s/^state=//p' "$AGENTBOX_HOME"/queue/*.req | tr '\n' ' ' | sed 's/ $//')"
assert_no_file "no drop directory was made for it" "$AGENTBOX_HOME/drop/m1"
assert_status "the runner, without Docker" 0 "$BOX" runner --once
RESULT="$(last_result m1 test)"
assert_file "there is a result" "$RESULT"
assert_eq "for that commit, passed" "$HEAD_SHA 0 false" "$(jq -r '"\(.sha) \(.exit_code) \(.cancelled)"' "$RESULT")"
assert_contains "the command ran in the folder of the project, as the person" "test of m1 in $W/sub" "$(cat "${RESULT%.json}.log")"
assert_eq "the queue is empty" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c '\.req$')"
assert_eq "the test counts are read from its output" "5 2" "$(jq -r '"\(.tests.ran) \(.tests.skipped)"' "$RESULT")"
assert_eq "a commit that touches no check and no test" "false [] 0" "$(jq -r '"\(.changesChecks) \(.checksTouched | tostring) \(.testsTouched.modified)"' "$RESULT")"
assert_eq "facts show it as the result of the head" "0 true" "$("$BOX" facts m1 --json | jq -r '.results.test | "\(.exitCode) \(.isHead)"')"
assert_eq "no container was started" "0" "$(grep -c -e '^run ' -e '^volume create' -e '^network create' "$FAKE_DOCKER_LOG")"

echo " what the commit changes is computed with plain git against the base"
echo '{"changed":1}' >"$W/sub/package.json"
echo "edited" >>"$W/sub/existing.spec.ts"
git -C "$W" rm -q sub/other.spec.ts
echo "new" >"$W/sub/new.spec.ts"
commit_in "$W" "touch the checks and the tests"
"$BOX" verify m1 test --no-wait >/dev/null
"$BOX" runner --once >/dev/null 2>&1
RESULT="$(last_result m1 test)"
assert_eq "a protected path was touched" 'true ["sub/package.json"]' "$(jq -r '"\(.changesChecks) \(.checksTouched | tostring)"' "$RESULT")"
assert_eq "one test file modified, one deleted, the new one does not count" '{"modified":1,"deleted":1,"files":["sub/existing.spec.ts","sub/other.spec.ts"]}' "$(jq -c .testsTouched "$RESULT")"

echo " a check is for a commit: uncommitted changes are refused"
echo "loose" >"$W/sub/loose.txt"
assert_status "at box verify" 1 "$BOX" verify m1 test --no-wait
assert_contains "it says so" "commit first: a check is for a commit" "$OUTPUT"
assert_eq "nothing was queued" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c '\.req$')"
rm "$W/sub/loose.txt"
"$BOX" verify m1 mark --no-wait >/dev/null
echo "loose again" >"$W/sub/loose.txt"
rm -f "$TMP/order"
"$BOX" runner --once >/dev/null 2>&1
RESULT="$(last_result m1 mark)"
assert_eq "at the runner: the result is a refusal" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "that says why" "commit first: a check is for a commit" "$(cat "${RESULT%.json}.log")"
assert_no_file "and the command did not run" "$TMP/order"
rm "$W/sub/loose.txt"
"$BOX" verify m1 mark --no-wait >/dev/null
echo "moved" >"$W/sub/moved.txt"
commit_in "$W" "the head moves after the request"
"$BOX" runner --once >/dev/null 2>&1
RESULT="$(last_result m1 mark)"
assert_eq "a worktree that moved on since the request is refused too" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "it says so" "not at the commit that was requested" "$(cat "${RESULT%.json}.log")"
assert_no_file "and the command did not run" "$TMP/order"

echo " the exit code of the command is the result"
"$BOX" verify m1 build --no-wait >/dev/null
"$BOX" runner --once >/dev/null 2>&1
assert_eq "a failing check" "3" "$(jq -r .exit_code "$(last_result m1 build)")"
assert_status "a verb the project does not name" 1 "$BOX" verify m1 lint --no-wait

echo " the deadline ends the command and everything it started"
"$BOX" verify m1 slow --no-wait >/dev/null
BOX_VERB_TIMEOUT=2 "$BOX" runner --once >/dev/null 2>&1
RESULT="$(last_result m1 slow)"
assert_eq "exit 124, timed out" "124 true" "$(jq -r '"\(.exit_code) \(.timed_out)"' "$RESULT")"
sleep 1
assert_eq "nothing of it is left" "0 0" "$(still_alive "$(cat "$TMP/slow.pid")") $(still_alive "$(cat "$TMP/slow.child")")"

echo " cancel ends a running check, and the result is final"
rm -f "$TMP/slow.pid" "$TMP/slow.child"
echo "again" >"$W/sub/again.txt"
commit_in "$W" "again"
"$BOX" verify m1 slow --no-wait >/dev/null
"$BOX" runner --once >"$TMP/runner.out" 2>&1 &
RUNNER=$!
for _ in $(seq 1 100); do [ -s "$TMP/slow.child" ] && break; sleep 0.2; done
assert_eq "(the check runs)" "1" "$(is_alive "$(cat "$TMP/slow.child")")"
assert_status "box cancel" 0 "$BOX" cancel m1 slow
wait "$RUNNER"
RESULT="$(last_result m1 slow)"
assert_eq "the result is cancelled" "true 130" "$(jq -r '"\(.cancelled) \(.exit_code)"' "$RESULT")"
sleep 1
assert_eq "nothing of it is left" "0 0" "$(still_alive "$(cat "$TMP/slow.pid")") $(still_alive "$(cat "$TMP/slow.child")")"
assert_eq "the queue is empty" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c '\.req$')"
assert_status "nothing more to cancel" 78 "$BOX" cancel m1 slow

echo " stopping the runner ends the check that runs; the request waits for the next runner"
rm -f "$TMP/slow.pid" "$TMP/slow.child"
echo "once more" >"$W/sub/once-more.txt"
commit_in "$W" "once more"
"$BOX" verify m1 slow --no-wait >/dev/null
assert_status "the runner daemon starts without Docker" 0 "$BOX" runner --daemon
for _ in $(seq 1 100); do [ -s "$TMP/slow.child" ] && break; sleep 0.2; done
assert_eq "(the check runs)" "1" "$(is_alive "$(cat "$TMP/slow.child")")"
assert_status "and stops without Docker" 0 "$BOX" runner --stop
sleep 1
assert_eq "nothing of the check is left" "0 0" "$(still_alive "$(cat "$TMP/slow.pid")") $(still_alive "$(cat "$TMP/slow.child")")"
assert_eq "its request is still in the queue" "1" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c 'slow.m1.req$')"
BOX_VERB_TIMEOUT=2 "$BOX" runner --once >/dev/null 2>&1
assert_eq "the next runner runs it again, to its end" "124" "$(jq -r .exit_code "$(last_result m1 slow)")"
sleep 1
assert_eq "and leaves nothing either" "0" "$(still_alive "$(cat "$TMP/slow.child")")"

echo " one check at a time, across all lavori"
"$BOX" new m2 app >/dev/null
rm -f "$TMP/order"
"$BOX" verify m1 mark --no-wait >/dev/null
"$BOX" verify m2 mark --no-wait >/dev/null
"$BOX" runner --once >/dev/null 2>&1
assert_eq "the second starts after the first ended" "start m1,end m1,start m2,end m2" "$(tr '\n' ',' <"$TMP/order" | sed 's/,$//')"

echo " box verify waits for the result and ends with the exit code of the check"
assert_status "without a runner" 75 "$BOX" verify m1 build
assert_contains "it says so" "the runner is not running" "$OUTPUT"
"$BOX" runner >"$TMP/runner.out" 2>&1 &
RUNNER=$!
for _ in $(seq 1 50); do [ -f "$AGENTBOX_HOME/locks/runner.lock" ] && break; sleep 0.2; done
sleep 1
OUT="$("$BOX" verify m1 build 2>/dev/null)"
assert_eq "the exit code of the check" "3" "$?"
assert_contains "the end of its output" "building" "$OUT"
assert_contains "and the line that sums it up" "verb=build sha=$(git -C "$W" rev-parse HEAD) exit=3" "$OUT"
kill "$RUNNER" 2>/dev/null
wait "$RUNNER" 2>/dev/null

echo " a task in a box waits for Docker; the lavori on the Mac do not"
unset FAKE_DOCKER_DOWN
"$BOX" new b1 sandbox --where box >/dev/null 2>&1
commit_in_task b1 work
"$BOX" verify b1 test --no-wait >/dev/null 2>&1
export FAKE_DOCKER_DOWN=1
"$BOX" verify m2 mark --no-wait >/dev/null
"$BOX" runner --once >"$TMP/runner.out" 2>&1
assert_eq "the runner ends" "0" "$?"
assert_eq "the check of the lavoro on the Mac ran" "0" "$(jq -r .exit_code "$(last_result m2 mark)")"
assert_eq "the request of the box stays queued" "b1 queued" "$(sed -n 's/^task=//p; s/^state=//p' "$AGENTBOX_HOME"/queue/*.req | tr '\n' ' ' | sed 's/ $//')"
assert_contains "and one line says why" "Docker is not reachable" "$(cat "$TMP/runner.out")"
finish

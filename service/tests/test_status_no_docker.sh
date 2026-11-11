#!/bin/bash
# With only lavori on the Mac nothing asks Docker for anything: not box status in either form,
# not box tasks. The scan for volumes left by tasks in a box runs only when a task lives in a box.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --install true >/dev/null
"$BOX" new m1 app >/dev/null

echo " only lavori on the Mac"
: >"$FAKE_DOCKER_LOG"
STARTED=$SECONDS
OUT="$(FAKE_SLOW_CALL="volume ls" FAKE_SLOW_SECONDS=20 "$BOX" status 2>&1)"
assert_eq "box status works" "0" "$?"
assert_contains "and lists the lavoro" "m1" "$OUT"
assert_eq "without waiting for Docker (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 15 ] && echo 1 || echo 0)"
"$BOX" status --json >/dev/null
"$BOX" tasks --json >/dev/null
"$BOX" facts m1 --json >/dev/null
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
"$BOX" runner --once >/dev/null 2>&1
assert_eq "nor by a runner that starts" "" "$(cat "$FAKE_DOCKER_LOG")"

echo " once a task lives in a box"
"$BOX" new b1 app --where box >/dev/null
: >"$FAKE_DOCKER_LOG"
"$BOX" status >/dev/null 2>&1
assert_eq "box status looks for the volumes tasks in a box left behind" "1" "$(grep -c -e '^volume ls .*agentbox-nm-' "$FAKE_DOCKER_LOG")"
finish

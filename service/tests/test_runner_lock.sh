#!/bin/bash
# Only one runner at a time; the lock goes away with its holder.
. "$(dirname "$0")/lib.sh"
setup_sandbox

"$BOX" runner 2>/dev/null &
FIRST_PID=$!
sleep 2
assert_status "a second runner refuses to start" 1 "$BOX" runner --once
assert_contains "  message says why" "another runner is already running" "$OUTPUT"
assert_contains "status reports the runner" "runner: running" "$("$BOX" status 2>&1)"
kill "$FIRST_PID"
wait "$FIRST_PID" 2>/dev/null
sleep 1
assert_status "the lock is free after the runner dies" 0 "$BOX" runner --once
assert_contains "status reports it stopped" "runner: stopped" "$("$BOX" status 2>&1)"
finish

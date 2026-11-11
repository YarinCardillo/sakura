#!/bin/bash
# box tasks --json: the cheap inventory, read from host-private files only, no container started.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new b2 sandbox >/dev/null
"$BOX" new a1 sandbox >/dev/null
commit_in_task a1 work
"$BOX" take a1 >/dev/null 2>&1
: >"$FAKE_DOCKER_LOG"

echo " tasks --json"
START="$(date +%s)"
OUT="$("$BOX" tasks --json 2>"$TMP/tasks.err")"; STATUS=$?
assert_eq "exit 0" "0" "$STATUS"
assert_eq "valid JSON array" "array" "$(jq -r type <<<"$OUT")"
assert_eq "same order as box status" "$("$BOX" status --json | jq -c '[.tasks[].name]')" "$(jq -c '[.[].name]' <<<"$OUT")"
assert_eq "name and profile and base" "a1 sandbox main" "$(jq -r '.[0] | "\(.name) \(.profile) \(.base)"' <<<"$OUT")"
assert_eq "a taken task says so" "true" "$(jq -r '.[0].taken' <<<"$OUT")"
assert_eq "an untaken task says so" "false" "$(jq -r '.[1].taken' <<<"$OUT")"
assert_eq "only the five documented fields" '["base","name","profile","taken","where"]' "$(jq -c '.[0] | keys' <<<"$OUT")"

echo " no container is started"
: >"$FAKE_DOCKER_LOG"
"$BOX" tasks --json >/dev/null 2>&1
assert_eq "docker was not called" "0" "$(wc -c <"$FAKE_DOCKER_LOG" | tr -d ' ')"

echo " fast"
START="$(date +%s)"
"$BOX" tasks --json >/dev/null 2>&1
assert_eq "under two seconds (the clock has a resolution of one)" "1" "$([ $(( $(date +%s) - START )) -lt 2 ] && echo 1 || echo 0)"

echo " no tasks"
"$BOX" rm a1 --force >/dev/null 2>&1
"$BOX" rm b2 --force >/dev/null 2>&1
assert_eq "empty array" "[]" "$("$BOX" tasks --json)"
assert_status "without --json it refuses" 1 "$BOX" tasks
finish

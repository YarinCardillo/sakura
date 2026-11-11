#!/bin/bash
# box status --json reads the workspaces concurrently (at most four at a time), same output order.
. "$(dirname "$0")/lib.sh"
setup_sandbox
for task in t1 t2 t3 t4 t5; do "$BOX" new "$task" sandbox >/dev/null; done
export FAKE_ACTIVE_DIR="$TMP/active" FAKE_ACTIVE_LOG="$TMP/active.log"
mkdir -p "$FAKE_ACTIVE_DIR"

echo " the reference output, read one task at a time"
EXPECTED="$("$BOX" status --json 2>/dev/null | jq -c '.tasks')"

echo " five slow workspace reads"
export FAKE_FACTS_DELAY=2
: >"$FAKE_ACTIVE_LOG"
START="$(date +%s)"
OUT="$("$BOX" status --json --fresh 2>/dev/null)"
ELAPSED=$(( $(date +%s) - START ))
assert_eq "valid JSON" "0" "$(jq -e . <<<"$OUT" >/dev/null 2>&1; echo $?)"
assert_eq "same order and same content as before" "$EXPECTED" "$(jq -c '.tasks' <<<"$OUT")"
assert_eq "task order" '["t1","t2","t3","t4","t5"]' "$(jq -c '[.tasks[].name]' <<<"$OUT")"
assert_eq "faster than one after the other (10 s)" "1" "$([ "$ELAPSED" -lt 8 ] && echo 1 || echo 0)"
assert_eq "reads overlapped" "1" "$([ "$(sort -n "$FAKE_ACTIVE_LOG" | tail -1)" -ge 2 ] && echo 1 || echo 0)"
assert_eq "never more than four at once" "1" "$([ "$(sort -n "$FAKE_ACTIVE_LOG" | tail -1)" -le 4 ] && echo 1 || echo 0)"
assert_eq "every task was read once" "5" "$(wc -l <"$FAKE_ACTIVE_LOG" | tr -d ' ')"

echo " an unreadable workspace does not stop the others"
echo garbage >"$AGENTBOX_HOME/work/t3/.git/HEAD"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "five tasks reported" "5" "$(jq '.tasks | length' <<<"$OUT")"
assert_eq "t3 unreadable" "false" "$(jq -r '.tasks[2].readable' <<<"$OUT")"
assert_eq "t4 readable" "true" "$(jq -r '.tasks[3].readable' <<<"$OUT")"
finish

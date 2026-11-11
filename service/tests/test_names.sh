#!/bin/bash
# Task, account and verb names are validated before anything is created.
. "$(dirname "$0")/lib.sh"
setup_sandbox

for bad in "" "A" "a_b" "a b" "-a" "a-" "a--b" "../x" "a/b" 'a$b'; do
  assert_status "new rejects task name '$bad'" 1 "$BOX" new "$bad" sandbox
  assert_contains "  message names the rule" "invalid task name" "$OUTPUT"
done
for good in a a1 m1 fix-login-2; do
  assert_status "new accepts task name '$good'" 0 "$BOX" new "$good" sandbox
done
assert_status "login rejects account 'Work'" 1 "$BOX" login Work
assert_contains "  message names the rule" "invalid account name" "$OUTPUT"
assert_status "verify rejects verb 'Test'" 2 env BOX_TASK=a BOX_WORKSPACE="$AGENTBOX_HOME/work/a" "$BOX_ROOT/image/verify" Test
assert_status "verify rejects verb 'a b'" 2 env BOX_TASK=a BOX_WORKSPACE="$AGENTBOX_HOME/work/a" "$BOX_ROOT/image/verify" "a b"
assert_status "new rejects an unknown profile" 1 "$BOX" new zz nope
assert_status "new refuses an existing task" 1 "$BOX" new a sandbox
finish

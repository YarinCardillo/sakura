#!/bin/bash
# A task above its measured disk budget cannot start more work or import its branch.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 budget
printf 'DISK_BUDGET_MB=1\n' >>"$AGENTBOX_HOME/tasks/t1.project"
dd if=/dev/zero of="$AGENTBOX_HOME/work/t1/big" bs=1048576 count=2 2>/dev/null
assert_status 'run refuses excess storage' 1 "$BOX" run t1 -- true
assert_contains 'refusal names the disk budget' 'disk budget' "$OUTPUT"
assert_status 'agent refuses excess storage' 1 "$BOX" agent t1 main --detach
assert_status 'ACP refuses excess storage' 1 "$BOX" acp t1 main
assert_status 'take refuses excess storage' 1 "$BOX" take t1
assert_no_file 'refused take creates no branch' "$SOURCE/.git/refs/heads/box/t1"
drop_request t1 test
"$BOX" runner --once >/dev/null 2>&1
RESULT="$(find "$AGENTBOX_HOME/results/t1" -name '*.json' | head -1)"
assert_eq 'check refused before running' 1 "$(jq -r '.exit_code' "$RESULT")"
assert_eq 'no job was started' 0 "$(grep -c -e 'agentbox.kind=job' "$FAKE_DOCKER_LOG")"
FACTS="$("$BOX" facts t1 --json)"
assert_eq 'facts show the limit in bytes' 1048576 "$(jq -r '.disk.limitBytes' <<<"$FACTS")"
assert_eq 'facts show that it is over budget' true "$(jq -r '.disk.overBudget' <<<"$FACTS")"
assert_eq 'facts describe detection' detection "$(jq -r '.disk.enforcement' <<<"$FACTS")"
assert_eq 'facts measure stored bytes' true "$(jq -r '.disk.bytes >= 2097152' <<<"$FACTS")"
rm "$AGENTBOX_HOME/work/t1/big"
assert_status 'work below budget can start' 0 "$BOX" run t1 -- true
export FAKE_VOLUME_ROOT="$TMP/volumes"
for volume in agentbox-npm-cache-t1 agentbox-nm-t1 agentbox-cfg-t1.main agentbox-pristine.t1.abcdef; do
  mkdir -p "$FAKE_VOLUME_ROOT/$volume"
  dd if=/dev/zero of="$FAKE_VOLUME_ROOT/$volume/big" bs=1048576 count=2 2>/dev/null
  assert_status "$volume is counted in the task budget" 1 "$BOX" run t1 -- true
  rm "$FAKE_VOLUME_ROOT/$volume/big"
done
mkdir -p "$FAKE_VOLUME_ROOT/agentbox-npm-cache-other"
dd if=/dev/zero of="$FAKE_VOLUME_ROOT/agentbox-npm-cache-other/big" bs=1048576 count=2 2>/dev/null
assert_status 'another task does not consume this budget' 0 "$BOX" run t1 -- true
dd if=/dev/zero of="$AGENTBOX_HOME/drop/t1/payload" bs=1048576 count=2 2>/dev/null
"$BOX" runner --once >/dev/null 2>&1
assert_no_file 'the rejected payload moved out of the drop channel' "$AGENTBOX_HOME/drop/t1/payload"
assert_status 'moving payload into quarantine cannot evade the budget' 1 "$BOX" run t1 -- true
assert_eq 'facts count retained quarantine bytes' true "$("$BOX" facts t1 --json | jq -r '.disk.overBudget')"
rm -rf "$AGENTBOX_HOME/quarantine"/drop-t1.*
assert_status 'removing quarantined data restores room' 0 "$BOX" run t1 -- true
QUARANTINE="$AGENTBOX_HOME/quarantine/request-20261111T000000-$(workspace_head t1)-test.t1.fixture"
mkdir -p "$QUARANTINE"
dd if=/dev/zero of="$QUARANTINE/request" bs=1048576 count=2 2>/dev/null
assert_status 'request quarantine belongs to the same task budget' 1 "$BOX" run t1 -- true
rm -rf "$QUARANTINE"
mkdir -p "$AGENTBOX_HOME/quarantine/drop-t1-other.fixture"
dd if=/dev/zero of="$AGENTBOX_HOME/quarantine/drop-t1-other.fixture/entry" bs=1048576 count=2 2>/dev/null
assert_status 'another task quarantine does not consume this budget' 0 "$BOX" run t1 -- true
assert_status 'the project option accepts a disk budget' 0 "$BOX" project set sandbox --disk-budget-mb 64
assert_status 'invalid disk budgets are refused' 1 "$BOX" project set sandbox --disk-budget-mb 0
finish

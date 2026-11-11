#!/bin/bash
# box rm refuses to lose work; box take makes it safe to remove.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null

assert_status "rm of a clean, empty task works" 0 "$BOX" rm t1
assert_no_file "workspace is gone" "$AGENTBOX_HOME/work/t1"
assert_no_file "profile record is gone" "$AGENTBOX_HOME/tasks/t1.env"

"$BOX" new t2 sandbox >/dev/null
commit_in_task t2 work
assert_status "rm refuses commits that were not taken" 1 "$BOX" rm t2
assert_contains "  message says what to do" "box take t2" "$OUTPUT"
assert_file "  workspace is still there" "$AGENTBOX_HOME/work/t2/change.txt"

"$BOX" take t2 >/dev/null 2>&1
assert_eq "take creates the branch in the source repository" "box/t2" "$(git -C "$SOURCE" branch --list 'box/t2' --format '%(refname:short)')"
assert_status "rm works once taken" 0 "$BOX" rm t2

"$BOX" new t3 sandbox >/dev/null
echo scratch >"$AGENTBOX_HOME/work/t3/untracked.txt"
assert_status "rm refuses dirty files" 1 "$BOX" rm t3
assert_contains "  message counts the files" "1 uncommitted" "$OUTPUT"
assert_status "rm --force discards" 0 "$BOX" rm t3 --force

"$BOX" new t4 sandbox >/dev/null
commit_in_task t4 more
"$BOX" take t4 >/dev/null 2>&1
commit_in_task t4 later
assert_status "rm refuses commits made after the last take" 1 "$BOX" rm t4

"$BOX" new t5 sandbox >/dev/null
rm -rf "$AGENTBOX_HOME/work/t5"
assert_status "rm cleans up a workspace deleted by hand" 0 "$BOX" rm t5
finish

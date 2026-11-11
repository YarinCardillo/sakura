#!/bin/bash
# box take brings the commit it read and reports: when the agent moves the branch between the
# read of the workspace and the making of the bundle, the other commit is not imported under
# the first one's name.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
commit_in_task t1 "commit A"
A="$(workspace_head t1)"
commit_in_task t1 "commit B"
B="$(workspace_head t1)"
git -C "$W" reset -q --hard "$A"

echo " the branch moves to another commit right after the workspace was read"
export FAKE_AFTER_FACTS="git -C \"$W\" reset -q --hard $B"
OUT="$("$BOX" take t1 2>/dev/null)"
unset FAKE_AFTER_FACTS
assert_eq "(the agent did move the branch)" "$B" "$(workspace_head t1)"
assert_eq "take reports the commit it read" "$A" "$(tail -n 1 <<<"$OUT")"
assert_eq "and that is the commit in the source repository" "$A" "$(git -C "$SOURCE" rev-parse refs/heads/box/t1)"
assert_eq "the journal names the same commit" "$A" "$("$BOX" journal --json | jq -r '[.[] | select(.event == "take")][-1].sha')"
assert_eq "the other commit did not arrive" "absent" "$(git -C "$SOURCE" cat-file -e "$B^{commit}" 2>/dev/null && echo present || echo absent)"

echo " a later take brings the commit the branch moved to"
OUT="$("$BOX" take t1 2>/dev/null)"
assert_eq "reported" "$B" "$(tail -n 1 <<<"$OUT")"
assert_eq "and imported" "$B" "$(git -C "$SOURCE" rev-parse refs/heads/box/t1)"

echo " take does not report a commit the source repository does not have"
STATUS="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project sandbox journal task; do . "$BOX_ROOT/lib/$lib.sh"; done
  load_task_project t1
  (require_taken_commit t1 "'"$A"'") >/dev/null 2>&1; echo $?')"
assert_eq "a branch that is not at the commit that was read is an error" "1" "$STATUS"
finish

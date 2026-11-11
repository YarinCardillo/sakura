#!/bin/bash
# Every result of a check says where it ran: "mac" for a check run natively in the worktree of
# a lavoro on the Mac (it proves that the command passed on that commit, not that the agent,
# which runs as the person, could not have helped it), "box" for one run in a job container. The
# result file carries it, and so does what box facts and box status show of the last result.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --install true --check "test=echo ok" >/dev/null
"$BOX" new m1 app >/dev/null
"$BOX" new b1 app --where box >/dev/null
for task in m1 b1; do
  echo "work" >"$AGENTBOX_HOME/work/$task/sub/work.txt"
  git -C "$AGENTBOX_HOME/work/$task" add -A
  git -C "$AGENTBOX_HOME/work/$task" -c user.name=t -c user.email=t@example.com commit -q -m "work in $task"
  "$BOX" verify "$task" test --no-wait >/dev/null 2>&1
done
"$BOX" runner --once >/dev/null 2>&1

for pair in m1:mac b1:box; do
  task="${pair%%:*}" place="${pair#*:}"
  RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/$task"/*-test.json | head -1)"
  echo " a check of a task of the place $place"
  assert_eq "(it ran)" "0" "$(jq -r .exit_code "$RESULT")"
  assert_eq "the result file says where it ran" "$place" "$(jq -r .where "$RESULT")"
  assert_eq "box facts shows it with the last result" "$place" "$("$BOX" facts "$task" --json | jq -r '.results.test.where')"
  assert_eq "box status too" "$place" "$("$BOX" status --json | jq -r --arg task "$task" '.tasks[] | select(.name == $task) | .results.test.where')"
done

echo " a cancelled result says it too"
echo "more" >>"$AGENTBOX_HOME/work/m1/sub/work.txt"
git -C "$AGENTBOX_HOME/work/m1" add -A && git -C "$AGENTBOX_HOME/work/m1" -c user.name=t -c user.email=t@example.com commit -q -m "more"
"$BOX" verify m1 test --no-wait >/dev/null 2>&1
"$BOX" cancel m1 test >/dev/null 2>&1
assert_eq "cancelled" "true mac" "$(jq -r '"\(.cancelled) \(.where)"' "$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-test.json | head -1)")"

echo " and a refused one: the worktree is not at a clean commit when the runner comes to the request"
sleep 1  # (a later second than the cancelled result: the newest result is found by its name)
echo "again" >>"$AGENTBOX_HOME/work/m1/sub/work.txt"
git -C "$AGENTBOX_HOME/work/m1" add -A && git -C "$AGENTBOX_HOME/work/m1" -c user.name=t -c user.email=t@example.com commit -q -m "again"
"$BOX" verify m1 test --no-wait >/dev/null 2>&1
echo "loose" >"$AGENTBOX_HOME/work/m1/sub/loose.txt"
"$BOX" runner --once >/dev/null 2>&1
rm "$AGENTBOX_HOME/work/m1/sub/loose.txt"
RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-test.json | head -1)"
assert_eq "(it was refused: exit code 2, not cancelled)" "2 false" "$(jq -r '"\(.exit_code) \(.cancelled)"' "$RESULT")"
assert_eq "the result file says where" "mac" "$(jq -r .where "$RESULT")"
assert_eq "box facts shows it with the refused result" "2 mac" "$("$BOX" facts m1 --json | jq -r '"\(.results.test.exitCode) \(.results.test.where)"')"

echo " a result written before results said where they ran was run in a box"
RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/b1"/*-test.json | head -1)"
grep -v '"where"' "$RESULT" >"$TMP/old.json" && cat "$TMP/old.json" >"$RESULT"
assert_eq "(the file has no place)" "null" "$(jq -r .where "$RESULT")"
assert_eq "box facts shows it as box" "box" "$("$BOX" facts b1 --json | jq -r '.results.test.where')"
finish

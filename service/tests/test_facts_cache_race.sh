#!/bin/bash
# The facts cache needs .git/HEAD and the branch ref of a workspace. They are read inside a
# workspace container, never on the host, and even there through checked descriptors: the agent
# can swap them, or a directory on the way, for a symbolic link at any moment. The read must
# never give content from outside the workspace, and the cache must never serve an answer that
# is no longer true.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
GIT_DIR_OF_TASK="$W/.git"
readonly KEY_READS=150
readonly FACTS_CALLS=8

OUTSIDE="$TMP/outside"
OUTSIDE_SHA="$(printf 'e%.0s' {1..40})"
mkdir -p "$OUTSIDE/refs/heads/box"
printf '%s\n' "$OUTSIDE_SHA" >"$OUTSIDE/refs/heads/box/t1"
printf 'ref: refs/heads/box/t1\n' >"$OUTSIDE/HEAD"

# Swaps the ref directory and HEAD for links to their look-alikes outside, and back, until told to stop.
swap_until_stopped() {
  local box_dir="$GIT_DIR_OF_TASK/refs/heads/box" head="$GIT_DIR_OF_TASK/HEAD"
  while [ -e "$TMP/swapping" ]; do
    mv "$box_dir" "$box_dir.real" && ln -s "$OUTSIDE/refs/heads/box" "$box_dir"
    rm -f "$box_dir" && mv "$box_dir.real" "$box_dir"
    mv "$head" "$head.real" && ln -s "$OUTSIDE/HEAD" "$head"
    rm -f "$head" && mv "$head.real" "$head"
    echo swapped >>"$TMP/swaps"
  done
}

# Prints the key of the workspace KEY_READS times ("refused" when there is none), as the
# workspace container computes it.
read_keys() {
  local read
  for ((read = 0; read < KEY_READS; read++)); do
    python3 "$BOX_ROOT/image/runner/workspace_key.py" "$W" t1 2>/dev/null || echo refused
  done
}

echo " (warm cache at the first head)"
commit_in_task t1 one
OLD_HEAD="$(workspace_head t1)"
"$BOX" facts t1 --json >/dev/null 2>&1
: >"$FAKE_DOCKER_LOG"
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "the cache answers" "0" "$(grep -c -e '/box/commit_facts.py' "$FAKE_DOCKER_LOG")"
commit_in_task t1 two
NEW_HEAD="$(workspace_head t1)"

echo " while HEAD and the ref directory are swapped for links to files outside the workspace"
touch "$TMP/swapping"
swap_until_stopped 2>/dev/null &
SWAPPER=$!
KEYS="$(read_keys)"
ANSWERS=""
for _ in $(seq 1 "$FACTS_CALLS"); do
  ANSWERS="$ANSWERS$("$BOX" facts t1 --json 2>/dev/null | jq -r '.head')"$'\n'
done
rm -f "$TMP/swapping"
wait "$SWAPPER"
assert_eq "(the swapper swapped many times)" "1" "$([ "$(wc -l <"$TMP/swaps")" -gt 20 ] && echo 1 || echo 0)"
assert_eq "(every read gave a key or a refusal)" "$KEY_READS" "$(grep -c . <<<"$KEYS")"
assert_eq "no key holds the commit written outside the workspace" "0" "$(grep -c "$OUTSIDE_SHA" <<<"$KEYS")"
assert_eq "every key that was given holds the real head" "0" "$(grep -v '^refused$' <<<"$KEYS" | grep -vc "head=$NEW_HEAD")"
assert_eq "box facts never showed the commit from outside" "0" "$(grep -c "$OUTSIDE_SHA" <<<"$ANSWERS")"
assert_eq "box facts never served the head it had cached before the commit" "0" "$(grep -c "$OLD_HEAD" <<<"$ANSWERS")"

echo " afterwards the workspace is read and cached as usual"
assert_eq "the real head" "$NEW_HEAD" "$("$BOX" facts t1 --json --fresh 2>/dev/null | jq -r .head)"
: >"$FAKE_DOCKER_LOG"
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "served from the cache again" "0" "$(grep -c -e '/box/commit_facts.py' "$FAKE_DOCKER_LOG")"

echo " the host reads no path of a workspace for the cache"
assert_eq "no stat, cat, find or python of the host touches a workspace in the cache code" "0" \
  "$(grep -c -e 'task_dir' -e 'python3' -e 'find ' "$BOX_ROOT/lib/factscache.sh")"
assert_no_file "the helper is not a program of the host any more" "$BOX_ROOT/lib/workspace_key.py"
assert_contains "the key is computed by the workspace container" "workspace_key" "$(cat "$BOX_ROOT/image/runner/facts.py")"
finish

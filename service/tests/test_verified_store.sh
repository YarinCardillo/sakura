#!/bin/bash
# git that reads the untrusted workspace can be made to run a program of the agent's choosing
# (a clean filter named by .git/config and .git/info/attributes runs during status and diff).
# Such a process must never reach the verified store: the workspace is read in containers that
# do not mount the store, the store is written only by a container that does not mount the
# workspace, and what decisions rest on is computed where the workspace is absent.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
commit_in_task t1 one

echo " a clean filter of the workspace cannot write into the verified store"
git -C "$W" config filter.evil.clean "sh -c 'for store in \"$AGENTBOX_HOME/verified/\"*; do touch \"\$store/poisoned\"; done; echo ran >>\"$TMP/filter-ran\"; cat'"
echo '* filter=evil' >"$W/.git/info/attributes"
echo "an edit that is not staged" >>"$W/change.txt"
: >"$FAKE_DOCKER_LOG"
"$BOX" facts t1 --json --fresh >/dev/null 2>&1
"$BOX" diff t1 >/dev/null 2>&1
"$BOX" status >/dev/null 2>&1
assert_file "(the filter ran while the workspace was read: the attack is real)" "$TMP/filter-ran"
assert_eq "nothing of the filter is in a verified store" "0" "$(find "$AGENTBOX_HOME/verified" -name poisoned | wc -l | tr -d ' ')"

echo " which container can reach what"
"$BOX" log t1 --json >/dev/null 2>&1
"$BOX" verify t1 build --no-wait 2>/dev/null
"$BOX" runner --once 2>/dev/null
"$BOX" take t1 >/dev/null 2>&1
RUNS="$(grep '^run ' "$FAKE_DOCKER_LOG")"
assert_eq "no container has both the workspace and the verified store" "0" "$(grep -F -e "$W:" <<<"$RUNS" | grep -F -c -e "$AGENTBOX_HOME/verified/t1.store:")"
assert_eq "the store is writable only where the pack is verified" "0" "$(grep -e ':/verified ' <<<"$RUNS" | grep -vc -e '/box/verify_import.py')"
assert_eq "and that container has no workspace" "0" "$(grep -e '/box/verify_import.py' <<<"$RUNS" | grep -c -e ':/src')"
for script in changes.py extract.sh bundle.sh checkout.sh log.py commit_facts.py; do
  assert_eq "$script ran" "1" "$([ "$(grep -c -e "/box/$script" <<<"$RUNS")" -ge 1 ] && echo 1 || echo 0)"
  assert_eq "  without the workspace" "0" "$(grep -e "/box/$script" <<<"$RUNS" | grep -c -e ':/src')"
done
assert_eq "the result of the check was computed" "false" "$(jq -r .changesChecks "$(/bin/ls -t "$AGENTBOX_HOME/results/t1"/*-build.json | head -1)")"
assert_eq "take brought the commit of the workspace" "$(git -C "$W" rev-parse HEAD)" "$(git -C "$SOURCE" rev-parse refs/heads/box/t1)"

echo " a store left by an older box is not used"
mkdir -p "$AGENTBOX_HOME/verified/t1.git/box-verified"
touch "$AGENTBOX_HOME/verified/t1.git/box-verified/$(printf 'f%.0s' {1..40})"
assert_eq "facts still come from the new store" "$(git -C "$W" rev-parse HEAD)" "$("$BOX" facts t1 --json --fresh 2>/dev/null | jq -r .head)"
"$BOX" rm t1 --force >/dev/null 2>&1
assert_eq "box rm removes both" "0" "$(/bin/ls "$AGENTBOX_HOME/verified" | grep -c '^t1\.')"
finish

#!/bin/bash
# box facts and box status read each workspace in full only when something changed. Whether
# something changed is asked of one workspace container that runs no git (the host itself reads
# no path of a workspace); --fresh bypasses the cache.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
"$BOX" new t2 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"

# reads: how many full reads of a workspace (git over the working tree, then the commit in the
# verified store) the docker log shows since the last call of this function. A cached answer
# costs one workspace container that only compares the key, and no full read.
reads() {
  local count
  count="$(grep -c -e '/box/commit_facts.py' "$FAKE_DOCKER_LOG")"
  : >"$FAKE_DOCKER_LOG"
  echo "$count"
}

echo " two calls in a row start one sandbox"
: >"$FAKE_DOCKER_LOG"
FIRST="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "the first call reads" "1" "$(reads)"
SECOND="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "the second call asks one workspace container and two isolated storage measurements" "3 1" "$(grep -c '^run ' "$FAKE_DOCKER_LOG") $(grep -c -e '/box/facts.py' "$FAKE_DOCKER_LOG")"
assert_eq "both storage trust domains are measured even when git facts are cached" "2" "$(grep -c -e "/box/storage.py" "$FAKE_DOCKER_LOG")"
assert_eq "the second call starts no sandbox" "0" "$(reads)"
assert_eq "and gives the same answer" "$FIRST" "$SECOND"

echo " box status starts no sandbox for unchanged tasks"
"$BOX" status --json >/dev/null 2>&1
assert_eq "only the task not read yet is read (t2)" "1" "$(reads)"
"$BOX" status --json >/dev/null 2>&1
assert_eq "nothing changed: none" "0" "$(reads)"

echo " a commit makes the next call read again"
commit_in_task t1 one
THIRD="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read after a commit" "1" "$(reads)"
assert_eq "the new head is shown" "$(workspace_head t1)" "$(jq -r .head <<<"$THIRD")"
assert_eq "the other task is not read" "0" "$("$BOX" facts t2 --json >/dev/null 2>&1; reads)"

echo " an edit that git has not staged is seen too"
echo more >>"$W/change.txt"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read after an unstaged edit" "1" "$(reads)"
assert_eq "dirty files updated" "1" "$(jq -r .dirtyFiles <<<"$OUT")"
echo new >"$W/untracked.txt"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read after a new untracked file" "1" "$(reads)"
assert_eq "dirty files counts it" "2" "$(jq -r .dirtyFiles <<<"$OUT")"
rm "$W/untracked.txt"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read after a deleted file" "1" "$(reads)"
assert_eq "dirty files back" "1" "$(jq -r .dirtyFiles <<<"$OUT")"
git -C "$W" checkout -q -- change.txt
"$BOX" facts t1 --json >/dev/null 2>&1; reads >/dev/null

echo " a staged change and a branch move are seen"
echo staged >>"$W/change.txt"; git -C "$W" add change.txt
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "re-read after git add" "1" "$(reads)"
git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m staged
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "re-read after the commit" "1" "$(reads)"
git -C "$W" -c user.name=t -c user.email=t@example.com commit -q --amend -m amended
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "re-read after an amend (the branch ref moved)" "1" "$(reads)"

echo " --fresh bypasses the cache"
"$BOX" facts t1 --json --fresh >/dev/null 2>&1
assert_eq "facts --fresh reads" "1" "$(reads)"
"$BOX" status --json --fresh >/dev/null 2>&1
assert_eq "status --fresh reads every task" "2" "$(reads)"
"$BOX" status --json >/dev/null 2>&1
assert_eq "and the next plain call uses the refreshed cache" "0" "$(reads)"

echo " a cache entry that is too old is not trusted"
BOX_FACTS_CACHE_MAX_AGE=0 "$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "max age 0 always reads" "1" "$(reads)"

echo " a damaged cache entry is ignored"
echo 'not json' >"$AGENTBOX_HOME/cache/facts/t1.json"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read" "1" "$(reads)"
assert_eq "valid answer" "t1" "$(jq -r .name <<<"$OUT")"


echo " the agent cannot set the key: modification times are not trusted"
"$BOX" facts t1 --json --fresh >/dev/null 2>&1; reads >/dev/null
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "(warm: a hit)" "0" "$(reads)"
commit_in_task t1 backdate
touch -t 200001010000 "$W/.git/index" "$W/.git/HEAD" "$W/.git/refs/heads/box/t1" "$W/.git/refs/heads/box" "$W/.git" "$W" "$W/change.txt"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "a commit with every time set back is still read" "1" "$(reads)"
assert_eq "and the new head is shown" "$(workspace_head t1)" "$(jq -r .head <<<"$OUT")"
echo edited >>"$W/change.txt"
"$BOX" facts t1 --json >/dev/null 2>&1; reads >/dev/null
echo "different content, same size" >"$W/change.txt"
touch -t 200001010000 "$W/change.txt" "$W"
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "an edit rewritten in place with its time set back is seen" "1" "$(reads)"
git -C "$W" checkout -q -- change.txt
"$BOX" facts t1 --json >/dev/null 2>&1; reads >/dev/null

echo " rewriting the branch to another commit invalidates, whatever the size and time"
OLD_HEAD="$(workspace_head t1)"
PARENT="$(git -C "$W" rev-parse HEAD~1)"
REF="$W/.git/refs/heads/box/t1"
cp -p "$REF" "$TMP/ref.stamp"
printf '%s\n' "$PARENT" >"$REF"
touch -r "$TMP/ref.stamp" "$REF"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "re-read" "1" "$(reads)"
assert_eq "the head follows the branch" "$PARENT" "$(jq -r .head <<<"$OUT")"
printf '%s\n' "$OLD_HEAD" >"$REF"

echo " a cached answer whose head is not the branch is never served"
"$BOX" facts t1 --json --fresh >/dev/null 2>&1; reads >/dev/null
jq '.head = "'"$(printf 'f%.0s' {1..40})"'"' "$AGENTBOX_HOME/cache/facts/t1.json" >"$TMP/forged.json"
touch -r "$AGENTBOX_HOME/cache/facts/t1.json" "$TMP/forged.json"; cp -p "$TMP/forged.json" "$AGENTBOX_HOME/cache/facts/t1.json"
OUT="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "the forged entry is not used (a sandbox read)" "1" "$(reads)"
assert_eq "the real head is shown" "$(workspace_head t1)" "$(jq -r .head <<<"$OUT")"

echo " the key is refused for anything unexpected"
KEYTEST="$TMP/keytest"
# The key of the fixture: the workspace's part, as the workspace container computes it, and the
# host's part. Nothing when the workspace has no key.
key_for() {
  local workspace
  workspace="$(python3 "$BOX_ROOT/image/runner/workspace_key.py" "$KEYTEST/work/kt" kt 2>/dev/null)" || return 0
  echo "$workspace $(AGENTBOX_HOME="$KEYTEST" bash -c '
    for lib in common names docker project sandbox journal task request checks factscache session; do . "'"$BOX_ROOT"'/lib/$lib.sh"; done
    facts_host_key kt' 2>/dev/null)"
}
fixture() {
  rm -rf "$KEYTEST"; mkdir -p "$KEYTEST/work/kt/.git/refs/heads/box" "$KEYTEST/tasks"
  printf 'PROFILE=sandbox\nBASE_SHA=%s\n' "$(printf 'b%.0s' {1..40})" >"$KEYTEST/tasks/kt.env"
  printf 'ref: refs/heads/box/kt\n' >"$KEYTEST/work/kt/.git/HEAD"
  printf '%s\n' "$(printf 'a%.0s' {1..40})" >"$KEYTEST/work/kt/.git/refs/heads/box/kt"
  : >"$KEYTEST/work/kt/.git/index"
}
fixture
assert_contains "a plain workspace gets a key with the head" "$(printf 'a%.0s' {1..40})" "$(key_for)"
assert_contains "the base sha is part of the key" "$(printf 'b%.0s' {1..40})" "$(key_for)"
fixture; rm "$KEYTEST/work/kt/.git/HEAD"; ln -s /etc/hostname "$KEYTEST/work/kt/.git/HEAD"
assert_eq "a symlinked HEAD disables the cache" "" "$(key_for)"
fixture; rm "$KEYTEST/work/kt/.git/refs/heads/box/kt"; ln -s /etc/hostname "$KEYTEST/work/kt/.git/refs/heads/box/kt"
assert_eq "a symlinked branch ref disables it" "" "$(key_for)"
fixture; head -c 20000 /dev/zero | tr '\0' 'x' >>"$KEYTEST/work/kt/.git/HEAD"
assert_eq "an oversized HEAD disables it" "" "$(key_for)"
fixture; { cat "$KEYTEST/work/kt/.git/refs/heads/box/kt"; head -c 20000 /dev/zero | tr '\0' 'x'; } >"$KEYTEST/work/kt/.git/refs/heads/box/kt.new"; mv "$KEYTEST/work/kt/.git/refs/heads/box/kt.new" "$KEYTEST/work/kt/.git/refs/heads/box/kt"
assert_eq "an oversized ref disables it" "" "$(key_for)"
fixture; printf 'ref: refs/heads/main\n' >"$KEYTEST/work/kt/.git/HEAD"
assert_eq "HEAD on another branch disables it" "" "$(key_for)"
fixture; printf 'not a sha\n' >"$KEYTEST/work/kt/.git/refs/heads/box/kt"
assert_eq "a ref that is not a sha disables it" "" "$(key_for)"
fixture; rm -rf "$KEYTEST/work/kt/.git/refs/heads/box"; ln -s /etc "$KEYTEST/work/kt/.git/refs/heads/box"
assert_eq "a symlinked ref directory disables it" "" "$(key_for)"
fixture; mv "$KEYTEST/work/kt/.git" "$KEYTEST/git-elsewhere"; ln -s "$KEYTEST/git-elsewhere" "$KEYTEST/work/kt/.git"
assert_eq "a symlinked .git disables it" "" "$(key_for)"
fixture; rm "$KEYTEST/work/kt/.git/refs/heads/box/kt"
printf '# pack-refs\n%s refs/heads/box/kt\n' "$(printf 'c%.0s' {1..40})" >"$KEYTEST/work/kt/.git/packed-refs"
assert_contains "a packed ref is read too" "$(printf 'c%.0s' {1..40})" "$(key_for)"
fixture; rm "$KEYTEST/work/kt/.git/refs/heads/box/kt"
assert_eq "no ref at all disables it" "" "$(key_for)"

echo " a disabled cache means a sandbox read every time"
mv "$W/.git/HEAD" "$TMP/HEAD.real"; ln -s "$TMP/HEAD.real" "$W/.git/HEAD"
: >"$FAKE_DOCKER_LOG"
"$BOX" facts t1 --json >/dev/null 2>&1; "$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "two calls, two reads" "2" "$(grep -c -e '/box/facts.py' "$FAKE_DOCKER_LOG")"
reads >/dev/null
rm "$W/.git/HEAD"; mv "$TMP/HEAD.real" "$W/.git/HEAD"

echo " starting or stopping the agent invalidates the answer"
"$BOX" facts t1 --json --fresh >/dev/null 2>&1; reads >/dev/null
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "(warm: a hit)" "0" "$(reads)"
"$BOX" agent t1 acct --detach >/dev/null 2>&1; reads >/dev/null
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "re-read after the agent started" "1" "$(reads)"
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "served from the cache while it runs and nothing changed" "0" "$(reads)"
"$BOX" stop t1 >/dev/null 2>&1; reads >/dev/null
"$BOX" facts t1 --json >/dev/null 2>&1
assert_eq "re-read after the agent stopped" "1" "$(reads)"

echo " decisions never come from the cache"
"$BOX" facts t1 --json >/dev/null 2>&1; reads >/dev/null
jq '.head = "'"$(printf 'e%.0s' {1..40})"'" | .ahead = 0' "$AGENTBOX_HOME/cache/facts/t1.json" >"$TMP/forged.json"
touch -r "$AGENTBOX_HOME/cache/facts/t1.json" "$TMP/forged.json"; cp -p "$TMP/forged.json" "$AGENTBOX_HOME/cache/facts/t1.json"
TAKEN="$("$BOX" take t1 2>/dev/null | tail -n 1)"
assert_eq "box take reports and takes the real head" "$(workspace_head t1)" "$TAKEN"
assert_status "box verify refuses without a runner, with the real head" 75 "$BOX" verify t1 test
"$BOX" verify t1 test --no-wait 2>/dev/null
assert_contains "the queued request names the real head" "sha=$(workspace_head t1)" "$(cat "$AGENTBOX_HOME"/queue/*-test.t1.req)"

finish

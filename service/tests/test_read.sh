#!/bin/bash
# The read verbs of the app, the same for both places: box tree (one directory level), box file
# (the bytes of one file, bounded), box search (file names) and box graph (the commits of the
# base and of the lavori). A lavoro on the Mac is read natively, without Docker; a task in a box
# through its sandbox.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
readonly TOO_BIG_EXIT=82
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$REPO/sub/src"
printf 'one\ntwo\nthree\n' >"$REPO/sub/src/app.ts"
echo "ignored.txt" >"$REPO/.gitignore"
echo "build/" >>"$REPO/.gitignore"
git -C "$REPO" add -A && git -C "$REPO" -c user.name=t -c user.email=t@example.com commit -q -m "a source file and an ignore list"
"$BOX" project add app --repo "$REPO" --subdir sub --install true --check "test=true" >/dev/null
"$BOX" new m1 app >/dev/null
"$BOX" new b1 app --where box >/dev/null

# The same work in both places: an edit, a deletion, a new file, an ignored file, a commit, then
# one more edit that is not committed.
work_in() {
  local dir="$1"
  mkdir -p "$dir/sub/spec" && echo "helper" >"$dir/sub/spec/a.ts"
  printf 'one\ntwo changed\nthree\nfour\n' >"$dir/sub/src/app.ts"
  git -C "$dir" rm -q sub/other.spec.ts
  printf 'a\nb\n' >"$dir/sub/src/new.ts"
  echo "secret" >"$dir/ignored.txt"
  mkdir -p "$dir/build" && echo "out" >"$dir/build/out.js"
  git -C "$dir" add -A && git -C "$dir" -c user.name=t -c user.email=t@example.com commit -q -m "work in $2"
  echo "loose" >"$dir/sub/loose.txt"
  echo "edited" >>"$dir/sub/existing.spec.ts"
}
work_in "$AGENTBOX_HOME/work/m1" "the lavoro on the Mac"
work_in "$AGENTBOX_HOME/work/b1" "the task in a box"

entry() {
  jq -c --arg name "$2" '.entries[] | select(.name == $name) | [.kind, .changed, .added, .removed]' <<<"$1"
}

for task in m1 b1; do
  W="$AGENTBOX_HOME/work/$task"
  [ "$task" = "m1" ] && PLACE="on the Mac" || PLACE="in a box"
  echo " box tree, $PLACE"
  : >"$FAKE_DOCKER_LOG"
  ROOT="$("$BOX" tree "$task" --json)"
  assert_eq "the root: its entries, without the ignored ones and without .git" ".gitignore sub" "$(jq -r '[.entries[].name] | join(" ")' <<<"$ROOT")"
  assert_eq "the keys of the answer and of an entry" '["entries","path","truncated"] ["added","changed","kind","name","removed"]' "$(jq -c 'keys' <<<"$ROOT") $(jq -c '.entries[0] | keys' <<<"$ROOT")"
  assert_eq "a folder with changes below it differs from the base" '["dir",true,null,null]' "$(entry "$ROOT" sub)"
  assert_eq "a file that did not change" '["file",false,null,null]' "$(entry "$ROOT" .gitignore)"
  assert_eq "--all shows the ignored ones too" ".gitignore build ignored.txt sub" "$("$BOX" tree "$task" --all --json | jq -r '[.entries[].name] | join(" ")')"
  SUB="$("$BOX" tree "$task" --path sub --json)"
  assert_eq "a folder: its path" "sub" "$(jq -r .path <<<"$SUB")"
  assert_eq "a file edited after the commit: its lines against the base" '["file",true,1,0]' "$(entry "$SUB" existing.spec.ts)"
  assert_eq "a file git does not know yet: all its lines are added" '["file",true,1,0]' "$(entry "$SUB" loose.txt)"
  assert_eq "a file of the base that is gone" '["deleted",true,0,1]' "$(entry "$SUB" other.spec.ts)"
  SRC="$("$BOX" tree "$task" --path sub/src --json)"
  assert_eq "a committed edit: lines added and removed" '["file",true,2,1]' "$(entry "$SRC" app.ts)"
  assert_eq "a committed new file" '["file",true,2,0]' "$(entry "$SRC" new.ts)"
  assert_status "a folder that does not exist" 1 "$BOX" tree "$task" --path nowhere --json
  assert_status "a path that climbs out" 1 "$BOX" tree "$task" --path ../.. --json
  ln -s /etc "$W/sub/out"
  assert_status "a link that leaves the lavoro is not listed" 1 "$BOX" tree "$task" --path sub/out --json
  assert_eq "it shows as a link" '"link"' "$("$BOX" tree "$task" --path sub --json | jq -c '.entries[] | select(.name == "out") | .kind')"

  echo " box file, $PLACE"
  assert_eq "the bytes of a file, as they are" "$(cat "$W/sub/src/app.ts")" "$("$BOX" file "$task" sub/src/app.ts)"
  assert_eq "the file as the base has it" "$(printf 'one\ntwo\nthree')" "$("$BOX" file "$task" sub/src/app.ts --at "$(git -C "$REPO" rev-parse main)")"
  assert_eq "a file that is gone, at the commit that had it" "other" "$("$BOX" file "$task" sub/other.spec.ts --at "$(git -C "$REPO" rev-parse main)")"
  printf '\x00\x01\xff binary' >"$W/sub/blob.bin"
  assert_eq "bytes that are not text pass unchanged" "$(shasum <"$W/sub/blob.bin")" "$("$BOX" file "$task" sub/blob.bin | shasum)"
  head -c 2097152 /dev/zero >"$W/sub/limit.bin"
  assert_eq "a file of exactly 2 MiB is printed" "2097152" "$("$BOX" file "$task" sub/limit.bin | wc -c | tr -d ' ')"
  head -c 2097153 /dev/zero >"$W/sub/big.bin"
  OUT="$("$BOX" file "$task" sub/big.bin 2>"$TMP/file.err")"
  assert_eq "one byte more: exit code 82" "$TOO_BIG_EXIT" "$?"
  assert_eq "nothing on stdout, one box: line" " box: the file is larger than 2 MiB: box file does not print it" "$OUT $(cat "$TMP/file.err")"
  assert_status "a file that does not exist" 3 "$BOX" file "$task" sub/nothing.txt
  assert_status "a path that climbs out" 1 "$BOX" file "$task" ../../etc/passwd
  assert_status "an absolute path" 1 "$BOX" file "$task" /etc/passwd
  ln -s /etc/passwd "$W/sub/passwd"
  assert_status "a link that leaves the lavoro" 1 "$BOX" file "$task" sub/passwd
  assert_eq "nothing of the file it names is printed" "" "$("$BOX" file "$task" sub/passwd 2>/dev/null)"
  assert_status "a file behind a linked folder that leaves" 1 "$BOX" file "$task" sub/out/passwd
  ln -s src/app.ts "$W/sub/inside-link"
  assert_eq "a link that stays inside is followed" "$(cat "$W/sub/src/app.ts")" "$("$BOX" file "$task" sub/inside-link)"
  assert_status "the git metadata is not a file of the lavoro" 1 "$BOX" file "$task" .git/config
  assert_status "a commit that is not a sha" 1 "$BOX" file "$task" sub/src/app.ts --at main
  rm -f "$W/sub/big.bin" "$W/sub/limit.bin" "$W/sub/blob.bin" "$W/sub/passwd" "$W/sub/out" "$W/sub/inside-link"

  echo " box search, $PLACE"
  FOUND="$("$BOX" search "$task" spec --json)"
  assert_eq "a match in the file's own name comes before one in a folder's, however short" '["sub/existing.spec.ts","sub/spec/a.ts"]' "$(jq -c .matches <<<"$FOUND")"
  assert_eq "the keys of the answer" '["matches","query","truncated"]' "$(jq -c keys <<<"$FOUND")"
  assert_eq "letter case does not count" '["sub/src/app.ts"]' "$("$BOX" search "$task" "APP" --json | jq -c .matches)"
  assert_eq "a file git does not know yet is found" '["sub/loose.txt"]' "$("$BOX" search "$task" loose --json | jq -c .matches)"
  assert_eq "an ignored file is not" '[]' "$("$BOX" search "$task" ignored.txt --json | jq -c .matches)"
  assert_status "an empty text" 1 "$BOX" search "$task" " " --json
  if [ "$task" = "m1" ]; then
    assert_eq "all of it without Docker" "" "$(cat "$FAKE_DOCKER_LOG")"
  else
    assert_eq "all of it in containers that mount the workspace read-only" "0" "$(grep -e '/box/tree.py' -e '/box/file.py' -e '/box/search.py' "$FAKE_DOCKER_LOG" | grep -c -v -e "work/b1:/src:ro")"
  fi
done

echo " box graph: the base and every lavoro of a project"
: >"$FAKE_DOCKER_LOG"
GRAPH="$("$BOX" graph app --json)"
assert_eq "the keys of the answer" '["base","commits","lanes","limit","project"]' "$(jq -c keys <<<"$GRAPH")"
assert_eq "the lanes: the base, then each lavoro with its place and branch" "null:main:null b1:box/b1:box m1:sakura/m1:mac" "$(jq -r '[.lanes[] | "\(.lavoro):\(.branch):\(.where)"] | join(" ")' <<<"$GRAPH")"
assert_eq "the keys of a lane and of a commit" '["base","branch","head","lavoro","readable","where"] ["branches","lavoro","parents","sha","subject","time"]' "$(jq -c '.lanes[1] | keys' <<<"$GRAPH") $(jq -c '.commits[0] | keys' <<<"$GRAPH")"
BASE_HEAD="$(git -C "$REPO" rev-parse main)"
MAC_HEAD="$(git -C "$AGENTBOX_HOME/work/m1" rev-parse HEAD)"
BOX_HEAD="$(git -C "$AGENTBOX_HOME/work/b1" rev-parse HEAD)"
assert_eq "the heads of the lanes" "$BASE_HEAD $BOX_HEAD $MAC_HEAD" "$(jq -r '[.lanes[].head] | join(" ")' <<<"$GRAPH")"
assert_eq "every lavoro starts at the base" "$BASE_HEAD $BASE_HEAD" "$(jq -r '[.lanes[1:][].base] | join(" ")' <<<"$GRAPH")"
assert_eq "the commit of the lavoro on the Mac: its lavoro, its parent, its branch" "m1 $BASE_HEAD sakura/m1" "$(jq -r --arg sha "$MAC_HEAD" '.commits[] | select(.sha == $sha) | "\(.lavoro) \(.parents[0]) \(.branches[0])"' <<<"$GRAPH")"
assert_eq "the commit of the task in a box" "b1 $BASE_HEAD box/b1" "$(jq -r --arg sha "$BOX_HEAD" '.commits[] | select(.sha == $sha) | "\(.lavoro) \(.parents[0]) \(.branches[0])"' <<<"$GRAPH")"
assert_eq "the commits of the base belong to no lavoro and carry the base branch at its tip" "null main" "$(jq -r --arg sha "$BASE_HEAD" '.commits[] | select(.sha == $sha) | "\(.lavoro) \(.branches[0])"' <<<"$GRAPH")"
assert_eq "every commit once, newest first" "4 1" "$(jq -r '.commits | "\(length) \((map(.time) == (map(.time) | sort | reverse)) | if . then 1 else 0 end)"' <<<"$GRAPH")"
assert_eq "the times are in UTC" "0" "$(jq -r '.commits[].time' <<<"$GRAPH" | grep -c -v -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$')"
assert_eq "--limit bounds the commits of the base" "1 1" "$("$BOX" graph app --json --limit 1 | jq -r '"\(.limit) \([.commits[] | select(.lavoro == null)] | length)"')"
assert_status "a limit that is not a number" 1 "$BOX" graph app --json --limit many

echo " box graph for one lavoro"
: >"$FAKE_DOCKER_LOG"
ONE="$("$BOX" graph m1 --json)"
assert_eq "the base and that lavoro only" "null m1" "$(jq -r '[.lanes[].lavoro | tostring] | join(" ")' <<<"$ONE")"
assert_eq "its project and base are named" "app main" "$(jq -r '"\(.project) \(.base)"' <<<"$ONE")"
assert_eq "a lavoro on the Mac is drawn without Docker" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "a task in a box too, through its sandbox" "null b1" "$("$BOX" graph b1 --json | jq -r '[.lanes[].lavoro | tostring] | join(" ")')"
assert_status "a name that is neither a project nor a task" 1 "$BOX" graph nothing --json
finish

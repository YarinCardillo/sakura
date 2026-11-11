#!/bin/bash
# box file and box tree reach a path of a lavoro through directory descriptors, one component at
# a time from the root of the lavoro, never following a link by name: so a file or folder that
# is swapped for a link between a check and its use cannot lead a read out of the lavoro. What
# was opened is examined on its descriptor (a regular file, a directory), nothing that is not a
# regular file is read (a FIFO does not make a read wait), and every read is bounded.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
RUNNER="$BOX_ROOT/image/runner"
echo "outside" >"$TMP/outside.txt"
mkdir -p "$TMP/outside-dir" && echo "x" >"$TMP/outside-dir/secret.txt"

# opened <path> [directory]: what lavoro.open_inside does with a path of the worktree.
opened() {
  BOX_SRC="$W" python3 -B -c '
import os, sys
sys.path.insert(0, sys.argv[1])
import lavoro
descriptor = lavoro.open_inside(sys.argv[2], directory=len(sys.argv) > 3)
print("opened", os.fstat(descriptor).st_size if len(sys.argv) == 3 else "directory")' "$RUNNER" "$@" 2>&1
}

echo " a path is walked one component at a time"
assert_contains "a plain file is opened" "opened 3" "$(opened sub/package.json)"
assert_contains "a plain folder too" "opened directory" "$(opened sub directory)"
mkdir -p "$W/sub/deep/er" && echo "deep" >"$W/sub/deep/er/file.txt"
ln -s deep/er "$W/sub/inside"
ln -s ../deep/er/file.txt "$W/sub/deep/up-and-down"
assert_contains "a link that stays inside is followed, step by step" "opened 5" "$(opened sub/inside/file.txt)"
assert_contains "also one that goes up and comes down again" "opened 5" "$(opened sub/deep/up-and-down)"
ln -s "$TMP/outside.txt" "$W/sub/absolute"
ln -s ../../../outside.txt "$W/sub/climbing"
ln -s "$TMP/outside-dir" "$W/sub/folder-out"
ln -s "$W/sub/package.json" "$W/sub/absolute-inside"
ln -s loop-b "$W/sub/loop-a" && ln -s loop-a "$W/sub/loop-b"
for path in sub/absolute sub/climbing sub/folder-out/secret.txt sub/absolute-inside sub/loop-a ../outside.txt .git/config sub/inside/../../../outside.txt; do
  OUT="$(opened "$path")"
  assert_eq "refused: $path" "0" "$(grep -c opened <<<"$OUT")"
done
assert_eq "the scripts never resolve a path by name before they open it" "0" "$(cat "$RUNNER/lavoro.py" "$RUNNER/file.py" "$RUNNER/tree.py" | grep -c -e 'realpath' -e 'os.path.isdir' -e 'os.path.getsize' -e 'os.stat(')"

echo " the root of the lavoro is opened below its trusted parent, without following a link, and must be the folder that was checked"
IDENTITY="$(stat_of '%d:%i' '%d:%i' "$W")"
mkdir -p "$TMP/outside-root/sub" && echo "OUTSIDE" >"$TMP/outside-root/sub/package.json"
root_opened() {
  BOX_SRC="$1" BOX_ROOT_IDENTITY="$2" python3 -B -c '
import os, sys
sys.path.insert(0, sys.argv[1])
import lavoro
print("root-read", os.read(lavoro.open_inside("sub/package.json"), 20))' "$RUNNER" 2>&1
}
assert_contains "the folder that was checked is opened" "root-read" "$(root_opened "$W" "$IDENTITY")"
assert_eq "another folder under the same name is refused" "0" "$(root_opened "$W" "1:1" | grep -c root-read)"
ln -s "$TMP/outside-root" "$TMP/root-link"
assert_eq "a root that is a link is not followed, with or without a recorded identity" "0 0" "$(root_opened "$TMP/root-link" "" | grep -c root-read) $(root_opened "$TMP/root-link" "$IDENTITY" | grep -c root-read)"
# The worktree is swapped for a link to a folder outside between the check box makes and the
# moment the script opens it: a python3 that does the swap just before the real one starts.
REAL_PYTHON="$(command -v python3)"
mkdir -p "$TMP/pyshim"
printf '#!/bin/bash\nmv "%s" "%s.aside" && ln -s "%s" "%s"\nexec "%s" "$@"\n' "$W" "$W" "$TMP/outside-root" "$W" "$REAL_PYTHON" >"$TMP/pyshim/python3"
chmod +x "$TMP/pyshim/python3"
OUT="$(PATH="$TMP/pyshim:$PATH" "$BOX" file m1 sub/package.json 2>/dev/null)"
assert_eq "box file after such a swap: an error, and nothing of the outside file" "1 " "$([ "$?" -ne 0 ] && echo 1 || echo 0) $OUT"
rm "$W" && mv "$W.aside" "$W"
OUT="$(PATH="$TMP/pyshim:$PATH" "$BOX" tree m1 --path sub --json 2>/dev/null)"
assert_eq "box tree too" "1 " "$([ "$?" -ne 0 ] && echo 1 || echo 0) $OUT"
rm "$W" && mv "$W.aside" "$W"
assert_eq "(with the worktree back the file is read)" "{}" "$("$BOX" file m1 sub/package.json)"

echo " what is not a regular file is never read"
mkfifo "$W/sub/pipe"
STARTED=$SECONDS
assert_status "box file on a FIFO ends at once" 3 perl -e 'alarm 20; exec @ARGV' "$BOX" file m1 sub/pipe
assert_eq "without waiting for a writer (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 10 ] && echo 1 || echo 0)"
STARTED=$SECONDS
TREE="$(perl -e 'alarm 20; exec @ARGV' "$BOX" tree m1 --path sub --json)"
assert_eq "box tree lists it as other and does not read it (after $((SECONDS - STARTED)) s)" '"other"' "$(jq -c '.entries[] | select(.name == "pipe") | .kind' <<<"$TREE")"
assert_status "a folder is not a file" 3 "$BOX" file m1 sub/deep
rm "$W/sub/pipe"

echo " every read is bounded"
head -c 3000000 /dev/zero | tr '\0' '\n' >"$W/sub/many-lines.txt"
assert_eq "box tree does not count the lines of a file over the limit" "null" "$("$BOX" tree m1 --path sub --json | jq -c '.entries[] | select(.name == "many-lines.txt") | .added')"
assert_eq "a read of a descriptor stops at the limit plus one byte" "2097153" "$(BOX_SRC="$W" python3 -B -c '
import sys
sys.path.insert(0, sys.argv[1])
import lavoro
print(len(lavoro.read_bounded(lavoro.open_inside("sub/many-lines.txt"))))' "$RUNNER")"
rm "$W/sub/many-lines.txt"

echo " a swap between the look and the read does not lead out"
# The folder is listed while it is a real folder; before its files are counted it becomes a
# link to a folder outside. The count must not read the outside file.
mkdir -p "$W/sub/swap" && printf 'a\nb\n' >"$W/sub/swap/secret.txt"
OUT="$(BOX_SRC="$W" python3 -B -c '
import os, sys
sys.path.insert(0, sys.argv[1])
import lavoro
directory = lavoro.open_inside("sub/swap", directory=True)
os.rename(os.path.join(sys.argv[2], "sub/swap"), os.path.join(sys.argv[2], "sub/swap-real"))
os.symlink(sys.argv[3], os.path.join(sys.argv[2], "sub/swap"))
print("lines", lavoro.count_lines(directory, "secret.txt"))' "$RUNNER" "$W" "$TMP/outside-dir")"
assert_eq "the file counted is the one of the folder that was opened (2 lines, not the outside one)" "lines 2" "$OUT"
assert_status "and box file through the swapped folder is refused" 1 "$BOX" file m1 sub/swap/secret.txt
assert_eq "it prints nothing of the outside file" "" "$("$BOX" file m1 sub/swap/secret.txt 2>/dev/null)"
finish

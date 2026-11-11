#!/bin/bash
# The PATH box rebuilds for itself leaves out every folder of its data directory, and "inside
# the data directory" is decided by what the folder is (device and inode of the folder and of
# each folder above it), never by how its path is spelled. A worktree named in PATH under
# another spelling (other letter case on a volume that does not tell case apart, a part of the
# data directory that is a link to another place, the data directory given through a link)
# must not have its files run in the place of the programs box uses.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
readonly PLANTED_NAMES="dirname readlink git sed jq stat python3 perl cat mkdir head tr grep date id"

# plant <folder>: a file for each of the names, which says that it ran.
plant() {
  local name
  for name in $PLANTED_NAMES; do
    printf '#!/bin/sh\necho "$0" >>%q/planted-ran\nexit 97\n' "$TMP" >"$1/$name"
    chmod +x "$1/$name"
  done
}
planted() {
  [ -e "$TMP/planted-ran" ] && sort -u "$TMP/planted-ran" | tr '\n' ' '
  rm -f "$TMP/planted-ran"
}
# with_path <entry> <box arguments...>: box, started with the entry first in PATH.
with_path() {
  local entry="$1"
  shift
  PATH="$entry:$PATH" "$BOX" "$@"
}

"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
plant "$W"
cd "$TMP" || exit 1

echo " the worktree by its own path (the plain case)"
assert_eq "facts answers" "mac" "$(with_path "$W" facts m1 --json | jq -r .where)"
assert_eq "none of its files ran" "" "$(planted)"

echo " the data directory spelled in another letter case"
UPPER="$(dirname "$AGENTBOX_HOME")/$(basename "$AGENTBOX_HOME" | tr '[:lower:]' '[:upper:]')"
if [ "$UPPER" != "$AGENTBOX_HOME" ] && [ -d "$UPPER/work/m1" ]; then
  assert_eq "facts answers" "mac" "$(with_path "$UPPER/work/m1" facts m1 --json | jq -r .where)"
  assert_eq "none of its files ran" "" "$(planted)"
  assert_eq "status answers" "m1" "$(with_path "$UPPER/work/m1" status --json | jq -r '.tasks[0].name')"
  assert_eq "none of its files ran" "" "$(planted)"
else
  echo "  (this volume tells letter case apart: nothing to test)"
fi

echo " the data directory given through a link, the worktree named by its real path"
ln -s "$AGENTBOX_HOME" "$TMP/home-link"
assert_eq "facts answers" "mac" "$(AGENTBOX_HOME="$TMP/home-link" with_path "$W" facts m1 --json | jq -r .where)"
assert_eq "none of its files ran" "" "$(planted)"
assert_eq "and the other way round" "mac" "$(with_path "$TMP/home-link/work/m1" facts m1 --json | jq -r .where)"
assert_eq "none of its files ran" "" "$(planted)"

echo " a part of the data directory that is a link to another place"
mkdir -p "$TMP/elsewhere" "$AGENTBOX_HOME/play"
plant "$TMP/elsewhere"
rmdir "$AGENTBOX_HOME/play" && ln -s "$TMP/elsewhere" "$AGENTBOX_HOME/play"
assert_eq "facts answers" "mac" "$(with_path "$TMP/elsewhere" facts m1 --json | jq -r .where)"
assert_eq "none of the files of that place ran" "" "$(planted)"
mkdir -p "$TMP/elsewhere/deeper"
plant "$TMP/elsewhere/deeper"
assert_eq "a folder below it" "mac" "$(with_path "$TMP/elsewhere/deeper" facts m1 --json | jq -r .where)"
assert_eq "none of the files of that place ran" "" "$(planted)"

echo " a folder outside the data directory stays usable"
mkdir -p "$TMP/tools"
printf '#!/bin/sh\necho "$0" >>%q/tool-ran\nexec /usr/bin/dirname "$@"\n' "$TMP" >"$TMP/tools/dirname"
chmod +x "$TMP/tools/dirname"
assert_eq "facts answers" "mac" "$(with_path "$TMP/tools" facts m1 --json | jq -r .where)"
assert_eq "its dirname was the one box used" "1" "$([ -s "$TMP/tool-ran" ] && echo 1 || echo 0)"

echo " the second layer: a program whose path box records is never taken from the data directory"
found() {
  PATH="$1:$PATH" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project; do . "$BOX_ROOT/lib/$lib.sh"; done
    trusted_program node'
}
# (Folders that hold nothing but node: this part runs the functions without the PATH bin/box
# rebuilds, and must not find its own stat among the planted files.)
mkdir -p "$TMP/elsewhere/only-node"
printf '#!/bin/sh\nexit 0\n' >"$W/sub/node"
printf '#!/bin/sh\nexit 0\n' >"$TMP/elsewhere/only-node/node"
chmod +x "$W/sub/node" "$TMP/elsewhere/only-node/node"
assert_status "node of the worktree, by its own path: not found" 1 found "$W/sub"
if [ "$UPPER" != "$AGENTBOX_HOME" ] && [ -d "$UPPER/work/m1" ]; then
  assert_status "node of the worktree, in another letter case: not found" 1 found "$UPPER/work/m1/sub"
fi
assert_status "node of a place a part of the data directory links to: not found" 1 found "$TMP/elsewhere/only-node"
assert_status "the same with the data directory given through a link" 1 env AGENTBOX_HOME="$TMP/home-link" PATH="$W/sub:$PATH" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project; do . "$BOX_ROOT/lib/$lib.sh"; done
  trusted_program node'
printf '#!/bin/sh\nexit 0\n' >"$TMP/tools/node"
chmod +x "$TMP/tools/node"
assert_eq "node of a folder outside it: found" "$TMP/tools/node" "$(found "$TMP/tools")"
finish

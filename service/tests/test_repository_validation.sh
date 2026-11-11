#!/bin/bash
# Before git runs on the host, the repository must be plain: nothing in its git directory may
# lead git somewhere else. Borrowed objects, linked entries, included configuration and a folder
# that can be read as two repositories are refused, with the reason, and git is not run. A normal
# clone and a linked worktree of the person's repository are accepted.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly EXIT_REFUSED=6
readonly LINKED_ENTRIES="config HEAD objects refs packed-refs info worktrees"
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
MARKER="$TMP/ran-on-the-host"
git -C "$W" config core.fsmonitor "touch $MARKER-fsmonitor"
git -C "$W" config core.pager "touch $MARKER-pager"

# fresh <name>: a clone of the source that is the person's own, with every entry a link may replace.
fresh() {
  rm -rf "${TMP:?}/$1"
  git clone -q "$SOURCE" "$TMP/$1"
  git -C "$TMP/$1" pack-refs --all
  mkdir -p "$TMP/$1/.git/worktrees" "$TMP/$1/.git/info"
}

# assert_refused <name of the case> <folder> <words of the reason>: detect, add and a later use.
assert_refused() {
  assert_status "detect: $1" "$EXIT_REFUSED" "$BOX" project detect "$2" --json
  assert_contains "  it says why" "$3" "$OUTPUT"
  assert_eq "  in one box: line" "1" "$(grep -c '^box: ' <<<"$OUTPUT")"
  assert_status "  and it cannot be added" 1 "$BOX" project add refused --repo "$2"
  assert_no_file "  no project was written" "$AGENTBOX_HOME/projects/refused.env"
}

echo " the positive cases: a normal clone and a linked worktree"
fresh plain
assert_status "detect on a normal clone" 0 "$BOX" project detect "$TMP/plain" --json
assert_status "detect on a folder inside it" 0 "$BOX" project detect "$TMP/plain/sub" --json
assert_status "add" 0 "$BOX" project add plain --repo "$TMP/plain"
assert_status "branches" 0 "$BOX" branches plain --json
git -C "$TMP/plain" worktree add -q "$TMP/plain-tree" -b linked
assert_status "detect on a linked worktree of it" 0 "$BOX" project detect "$TMP/plain-tree" --json
assert_status "add of the linked worktree" 0 "$BOX" project add tree --repo "$TMP/plain-tree"
assert_status "branches of it" 0 "$BOX" branches tree --json

echo " borrowed objects (alternates) are refused whatever the file says"
fresh other
fresh borrowing
echo "$TMP/other/.git/objects" >"$TMP/borrowing/.git/objects/info/alternates"
assert_refused "a repository that borrows from another of the person" "$TMP/borrowing" "objects/info/alternates"
assert_contains "  and how to fix it" "git repack -a -d" "$OUTPUT"
echo "$W/.git/objects" >"$TMP/other/.git/objects/info/alternates"
assert_refused "one that borrows from a repository that borrows from a workspace" "$TMP/borrowing" "objects/info/alternates"
: >"$TMP/borrowing/.git/objects/info/alternates"
assert_refused "an empty alternates file" "$TMP/borrowing" "objects/info/alternates"
fresh later
"$BOX" project add later --repo "$TMP/later" >/dev/null
echo "$W/.git/objects" >"$TMP/later/.git/objects/info/alternates"
assert_status "a project that starts borrowing afterwards: box branches" 1 "$BOX" branches later --json
assert_contains "it says why" "objects/info/alternates" "$OUTPUT"
assert_status "and box new" 1 "$BOX" new x1 later

echo " a linked entry in the git directory is refused, whichever it is"
mkdir -p "$TMP/elsewhere"
for entry in $LINKED_ENTRIES; do
  fresh linked
  rm -rf "${TMP:?}/elsewhere/$entry"
  mv "$TMP/linked/.git/$entry" "$TMP/elsewhere/$entry"
  ln -s "$TMP/elsewhere/$entry" "$TMP/linked/.git/$entry"
  assert_refused "$entry is a link" "$TMP/linked" "$entry"
  assert_contains "  the reason names the link" "symbolic link" "$OUTPUT"
done
fresh linked
rm -rf "$TMP/linked/.git/info"
ln -s "$W/.git/info" "$TMP/linked/.git/info"
assert_refused "an entry that is a link into a workspace" "$TMP/linked" "info"
fresh linked
rm -f "$TMP/linked/.git/objects/info/alternates"
rm -rf "$TMP/linked/.git/objects/info"
ln -s "$TMP/other/.git/objects/info" "$TMP/linked/.git/objects/info"
assert_refused "the info folder of the objects is a link" "$TMP/linked" "objects/info"

echo " the same in the common directory of a linked worktree"
fresh common
git -C "$TMP/common" worktree add -q "$TMP/common-tree" -b linked
mv "$TMP/common/.git/config" "$TMP/elsewhere/common-config"
ln -s "$TMP/elsewhere/common-config" "$TMP/common/.git/config"
assert_refused "the main repository's config is a link" "$TMP/common-tree" "config"
mv "$TMP/elsewhere/common-config" "$TMP/common/.git/config.real"
rm "$TMP/common/.git/config" && mv "$TMP/common/.git/config.real" "$TMP/common/.git/config"
assert_status "(with the file back, the worktree is accepted)" 0 "$BOX" project detect "$TMP/common-tree" --json
echo "$TMP/other/.git/objects" >"$TMP/common/.git/objects/info/alternates"
assert_refused "the main repository borrows objects" "$TMP/common-tree" "objects/info/alternates"

echo " configuration that includes other files is refused"
fresh including
git -C "$TMP/including" config include.path "$W/.git/config"
assert_refused "include.path" "$TMP/including" "include"
fresh including
git -C "$TMP/including" config "includeIf.gitdir:$TMP/.path" "$W/.git/config"
assert_refused "includeIf" "$TMP/including" "include"
fresh including
printf '[core][INCLUDE] path = %s\n' "$W/.git/config" >>"$TMP/including/.git/config"
assert_refused "an include section behind another on one line, in capitals" "$TMP/including" "include"
fresh including
chmod 000 "$TMP/including/.git/config"
assert_refused "a configuration that cannot be read" "$TMP/including" "config"
chmod 600 "$TMP/including/.git/config"

echo " a folder that can be read as two repositories is refused"
git clone -q --bare "$SOURCE" "$TMP/both"
fresh donor
mv "$TMP/donor/.git" "$TMP/both/.git"
assert_refused "a bare layout that also has a .git" "$TMP/both" "two repositories"
fresh outer
mkdir -p "$TMP/outer/inner"
printf 'gitdir: %s\n' "$W/.git" >"$TMP/outer/inner/.git"
assert_status "a folder with its own .git is judged by that one, not by the repository above" "$EXIT_REFUSED" "$BOX" project detect "$TMP/outer/inner" --json

echo " a .git that is not a plain git directory is refused, not skipped"
fresh odd
rm -rf "$TMP/odd/.git/HEAD"
assert_refused "no HEAD" "$TMP/odd" "HEAD"
fresh odd
mkdir "$TMP/odd-link" && ln -s "$TMP/odd/.git" "$TMP/odd-link/.git"
assert_refused "a .git that is a link, even to the person's own" "$TMP/odd-link" "symbolic link"
mkdir "$TMP/odd-file" && head -c 5000 /dev/zero | tr '\0' 'x' >"$TMP/odd-file/.git"
assert_refused "a .git file that is not a pointer" "$TMP/odd-file" ".git"

echo " after git ran, the directories it used are the ones that were checked"
STATUS="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project; do . "$BOX_ROOT/lib/$lib.sh"; done
  check_git_entry "$1/plain/.git" || { echo "unexpected: $REPOSITORY_PROBLEM"; exit 0; }
  git_used_checked_directories "$1/plain"; echo "same=$?"
  REPOSITORY_GIT_DIR="$1/other/.git"
  git_used_checked_directories "$1/plain"; echo "other-git-dir=$?"
  check_git_entry "$1/plain-tree/.git" || { echo "unexpected: $REPOSITORY_PROBLEM"; exit 0; }
  git_used_checked_directories "$1/plain-tree"; echo "worktree=$?"
  REPOSITORY_COMMON_DIR="$1/other/.git"
  git_used_checked_directories "$1/plain-tree"; echo "other-common-dir=$?"' _ "$TMP" 2>/dev/null | tr '\n' ' ')"
assert_eq "equal directories pass, any difference refuses" "same=0 other-git-dir=1 worktree=0 other-common-dir=1 " "$STATUS"
assert_eq "nothing of the workspace's configuration ran on the host" "0" "$(/bin/ls "$TMP" | grep -c '^ran-on-the-host')"
finish

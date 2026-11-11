#!/bin/bash
# Host git never finds the repository of a lavoro on the Mac by looking in its worktree: it is
# given the git directory and the work tree explicitly, from what box recorded when it created
# the worktree (the directory worktrees/<name> of the project's repository). Before every call
# the .git file of the worktree must still name exactly that directory; anything else refuses.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
REPO="$(cd -P "$REPO" && pwd -P)"
MARKER="$TMP/ran-on-the-host"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
"$BOX" new t1 sandbox --where box >/dev/null 2>&1
W="$AGENTBOX_HOME/work/m1"
RECORDED="$(sed -n 's/^WORKTREE_GIT_DIR=//p' "$AGENTBOX_HOME/tasks/m1.env")"

readable() {
  "$BOX" facts m1 --json 2>/dev/null | jq -r .readable 2>/dev/null || echo refused
}

echo " the git directory of the worktree is recorded when it is created"
assert_eq "it is the worktrees/<task> directory of the project's repository" "$REPO/.git/worktrees/m1" "$RECORDED"
assert_eq "the .git file of the worktree names it" "gitdir: $RECORDED" "$(cat "$W/.git")"
assert_eq "(the facts are read)" "true" "$(readable)"

echo " every host git call names the git directory and the work tree itself"
REAL_GIT="$(command -v git)"
mkdir -p "$TMP/shim"
printf '#!/bin/bash\nprintf "%%s\\n" "$*" >>%q\nexec %q "$@"\n' "$TMP/git-calls.log" "$REAL_GIT" >"$TMP/shim/git"
chmod +x "$TMP/shim/git"
PATH="$TMP/shim:$PATH" "$BOX" facts m1 --json >/dev/null
PATH="$TMP/shim:$PATH" "$BOX" status --json >/dev/null
WORKTREE_CALLS="$(grep -e "$W" -e "$RECORDED" "$TMP/git-calls.log")"
assert_eq "(git was called for the worktree)" "1" "$([ "$(wc -l <<<"$WORKTREE_CALLS")" -ge 6 ] && echo 1 || echo 0)"
assert_eq "each call carries --git-dir and --work-tree" "0" "$(grep -c -v -e "--git-dir=$RECORDED --work-tree=$W " <<<"$WORKTREE_CALLS")"
assert_eq "no call is started in the worktree to let git look for itself" "0" "$(grep -c -e "-C $W" "$TMP/git-calls.log")"

echo " a .git that names anything else is refused, and git is not run"
cp "$W/.git" "$TMP/pointer"
git -C "$AGENTBOX_HOME/work/t1" config core.fsmonitor "touch $MARKER-fsmonitor"
# refused_with <name of the case>: the facts are not read and no git call reached the worktree.
refused_with() {
  : >"$TMP/git-calls.log"
  assert_eq "$1" "false" "$(PATH="$TMP/shim:$PATH" "$BOX" facts m1 --json 2>/dev/null | jq -r .readable)"
  assert_eq "  no git call for the worktree" "0" "$(grep -c -e "$W" -e "$RECORDED" "$TMP/git-calls.log")"
}
printf 'gitdir: %s\n' "$AGENTBOX_HOME/work/t1/.git" >"$W/.git"
refused_with "a .git that names the workspace of a box"
git clone -q "$SOURCE" "$TMP/another"
git -C "$TMP/another" worktree add -q "$TMP/another-tree" -b elsewhere
cp "$TMP/another-tree/.git" "$W/.git"
refused_with "one that names a worktree of another repository of the person"
printf 'gitdir: %s/\n' "$RECORDED" >"$W/.git"
refused_with "the same directory spelled another way"
printf 'gitdir: %s\nsomething else\n' "$RECORDED" >"$W/.git"
refused_with "the right line followed by another"
rm "$W/.git" && ln -s "$TMP/pointer" "$W/.git"
refused_with "a .git that is a link, even to the right pointer"
rm "$W/.git" && cp -R "$AGENTBOX_HOME/work/t1/.git" "$W/.git"
refused_with "a .git that is a git directory of its own"
rm -rf "$W/.git" && cp "$TMP/pointer" "$W/.git"
assert_eq "with its own pointer back it is read again" "true" "$(readable)"

echo " the recorded directory must be a worktree of the project's repository"
cp "$AGENTBOX_HOME/tasks/m1.env" "$TMP/record"
edit_in_place "s|^WORKTREE_GIT_DIR=.*|WORKTREE_GIT_DIR=$TMP/another/.git/worktrees/another-tree|" "$AGENTBOX_HOME/tasks/m1.env"
cp "$TMP/another-tree/.git" "$W/.git"
refused_with "a record and a pointer that agree on another repository"
edit_in_place '/^WORKTREE_GIT_DIR=/d' "$AGENTBOX_HOME/tasks/m1.env"
refused_with "a record without the directory"
cp "$TMP/record" "$AGENTBOX_HOME/tasks/m1.env"
cp "$TMP/pointer" "$W/.git"
mv "$RECORDED/commondir" "$TMP/commondir"
echo "$TMP/another/.git" >"$RECORDED/commondir"
refused_with "a git directory whose common directory is not the project's"
mv "$TMP/commondir" "$RECORDED/commondir"
mv "$W" "$TMP/moved"
ln -s "$TMP/moved" "$W"
refused_with "a folder of the lavoro that became a link"
rm "$W" && mv "$TMP/moved" "$W"
assert_eq "with everything back it is read again" "true" "$(readable)"
assert_eq "nothing of the workspace's configuration ran on the host" "0" "$(/bin/ls "$TMP" | grep -c '^ran-on-the-host')"
finish

#!/bin/bash
# box runs git on the host only in a repository of the person. A folder that is really a
# workspace an agent writes must never be taken for one, however its path is spelled (another
# letter case, a link, another name of the same directory), and nothing the configuration of any
# repository names is ever run by box on the host.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly EXIT_NOT_A_REPOSITORY=4
readonly EXIT_INSIDE_DATA_DIRECTORY=6
readonly EXIT_NOT_OWNED=7
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
MARKER="$TMP/ran-on-the-host"

# hostile_config <repository>: everything in a git configuration that can start a program.
hostile_config() {
  mkdir -p "$1/.git/evil-hooks"
  for hook in post-checkout reference-transaction post-index-change pre-auto-gc; do
    printf '#!/bin/sh\ntouch %q-hook\n' "$MARKER" >"$1/.git/evil-hooks/$hook"
    chmod +x "$1/.git/evil-hooks/$hook"
    cp "$1/.git/evil-hooks/$hook" "$1/.git/hooks/$hook" 2>/dev/null
  done
  git -C "$1" config core.hooksPath "$1/.git/evil-hooks"
  git -C "$1" config core.fsmonitor "touch $MARKER-fsmonitor"
  git -C "$1" config core.pager "touch $MARKER-pager"
  git -C "$1" config diff.external "touch $MARKER-diff"
  git -C "$1" config core.sshCommand "touch $MARKER-ssh"
  git -C "$1" config protocol.ext.allow always
}
nothing_ran() {
  assert_eq "$1" "0" "$(/bin/ls "$TMP" | grep -c '^ran-on-the-host')"
}
hostile_config "$W"

# assert_workspace_refused <name of the spelling> <path>: neither detect nor add take it.
assert_workspace_refused() {
  assert_status "detect, $1" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$2" --json
  assert_contains "  it says why" "data directory" "$OUTPUT"
  assert_status "add, $1" 1 "$BOX" project add stolen --repo "$2"
  assert_no_file "  no project was written" "$AGENTBOX_HOME/projects/stolen.env"
}

echo " a workspace is refused however its path is spelled"
assert_workspace_refused "as it is" "$W"
assert_workspace_refused "a folder inside it" "$W/sub"
PHYSICAL="$(cd -P "$W" && pwd -P)"
assert_workspace_refused "its physical path" "$PHYSICAL"
UPPER="$(printf '%s' "$PHYSICAL" | tr '[:lower:]' '[:upper:]')"
if [ -d "$UPPER" ]; then
  assert_workspace_refused "in capital letters" "$UPPER"
  assert_workspace_refused "with one folder in capital letters" "$AGENTBOX_HOME/WORK/t1"
else
  pass "(this file system tells capital letters apart: the capital spelling is another path)"
fi
for volume in /Volumes/*; do
  if [ -L "$volume" ] && [ "$(readlink "$volume")" = "/" ]; then
    assert_workspace_refused "through the volume's own name" "$volume$PHYSICAL"
  fi
done
ln -s "$W" "$TMP/link"
assert_workspace_refused "through a link" "$TMP/link"
ln -s "$TMP/link" "$TMP/link-to-link"
assert_workspace_refused "through a link to a link" "$TMP/link-to-link/sub"
ln -s "$AGENTBOX_HOME" "$TMP/other-name-of-home"
assert_workspace_refused "through another name of the data directory" "$TMP/other-name-of-home/work/t1"
nothing_ran "nothing of the workspace's configuration ran"

echo " the data directory itself may be named through a link"
OUT="$(AGENTBOX_HOME="$TMP/other-name-of-home" "$BOX" project detect "$PHYSICAL" --json 2>&1)"
assert_eq "the workspace is still refused" "$EXIT_INSIDE_DATA_DIRECTORY" "$?"
nothing_ran "and nothing ran"

echo " a project whose repository later becomes a workspace is refused when it is used"
REAL="$TMP/real-repo"
git clone -q "$SOURCE" "$REAL"
"$BOX" project add moving --repo "$REAL" >/dev/null
printf 'REPO=%s\nBASE=main\n' "$TMP/swapped" >"$AGENTBOX_HOME/projects/linked.env"
ln -s "$W" "$TMP/swapped"
assert_status "box new on a project file that names a link into a workspace" 1 "$BOX" new x1 linked
assert_contains "it says why" "data directory" "$OUTPUT"
assert_status "box branches too" 1 "$BOX" branches linked --json
mv "$REAL" "$TMP/real-repo-moved"
ln -s "$W" "$REAL"
assert_status "box new after the folder of a project was replaced by a link" 1 "$BOX" new x2 moving
assert_status "box project set on it" 1 "$BOX" project set moving --install "true"
assert_no_file "no task was created" "$AGENTBOX_HOME/work/x1"
nothing_ran "nothing of the workspace's configuration ran"

echo " git metadata that leads into a workspace is not followed"
mkdir -p "$TMP/shell-link" "$TMP/shell-file"
ln -s "$W/.git" "$TMP/shell-link/.git"
printf 'gitdir: %s\n' "$W/.git" >"$TMP/shell-file/.git"
assert_status "a folder whose .git is a link to a workspace's" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$TMP/shell-link" --json
assert_status "a folder whose .git file names a workspace's" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$TMP/shell-file" --json
assert_status "neither can be added" 1 "$BOX" project add shell --repo "$TMP/shell-link"
git clone -q "$SOURCE" "$TMP/borrowing"
echo "$W/.git/objects" >"$TMP/borrowing/.git/objects/info/alternates"
assert_status "a repository that borrows objects from a workspace" "$EXIT_INSIDE_DATA_DIRECTORY" "$BOX" project detect "$TMP/borrowing" --json
git clone -q "$SOURCE" "$TMP/later"
"$BOX" project add later --repo "$TMP/later" >/dev/null
mv "$TMP/later/.git" "$TMP/later-git-moved"
ln -s "$W/.git" "$TMP/later/.git"
assert_status "a project whose .git was replaced by a link afterwards: box branches" 1 "$BOX" branches later --json
assert_contains "it says why" "data directory" "$OUTPUT"
assert_status "and box new" 1 "$BOX" new x3 later
git clone -q "$SOURCE" "$TMP/main-tree"
git -C "$TMP/main-tree" worktree add -q "$TMP/linked-tree" -b linked
assert_status "a linked worktree of the person's own repository is fine" 0 "$BOX" project detect "$TMP/linked-tree" --json
nothing_ran "nothing of the workspace's configuration ran"

echo " a folder of another user is not read"
# (/etc is a link to /private/etc on macOS and a folder of root on Linux.)
ROOT_FOLDER="$([ -d /private/etc ] && echo /private/etc || echo /etc)"
assert_status "detect on a folder root owns" "$EXIT_NOT_OWNED" "$BOX" project detect "$ROOT_FOLDER" --json
assert_contains "it says why" "not owned" "$OUTPUT"

echo " a repository that points its working tree elsewhere is not followed"
ELSEWHERE="$TMP/elsewhere"
git clone -q "$SOURCE" "$TMP/pointing"
mkdir -p "$ELSEWHERE"
git -C "$TMP/pointing" config core.worktree "$ELSEWHERE"
assert_status "detect" "$EXIT_NOT_A_REPOSITORY" "$BOX" project detect "$TMP/pointing" --json
assert_contains "it says that the folder is not a working tree" "not inside a git repository" "$OUTPUT"
assert_status "add" 1 "$BOX" project add pointing --repo "$TMP/pointing"

echo " the person's own repository: its configuration starts nothing either"
OWN="$TMP/own-repo"
git clone -q "$SOURCE" "$OWN"
hostile_config "$OWN"
assert_status "detect" 0 "$BOX" project detect "$OWN/sub" --json
assert_status "add" 0 "$BOX" project add own --repo "$OWN" --subdir sub --install true --check "build=echo build" --test-files "**/*.spec.ts"
assert_status "branches" 0 "$BOX" branches own --json
assert_status "projects" 0 "$BOX" projects --json
assert_status "new" 0 "$BOX" new o1 own
commit_in_task o1 work
assert_status "take" 0 "$BOX" take o1
assert_eq "the branch arrived" "$(workspace_head o1)" "$(git -C "$OWN" rev-parse refs/heads/box/o1 2>/dev/null)"
"$BOX" status --json >/dev/null 2>&1
"$BOX" tasks --json >/dev/null 2>&1
assert_status "rm" 0 "$BOX" rm o1
nothing_ran "no hook, monitor, pager or helper of the repository ran"
finish

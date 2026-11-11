#!/bin/bash
# A lavoro on the Mac: a worktree of the person's repository on a branch with the project's
# prefix, a setup command that runs once, its commits and patch read with plain git, and a
# removal that runs the archive command, refuses while work could be lost and keeps the branch.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
REPO="$(cd -P "$REPO" && pwd -P)"
MARKER="$TMP/ran-on-the-host"

commit_in() {
  echo "$2" >>"$1/sub/change.txt"
  git -C "$1" add -A && git -C "$1" -c user.name=t -c user.email=t@example.com commit -q -m "$2"
}

echo " the branch prefix of a project"
assert_status "add with a prefix" 0 "$BOX" project add app --repo "$REPO" --subdir sub --branch-prefix "me/" --check "test=true"
assert_eq "projects --json carries it" "me/" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .branchPrefix')"
assert_status "box new" 0 "$BOX" new p1 app
assert_eq "the branch has the prefix" "me/p1" "$(git -C "$AGENTBOX_HOME/work/p1" rev-parse --abbrev-ref HEAD)"
assert_status "a prefix that is an option of git is refused" 1 "$BOX" project set app --branch-prefix "--evil/"
assert_status "one with .. too" 1 "$BOX" project set app --branch-prefix "a/../b/"
assert_status "a project without one" 0 "$BOX" project add plain --repo "$REPO" --subdir sub
assert_eq "gets sakura/" "sakura/" "$("$BOX" projects --json | jq -r '.[] | select(.name == "plain") | .branchPrefix')"

echo " the setup command runs once, in the folder of the project, with its output in a private log"
assert_status "set --setup" 0 "$BOX" project set app --setup 'echo "setup of $SAKURA_TASK in $PWD"; echo "root $SAKURA_REPO and $CONDUCTOR_ROOT_PATH"; echo "tree $SAKURA_WORKTREE"; touch setup-ran'
assert_eq "projects --json carries it" "1" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .setup' | grep -c 'touch setup-ran')"
: >"$FAKE_DOCKER_LOG"
assert_status "box new" 0 "$BOX" new s1 app
W="$AGENTBOX_HOME/work/s1"
assert_file "it ran in the project's folder of the worktree" "$W/sub/setup-ran"
SETUP_LOG="$("$BOX" log s1 --setup)"
assert_contains "box log --setup shows its output" "setup of s1 in $W/sub" "$SETUP_LOG"
assert_contains "it knew the person's repository, under both names" "root $REPO and $REPO" "$SETUP_LOG"
assert_contains "and its worktree" "tree $W" "$SETUP_LOG"
assert_eq "the log is not in the worktree" "0" "$(find "$W" -name '*.log' | wc -l | tr -d ' ')"
assert_file "but in the data directory" "$AGENTBOX_HOME/setup/s1.log"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
"$BOX" project set app --setup 'echo "it breaks"; exit 7' >/dev/null
assert_status "a setup that fails: box new says so" 1 "$BOX" new s2 app
assert_contains "with the exit code and where the output is" "exit 7" "$OUTPUT"
assert_contains "and how to read it" "box log s2 --setup" "$OUTPUT"
assert_eq "the lavoro is kept" "1" "$([ -d "$AGENTBOX_HOME/work/s2" ] && echo 1 || echo 0)"
assert_contains "its log has the output" "it breaks" "$("$BOX" log s2 --setup)"
"$BOX" project set app --setup "sh -c 'echo \$\$ >\"$TMP/child.pid\"; sleep 300' & echo \$! >\"$TMP/parent.pid\"; wait" >/dev/null
STARTED=$SECONDS
assert_status "a setup that never ends is ended at its deadline" 1 env BOX_INSTALL_TIMEOUT=2 "$BOX" new s3 app
assert_eq "soon (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le 30 ] && echo 1 || echo 0)"
sleep 1
assert_eq "and nothing it started is left" "0" "$(still_alive "$(cat "$TMP/child.pid")")"
"$BOX" project set app --setup "" >/dev/null
"$BOX" new b1 app --where box >/dev/null 2>&1
assert_status "box log --setup of a task in a box" 80 "$BOX" log b1 --setup

echo " the commits and the patch of a lavoro on the Mac"
"$BOX" new r1 app >/dev/null
W="$AGENTBOX_HOME/work/r1"
commit_in "$W" "first"
commit_in "$W" "second"
echo "loose" >"$W/sub/loose.txt"
echo "edited" >>"$W/sub/existing.spec.ts"
: >"$FAKE_DOCKER_LOG"
LOG="$("$BOX" log r1 --json)"
assert_eq "box log --json: newest first" "second first" "$(jq -r '[.[].subject] | join(" ")' <<<"$LOG")"
assert_eq "each with its figures" "1 1 0" "$(jq -r '.[0] | "\(.filesChanged) \(.insertions) \(.deletions)"' <<<"$LOG")"
assert_eq "and the keys of a box" '["deletions","filesChanged","insertions","sha","subject","time"]' "$(jq -c '.[0] | keys' <<<"$LOG")"
assert_contains "box log without --json" "second" "$("$BOX" log r1)"
PATCH="$("$BOX" diff r1)"
assert_contains "box diff: the committed changes" "+second" "$PATCH"
assert_contains "the edits not committed" "+edited" "$PATCH"
assert_contains "and the files git does not know yet" "+loose" "$PATCH"
FIRST="$(jq -r '.[1].sha' <<<"$LOG")"
PATCH="$("$BOX" diff r1 --commit "$FIRST")"
assert_contains "box diff --commit: that commit" "+first" "$PATCH"
assert_eq "and nothing else" "0" "$(grep -c -e '+second' -e '+loose' <<<"$PATCH")"
assert_status "a commit that is not one of the lavoro" 1 "$BOX" diff r1 --commit "$(git -C "$REPO" rev-parse main)"
assert_eq "all of it without Docker" "" "$(cat "$FAKE_DOCKER_LOG")"

echo " box rm: refuses while work could be lost, runs the archive command first, keeps the branch"
assert_status "uncommitted files" 1 "$BOX" rm r1
assert_contains "it says so" "uncommitted" "$OUTPUT"
git -C "$W" add -A && git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m "third"
assert_status "commits that are neither pushed nor merged" 1 "$BOX" rm r1
assert_contains "it says so" "neither pushed nor merged" "$OUTPUT"
assert_eq "the worktree is still there" "1" "$([ -d "$W" ] && echo 1 || echo 0)"
git init -q --bare "$TMP/remote.git"
git -C "$REPO" remote add backup "$TMP/remote.git"
git -C "$W" push -q backup me/r1
"$BOX" project set app --archive "echo \"archive of \$SAKURA_TASK in \$PWD\" >\"$TMP/archived\"" >/dev/null
"$BOX" new r2 app >/dev/null
: >"$FAKE_DOCKER_LOG"
assert_status "once pushed, it goes" 0 "$BOX" rm r1
assert_no_file "the worktree is gone" "$W"
assert_eq "the person's repository no longer lists it" "0" "$(git -C "$REPO" worktree list | grep -c '/work/r1')"
assert_eq "the branch stays, with the work" "third" "$(git -C "$REPO" log -1 --format=%s me/r1)"
assert_no_file "the record is gone" "$AGENTBOX_HOME/tasks/r1.env"
assert_no_file "and the setup log" "$AGENTBOX_HOME/setup/r1.log"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_no_file "r1 kept the project values it was created with: no archive command ran" "$TMP/archived"
assert_status "a lavoro without changes goes at once" 0 "$BOX" rm r2
assert_contains "after its archive command ran in its folder" "archive of r2 in $AGENTBOX_HOME/work/r2/sub" "$(cat "$TMP/archived")"
"$BOX" new r3 app >/dev/null
commit_in "$AGENTBOX_HOME/work/r3" "merged later"
git -C "$REPO" merge -q --ff-only me/r3
assert_status "commits merged into the base are safe too" 0 "$BOX" rm r3
"$BOX" project set app --archive "echo no; exit 3" >/dev/null
"$BOX" new r4 app >/dev/null
assert_status "an archive command that fails stops the removal" 1 "$BOX" rm r4
assert_contains "it says so" "archive command" "$OUTPUT"
assert_eq "the lavoro is kept" "1" "$([ -d "$AGENTBOX_HOME/work/r4" ] && echo 1 || echo 0)"
echo "dirty" >"$AGENTBOX_HOME/work/r4/sub/dirty.txt"
assert_status "--force removes it all the same" 0 "$BOX" rm r4 --force
assert_no_file "gone" "$AGENTBOX_HOME/work/r4"
assert_eq "its branch stays" "1" "$(git -C "$REPO" branch --list me/r4 | wc -l | tr -d ' ')"
"$BOX" project set app --archive "" >/dev/null
"$BOX" new r5 app >/dev/null
rm -rf "$AGENTBOX_HOME/work/r5"
assert_status "a worktree deleted by hand: rm cleans up the rest" 0 "$BOX" rm r5
assert_eq "also in the person's repository" "0" "$(git -C "$REPO" worktree list | grep -c '/work/r5')"

echo " the worktree is checked like the repository, every time it is used"
"$BOX" new t1 sandbox --where box >/dev/null 2>&1
git -C "$AGENTBOX_HOME/work/t1" config core.fsmonitor "touch $MARKER-fsmonitor"
"$BOX" new v1 app >/dev/null
V="$AGENTBOX_HOME/work/v1"
cp "$V/.git" "$TMP/v1-pointer"
printf 'gitdir: %s\n' "$AGENTBOX_HOME/work/t1/.git" >"$V/.git"
assert_eq "a worktree whose .git names a workspace of a box is not read" "false" "$("$BOX" facts v1 --json | jq -r .readable)"
assert_status "box log" 1 "$BOX" log v1 --json
assert_status "box diff" 1 "$BOX" diff v1
assert_status "box rm" 1 "$BOX" rm v1
assert_contains "it says why" "box: " "$OUTPUT"
git clone -q "$SOURCE" "$TMP/another"
git -C "$TMP/another" worktree add -q "$TMP/another-tree" -b elsewhere
cp "$TMP/another-tree/.git" "$V/.git"
assert_eq "nor one that belongs to another repository of the person" "false" "$("$BOX" facts v1 --json | jq -r .readable)"
assert_status "box log says why" 1 "$BOX" log v1 --json
assert_contains "in one line" "does not name the git directory box recorded" "$OUTPUT"
cp "$TMP/v1-pointer" "$V/.git"
assert_eq "with its own pointer back it is read again" "true" "$("$BOX" facts v1 --json | jq -r .readable)"
echo "x" >"$REPO/.git/objects/info/alternates"
assert_status "a repository that starts borrowing objects stops every read" 1 "$BOX" facts v1 --json
assert_contains "with the reason" "objects/info/alternates" "$OUTPUT"
rm "$REPO/.git/objects/info/alternates"
assert_eq "nothing of the workspace's configuration ran on the host" "0" "$(/bin/ls "$TMP" | grep -c '^ran-on-the-host')"
finish

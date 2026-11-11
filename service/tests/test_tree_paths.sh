#!/bin/bash
# The checks of a task in a box run on Linux, where two names that differ in a single bit are
# two files. The person checks the branch out on a Mac, where names that differ only in letter
# case, or only in how an accented letter is composed, are one file, and where some names are
# taken for .git. A commit whose tree has such paths would be checked as one thing and checked
# out as another: it is refused before a check and before a take, and nothing reaches the
# repository of the person.
. "$(dirname "$0")/lib.sh"
setup_sandbox
problem_of() {
  PYTHONPATH="$BOX_ROOT/image/runner" python3 -B -c '
import sys, tree_check
print(tree_check.problem(sys.argv[1:]) or "none")' "$@"
}
E_COMPOSED="$(printf 'caf\xc3\xa9.txt')" E_DECOMPOSED="$(printf 'cafe\xcc\x81.txt')"

echo " names, as the check sees them"
assert_eq "ordinary paths" "none" "$(problem_of sub/package.json sub/src/a.ts README.md .gitignore .github/workflows/ci.yml)"
assert_contains "letter case only" "differ only in letter case" "$(problem_of sub/package.json sub/Package.json)"
assert_contains "letter case in a folder" "differ only in letter case" "$(problem_of sub/a.ts SUB/b.ts)"
assert_contains "the same letter composed in two ways" "differ only in letter case" "$(problem_of "sub/$E_COMPOSED" "sub/$E_DECOMPOSED")"
assert_eq "the same path twice is one path" "none" "$(problem_of sub/a.ts sub/a.ts)"
for name in .GIT/config sub/.Git/hooks/pre-commit "git~1/config" "sub/GIT~2" "$(printf '.g\xe2\x80\x8cit/config')" ".git./config" ".git /config"; do
  assert_contains "a name taken for .git: $(printf '%s' "$name" | LC_ALL=C tr -cd '[:print:]')" "take for .git" "$(problem_of "$name")"
done
assert_contains "a name that ends with a dot" "ends with a dot or a space" "$(problem_of 'sub/notes.')"
assert_contains "a name that ends with a space" "ends with a dot or a space" "$(problem_of 'sub/notes ')"
assert_contains "a name with a colon" "holds a colon" "$(problem_of 'sub/file.txt:hidden')"

for point in 200c 200d 200e 200f 202a 202b 202c 202d 202e 206a 206b 206c 206d 206e 206f feff; do
  NAME="$(python3 -c 'import sys; print("f" + chr(int(sys.argv[1], 16)) + "oo")' "$point")"
  assert_contains "HFS ignored character $point in a file" "ignored by HFS" "$(problem_of sub/foo "sub/$NAME")"
  assert_contains "HFS ignored character $point in a directory" "ignored by HFS" "$(problem_of sub/foo "$NAME/file")"
done

echo " a commit whose tree has package.json and Package.json in the same folder"
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
commit_in_task t1 work
# Made in the index: a Mac cannot hold the two files side by side, an agent's Linux can.
BLOB="$(echo '{"other":true}' | git -C "$W" hash-object -w --stdin)"
git -C "$W" update-index --add --cacheinfo "100644,$BLOB,sub/Package.json"
git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m "a second package file, by case"
HEAD_SHA="$(git -C "$W" rev-parse HEAD)"
assert_eq "(the tree holds both)" "2" "$(git -C "$W" ls-tree -r --name-only "$HEAD_SHA" | grep -c -i '^sub/package\.json$')"

echo " box take"
assert_status "is refused" 1 "$BOX" take t1
assert_contains "with a plain line that names the two paths" "differ only in letter case" "$OUTPUT"
assert_contains "and says that nothing was changed" "Nothing was changed" "$OUTPUT"
assert_eq "the branch did not reach the repository of the person" "0" "$(git -C "$SOURCE" branch --list box/t1 | wc -l | tr -d ' ')"

echo " a check"
: >"$FAKE_DOCKER_LOG"
"$BOX" verify t1 build --no-wait 2>/dev/null
"$BOX" runner --once >/dev/null 2>&1
RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/t1"/*-build.json | head -1)"
assert_eq "ends as refused (exit code 2)" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "the commit was not checked: the paths" "$(cat "${RESULT%.json}.log")"
assert_eq "no job was started for it" "0" "$(grep -c -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"

echo " a tree without such paths is checked and taken as before"
"$BOX" new t2 sandbox >/dev/null
commit_in_task t2 work
"$BOX" verify t2 build --no-wait 2>/dev/null
"$BOX" runner --once >/dev/null 2>&1
assert_eq "the check ran" "0" "$(jq -r .exit_code "$(/bin/ls -t "$AGENTBOX_HOME/results/t2"/*-build.json | head -1)")"
assert_status "box take" 0 "$BOX" take t2
assert_eq "the branch is in the repository" "$(workspace_head t2)" "$(git -C "$SOURCE" rev-parse refs/heads/box/t2)"
finish

#!/bin/bash
# The command of a check runs in the folder of the project (SUBDIR), and it can call a file of
# the repository that lies outside that folder (../run-tests.sh). So a commit that changes any
# path outside SUBDIR counts as a change to the checks, with the reason, unless the project
# lists the path as safe (SAFE_OUTSIDE, patterns from the root of the repository). And a
# protected pattern protects a file wherever it lies in the repository, not only under SUBDIR.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t1"
# result_of <task>: the build verb on the head of the task, the newest result file.
result_of() {
  "$BOX" verify "$1" build --no-wait 2>/dev/null
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-build.json | head -1
}
commit_file() {
  mkdir -p "$(dirname "$W/$1")"
  echo "$RANDOM" >>"$W/$1"
  # (--sparse: the workspace of a box holds the cone of the project; an agent can add outside it.)
  git -C "$W" add --sparse -A && git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m "change $1"
}
checks() {
  jq -c '[.changesChecks, .checksTouched, .checksReason]' "$1"
}
readonly REASON="the commit changes files outside the folder of the project"

echo " a commit that changes only a script at the top of the repository, outside sub/"
commit_file run-tests.sh
assert_eq "counts as a change to the checks, names the file and says why" "[true,[\"run-tests.sh\"],\"$REASON\"]" "$(checks "$(result_of t1)")"
assert_eq "box facts shows the reason" "$REASON" "$("$BOX" facts t1 --json | jq -r '.results.build.checksReason')"

echo " a commit that changes only a file inside sub/ that is not protected"
"$BOX" new t2 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t2"
commit_file sub/src/feature.ts
assert_eq "is not a change to the checks" '[false,[],null]' "$(checks "$(result_of t2)")"

echo " a path the project lists as safe"
"$BOX" project set sandbox --safe-outside 'change.txt docs/**' >/dev/null
"$BOX" new t3 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t3"
commit_file docs/notes/readme.md
assert_eq "outside sub/ and listed: not a change" '[false,[],null]' "$(checks "$(result_of t3)")"
commit_file other/readme.md
assert_eq "outside sub/ and not listed: a change" "[true,[\"other/readme.md\"],\"$REASON\"]" "$(checks "$(result_of t3)")"
assert_eq "projects --json carries the list" "change.txt docs/**" "$("$BOX" projects --json | jq -r '.[] | select(.name == "sandbox") | .safeOutside')"

echo " a protected pattern protects a file outside sub/ too, even where everything outside is listed as safe"
"$BOX" project set sandbox --safe-outside '**' >/dev/null
"$BOX" new t4 sandbox >/dev/null
W="$AGENTBOX_HOME/work/t4"
commit_file tools/helper.txt
assert_eq "(a file outside that is listed and not protected: not a change)" '[false,[],null]' "$(checks "$(result_of t4)")"
commit_file scripts/build.mjs
assert_eq "scripts/** at the top of the repository: a change, as a protected file" '[true,["scripts/build.mjs"],null]' "$(checks "$(result_of t4)")"

echo " a lavoro on the Mac follows the same rule"
unset BOX_DEFAULT_WHERE
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --check "build=true" --protected 'package.json' --test-files '**/*.spec.ts' >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
commit_file run-tests.sh
assert_eq "a script outside sub/: a change, with the reason" "[true,[\"run-tests.sh\"],\"$REASON\"]" "$(checks "$(result_of m1)")"
finish

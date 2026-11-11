#!/bin/bash
# box new <task> <project> [--base <branch>]: a task starts from the project's base branch, or
# from another branch of the repository that the person names. Nothing else is accepted.
. "$(dirname "$0")/lib.sh"
setup_sandbox
git -C "$SOURCE" checkout -q -b release
echo "only on release" >"$SOURCE/sub/release.txt"
git -C "$SOURCE" add -A && git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m "release work"
git -C "$SOURCE" checkout -q main
MAIN="$(git -C "$SOURCE" rev-parse main)"
RELEASE="$(git -C "$SOURCE" rev-parse release)"

echo " without --base the task starts from the project's base"
assert_status "box new" 0 "$BOX" new t1 sandbox
assert_eq "at the tip of that branch" "$MAIN" "$(workspace_head t1)"
assert_contains "the task records the branch" "BASE=main" "$(cat "$AGENTBOX_HOME/tasks/t1.env")"
assert_contains "and its commit" "BASE_SHA=$MAIN" "$(cat "$AGENTBOX_HOME/tasks/t1.env")"

echo " with --base it starts from that branch"
assert_status "box new --base release" 0 "$BOX" new t2 sandbox --base release
assert_eq "at the tip of that branch" "$RELEASE" "$(workspace_head t2)"
assert_file "with its files" "$AGENTBOX_HOME/work/t2/sub/release.txt"
assert_contains "the task records the branch" "BASE=release" "$(cat "$AGENTBOX_HOME/tasks/t2.env")"
assert_contains "and its commit" "BASE_SHA=$RELEASE" "$(cat "$AGENTBOX_HOME/tasks/t2.env")"
assert_eq "the task's own branch is still box/<task>" "box/t2" "$(git -C "$AGENTBOX_HOME/work/t2" rev-parse --abbrev-ref HEAD)"
assert_eq "box facts shows the task's base, not the project's" "release $RELEASE 0" "$("$BOX" facts t2 --json 2>/dev/null | jq -r '"\(.base) \(.baseSha) \(.ahead)"')"
assert_eq "box tasks too" "release" "$("$BOX" tasks --json | jq -r '.[] | select(.name == "t2") | .base')"
assert_eq "the other task keeps the project's" "main" "$("$BOX" tasks --json | jq -r '.[] | select(.name == "t1") | .base')"
assert_eq "the project itself is unchanged" "main" "$("$BOX" projects --json | jq -r '.[] | select(.name == "sandbox") | .base')"

echo " a branch that the repository does not have"
assert_status "is refused" 1 "$BOX" new t3 sandbox --base nope
assert_contains "with one line that says what to do" "box branches sandbox" "$OUTPUT"
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/t3"
assert_no_file "no record either" "$AGENTBOX_HOME/tasks/t3.env"
assert_status "a commit id is not a branch" 1 "$BOX" new t3 sandbox --base "$RELEASE"
assert_status "something that looks like an option is not a branch" 1 "$BOX" new t3 sandbox --base "--upload-pack=touch"
assert_status "a remote branch is not a local one" 1 "$BOX" new t3 sandbox --base origin/main

echo " nothing else is accepted"
assert_status "an unknown option" 1 "$BOX" new t3 sandbox --preset builder
assert_contains "it says which" "unknown option '--preset'" "$OUTPUT"
assert_status "--base without a branch" 1 "$BOX" new t3 sandbox --base
assert_status "a word too many" 1 "$BOX" new t3 sandbox release
assert_status "no project" 1 "$BOX" new t3
assert_contains "the usage" "box new <task> <project>" "$OUTPUT"
assert_no_file "nothing was created by any of them" "$AGENTBOX_HOME/work/t3"
finish

#!/bin/bash
# A lavoro has a place, chosen when it is created: the Mac (a worktree of the person's
# repository) or a box (a clone in a container). The project names the default; the task
# records its place; tasks, status and facts carry it; a verb that does not apply says so.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
readonly DOES_NOT_APPLY_EXIT=80
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
git -C "$REPO" branch other

echo " a project names the place of its lavori; the Mac when it does not say"
assert_status "add without a place" 0 "$BOX" project add app --repo "$REPO" --subdir sub --check "test=true"
assert_eq "projects --json says mac" "mac" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .where')"
assert_status "set --where box" 0 "$BOX" project set app --where box
assert_eq "it is box now" "box" "$("$BOX" projects --json | jq -r '.[] | select(.name == "app") | .where')"
assert_contains "and the file says so" "WHERE=box" "$(cat "$AGENTBOX_HOME/projects/app.env")"
assert_status "a place that does not exist is refused" 1 "$BOX" project set app --where moon
assert_contains "it says which there are" "mac or box" "$OUTPUT"
assert_status "back to mac" 0 "$BOX" project set app --where mac

echo " box new on the Mac: a worktree of the repository on its own branch"
: >"$FAKE_DOCKER_LOG"
assert_status "box new" 0 "$BOX" new m1 app
assert_contains "it says where the folder is" "$AGENTBOX_HOME/work/m1" "$OUTPUT"
assert_eq "the folder is a worktree (its .git is a file)" "1" "$([ -f "$AGENTBOX_HOME/work/m1/.git" ] && echo 1 || echo 0)"
assert_file "with the files of the base" "$AGENTBOX_HOME/work/m1/sub/package.json"
assert_eq "the branch is in the person's repository, at the base" "$(git -C "$REPO" rev-parse main)" "$(git -C "$REPO" rev-parse refs/heads/sakura/m1 2>/dev/null)"
assert_eq "the worktree is on it" "sakura/m1" "$(git -C "$AGENTBOX_HOME/work/m1" rev-parse --abbrev-ref HEAD)"
assert_contains "the record says mac" "WHERE=mac" "$(cat "$AGENTBOX_HOME/tasks/m1.env")"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_status "a second lavoro of that name is refused" 1 "$BOX" new m1 app
git -C "$REPO" branch sakura/taken
assert_status "a branch sakura/<task> that the repository already has is not taken over" 1 "$BOX" new taken app
assert_contains "it says so" "sakura/taken already exists" "$OUTPUT"
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/taken"

echo " --where chooses the place for one lavoro, with --base in either order"
assert_status "--where box" 0 "$BOX" new b1 app --where box
assert_contains "the record says box" "WHERE=box" "$(cat "$AGENTBOX_HOME/tasks/b1.env")"
assert_eq "it is a clone (its .git is a folder)" "1" "$([ -d "$AGENTBOX_HOME/work/b1/.git" ] && echo 1 || echo 0)"
assert_status "--base then --where" 0 "$BOX" new m2 app --base other --where mac
assert_status "--where then --base" 0 "$BOX" new m3 app --where mac --base other
assert_eq "both start from the branch named" "other other" "$(sed -n 's/^BASE=//p' "$AGENTBOX_HOME/tasks/m2.env" "$AGENTBOX_HOME/tasks/m3.env" | tr '\n' ' ' | sed 's/ $//')"
assert_status "a place that does not exist" 1 "$BOX" new x1 app --where moon
assert_status "an option given twice" 1 "$BOX" new x1 app --where mac --where box
assert_status "an unknown option" 1 "$BOX" new x1 app --here
assert_no_file "nothing was created" "$AGENTBOX_HOME/work/x1"

echo " tasks, facts and status carry the place"
assert_eq "tasks --json" "b1:box m1:mac m2:mac m3:mac" "$("$BOX" tasks --json | jq -r '[.[] | "\(.name):\(.where)"] | join(" ")')"
: >"$FAKE_DOCKER_LOG"
FACTS="$("$BOX" facts m1 --json)"
assert_eq "facts of a lavoro on the Mac" "mac" "$(jq -r .where <<<"$FACTS")"
assert_eq "read without Docker" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "facts of one in a box" "box" "$("$BOX" facts b1 --json | jq -r .where)"
assert_eq "status --json" "b1:box m1:mac m2:mac m3:mac" "$("$BOX" status --json | jq -r '[.tasks[] | "\(.name):\(.where)"] | join(" ")')"
edit_in_place '/^WHERE=/d' "$AGENTBOX_HOME/tasks/b1.env"
assert_eq "a task created before places existed is in a box" "box" "$("$BOX" facts b1 --json | jq -r .where)"

echo " the facts of a lavoro on the Mac are read with plain git"
W="$AGENTBOX_HOME/work/m1"
assert_eq "at the start" "sakura/m1 0 0 0 true" "$(jq -r '"\(.branch) \(.ahead) \(.dirtyFiles) \(.filesChanged) \(.readable)"' <<<"$FACTS")"
assert_eq "its head is the base" "$(git -C "$REPO" rev-parse main)" "$(jq -r .head <<<"$FACTS")"
printf 'one\ntwo\n' >"$W/sub/new.txt"
git -C "$W" add -A && git -C "$W" -c user.name=t -c user.email=t@example.com commit -q -m "a first commit"
echo "more" >>"$W/sub/new.txt"
echo "untracked" >"$W/sub/loose.txt"
FACTS="$("$BOX" facts m1 --json)"
assert_eq "after a commit and two loose changes" "1 2 1 3 0" "$(jq -r '"\(.ahead) \(.dirtyFiles) \(.filesChanged) \(.insertions) \(.deletions)"' <<<"$FACTS")"
assert_eq "the head and its subject" "$(git -C "$W" rev-parse HEAD) a first commit" "$(jq -r '"\(.head) \(.headSubject)"' <<<"$FACTS")"
assert_eq "the time of the head, in UTC" "1" "$(jq -r .headTime <<<"$FACTS" | grep -c -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$')"
assert_eq "what does not exist on the Mac is null or empty" "null null 0" "$(jq -r '"\(.taken) \(.agent) \(.otherContainers)"' <<<"$FACTS")"
assert_eq "the project and its checks are there as for a box" "app test main" "$(jq -r '"\(.project) \(.checks | join(",")) \(.base)"' <<<"$FACTS")"

git -C "$REPO" branch box/m1
assert_eq "a branch box/<task> the repository happens to have does not make a lavoro on the Mac taken" "false" "$("$BOX" tasks --json | jq -r '.[] | select(.name == "m1") | .taken')"

echo " a verb that does not apply to the place says so in one line"
assert_status "take" "$DOES_NOT_APPLY_EXIT" "$BOX" take m1
assert_contains "why" "the branch is already in your repository" "$OUTPUT"
assert_eq "one box: line" "1" "$(grep -c '^box: ' <<<"$OUTPUT")"
for verb in run agent attach; do
  assert_status "$verb" "$DOES_NOT_APPLY_EXIT" "$BOX" "$verb" m1 personal
  assert_eq "  one box: line" "1 1" "$(grep -c '^box: ' <<<"$OUTPUT") $(wc -l <<<"$OUTPUT" | tr -d ' ')"
done
finish

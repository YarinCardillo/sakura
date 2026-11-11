#!/bin/bash
# The two places never mix. A lavoro on the Mac is the person's own worktree, read with host
# git; a task in a box is an untrusted workspace, read only in containers. So a task on the Mac
# never reaches a container (nothing of it is mounted, no drop directory exists for it) and a
# task in a box is never read with host git. Every verb of box help is walked against one task
# of each place and the path it takes is asserted.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_ACCOUNTS="personal"
readonly DOES_NOT_APPLY_EXIT=80
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub --install true --check "test=echo test" >/dev/null
mkdir -p "$TMP/login"
# box shell on the Mac opens the person's shell: here one that ends at once.
printf '#!/bin/bash\nexit 0\n' >"$TMP/shell"
chmod +x "$TMP/shell"
export SHELL="$TMP/shell"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
"$BOX" new m1 app >/dev/null
"$BOX" new m2 app >/dev/null
"$BOX" new b1 app --where box >/dev/null
"$BOX" new b2 app --where box >/dev/null
MAC_DIR="$AGENTBOX_HOME/work/m1"
OTHER_MAC_DIR="$AGENTBOX_HOME/work/m2"

# What each verb does with a task of each place.
#   refused:   one box: line, exit code 80, Docker is not requested anything
#   native:    exit code 0, Docker is not requested anything
#   nothing:   the native path had nothing to do: one box: line, exit code 1, no Docker
#   host:      it reads files of the data directory only: the same for both places, no Docker
#   unbuilt:   the part of the verb for this place does not exist yet: one box: line, exit code 81, no Docker
#   container: Docker is asked (the verb belongs to a box, or reads its workspace in a sandbox)
#   none:      the verb takes no task
# <verb>|<arguments, T is the task>|<on the Mac>|<in a box>
readonly PATHS='
run|run T -- true|refused|container
agent|agent T personal --detach|refused|container
acp|acp T personal|native|container
shell|shell T|native|container
attach|attach T|refused|container
stop|stop T|nothing|container
verify|verify T test --no-wait|native|container
cancel|cancel T test|native|container
take|take T|refused|container
play|play T status|native|unbuilt
facts|facts T --json|native|container
diff|diff T|native|container
log|log T --json|native|container
tree|tree T --json|native|container
file|file T sub/package.json|native|container
search|search T package --json|native|container
graph|graph T --json|native|container
result-log|result-log T test|host|host
rm|rm T|native|container
new|-|none|none
up|-|none|none
project|-|none|none
projects|-|none|none
branches|-|none|none
login|-|none|none
runner|-|none|none
tasks|-|none|none
status|-|none|none
journal|-|none|none
accounts|-|none|none
'

echo " every verb of box help is in the table"
HELP_VERBS="$("$BOX" help | sed -n 's/^  \([a-z][a-z-]*\).*/\1/p' | sort -u)"
TABLE_VERBS="$(printf '%s' "$PATHS" | sed -n 's/^\([a-z-]*\)|.*/\1/p' | sort -u)"
assert_eq "the verbs of box help and of the table are the same" "$HELP_VERBS" "$TABLE_VERBS"
for verb in run agent acp shell attach stop verify cancel take play facts diff log tree file search graph result-log rm; do
  assert_contains "box help names $verb" "$verb" "$HELP_VERBS"
done

# walk <place> <task>: runs every verb that takes a task and asserts its path.
walk() {
  local place="$1" task="$2" verb arguments on_mac in_box expected status calls
  while IFS='|' read -r verb arguments on_mac in_box; do
    [ -n "$verb" ] && [ "$arguments" != "-" ] || continue
    expected="$on_mac"
    [ "$place" = "mac" ] || expected="$in_box"
    : >"$FAKE_DOCKER_LOG"
    # shellcheck disable=SC2086
    OUTPUT="$("$BOX" ${arguments//T/$task} </dev/null 2>&1)"
    status=$?
    calls="$(grep -c . "$FAKE_DOCKER_LOG")"
    case "$expected" in
      refused)
        assert_eq "$verb, $place: exit code 80" "$DOES_NOT_APPLY_EXIT" "$status"
        assert_eq "  one box: line" "1 1" "$(grep -c '^box: ' <<<"$OUTPUT") $(wc -l <<<"$OUTPUT" | tr -d ' ')"
        assert_eq "  Docker is not requested anything" "0" "$calls" ;;
      native)
        assert_eq "$verb, $place: exit code 0" "0" "$status"
        assert_eq "  Docker is not requested anything" "0" "$calls" ;;
      unbuilt)
        assert_eq "$verb, $place: exit code 81" "81" "$status"
        assert_eq "  one box: line" "1 1" "$(grep -c '^box: ' <<<"$OUTPUT") $(wc -l <<<"$OUTPUT" | tr -d ' ')"
        assert_eq "  Docker is not requested anything" "0" "$calls" ;;
      nothing)
        assert_eq "$verb, $place: exit code 1" "1" "$status"
        assert_eq "  one box: line" "1 1" "$(grep -c '^box: ' <<<"$OUTPUT") $(wc -l <<<"$OUTPUT" | tr -d ' ')"
        assert_eq "  Docker is not requested anything" "0" "$calls" ;;
      host)
        assert_eq "$verb, $place: Docker is not requested anything" "0" "$calls" ;;
      container)
        assert_eq "$verb, $place: Docker is asked" "1" "$([ "$calls" -gt 0 ] && echo 1 || echo 0)" ;;
    esac
    if [ "$place" = "mac" ]; then
      assert_eq "  nothing of the worktree reaches Docker" "0" "$(grep -c -e "/work/$task" -e "/drop/$task" "$FAKE_DOCKER_LOG")"
    fi
  done <<<"$PATHS"
}

echo " a task on the Mac: native or refused, never a container"
walk mac m1
assert_no_file "no drop directory exists for it" "$AGENTBOX_HOME/drop/m1"
assert_no_file "box rm removed it natively" "$MAC_DIR"
assert_eq "and its branch stays in the person's repository" "1" "$(git -C "$REPO" branch --list sakura/m1 | wc -l | tr -d ' ')"

echo " the verbs without a task, with only a task on the Mac beside one in a box"
: >"$FAKE_DOCKER_LOG"
"$BOX" status --json >/dev/null 2>&1
"$BOX" status >/dev/null 2>&1
"$BOX" tasks --json >/dev/null 2>&1
assert_eq "status and tasks hand nothing of the worktree to Docker" "0" "$(grep -c -e "/work/m2" -e "/drop/m2" "$FAKE_DOCKER_LOG")"

echo " the runner never starts a container for a task on the Mac"
mkdir -p "$AGENTBOX_HOME/queue" "$AGENTBOX_HOME/drop/m2"
HEAD_SHA="$(git -C "$OTHER_MAC_DIR" rev-parse HEAD)"
printf 'task=m2\nverb=test\nsha=%s\nrequested=2026-11-10T00:00:00Z\ngeneration=%s\nstate=queued\n' "$HEAD_SHA" \
  "$(sed -n 's/^GENERATION=//p' "$AGENTBOX_HOME/tasks/m2.env")" >"$AGENTBOX_HOME/queue/20261110T000000-$HEAD_SHA-test.m2.req"
: >"$AGENTBOX_HOME/drop/m2/test"
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once >"$TMP/runner.out" 2>&1
assert_eq "a request for it gets no container: its check runs natively" "0" "$(grep -c -e "/work/m2" -e "/drop/m2" -e 'agentbox.task=m2' "$FAKE_DOCKER_LOG")"
assert_eq "it leaves the queue" "0" "$(/bin/ls "$AGENTBOX_HOME/queue" | grep -c 'm2')"
assert_eq "a drop directory made by hand for it is not read" "1" "$([ -e "$AGENTBOX_HOME/drop/m2/test" ] && echo 1 || echo 0)"
rm -rf "$AGENTBOX_HOME/drop/m2"

echo " the functions that mount a workspace refuse a task on the Mac themselves"
for call in 'build_agent_flags m2; add_workspace_flags m2 agentbox-ws-m2-x' 'workspace_sandbox m2 -- /box/head.py' \
  'verified_sandbox m2 -- /box/log.py a b' 'import_commit m2 '"$HEAD_SHA" 'drop_kinds m2' 'install_dependencies m2' 'run_job m2 key '"$HEAD_SHA"' true'; do
  : >"$FAKE_DOCKER_LOG"
  OUTPUT="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project project_detect project_admin sandbox native journal task request checks factscache cancel launchd runner; do . "$BOX_ROOT/lib/$lib.sh"; done
    load_task_project m2
    '"$call" 2>&1)"
  assert_eq "${call%% *}: exit code 80" "$DOES_NOT_APPLY_EXIT" "$?"
  assert_contains "  one line that says why" "box: task 'm2' is on the Mac" "$OUTPUT"
  assert_eq "  Docker is not requested anything" "0" "$(grep -c . "$FAKE_DOCKER_LOG")"
done

echo " a task in a box: its workspace is read in containers, never with host git"
walk box b1
OUTPUT="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project project_detect project_admin sandbox native; do . "$BOX_ROOT/lib/$lib.sh"; done
  load_task_project b2
  lavoro_git b2 rev-parse HEAD; echo "status=$?"' 2>&1)"
assert_contains "host git on the workspace of a box is refused" "status=1" "$OUTPUT"
assert_eq "and prints no commit" "0" "$(grep -c -E '^[0-9a-f]{40}$' <<<"$OUTPUT")"
finish

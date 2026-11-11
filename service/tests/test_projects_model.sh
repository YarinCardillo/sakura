#!/bin/bash
# A project (repository, subfolder, base branch, install and check commands) is a data file in
# the data directory, not a file of the code: $AGENTBOX_HOME/projects/<name>.env. It is read as
# data, never evaluated, and refused when it holds an unknown key or an unsafe value.
. "$(dirname "$0")/lib.sh"
setup_sandbox
PROJECTS="$AGENTBOX_HOME/projects"

# project_file <name> <lines...>: a project file as a person might write it by hand.
project_file() {
  local name="$1"
  shift
  mkdir -p "$PROJECTS"
  printf '%s\n' "$@" >"$PROJECTS/$name.env"
}

echo " a task is created from a project of the data directory"
assert_file "(the test project is a file of the data directory)" "$PROJECTS/sandbox.env"
assert_status "box new with the project" 0 "$BOX" new t1 sandbox
assert_contains "the task records its project" "PROFILE=sandbox" "$(cat "$AGENTBOX_HOME/tasks/t1.env")"
assert_status "a project that does not exist" 1 "$BOX" new t2 nope
assert_contains "it says so" "box: unknown project 'nope'" "$OUTPUT"

echo " nothing in a project file is evaluated"
project_file evil "REPO=$SOURCE" "BASE=main" "SUBDIR=sub" 'INSTALL=true $(touch '"$TMP"'/pwned-install)' \
  'VERB_test=echo `touch '"$TMP"'/pwned-verb`' 'PROTECTED=$(touch '"$TMP"'/pwned-protected)'
"$BOX" new t3 evil >/dev/null 2>&1
"$BOX" facts t3 --json >/dev/null 2>&1
"$BOX" verify t3 test --no-wait >/dev/null 2>&1
"$BOX" runner --once >/dev/null 2>&1
assert_eq "no command of the file ran on the host" "0" "$(/bin/ls "$TMP" | grep -c pwned)"
assert_contains "the command reached the job container as text" 'BOX_VERB_CMD=echo `touch' "$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"

echo " a file with an unknown key or an unsafe value is refused"
# assert_refused <name of the case> <words expected in the message> <lines of the file...>
assert_refused() {
  local name="$1" words="$2"
  shift 2
  project_file bad "$@"
  assert_status "$name" 1 "$BOX" new tbad bad
  assert_contains "  one box: line that names the project" "box: project 'bad'" "$OUTPUT"
  assert_contains "  and the reason" "$words" "$OUTPUT"
  assert_no_file "  nothing was created" "$AGENTBOX_HOME/work/tbad"
}
assert_refused "an unknown key" "unknown key 'PATH'" "REPO=$SOURCE" "BASE=main" "PATH=/tmp"
assert_refused "a shell line" "not KEY=value" "REPO=$SOURCE" "BASE=main" "touch /tmp/x"
assert_refused "an exported variable" "unknown key" "REPO=$SOURCE" "BASE=main" "export FOO=1"
assert_refused "a repository that is not an absolute path" "REPO" "REPO=relative/path" "BASE=main"
assert_refused "a repository with .." "REPO" "REPO=$SOURCE/../source" "BASE=main"
assert_refused "a subfolder that climbs out" "SUBDIR" "REPO=$SOURCE" "BASE=main" "SUBDIR=../other"
assert_refused "an absolute subfolder" "SUBDIR" "REPO=$SOURCE" "BASE=main" "SUBDIR=/etc"
assert_refused "a base that looks like an option" "BASE" "REPO=$SOURCE" "BASE=--upload-pack=x"
assert_refused "a control character in a command" "INSTALL" "REPO=$SOURCE" "BASE=main" "INSTALL=npm ci"$'\x1b'"[2J"
assert_refused "a sparse flag that is not 0 or 1" "SPARSE" "REPO=$SOURCE" "BASE=main" "SPARSE=yes"
assert_refused "a verb key that is not a verb" "unknown key" "REPO=$SOURCE" "BASE=main" "VERB_Test=echo"
assert_refused "no repository" "REPO" "BASE=main"
assert_refused "no base branch" "BASE" "REPO=$SOURCE"
assert_refused "a repository that does not exist" "does not exist" "REPO=$TMP/gone" "BASE=main"
assert_refused "a repository inside the data directory" "data directory" "REPO=$AGENTBOX_HOME/work/t1" "BASE=box/t1"
rm -f "$PROJECTS/bad.env"

echo " a project at the root of a repository, without an install command"
project_file rootp "REPO=$SOURCE" "BASE=main" "VERB_check=echo check" "PROTECTED=sub/package.json" "TEST_FILES=**/*.spec.ts"
: >"$FAKE_DOCKER_LOG"
assert_status "box new" 0 "$BOX" new r1 rootp
assert_eq "no dependencies are mounted into its boxes" "0" "$(grep -c 'node_modules' "$FAKE_DOCKER_LOG")"
assert_file "the whole repository is checked out" "$AGENTBOX_HOME/work/r1/sub/package.json"
commit_in_task r1 change
"$BOX" verify r1 check --no-wait 2>/dev/null
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
RESULT="$(/bin/ls "$AGENTBOX_HOME/results/r1"/*-check.json)"
assert_eq "its check ran and passed" "0" "$(jq -r .exit_code "$RESULT")"
assert_eq "no dependencies were prepared" "0" "$(grep -c -e 'agentbox-install-' -e '/box/extract.sh' -e 'node_modules' "$FAKE_DOCKER_LOG")"
assert_contains "the job ran at the root of the checkout" "BOX_SUBDIR= " "$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG")"
assert_eq "the protected paths are relative to the root" "false" "$(jq -r .changesChecks "$RESULT")"
echo '{"x":1}' >"$AGENTBOX_HOME/work/r1/sub/package.json"
commit_in_task r1 protected
"$BOX" verify r1 check --no-wait 2>/dev/null
"$BOX" runner --once 2>/dev/null
assert_eq "a change to one of them is seen" '["sub/package.json"]' "$(jq -c .checksTouched "$(/bin/ls -t "$AGENTBOX_HOME/results/r1"/*-check.json | head -1)")"

echo " on upgrade the shipped profiles become the first projects"
UPGRADE="$TMP/upgrade-home"
mkdir -p "$BOX_PROFILES_DIR"
printf '%s\n' "# shipped" "REPO=$SOURCE" "BASE=main" "SUBDIR=sub" "INSTALL=true" "VERB_test=echo test" >"$BOX_PROFILES_DIR/sample-app.env"
printf '%s\n' "REPO=@AGENTBOX_HOME@/selftest-source" "BASE=main" "SUBDIR=sub" "INSTALL=true" "VERB_probe=echo probe" >"$BOX_PROFILES_DIR/selftest.env"
export AGENTBOX_HOME="$UPGRADE"
git clone -q "$SOURCE" "$UPGRADE/selftest-source"
"$BOX" tasks --json >/dev/null 2>&1
assert_file "the main one is a project after the first command" "$UPGRADE/projects/sample-app.env"
assert_contains "with the fields of the shipped file" "VERB_test=echo test" "$(cat "$UPGRADE/projects/sample-app.env")"
assert_no_file "the self-test one is not, until something asks for it" "$UPGRADE/projects/selftest.env"
assert_status "a task of the main project" 0 "$BOX" new m1 sample-app
assert_status "a task of the self-test project asks for it" 0 "$BOX" new s1 selftest
assert_contains "its repository is the one of this data directory" "REPO=$UPGRADE/selftest-source" "$(cat "$UPGRADE/projects/selftest.env")"

echo " existing tasks keep working when the projects directory appears later"
rm -rf "$UPGRADE/projects"
assert_eq "the task of the main project is read" "sample-app main" "$("$BOX" facts m1 --json 2>/dev/null | jq -r '"\(.profile) \(.base)"')"
assert_eq "the task of the self-test project too" "selftest" "$("$BOX" facts s1 --json 2>/dev/null | jq -r .profile)"

echo " a shipped profile is imported once: a project the person removed or changed stays that way"
rm -f "$UPGRADE/projects/sample-app.env"
"$BOX" tasks --json >/dev/null 2>&1
assert_no_file "the removed project does not come back" "$UPGRADE/projects/sample-app.env"
"$BOX" new s2 selftest >/dev/null 2>&1
edit_in_place 's/^VERB_probe=.*/VERB_probe=echo changed/' "$UPGRADE/projects/selftest.env"
"$BOX" new s3 selftest >/dev/null 2>&1
assert_contains "the changed project is not overwritten" "VERB_probe=echo changed" "$(cat "$UPGRADE/projects/selftest.env")"

echo " a shipped profile whose repository is not on this Mac is not imported"
export AGENTBOX_HOME="$TMP/fresh-home"
printf '%s\n' "REPO=$TMP/not-here" "BASE=main" >"$BOX_PROFILES_DIR/sample-app.env"
assert_status "the first command works" 0 "$BOX" tasks --json
assert_no_file "and there is no such project" "$TMP/fresh-home/projects/sample-app.env"
finish

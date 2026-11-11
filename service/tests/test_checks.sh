#!/bin/bash
# Results say whether a commit changes the checks (changesChecks, checksTouched), which existing
# test files it modified or deleted (testsTouched) and how many tests ran (tests).
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/t1"

# run_verb <task> <verb>: queue, run, print the newest result file of the task for the verb.
run_verb() {
  "$BOX" verify "$1" "$2" --no-wait 2>/dev/null
  "$BOX" runner --once 2>/dev/null
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-"$2".json | head -1
}

echo " a commit that does not touch the checks"
commit_in_task t1 plain
R="$(run_verb t1 build)"
assert_eq "changesChecks false" "false" "$(jq -r .changesChecks "$R")"
assert_eq "nothing touched" "[]" "$(jq -c .checksTouched "$R")"
assert_eq "no test file touched" '{"modified":0,"deleted":0,"files":[]}' "$(jq -c .testsTouched "$R")"
assert_eq "not a test verb: tests null" "null" "$(jq -c .tests "$R")"

echo " a commit that changes a protected file"
echo '{"x":1}' >"$WORKSPACE/sub/package.json"
git -C "$WORKSPACE" add -A && git -C "$WORKSPACE" -c user.name=t -c user.email=t@example.com commit -q -m pkg
R="$(run_verb t1 build)"
assert_eq "changesChecks true" "true" "$(jq -r .changesChecks "$R")"
assert_eq "the file is named" '["sub/package.json"]' "$(jq -c .checksTouched "$R")"
assert_eq "status shows the field" "true" "$("$BOX" facts t1 --json | jq -r '.results.build.changesChecks')"
assert_eq "status shows the list" '["sub/package.json"]' "$("$BOX" facts t1 --json | jq -c '.results.build.checksTouched')"

echo " a directory pattern, and at most 20 names"
mkdir -p "$WORKSPACE/sub/scripts"
for i in $(seq 1 25); do echo "$i" >"$WORKSPACE/sub/scripts/f$i.mjs"; done
git -C "$WORKSPACE" add -A && git -C "$WORKSPACE" -c user.name=t -c user.email=t@example.com commit -q -m scripts
R="$(run_verb t1 build)"
assert_eq "scripts/** matches" "true" "$(jq -r .changesChecks "$R")"
assert_eq "the list is capped" "20" "$(jq '.checksTouched | length' "$R")"

echo " existing test files modified or deleted, new ones do not count"
"$BOX" new t2 sandbox >/dev/null
W2="$AGENTBOX_HOME/work/t2"
echo changed >>"$W2/sub/existing.spec.ts"
git -C "$W2" rm -q sub/other.spec.ts
echo new >"$W2/sub/brand-new.spec.ts"
git -C "$W2" add -A && git -C "$W2" -c user.name=t -c user.email=t@example.com commit -q -m tests
R="$(run_verb t2 build)"
assert_eq "one modified, one deleted" "1 1" "$(jq -r '.testsTouched | "\(.modified) \(.deleted)"' "$R")"
assert_eq "the files, not the new one" '["sub/existing.spec.ts","sub/other.spec.ts"]' "$(jq -c '.testsTouched.files | sort' "$R")"
assert_eq "tests touched are shown by facts" "1" "$("$BOX" facts t2 --json | jq '.results.build.testsTouched.modified')"
assert_eq "changing tests does not change the checks" "false" "$(jq -r .changesChecks "$R")"

echo " a commit the sandbox cannot read counts as a change (fail closed)"
queue_directly t1 build 20301201T000000 "$(printf 'a%.0s' {1..40})"
"$BOX" runner --once 2>/dev/null
R="$AGENTBOX_HOME/results/t1/$QUEUED_ID.json"
assert_eq "changesChecks is true, never false or absent" "true" "$(jq -r .changesChecks "$R")"
assert_contains "the reason is recorded" "could not be computed" "$(jq -r .checksReason "$R")"
assert_eq "testsTouched is present (null)" "true" "$(jq 'has("testsTouched")' "$R")"
assert_eq "facts show it as a change" "true" "$("$BOX" facts t1 --json --fresh | jq -r '.results.build.changesChecks')"
assert_contains "facts show the reason" "could not be computed" "$("$BOX" facts t1 --json --fresh | jq -r '.results.build.checksReason')"
assert_contains "status shows the reason too" "could not be computed" "$("$BOX" status --json --fresh | jq -r '.tasks[] | select(.name == "t1") | .results.build.checksReason')"

echo " test counts from the summary line"
"$BOX" new t3 sandbox >/dev/null
export FAKE_JOB_SUMMARY=' Test Files  3 passed (3)\n      Tests  12 passed | 2 skipped (14)'
R="$(run_verb t3 test)"
assert_eq "ran and skipped" "12 2" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
assert_eq "no base yet" "null null" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"
export FAKE_JOB_SUMMARY='\033[2m      Tests \033[22m \033[1m\033[31m2 failed\033[39m | \033[32m10 passed\033[39m | 2 skipped\033[90m (14)\033[39m'
commit_in_task t3 change
R="$(run_verb t3 test)"
assert_eq "failed tests count as run, colours are ignored" "12 2" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
assert_eq "base is the count recorded for the base commit" "12 2" "$(jq -r '.tests | "\(.base) \(.baseSkipped)"' "$R")"
assert_eq "facts show the counts" "12" "$("$BOX" facts t3 --json | jq '.results.test.tests.ran')"
export FAKE_JOB_SUMMARY='      Tests  7 passed (7)'
commit_in_task t3 more
R="$(run_verb t3 test)"
assert_eq "no skipped line means 0" "7 0" "$(jq -r '.tests | "\(.ran) \(.skipped)"' "$R")"
export FAKE_JOB_SUMMARY='      Tests  7 passed | 1 todo (9)'
commit_in_task t3 sums
R="$(run_verb t3 test)"
assert_eq "counts that do not add up are null" "null" "$(jq -c .tests "$R")"
export FAKE_JOB_SUMMARY='no summary here'
commit_in_task t3 nosummary
R="$(run_verb t3 test)"
assert_eq "no summary line is null" "null" "$(jq -c .tests "$R")"

echo " a log written by the agent cannot drop the checks fields"
# summary_result <summary text>: runs the test verb with that log and prints the result file.
summary_result() {
  export FAKE_JOB_SUMMARY="$1"
  commit_in_task t3 "log $RANDOM"
  run_verb t3 test
}
assert_still_carries_checks() {
  assert_eq "$1: changesChecks is a boolean" "boolean" "$(jq -r '.changesChecks | type' "$R")"
  assert_eq "$1: testsTouched is present" "true" "$(jq 'has("testsTouched")' "$R")"
  assert_eq "$1: tests is null" "null" "$(jq -c .tests "$R")"
}
R="$(summary_result '      Tests  1234567890123456789012345678901234567890 passed (1234567890123456789012345678901234567890)')"
assert_still_carries_checks "a 40 digit count"
R="$(summary_result '      Tests  1\" passed (1\")')"
assert_still_carries_checks "a count with a quote"
R="$(summary_result '      Tests  12\n3 passed (123)')"
assert_still_carries_checks "a count split by a newline"
R="$(summary_result '      Tests  5 passed (5)\n      Tests  9 passed (9)')"
assert_still_carries_checks "two summary lines"
R="$(summary_result '      Tests  1234567890 passed (1234567890)')"
assert_still_carries_checks "a ten digit count"
R="$(summary_result '      Tests  123456789 passed (123456789)')"
assert_eq "a nine digit count is accepted" "123456789" "$(jq .tests.ran "$R")"

echo " a failing tests computation never drops the changes object"
LIBS="common names docker project sandbox journal task request checks"
RESULT="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in '"$LIBS"'; do . "$BOX_ROOT/lib/$lib.sh"; done
  changes_json() { echo "{\"changesChecks\":false,\"checksTouched\":[],\"testsTouched\":null}"; }
  parse_test_counts() { echo "garbage not numbers"; }
  recorded_test_counts() { echo "x y"; }
  result_extras_json t3 test "$(printf "c%.0s" {1..40})" /dev/null')"
assert_eq "changesChecks survived" "false" "$(jq -r .changesChecks <<<"$RESULT")"
assert_eq "tests is null" "null" "$(jq -c .tests <<<"$RESULT")"
RESULT="$(AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in '"$LIBS"'; do . "$BOX_ROOT/lib/$lib.sh"; done
  changes_json() { echo "not json at all"; }
  result_extras_json t3 build "$(printf "c%.0s" {1..40})" /dev/null')"
assert_eq "a broken changes computation counts as a change" "true" "$(jq -r .changesChecks <<<"$RESULT")"
assert_eq "with its reason" "could not be computed" "$(jq -r .checksReason <<<"$RESULT")"
unset FAKE_JOB_SUMMARY

echo " the test counts are no part of what counts as a change (see test_tests_from_log.sh)"
R="$(summary_result '      Tests  5 passed (5)\n      Tests  9 passed (9)')"
assert_eq "an ambiguous test log on untouched checks: no counts, and not a change for that" "null false" "$(jq -c .tests "$R") $(jq -r .changesChecks "$R")"
assert_eq "no reason is given" "null" "$(jq -r .checksReason "$R")"
assert_eq "no protected path was touched" "[]" "$(jq -c .checksTouched "$R")"
R="$(summary_result 'no summary at all')"
assert_eq "a test log without counts: not a change for that" "false" "$(jq -r .changesChecks "$R")"
R="$(summary_result '      Tests  5 passed (5)')"
assert_eq "readable counts on untouched checks: the same" "false" "$(jq -r .changesChecks "$R")"
assert_eq "and no reason" "null" "$(jq -r .checksReason "$R")"
assert_eq "the counts say where they come from" "log" "$(jq -r .tests.source "$R")"
unset FAKE_JOB_SUMMARY

# written_result <id> <verb> <TEST_FILES of the profile> [extras]: what write_result puts into the file.
written_result() {
  AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in '"$LIBS"'; do . "$BOX_ROOT/lib/$lib.sh"; done
    TEST_FILES="$3"
    write_result t3 "$1" "$2" "$(printf "d%.0s" {1..40})" 0 1 0 2026-01-01T00:00:00Z false false false ${4:+"$4"} >/dev/null 2>&1
    cat "$AGENTBOX_HOME/results/t3/$1.json"' _ "$@"
}
COMPUTED='{"changesChecks":false,"checksTouched":[],"testsTouched":null}'
OUT="$(written_result unit-1 build '**/*.spec.ts' "$COMPUTED")"
assert_eq "touched test files unknown for a profile that names them: true" "true" "$(jq -r .changesChecks <<<"$OUT")"
assert_contains "with its reason" "test files" "$(jq -r .checksReason <<<"$OUT")"
OUT="$(written_result unit-2 build '' "$COMPUTED")"
assert_eq "a project that names no test files: unknown too, so a change" "true" "$(jq -r .changesChecks <<<"$OUT")"
assert_contains "with its own reason" "names no test files" "$(jq -r .checksReason <<<"$OUT")"
OUT="$(written_result unit-2b build '' '{"changesChecks":false,"checksTouched":[],"testsTouched":{"modified":0,"deleted":0,"files":[]}}')"
assert_eq "test files that were looked at and not touched are known" "false" "$(jq -r .changesChecks <<<"$OUT")"
OUT="$(written_result unit-3 build '')"
assert_eq "no extras at all: the result is still written, as a change" "true" "$(jq -r .changesChecks <<<"$OUT")"
assert_contains "with a reason" "could not be computed" "$(jq -r .checksReason <<<"$OUT")"
OUT="$(written_result unit-4 build '' 'not json')"
assert_eq "unusable extras: written as a change" "true" "$(jq -r .changesChecks <<<"$OUT")"
OUT="$(written_result unit-5 test '' '{"changesChecks":false,"checksTouched":[],"testsTouched":null,"tests":null}')"
assert_eq "a test verb without counts: true" "true" "$(jq -r .changesChecks <<<"$OUT")"

echo " a project without a test file pattern cannot say that no test was touched"
{ grep -v '^TEST_FILES=' "$PROJECTS_DIR/sandbox.env"; } >"$PROJECTS_DIR/no-pattern.env"
"$BOX" new t5 no-pattern >/dev/null
echo "weakened" >"$AGENTBOX_HOME/work/t5/sub/existing.spec.ts"
commit_in_task t5 "edit an existing test"
R="$(run_verb t5 build)"
assert_eq "nothing protected was touched" "[]" "$(jq -c .checksTouched "$R")"
assert_eq "the touched tests are unknown" "null" "$(jq -c .testsTouched "$R")"
assert_eq "so the result counts as a change" "true" "$(jq -r .changesChecks "$R")"
assert_contains "and says why" "names no test files" "$(jq -r .checksReason "$R")"

echo " every terminal path carries the fields"
# assert_carries_checks <name> <result file> <expected changesChecks>
assert_carries_checks() {
  assert_eq "$1: changesChecks" "$3" "$(jq -r '.changesChecks' "$2")"
  assert_eq "$1: checksTouched is a list" "array" "$(jq -r '.checksTouched | type' "$2")"
  assert_eq "$1: checksReason, testsTouched and tests are present" "true true true" \
    "$(jq -r '"\(has("checksReason")) \(has("testsTouched")) \(has("tests"))"' "$2")"
}
"$BOX" new t4 sandbox >/dev/null
commit_in_task t4 failing
R="$(FAKE_JOB_EXIT=1 run_verb t4 build)"
assert_carries_checks "a failed job" "$R" false
commit_in_task t4 slow
R="$(FAKE_JOB_EXIT=124 FAKE_JOB_DELAY=2 BOX_VERB_TIMEOUT=1 run_verb t4 build)"
assert_eq "(the job timed out)" "true" "$(jq -r .timed_out "$R")"
assert_carries_checks "a job that timed out" "$R" false
commit_in_task t4 cancelled
"$BOX" verify t4 build --no-wait 2>/dev/null
"$BOX" cancel t4 build >/dev/null 2>&1
R="$(/bin/ls -t "$AGENTBOX_HOME/results/t4"/*-build.json | head -1)"
assert_eq "(the request was cancelled)" "true" "$(jq -r .cancelled "$R")"
assert_carries_checks "a cancelled request" "$R" true
assert_contains "a cancelled request: the reason says so" "cancelled" "$(jq -r .checksReason "$R")"
commit_in_task t4 quarantined
queue_directly t4 deploy 20301202T000000 "$(workspace_head t4)"
"$BOX" runner --once 2>/dev/null
R="$AGENTBOX_HOME/results/t4/$QUEUED_ID.json"
assert_eq "(the request was quarantined)" "70" "$(jq -r .exit_code "$R")"
assert_carries_checks "a quarantined request" "$R" true
assert_contains "a quarantined request: the reason says so" "quarantined" "$(jq -r .checksReason "$R")"
assert_eq "facts show a cancelled result as a change with its reason" "true" "$("$BOX" facts t4 --json --fresh | jq -r '.results.build.changesChecks')"
finish

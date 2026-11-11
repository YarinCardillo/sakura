# shellcheck shell=bash
# What a result says about its commit besides the exit code: whether it changes the checks
# themselves. The profile (host-private) names the paths; the comparison runs in the sandbox.

# jq filter for the output of changes.py: types and lengths are enforced. Anything unexpected
# (a missing or null field, a wrong type) makes the filter print nothing, which counts as unknown.
readonly CHANGES_FILTER='
  def path: select(type == "string") | gsub("[\u0000-\u001f\u007f]"; " ") | .[0:300];
  def count: select(type == "number" and . >= 0 and . == floor);
  { changesChecks: (.changesChecks | select(type == "boolean")),
    checksTouched: [ (.checksTouched // [])[:20][] | path ],
    checksReason: (.checksReason | if type == "string" then path else null end),
    testsTouched: (.testsTouched | if type == "object" then {
      modified: (.modified | count), deleted: (.deleted | count),
      files: [ (.files // [])[:20][] | path ] } else null end) }'
# Fail closed: when what a commit changes is not known, it counts as a change to the checks.
readonly UNKNOWN_CHECKS='{"changesChecks":true,"checksTouched":[],"checksReason":"could not be computed","testsTouched":null}'
readonly UNNAMED_CHECKS='{"changesChecks":true,"checksTouched":[],"checksReason":"the profile names no protected paths","testsTouched":null}'
# A result must carry these fields; the filter succeeds only when they are all there.
readonly CHECKS_PRESENT_FILTER='(.changesChecks | type) == "boolean" and has("checksTouched") and has("testsTouched")'
readonly NOT_COMPUTED_REASON="could not be computed"
readonly CANCELLED_REASON="not computed: the request was cancelled"
readonly QUARANTINED_REASON="not computed: the request was quarantined"
# What a result needed and does not know makes its commit count as a change: which existing
# test files the commit touched. A project that names no test file pattern does not show that
# there are no tests: it shows that nobody looked.
# The test counts are not part of this, either way. They are read from the log, which is
# written by the test command and so by code of the commit: a commit can print a summary line
# without running a test. Counts that are there prove nothing and counts that are missing prove
# nothing, so neither changes what a result says about the checks.
readonly REQUIRED_KNOWLEDGE_FILTER='
  def unknown($why): .changesChecks = true | .checksReason = (.checksReason // $why);
  . + {checksReason: (.checksReason // null), tests: (.tests // null)}
  | if .testsTouched != null then .
    elif $testFiles == "" then unknown("the project names no test files")
    else unknown("the test files the commit touches could not be computed") end'

# unknown_checks_json <reason>: the fields of a result for which nothing is known.
unknown_checks_json() {
  jq -nc --arg reason "$1" '{changesChecks: true, checksTouched: [], checksReason: $reason, testsTouched: null, tests: null}'
}

# result_checks_json <cancelled: true|false> [extras]: the checks fields that write_result
# puts into every result, on every terminal path (passed, failed, timed out, cancelled,
# quarantined). The conservative defaults are applied here and not by the callers: without a
# usable extras object nothing is known, and what the result needed and does not know (see
# REQUIRED_KNOWLEDGE_FILTER) makes changesChecks true with the reason.
result_checks_json() {
  local cancelled="$1" extras="${2:-}" reason="$NOT_COMPUTED_REASON" json
  [ "$cancelled" != "true" ] || reason="$CANCELLED_REASON"
  jq -e "$CHECKS_PRESENT_FILTER" <<<"$extras" >/dev/null 2>&1 || extras="$(unknown_checks_json "$reason")"
  json="$(jq -c --arg testFiles "${TEST_FILES:-}" "$REQUIRED_KNOWLEDGE_FILTER" <<<"$extras" 2>/dev/null)" \
    && [ -n "$json" ] || json="$(unknown_checks_json "$reason")"
  echo "$json"
}

# changes_json <task> <sha>: the checks fields and testsTouched of a result. Never empty and never
# "false" by default: a profile without paths or a sandbox that could not compare the commit gives
# changesChecks true with the reason.
changes_json() {
  local task="$1" sha="$2" raw json
  if [ -z "${PROTECTED:-}" ]; then echo "$UNNAMED_CHECKS"; return 0; fi
  if task_is_on_mac "$task"; then
    raw="$(native_changes_json "$task" "$sha" | head -c "$MAX_SANDBOX_JSON_BYTES")" || { echo "$UNKNOWN_CHECKS"; return 0; }
    json="$(printf '%s' "$raw" | jq -c "$CHANGES_FILTER" 2>/dev/null)" && [ -n "$json" ] || json="$UNKNOWN_CHECKS"
    echo "$json"
    return 0
  fi
  import_commit "$task" "$sha" || { echo "$UNKNOWN_CHECKS"; return 0; }
  raw="$(verified_sandbox "$task" -- /box/changes.py "$(task_base_sha "$task")" "$sha" "$SUBDIR" "${PROTECTED:-}" "${TEST_FILES:-}" "${SAFE_OUTSIDE:-}" 2>/dev/null \
    | head -c "$MAX_SANDBOX_JSON_BYTES")" || { echo "$UNKNOWN_CHECKS"; return 0; }
  json="$(printf '%s' "$raw" | jq -c "$CHANGES_FILTER" 2>/dev/null)" && [ -n "$json" ] || json="$UNKNOWN_CHECKS"
  echo "$json"
}

# ---- test counts ------------------------------------------------------------------------

# What is kept of the log of a job; the runner drops what a job prints beyond it. A log that
# reached the limit was cut and is not parsed. Reading anything out of a log has a deadline.
readonly MAX_LOG_BYTES="${BOX_MAX_LOG_BYTES:-16777216}"
readonly LOG_PARSE_TIMEOUT_SECONDS=20

# Prints "<ran> <skipped>" from the one vitest summary line of a log (a log with two such lines is
# ambiguous, because the agent controls what its tests print) ("Tests  2 failed | 10 passed |
# 2 skipped (14)", colours ignored): ran is passed plus failed, skipped is skipped plus todo. Fails
# when there is no such line, when it names another outcome, or when the parts do not add up to the
# total in brackets, or when a number has more than 9 digits: a count is never guessed. It also
# fails for a log that reached MAX_LOG_BYTES (it was cut) and when the deadline passes.
parse_test_counts() {
  [ "$(wc -c <"$1" | tr -d ' ')" -le "$MAX_LOG_BYTES" ] || return 1
  run_with_timeout "$LOG_PARSE_TIMEOUT_SECONDS" awk -v escape="$(printf '\033')" '
    /Tests/ { gsub(escape "\\[[0-9;]*[A-Za-z]", "") }
    /^[[:space:]]*Tests[[:space:]]+[0-9]/ { line = $0; found++ }
    END {
      if (found != 1) exit 1
      sub(/^[[:space:]]*Tests[[:space:]]+/, "", line)
      sub(/[[:space:]]+$/, "", line)
      if (!match(line, /\([0-9]+\)$/)) exit 1
      total = substr(line, RSTART + 1, RLENGTH - 2)
      if (total !~ /^[0-9]+$/ || length(total) > 9) exit 1
      count = split(substr(line, 1, RSTART - 1), parts, "|")
      for (i = 1; i <= count; i++) {
        split(parts[i], word, " ")
        if (word[1] !~ /^[0-9]+$/ || length(word[1]) > 9) exit 1
        if (word[2] == "passed" || word[2] == "failed") ran += word[1]
        else if (word[2] == "skipped" || word[2] == "todo") skipped += word[1]
        else exit 1
        sum += word[1]
      }
      if (sum != total) exit 1
      print ran + 0, skipped + 0
    }' "$1"
}

# suite_json <task> <verb>: what identifies the suite a result counted: the profile, its
# subdirectory and the command of the verb. Two profiles can share a repository and a base commit
# and run different tests. Needs the profile of the task loaded.
suite_json() {
  jq -nc --arg profile "$(task_project "$1")" --arg subdir "${SUBDIR:-}" --arg command "$(verb_command "$2")" \
    '{profile: $profile, subdir: $subdir, command: $command}'
}

# recorded_test_counts <sha> <suite>: prints "<ran> <skipped>" of the newest test result recorded
# for a commit by any task of the same suite, nothing when there is none. The name of the file
# only narrows the search: the whole sha inside it is what is compared. (Results written before
# names held the whole sha are not found, so they are no base: the counts are for display.)
recorded_test_counts() {
  local file
  for file in "$RESULTS_DIR"/*/*-"$1"-test.json; do
    [ -f "$file" ] && jq -r --arg sha "$1" --argjson suite "$2" '
      select(.verb == "test" and .sha == $sha and .cancelled != true and .suite == $suite)
      | select((.tests.ran | type) == "number" and (.tests.skipped | type) == "number")
      | "\(.finished) \(.tests.ran) \(.tests.skipped)"' "$file" 2>/dev/null
  done | sort | tail -n 1 | cut -d' ' -f2-
}

# tests_json <task> <verb> <log>: {ran, skipped, base, baseSkipped, source} for the test verb, null
# for any other verb. It prints null whenever anything in it fails or prints something unusable: the
# log is written by code the agent controls, and a failure here must never reach the caller.
# "source" is always "log": the counts are what the test command printed, nothing more. They are
# for showing, labelled as such, and no decision of box rests on them.
tests_json() {
  local json
  json="$(compute_tests_json "$@" 2>/dev/null)" && jq -e 'type == "object" or . == null' <<<"$json" >/dev/null 2>&1 \
    && [ -n "$json" ] && echo "$json" || echo null
}

compute_tests_json() {
  local task="$1" verb="$2" log="$3" counts base
  [ "$verb" = "test" ] || { echo null; return 0; }
  counts="$(parse_test_counts "$log")" || { echo null; return 0; }
  [[ "$counts" =~ ^[0-9]{1,9}\ [0-9]{1,9}$ ]] || { echo null; return 0; }
  base="$(recorded_test_counts "$(task_base_sha "$task")" "$(suite_json "$task" "$verb")")"
  [[ "$base" =~ ^([0-9]+\ [0-9]+)?$ ]] || base=""
  jq -nc --arg counts "$counts" --arg base "$base" '
    ($counts | split(" ") | map(tonumber)) as $c
    | ($base | if . == "" then [null, null] else split(" ") | map(tonumber) end) as $b
    | { ran: $c[0], skipped: $c[1], base: $b[0], baseSkipped: $b[1], source: "log" }'
}

# result_extras_json <task> <verb> <sha> <log>: the object merged into a result file. It always
# carries changesChecks (a boolean), checksTouched and testsTouched: a failure in the tests part
# only makes tests null, and a failure in the changes part makes the commit count as a change.
# A result with test counts also names the suite that was counted.
result_extras_json() {
  local changes tests suite extras
  changes="$(changes_json "$1" "$3" 2>/dev/null)"
  jq -e "$CHECKS_PRESENT_FILTER" <<<"$changes" >/dev/null 2>&1 || changes="$UNKNOWN_CHECKS"
  tests="$(tests_json "$1" "$2" "$4")"
  suite="$(suite_json "$1" "$2" 2>/dev/null)" && [ -n "$suite" ] || suite=null
  extras="$(jq -nc --argjson changes "$changes" --argjson tests "$tests" --argjson suite "$suite" \
    '$changes + {tests: $tests} + (if $tests == null then {} else {suite: $suite} end)' 2>/dev/null)" || extras=""
  jq -e "$CHECKS_PRESENT_FILTER" <<<"$extras" >/dev/null 2>&1 || extras="$(jq -nc --argjson c "$UNKNOWN_CHECKS" '$c + {tests: null}')"
  echo "$extras"
}

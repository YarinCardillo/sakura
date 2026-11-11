# shellcheck shell=bash
# Checks for a lavoro on the Mac. box verify queues a request as for a box; the runner runs the
# project's check command natively in the folder of the project inside the worktree, as the
# person, one request at a time across all lavori (this is what keeps the Mac from being
# crushed). The waiting box verify of a task in a box uses the same request_and_wait: on the
# host a request is always published through the queue, never through a drop directory.
# A check is for a commit: it runs only when the worktree is at the commit that was
# requested and has no uncommitted changes. The result has the fields of a box result: the exit
# code and the seconds are the runner's, what the commit changes is computed with plain git
# against the base, the test counts are read from the log. The deadline, cancel and finality
# are those of a box request. No container, no drop directory, no Docker.

readonly VERIFY_NO_RUNNER_EXIT_CODE=75
readonly VERIFY_REJECTED_EXIT_CODE=76
readonly VERIFY_DEADLINE_EXIT_CODE=77
readonly QUARANTINE_LOG_MARKER='[box] request quarantined: '
readonly VERIFY_MAX_WAIT_SECONDS="${BOX_VERIFY_WAIT:-4500}"
readonly VERIFY_POLL_SECONDS="${BOX_VERIFY_POLL:-2}"
readonly VERIFY_LOG_TAIL_LINES=40
readonly COMMIT_FIRST="commit first: a check is for a commit"

# Succeeds when the key of a request (<id>.<task>) names a lavoro on the Mac.
request_is_native() {
  [[ "$1" == *.* ]] && task_is_on_mac "${1##*.}"
}

# ---- box verify ---------------------------------------------------------------------------------

# box verify <task> <verb> [--no-wait] for a lavoro on the Mac.
verify_on_mac() {
  local task="$1" verb="$2" no_wait="$3" problem generation
  [[ "$verb" =~ $VERB_PATTERN ]] || die "invalid verb '$verb'"
  ! task_is_retiring "$task" || die "task '$task' is being removed (box rm $task --force finishes it)"
  require_checked_lavoro "$task"
  [ -n "$(verb_command "$verb")" ] || die "verb '$verb' is not in the profile of task '$task'"
  generation="$(task_generation "$task")"
  workspace_facts "$task"
  [ "$FACT_DIRTY" -eq 0 ] || die "$COMMIT_FIRST (task '$task' has $FACT_DIRTY uncommitted file(s))"
  if problem="$(hidden_entries_problem "$task")"; then die "${problem#\[box\] }"; fi
  if [ "$no_wait" = "--no-wait" ]; then queue_request "$task" "$verb"; return 0; fi
  require_runner
  request_and_wait "$task" "$verb" "$FACT_HEAD" "$generation"
}

# Ends box with the exit code that says so when no runner would answer.
require_runner() {
  runner_is_running && return 0
  warn "the runner is not running, nothing would answer; start it with: box runner --daemon"
  exit "$VERIFY_NO_RUNNER_EXIT_CODE"
}

# request_and_wait <task> <verb> <sha> <generation>: publishes the request for that commit of
# that incarnation of the task through the queue of box, waits for its result, prints it and
# ends box with the exit code of the check. Results that are there before the request do not
# count.
request_and_wait() {
  local task="$1" verb="$2" sha="$3" generation="$4" before result
  before="$(matching_results "$task" "$sha" "$verb" | tr '\0' '\n')"
  queue_commit "$task" "$verb" "$sha" "$generation" || die "task '$task' was removed or made again meanwhile; nothing was queued"
  warn "requested '$verb' for ${sha:0:12}, waiting for the runner"
  result="$(wait_for_result "$task" "$sha" "$verb" "$before")" || exit "$?"
  print_verify_result "$result"
}

# The result files of a task for a commit and a verb, NUL-delimited: named by the whole sha, and
# saying inside that they are for this task, this verb and this commit. A file that only has
# the right name is not a result of the request.
matching_results() {
  local file
  for file in "$(task_results_dir "$1")"/*-"$2"-"$3".json; do
    if [ -f "$file" ] && result_is_for "$file" "$1" "$2" "$3"; then printf '%s\0' "$file"; fi
  done
}

# result_is_for <result file> <task> <sha> <verb>
result_is_for() {
  [ "$(result_field "$1" task)" = "$2" ] && [ "$(result_field "$1" sha)" = "$3" ] && [ "$(result_field "$1" verb)" = "$4" ]
}

# Prints the result file that was not there before; ends when the runner is gone or the wait is over.
wait_for_result() {
  local task="$1" sha="$2" verb="$3" before="$4" waited=0 file
  while true; do
    while IFS= read -r -d '' file; do
      case "$before" in *"$file"*) ;; *) echo "$file"; return 0 ;; esac
    done < <(matching_results "$task" "$sha" "$verb")
    runner_is_running || { warn "the runner stopped while waiting; the request stays queued, start it with: box runner --daemon"; exit "$VERIFY_NO_RUNNER_EXIT_CODE"; }
    [ "$waited" -lt "$VERIFY_MAX_WAIT_SECONDS" ] || { warn "no result after ${VERIFY_MAX_WAIT_SECONDS}s, giving up (the request may still run; check box status)"; exit "$VERIFY_DEADLINE_EXIT_CODE"; }
    sleep "$VERIFY_POLL_SECONDS"
    waited=$((waited + VERIFY_POLL_SECONDS))
  done
}

# The end of the log, the line that sums the result up, and the exit code of the check.
print_verify_result() {
  local result="$1" exit_code first_line
  [ -f "$result" ] || exit 1
  exit_code="$(number_or_zero "$(result_field "$result" exit_code)")"
  # The failure the runner writes for a request it could not run says why in its first line.
  IFS= read -r first_line <"${result%.json}.log" 2>/dev/null || first_line=""
  if [ "$exit_code" -eq "$RUNNER_ERROR_EXIT_CODE" ] && [[ "$first_line" == "$QUARANTINE_LOG_MARKER"* ]]; then
    warn "rejected: the runner quarantined the request: $(printf '%s' "${first_line#"$QUARANTINE_LOG_MARKER"}" | sanitize)"
    exit "$VERIFY_REJECTED_EXIT_CODE"
  fi
  tail -n "$VERIFY_LOG_TAIL_LINES" "${result%.json}.log" 2>/dev/null | LC_ALL=C tr -cd '\n\t[:print:]' || true
  echo "---"
  echo "verb=$(result_field "$result" verb) sha=$(result_field "$result" sha) exit=$exit_code seconds=$(result_field "$result" seconds) peak_memory_bytes=$(result_field "$result" peak_memory_bytes)"
  exit "$exit_code"
}

# ---- the runner's side --------------------------------------------------------------------------

# run_native_job <task> <key> <sha> <command>: the job of a request for a lavoro on the Mac, in
# place of run_job. Prints what it refuses into the log; returns the exit code of the command,
# 124 at the deadline, 2 when the check is refused or its result does not count.
#
# The agent of a lavoro can edit files at any moment, so nothing is checked first and looked at
# again later. One reading of the worktree (worktree_state) is taken before the command: it
# must itself show the commit that was requested and nothing else. The same reading is taken
# after the command and must be equal. A reading that cannot be taken never counts as equal.
run_native_job() {
  local task="$1" key="$2" sha="$3" command="$4" host_dir started code=0 before after
  checked_lavoro "$task" || { echo "[box] $LAVORO_PROBLEM"; return 1; }
  hidden_entries_problem "$task" && return "$REJECTED_EXIT_CODE"
  before="$(worktree_state "$task")" || { echo "[box] $STATE_UNREADABLE: the check was not run"; return "$REJECTED_EXIT_CODE"; }
  state_problem "$task" "$sha" "$before" && return "$REJECTED_EXIT_CODE"
  cancel_requested "$key" && return "$CANCELLED_EXIT_CODE"
  host_dir="$(request_host_dir "$key")"
  mkdir -p "$host_dir"
  started="$(monotonic_seconds)"
  run_native "$task" "$VERB_TIMEOUT_SECONDS" "$host_dir/group" "$command" || code=$?
  echo "$(( $(monotonic_seconds) - started ))" >"$host_dir/seconds"
  after="$(worktree_state "$task")" || after="unreadable"
  if [ "$after" != "$before" ]; then
    echo "[box] $WORKTREE_CHANGED (the command ended with $code): this is not a result for ${sha:0:12}"
    return "$REJECTED_EXIT_CODE"
  fi
  return "$code"
}

readonly WORKTREE_CHANGED="the worktree changed while the check ran"
readonly STATE_UNREADABLE="the state of the worktree cannot be read"
readonly HIDDEN_ENTRIES="hides files from git (assume-unchanged or skip-worktree entries): a check cannot vouch for them. Clear them (git update-index --no-assume-unchanged or --no-skip-worktree) and ask again"

# worktree_state <task>: one reading of the worktree, printed as
#   <head> <tree of the tracked files as they are> <number of files git neither knows nor ignores> <hash of their names>
# The tree is built in an index of its own that starts from the head, never from the index of
# the worktree: that one can carry assume-unchanged or skip-worktree marks and stat data that
# hide an edit. Every tracked file is read again. Any step that fails makes the reading fail;
# the caller treats a failed reading as a state that is not equal to any other.
worktree_state() {
  local task="$1" head scratch tree="" count="" names=""
  checked_lavoro "$task" || return 1
  head="$(lavoro_git "$task" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null)" && [[ "$head" =~ ^[0-9a-f]{40}$ ]] || return 1
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/box-state.XXXXXX")" || return 1
  if snapshot_git "$scratch/index" read-tree "$head" >/dev/null && snapshot_git "$scratch/index" add -u >/dev/null \
    && tree="$(snapshot_git "$scratch/index" write-tree)" \
    && snapshot_git "$scratch/index" ls-files --others --exclude-standard -z >"$scratch/others"; then
    count="$(tr -cd '\0' <"$scratch/others" | wc -c | tr -d ' ')"
    names="$(shasum <"$scratch/others" | cut -d' ' -f1)"
  fi
  rm -rf "$scratch"
  [[ "$tree" =~ ^[0-9a-f]{40}$ ]] && [[ "$count" =~ ^[0-9]+$ ]] && [ -n "$names" ] || return 1
  echo "$head $tree $count $names"
}

# git for the worktree with an index of its own. Needs checked_lavoro to have run.
snapshot_git() {
  local index="$1"
  shift
  person_environment env "GIT_INDEX_FILE=$index" git "${PERSON_GIT_OPTIONS[@]}" -c core.untrackedCache=false \
    --git-dir="$LAVORO_GIT_DIR" --work-tree="$LAVORO_DIR" "$@" 2>/dev/null
}

# state_problem <task> <sha> <reading>: prints why the reading is not "the commit that was asked
# for and nothing else" and succeeds; fails when it is exactly that.
state_problem() {
  local task="$1" sha="$2" head tree count names wanted
  read -r head tree count names <<<"$3"
  if [ "$head" != "$sha" ]; then
    echo "[box] the worktree is at ${head:0:12}, not at the commit that was requested (${sha:0:12}): a check on the Mac runs on the worktree as it is, ask again"
    return 0
  fi
  wanted="$(lavoro_git "$task" rev-parse --verify --quiet "$sha^{tree}" 2>/dev/null)" || { echo "[box] $STATE_UNREADABLE: the check was not run"; return 0; }
  [ "$tree" = "$wanted" ] && [ "$count" = "0" ] && return 1
  echo "[box] $COMMIT_FIRST (the files of the worktree are not the ones of ${sha:0:12}$([ "$count" = "0" ] || echo ", and $count file(s) are not known to git"))"
}

# Prints the problem and succeeds when the index of the worktree has entries git is told not to
# look at, or cannot be listed; fails when it has none.
hidden_entries_problem() {
  local listing
  listing="$(lavoro_git "$1" ls-files -v 2>/dev/null)" || { echo "[box] $STATE_UNREADABLE: the check was not run"; return 0; }
  grep -q -e '^[a-z] ' -e '^S ' <<<"$listing" || return 1
  echo "[box] the index of the worktree of task '$1' $HIDDEN_ENTRIES"
}

# Ends the process group of the native job of a request, when one runs: asked, then killed.
end_native_job() {
  local group
  group="$(recorded_group "$(request_host_dir "$1")/group")" || return 0
  kill -TERM -- "-$group" 2>/dev/null || return 0
  sleep "$NATIVE_JOB_END_SECONDS" 9>&-
  kill_if_still_recorded "$(request_host_dir "$1")/group" "$group"
}
readonly NATIVE_JOB_END_SECONDS=1

# native_changes_json <task> <sha>: the output of the changes rules for a commit of a lavoro on
# the Mac against its base, from a name list that plain git produces.
native_changes_json() {
  local task="$1" sha="$2"
  checked_lavoro "$task" || return 1
  lavoro_git "$task" diff --name-status -z --no-renames --no-ext-diff --no-textconv "$(task_base_sha "$task")" "$sha" 2>/dev/null \
    | python3 -B -I "$BOX_ROOT/lib/native_changes.py" "$SUBDIR" "${PROTECTED:-}" "${TEST_FILES:-}" "${SAFE_OUTSIDE:-}" 2>/dev/null
}

# Prints the path of the oldest queued request that can run now: any request of a lavoro on the
# Mac, a request of a box only while Docker answers. Sets nothing; prints nothing when none can.
next_runnable_request() {
  local id file docker=""
  while read -r id file; do
    [ "$(request_field "$file" state)" = "queued" ] || continue
    if request_is_native "$id"; then echo "$file"; return 0; fi
    [ -n "$docker" ] || { dk info >/dev/null 2>&1 && docker=up || docker=down; }
    if [ "$docker" = "up" ]; then echo "$file"; return 0; fi
  done < <(list_requests)
  return 0
}

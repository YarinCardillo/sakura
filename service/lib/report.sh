# shellcheck shell=bash
# Machine-readable reports (docs/interface.md). JSON is built by jq from validated pieces
# (--arg and --argjson), never by concatenating text.

readonly RESULT_LOG_LINES=400
readonly NOT_FOUND_EXIT_CODE=3
readonly ACCOUNT_CHECK_SECONDS="${BOX_ACCOUNT_CHECK_SECONDS:-20}"
readonly MAX_STATUS_BYTES=100000
readonly COMMIT_PATTERN='^[0-9a-f]{40}$'
readonly STATUS_PARALLEL_READS=4

# ---- Docker facts, collected once per call ----------------------------------------------

# Docker is asked only when a task lives in a box: lavori on the Mac are read with git alone.
collect_container_facts_if_needed() {
  STATUS_CONTAINERS="" STATUS_STATS="" STATUS_HAS_BOX_TASK=0
  any_box_task || return 0
  require_docker
  collect_container_facts
  STATUS_HAS_BOX_TASK=1
}

# Fills STATUS_CONTAINERS ("<task> <container>" per running container) and STATUS_STATS.
collect_container_facts() {
  STATUS_CONTAINERS="$(dk ps --filter label=agentbox.task --format '{{.Label "agentbox.task"}} {{.Names}}')"
  STATUS_STATS="$(container_stats)"
}

# "812.3MiB" to a whole number of MB.
to_megabytes() {
  awk -v v="$1" 'BEGIN {
    n = v + 0
    if (v ~ /GiB$/) n *= 1024; else if (v ~ /KiB$/) n /= 1024; else if (v ~ /MiB$/) n *= 1; else n /= 1048576
    printf "%d", n }'
}

timestamp_to_zulu() {
  local value
  value="$(printf '%s' "$1" | sed 's/\.[0-9]*Z$/Z/')"
  [[ "$value" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]] && echo "$value" || echo ""
}

agent_json() {
  local task="$1" name info status started exit_code state cpu memory stats
  name="$(agent_container_name "$task")"
  info="$(dk inspect -f '{{.State.Status}} {{.State.StartedAt}} {{.State.ExitCode}}' "$name" 2>/dev/null)" || { echo null; return 0; }
  read -r status started exit_code <<<"$info"
  state=exited
  [ "$status" = "running" ] && state=running
  [[ "$exit_code" =~ ^[0-9]+$ ]] || exit_code=0
  [ "$state" = "exited" ] && note_agent_exit_once "$task" "$exit_code"
  cpu=0
  memory=0
  stats="$(echo "$STATUS_STATS" | awk -v n="$name" '$1 == n {print $2, $3}')"
  if [ "$state" = "running" ] && [ -n "$stats" ]; then
    cpu="$(echo "${stats%% *}" | tr -d '%')"
    memory="$(to_megabytes "${stats##* }")"
  fi
  [[ "$cpu" =~ ^[0-9.]+$ ]] || cpu=0
  jq -nc --arg container "$name" --arg account "$(agent_record_field "$task" ACCOUNT)" --arg state "$state" \
    --argjson cpu "$cpu" --argjson memory "$memory" --arg started "$(timestamp_to_zulu "$started")" \
    --argjson exitCode "$([ "$state" = exited ] && echo "$exit_code" || echo null)" '
    { container: $container, account: (if $account == "" then null else $account end), state: $state,
      cpuPercent: (if $state == "running" then $cpu else null end),
      memoryMb: (if $state == "running" then $memory else null end),
      startedAt: (if $started == "" then null else $started end), exitCode: $exitCode }'
}

other_containers() {
  echo "$STATUS_CONTAINERS" | awk -v t="$1" -v a="$(agent_container_name "$1")" '$1 == t && $2 != a' | wc -l | tr -d ' '
}

# ---- results and taken ------------------------------------------------------------------

# Path of the newest result file for a verb of a task, nothing when there is none.
# The newest result file of a task for a verb. Names hold the whole sha (40 characters); a
# result written by an older box holds 12 and is still shown: here the name only finds the
# file, and what is shown and compared with the head is the whole sha the file says inside.
readonly SHA_GLOB='????????????????????????????????????????'
latest_result_file() {
  local task="$1" verb="$2" file latest=""
  for file in "$(task_results_dir "$task")"/????????T??????-????????????-"$verb".json \
    "$(task_results_dir "$task")"/????????T??????-$SHA_GLOB-"$verb".json; do
    if [ -f "$file" ] && [[ "${file##*/}" > "${latest##*/}" ]]; then latest="$file"; fi
  done
  echo "$latest"
}

number_or_zero() {
  [[ "${1:-}" =~ ^[0-9]+$ ]] && echo "$1" || echo 0
}

result_json() {
  local file="$1" sha
  sha="$(result_field "$file" sha)"
  is_valid_sha "$sha" || return 1
  jq -nc --arg sha "$sha" --argjson exitCode "$(number_or_zero "$(result_field "$file" exit_code)")" \
    --argjson seconds "$(number_or_zero "$(result_field "$file" seconds)")" \
    --arg finishedAt "$(timestamp_to_zulu "$(result_field "$file" finished)")" \
    --argjson cancelled "$([ "$(result_field "$file" cancelled)" = true ] && echo true || echo false)" \
    --argjson recorded "$(jq -c '{changesChecks, checksTouched, checksReason, testsTouched, tests}' "$file" 2>/dev/null || echo '{}')" \
    --arg where "$([ "$(result_field "$file" where)" = "mac" ] && echo mac || echo box)" \
    '{sha: $sha, where: $where, exitCode: $exitCode, seconds: $seconds, finishedAt: $finishedAt, cancelled: $cancelled} + $recorded'
}

# {verb: result} for every verb of the profile that has a result. Needs the profile loaded.
results_json() {
  local task="$1" verb file entry acc="{}"
  for verb in $(project_verbs); do
    file="$(latest_result_file "$task" "$verb")"
    [ -n "$file" ] || continue
    entry="$(result_json "$file")" || continue
    acc="$(jq -c --arg verb "$verb" --argjson entry "$entry" '. + {($verb): $entry}' <<<"$acc")"
  done
  echo "$acc"
}

# {sha, at} when box/<task> exists in the source repository (the person's trusted repository).
taken_json() {
  local task="$1" sha at epoch
  sha="$(person_git -C "$REPO" rev-parse --verify --quiet "refs/heads/box/$task^{commit}" 2>/dev/null)" || { echo null; return 0; }
  is_valid_sha "$sha" || { echo null; return 0; }
  at="$(journal_last_time take "$task")"
  if [ -z "$at" ]; then
    epoch="$(person_git -C "$REPO" log -1 --format=%ct "$sha" 2>/dev/null)"
    at="$(epoch_to_zulu "$(number_or_zero "$epoch")")"
  fi
  jq -nc --arg sha "$sha" --arg at "$at" '{sha: $sha, at: $at}'
}

# ---- one task ---------------------------------------------------------------------------

# One element of "tasks". An unreadable workspace gives readable false and null git fields.
task_report_json() {
  local task="$1" facts where taken=null agent=null others=0 folder="" disk=null
  load_task_project "$task"
  where="$(task_where "$task")"
  if [ "$where" = "mac" ]; then
    facts="$(native_facts_json "$task")" || facts=null
    folder="$(task_dir "$task")"
  else
    facts="$(cached_workspace_facts_json "$task")" || facts=null
    taken="$(taken_json "$task")"
    agent="$(agent_json "$task")"
    others="$(other_containers "$task")"
    disk="$(task_disk_json "$task")" || disk=null
  fi
  jq -nc --arg name "$task" --arg profile "$(task_project "$task")" --arg base "$(task_base_branch "$task")" \
    --arg baseSha "$(task_base_sha "$task")" --argjson facts "$facts" --arg where "$where" --arg folder "$folder" \
    --argjson taken "$taken" --argjson agent "$agent" --argjson disk "$disk" \
    --argjson others "$others" --argjson results "$(results_json "$task")" \
    --argjson checks "$(project_verbs | jq -cR . | jq -cs .)" '
    { name: $name, project: $profile, profile: $profile, where: $where,
      folder: (if $folder == "" then null else $folder end), checks: $checks, base: $base, baseSha: $baseSha, readable: ($facts != null),
      branch: $facts.branch, head: $facts.head, headSubject: $facts.headSubject, headTime: $facts.headTime,
      ahead: $facts.ahead, dirtyFiles: $facts.dirtyFiles, filesChanged: $facts.filesChanged,
      insertions: $facts.insertions, deletions: $facts.deletions,
      taken: $taken, agent: $agent, otherContainers: $others, disk: $disk,
      results: ($results | with_entries(.value.isHead = (.value.sha == $facts.head))) }'
}

# ---- runner -----------------------------------------------------------------------------

runner_json() {
  local id file state current="null" queue="[]" entry
  state=stopped
  runner_is_running && state=running
  while read -r id file; do
    entry="$(jq -nc --arg task "$(request_field "$file" task)" --arg verb "$(request_field "$file" verb)" \
      --arg sha "$(request_field "$file" sha)" --arg at "$(request_field "$file" requested)" \
      --arg started "$(request_field "$file" started)" '{task: $task, verb: $verb, sha: $sha, queuedAt: $at, startedAt: $started}')"
    if [ "$(request_field "$file" state)" = "running" ]; then
      current="$(jq -c '{task, verb, sha, startedAt}' <<<"$entry")"
    else
      queue="$(jq -c --argjson e "$entry" '. + [$e | {task, verb, sha, queuedAt}]' <<<"$queue")"
    fi
  done < <(list_requests)
  jq -nc --arg state "$state" --argjson current "$current" --argjson queue "$queue" \
    '{state: $state, current: $current, queue: $queue}'
}

# ---- commands ---------------------------------------------------------------------------

cmd_status_json() {
  local task tasks="[]" report_dir report
  [ "${1:-}" != "--fresh" ] || FACTS_FRESH=1
  require_jq
  collect_container_facts_if_needed
  report_dir="$(mktemp -d "${TMPDIR:-/tmp}/box-status.XXXXXX")"
  collect_task_reports "$report_dir"
  for task in $(list_task_names); do
    report="$(cat "$report_dir/$task.json" 2>/dev/null)" || continue
    tasks="$(jq -c --argjson r "$report" '. + [$r]' <<<"$tasks")"
  done
  rm -rf "$report_dir"
  jq -nc --argjson runner "$(runner_json)" --argjson tasks "$tasks" '{runner: $runner, tasks: $tasks}'
}

# Writes <dir>/<task>.json for every task, reading at most STATUS_PARALLEL_READS workspaces at a
# time (each read starts a sandbox container). A task whose report fails has no file.
collect_task_reports() {
  local dir="$1" task pids=() count=0
  for task in $(list_task_names); do
    if [ "$count" -ge "$STATUS_PARALLEL_READS" ]; then wait "${pids[count - STATUS_PARALLEL_READS]}" || true; fi
    (task_report_json "$task" >"$dir/$task.json" || rm -f "$dir/$task.json") &
    pids+=("$!")
    count=$((count + 1))
  done
  wait
}

# box facts <task> --json [--fresh]
cmd_facts() {
  local task="${1:-}"
  require_existing_task "$task"
  case " $* " in *" --fresh "*) FACTS_FRESH=1 ;; esac
  require_jq
  if ! task_is_on_mac "$task"; then
    require_docker
    collect_container_facts
  fi
  task_report_json "$task"
}

# box log <task> --json
cmd_log() {
  local task="${1:-}" json
  require_existing_task "$task"
  if [ "${2:-}" = "--setup" ]; then print_setup_log "$task"; return; fi
  if task_is_on_mac "$task"; then
    require_jq
    require_checked_lavoro "$task"
    json="$(native_log_json "$task")" || die "could not read the commits of task '$task'"
  else
    require_docker
    json="$(workspace_log_json "$task")" || die "could not read the commits of task '$task'"
  fi
  if [ "${2:-}" = "--json" ]; then echo "$json"; else jq -r '.[] | "\(.sha[0:12]) \(.subject)"' <<<"$json"; fi
}

# box diff <task> [--commit <sha>]: raw patch, produced in a sandbox container.
cmd_diff() {
  local task="${1:-}" commit="" head
  require_existing_task "$task"
  shift
  if [ "${1:-}" = "--commit" ]; then
    commit="${2:-}"
    [[ "$commit" =~ $COMMIT_PATTERN ]] || die "invalid commit sha '$commit'"
  fi
  if task_is_on_mac "$task"; then native_diff "$task" "$commit"; return; fi
  require_docker
  load_task_project "$task"
  if [ -z "$commit" ]; then
    workspace_sandbox "$task" -- /box/diff.py "$(task_base_sha "$task")" || die "could not produce the diff of task '$task'"
    return 0
  fi
  head="$(workspace_verified_head "$task")" || die "could not read the commits of task '$task'"
  verified_sandbox "$task" -- /box/diff.py "$(task_base_sha "$task")" "$head" "$commit" \
    || die "could not produce the diff of task '$task' (is $commit one of the commits box log lists?)"
}

# box result-log <task> <verb>: the last lines of the log of the last result for the verb.
cmd_result_log() {
  local task="${1:-}" verb="${2:-}" file
  require_existing_task "$task"
  [[ "$verb" =~ $VERB_PATTERN ]] || die "invalid verb '$verb'"
  file="$(latest_result_file "$task" "$verb")"
  [ -n "$file" ] && [ -f "${file%.json}.log" ] || { warn "no result for verb '$verb' of task '$task'"; return "$NOT_FOUND_EXIT_CODE"; }
  tail -n "$RESULT_LOG_LINES" "${file%.json}.log" | LC_ALL=C tr -cd '\n\t[:print:]'
}

# box tasks --json: name, profile, base and whether box/<task> was taken, for every task in
# `box status` order. Read from host-private files and the source repository only.
cmd_tasks() {
  local task entry tasks="[]"
  [ "${1:-}" = "--json" ] || die "usage: box tasks --json"
  require_jq
  for task in $(list_task_names); do
    entry="$(task_inventory_json "$task")" || continue
    tasks="$(jq -c --argjson e "$entry" '. + [$e]' <<<"$tasks")"
  done
  echo "$tasks"
}

task_inventory_json() {
  (
    load_task_project "$1" || exit 1
    jq -nc --arg name "$1" --arg profile "$(task_project "$1")" --arg base "$(task_base_branch "$1")" \
      --arg where "$(task_where "$1")" --argjson taken "$(if task_is_on_mac "$1"; then echo null; else taken_json "$1"; fi)" \
      '{name: $name, profile: $profile, where: $where, base: $base, taken: ($taken != null)}'
  )
}

# box accounts --json: the accounts of the boxes, then those of the Mac (accounts.sh), each with
# its place. The accounts of the Mac need no Docker; without Docker the others are left out and
# a line on stderr says so.
cmd_accounts() {
  require_jq
  if dk info >/dev/null 2>&1; then
    { box_accounts_json | jq -c '.[]'; mac_accounts_json; } | jq -cs '.'
  else
    warn "Docker is not reachable: the accounts of the boxes are not listed"
    mac_accounts_json | jq -cs '.'
  fi
}

# An account of the boxes exists when its volume exists. loggedIn is what `claude auth status
# --json` says inside a confined container with that volume, checked concurrently with a
# deadline per account.
box_accounts_json() {
  local volume name names=() dir
  for volume in $(dk volume ls -q --filter "name=$RESOURCE_PREFIX-acct-"); do
    name="${volume#"$RESOURCE_PREFIX"-acct-}"
    [[ "$name" =~ $ACCOUNT_NAME_PATTERN ]] && names+=("$name")
  done
  dir="$(mktemp -d "${TMPDIR:-/tmp}/box-accounts.XXXXXX")"
  for name in ${names[@]+"${names[@]}"}; do
    account_entry_json "$name" >"$dir/$name.json" &
  done
  wait
  for name in ${names[@]+"${names[@]}"}; do cat "$dir/$name.json"; done | jq -cs '.'
  rm -rf "$dir"
}

# One element of box accounts --json. Only loggedIn and a validated authMethod are taken from
# the status; nothing else it prints is kept. A check that fails or times out counts as logged out.
account_entry_json() {
  local name="$1" container raw=""
  container="$(account_check_container_name "$name")"
  build_agent_flags
  add_account_flags "$name"
  raw="$(run_with_timeout "$ACCOUNT_CHECK_SECONDS" docker run --rm --name "$container" --label agentbox.kind=acctcheck \
    "${AGENT_FLAGS[@]}" -w /home/agent "$IMAGE" bash -c "$ACCOUNT_ENTRY" _ claude auth status --json 2>/dev/null | head -c "$MAX_STATUS_BYTES")" \
    || dk rm -f "$container" >/dev/null 2>&1 || true
  jq -nc --arg name "$name" --arg raw "$raw" --arg at "$(now_iso)" '
    ($raw | try fromjson catch null) as $s
    | { name: $name, where: "box",
        loggedIn: (($s | type) == "object" and $s.loggedIn == true),
        authMethod: (if ($s | type) == "object" and ($s.authMethod | type) == "string"
                        and ($s.authMethod | test("^[A-Za-z0-9._-]{1,40}$")) then $s.authMethod else null end),
        checkedAt: $at }'
}

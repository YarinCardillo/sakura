# shellcheck shell=bash
# box status: one report derived from git, Docker and the request files. Nothing is stored.

readonly STATS_TIMEOUT_SECONDS=20

cmd_status() {
  local task
  collect_container_facts_if_needed
  print_runner_state
  printf '\n%-16s %-22s %6s %6s\n' TASK BRANCH AHEAD DIRTY
  for task in $(list_task_names); do
    print_task "$task"
  done
  # Volumes are left behind by tasks in a box only: without one Docker is not asked.
  [ "$STATUS_HAS_BOX_TASK" -eq 0 ] || print_orphan_volumes
}

list_task_names() {
  local dir name
  for dir in "$WORK_DIR"/*/; do
    [ -d "$dir" ] || continue
    name="$(basename "$dir")"
    is_valid_task_name "$name" && echo "$name"
  done
}

# Succeeds when a task lives in a box: only then is there anything to ask Docker about.
any_box_task() {
  local task
  for task in $(list_task_names); do
    task_is_on_mac "$task" || return 0
  done
  return 1
}

# "<name> <cpu> <memory>" for every running agentbox container, from one docker call.
container_stats() {
  local names
  names="$(dk ps --filter label=agentbox.task --format '{{.Names}}')"
  [ -n "$names" ] || return 0
  # shellcheck disable=SC2086
  run_with_timeout "$STATS_TIMEOUT_SECONDS" docker stats --no-stream \
    --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}' $names 2>/dev/null || true
}

print_runner_state() {
  if runner_is_running; then echo "runner: running"; else echo "runner: stopped"; fi
}

runner_is_running() {
  [ -f "$RUNNER_LOCK_FILE" ] || return 1
  ! perl -MFcntl=:flock -e 'open(my $f, "<", $ARGV[0]) or exit 1; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$RUNNER_LOCK_FILE"
}

# Facts come from a sandbox read of the workspace; a workspace that cannot be read is
# reported and the report goes on.
print_task() {
  local task="$1" summary head=""
  if summary="$(task_summary "$task")"; then
    echo "$summary" | sed '$d'
    head="$(echo "$summary" | tail -n 1)"
  else
    printf '%-16s %s\n' "$task" "unreadable workspace"
  fi
  print_task_containers "$task"
  print_task_requests "$task"
  print_task_results "$task" "$head"
}

# The table line, then the HEAD sha on a last line.
task_summary() {
  load_task_project "$1"
  workspace_facts "$1"
  printf '%-16s %-22s %6s %6s\n' "$1" "$FACT_BRANCH" "$FACT_AHEAD" "$FACT_DIRTY"
  echo "$FACT_HEAD"
}

print_task_containers() {
  local task="$1" name
  for name in $(echo "$STATUS_CONTAINERS" | awk -v t="$task" '$1 == t {print $2}'); do
    echo "$STATUS_STATS" | awk -v n="$name" '$1 == n {printf "    container %s cpu %s mem %s %s %s\n", $1, $2, $3, $4, $5}'
  done
}

print_task_requests() {
  local task="$1" id file
  while read -r id file; do
    [ "$(request_field "$file" task)" = "$task" ] || continue
    printf '    request   %s %s\n' "$id" "$(request_field "$file" state)"
  done < <(list_requests)
}

# The newest result of every verb, with its commit and whether it is the workspace HEAD.
# Results are read from the host-private directory only.
print_task_results() {
  local task="$1" head="$2" verb file sha mark
  for file in "$(task_results_dir "$task")"/*.json; do
    if [ -f "$file" ]; then echo "$(result_field "$file" verb) $file"; fi
  done | awk '{verb = $1; sub(/^[^ ]* /, ""); last[verb] = $0} END {for (v in last) print v, last[v]}' | sort | while read -r verb file; do
    sha="$(result_field "$file" sha)"
    mark="stale"
    [ "$sha" = "$head" ] && mark="HEAD"
    printf '    result    %-6s exit %s %ss sha %s (%s)\n' "$verb" "$(result_field "$file" exit_code)" \
      "$(result_field "$file" seconds)" "${sha:0:12}" "$mark"
  done
}

# Task volumes whose workspace is gone.
print_orphan_volumes() {
  local name task
  for name in $(dk volume ls -q --filter "name=$RESOURCE_PREFIX-nm-" 2>/dev/null); do
    task="${name#"$RESOURCE_PREFIX"-nm-}"
    [ -d "$WORK_DIR/$task" ] && continue
    echo "orphan volume $name (workspace deleted by hand; box rm $task --force)"
  done
}

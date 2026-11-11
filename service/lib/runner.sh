# shellcheck shell=bash
# The runner: one loop, one request at a time, across all tasks.
#
# Per request: the commit is checked out in a throwaway volume, the dependencies come from a
# pristine volume of the task, built once for its package files and install commands and never
# writable by a job, the verb runs in a job container with a deadline. A request that goes wrong is quarantined, the loop goes on.

readonly RUNNER_LOCK_FILE="$LOCK_DIR/runner.lock"
readonly RUNNER_LOG="$AGENTBOX_HOME/runner.log"
readonly DAEMON_START_WAIT_TICKS=20
readonly DAEMON_STOP_WAIT_TICKS=60
readonly TICK_SECONDS=0.5
readonly HEARTBEAT_SECONDS="${BOX_HEARTBEAT_SECONDS:-5}"
readonly POLL_SECONDS="${BOX_RUNNER_POLL:-2}"
readonly VERB_TIMEOUT_SECONDS="${BOX_VERB_TIMEOUT:-2700}"
readonly DEADLINE_GRACE_SECONDS="${BOX_DEADLINE_GRACE:-120}"
readonly CHECKOUT_TIMEOUT_SECONDS=300
readonly COPY_TIMEOUT_SECONDS="${BOX_COPY_TIMEOUT:-600}"
# After a deadline, how long a docker client that was asked to end is given before it is killed.
readonly DEADLINE_KILL_SECONDS=5
# ro: jobs mount the pristine dependencies read-only. copy: each job gets a private copy.
readonly DEPS_MODE="${BOX_DEPS_MODE:-ro}"
readonly TIMEOUT_EXIT_CODE=124
readonly RUNNER_ERROR_EXIT_CODE=70
readonly REJECTED_EXIT_CODE=2
# How often the memory of a running job is read through Docker, from outside the container.
readonly MEMORY_SAMPLE_SECONDS="${BOX_MEMORY_SAMPLE_SECONDS:-5}"
readonly HOST_NUMBER_MAX_DIGITS=15

# box runner [--once | --daemon | --stop | --install | --uninstall]: processes the queue forever,
# or until it is empty with --once; --daemon runs it detached (log in runner.log, pid in the lock
# file); --install and --uninstall manage the LaunchAgent that starts it at login.
cmd_runner() {
  local once=0 request
  case "${1:-}" in
    --daemon) runner_daemon_start; return 0 ;;
    --stop) runner_daemon_stop; return 0 ;;
    --install) runner_install; return 0 ;;
    --uninstall) runner_uninstall; return 0 ;;
    --once) once=1 ;;
    --*) die "unknown option '$1' (box runner [--once | --daemon | --stop | --install | --uninstall])" ;;
  esac
  acquire_runner_lock
  trap 'stop_current_request; exit 143' TERM INT
  trap 'stop_heartbeat' EXIT
  start_heartbeat
  if any_box_task; then prepare_container_resources || die "could not stage the container resources in the data directory"; fi
  recover_interrupted_requests
  while true; do
    # One slice of every task's drops (see DROP_INTAKE_BUDGET), then the queue: never all the
    # drops of a task before the queued work of the others.
    INTAKE_LEFT=0
    ingest_drops || warn "reading the drop directories failed"
    request="$(next_runnable_request)"
    if [ -n "$request" ]; then
      guarded_process "$request"
    elif [ "$once" -eq 1 ] && [ "$INTAKE_LEFT" -eq 1 ]; then
      continue
    elif [ "$once" -eq 1 ]; then
      [ -z "$(next_queued_request)" ] || warn "Docker is not reachable: the requests of tasks in a box stay queued"
      break
    else
      sleep "$POLL_SECONDS" 9>&-
    fi
  done
}

# The lock is held by the open file description on fd 9 and released when the runner exits,
# however it exits. The pid of the holder is written into the lock file.
acquire_runner_lock() {
  mkdir -p "$LOCK_DIR"
  exec 9>>"$RUNNER_LOCK_FILE"
  perl -MFcntl=:flock -e 'open(my $f, ">&=", 9) or die "fd 9: $!"; flock($f, LOCK_EX | LOCK_NB) or exit 1' \
    || die "another runner is already running"
  echo "$$" >"$RUNNER_LOCK_FILE"
}

# ---- heartbeat ----------------------------------------------------------------------------
# A waiting `verify` (inside a container it sees only its task's results directory) cannot tell
# whether a runner exists. The runner writes the epoch into <results>/<task>/.runner-alive every
# few seconds, also while a long job runs, and the ticker ends when the runner does.

start_heartbeat() {
  local runner_pid=$$
  write_heartbeats
  (
    while kill -0 "$runner_pid" 2>/dev/null; do
      sleep "$HEARTBEAT_SECONDS" 9>&-
      write_heartbeats
    done
  ) 9>&- &
  HEARTBEAT_PID=$!
}

stop_heartbeat() {
  [ -z "${HEARTBEAT_PID:-}" ] || kill "$HEARTBEAT_PID" 2>/dev/null || true
}

write_heartbeats() {
  local record task dir
  for record in "$TASKS_DIR"/*.env; do
    [ -f "$record" ] || continue
    task="$(basename "$record" .env)"
    is_valid_task_name "$task" || continue
    dir="$(task_results_dir "$task")"
    mkdir -p "$dir" 2>/dev/null || continue
    { now_epoch >"$dir/.runner-alive.tmp" && mv "$dir/.runner-alive.tmp" "$dir/.runner-alive"; } 2>/dev/null || true
  done
}

runner_pid() {
  local pid
  pid="$(head -n 1 "$RUNNER_LOCK_FILE" 2>/dev/null)"
  [[ "$pid" =~ ^[0-9]+$ ]] && echo "$pid"
}

runner_daemon_start() {
  local pid tick
  runner_is_running && die "a runner is already running (pid $(runner_pid))"
  mkdir -p "$AGENTBOX_HOME"
  nohup "$BOX_ROOT/bin/box" runner >>"$RUNNER_LOG" 2>&1 </dev/null &
  pid=$!
  for ((tick = 0; tick < DAEMON_START_WAIT_TICKS; tick++)); do
    if runner_is_running; then echo "runner started (pid $(runner_pid)), log: $RUNNER_LOG"; return 0; fi
    sleep "$TICK_SECONDS"
  done
  kill "$pid" 2>/dev/null || true
  die "the runner did not start, see $RUNNER_LOG"
}

# Stops the runner by the pid in the lock file, then ends what it left behind: the native job of
# a request that was running, and the job containers when Docker answers. Requests that were
# running go back to the queue when the next runner starts.
runner_daemon_stop() {
  local pid tick jobs key file
  runner_is_running || die "no runner is running"
  pid="$(runner_pid)" || die "the lock file holds no pid"
  kill "$pid"
  for ((tick = 0; tick < DAEMON_STOP_WAIT_TICKS; tick++)); do
    runner_is_running || break
    sleep "$TICK_SECONDS"
  done
  runner_is_running && die "the runner (pid $pid) did not stop"
  while read -r key file; do
    [ "$(request_field "$file" state)" != "running" ] || end_native_job "$key"
  done < <(list_requests)
  if dk info >/dev/null 2>&1; then
    jobs="$(dk ps -aq --filter label=agentbox.kind=job)"
    [ -z "$jobs" ] || echo "$jobs" | xargs docker rm -f >/dev/null
  fi
  echo "runner stopped"
}

# After a crash a request can be left in state "running". Its containers are removed. When its
# result was already published (the crash came between the result and the removal of the queue
# file) it only leaves the queue; otherwise it goes back to the queue.
recover_interrupted_requests() {
  local key file name
  mkdir -p "$QUEUE_DIR"
  while read -r key file; do
    [ "$(request_field "$file" state)" = "running" ] || continue
    remove_request_containers "$key"
    if request_has_result "$file"; then finish_answered "$file"; continue; fi
    if request_is_cancelled "$file"; then
      finish_cancelled "$file" || quarantine_request "$file" "the cancelled result could not be written"
      continue
    fi
    warn "requeueing interrupted request $key"
    with_queue_lock set_request_state "$file" queued
  done < <(list_requests)
  rm -rf "${STAGE_DIR:?}"/*
  # The volumes of jobs are left by tasks in a box only: without one Docker is not asked.
  any_box_task || return 0
  for name in $(dk volume ls -q --filter "name=$CHECKOUT_VOLUME_PREFIX-") $(dk volume ls -q --filter "name=$JOB_DEPS_VOLUME_PREFIX-"); do
    dk volume rm "$name" >/dev/null 2>&1 || true
  done
}

# Runs one request in a background subshell (so that errexit stays in force inside it and an
# exit cannot reach the loop). Whatever goes wrong, the request is quarantined with a reason.
guarded_process() {
  local file="$1" status=0
  CURRENT_REQUEST_FILE="$file"
  (process_request "$file") &
  CURRENT_REQUEST_PID=$!
  wait "$CURRENT_REQUEST_PID" || status=$?
  CURRENT_REQUEST_PID="" CURRENT_REQUEST_FILE=""
  [ "$status" -eq 0 ] || quarantine_request "$file" "runner error (status $status)"
}

# On SIGTERM: the request being processed is stopped; it stays "running" and is requeued by the
# next runner.
stop_current_request() {
  [ -z "${CURRENT_REQUEST_PID:-}" ] || kill "$CURRENT_REQUEST_PID" 2>/dev/null || true
  # A native job is a process group of its own: it does not end with the shell that waited for it.
  [ -z "${CURRENT_REQUEST_FILE:-}" ] || end_native_job "$(request_key "$CURRENT_REQUEST_FILE")"
}

process_request() {
  local file="$1" key id task verb sha command reason
  [ -e "$file" ] || return 0
  key="$(request_key "$file")"
  id="$(result_id_of_key "$key")"
  task="$(request_field "$file" task)"
  verb="$(request_field "$file" verb)"
  sha="$(request_field "$file" sha)"
  if ! reason="$(validate_request "$task" "$key" "$verb" "$sha")"; then
    quarantine_request "$file" "$reason"
    return 0
  fi
  if ! request_is_of_this_task "$file" "$task"; then
    warn "dropping request $key: it belongs to an earlier task named '$task'"
    cleanup_request_objects "$key"
    with_queue_lock rm -f "$file"
    return 0
  fi
  if request_has_result "$file"; then finish_answered "$file"; return 0; fi
  if request_is_cancelled "$file"; then finish_cancelled "$file"; return 0; fi
  load_task_project "$task"
  command="$(verb_command "$verb")"
  [ -n "$command" ] || { quarantine_request "$file" "verb '$verb' is not in the profile"; return 0; }
  run_request "$file" "$task" "$id" "$verb" "$sha" "$command"
}

# Fails when the request names another incarnation of the task than the one that exists (the
# task was removed and created again after the request was made). A request that names none
# was written by an older box and is taken as it is.
request_is_of_this_task() {
  local generation
  generation="$(request_field "$1" generation)"
  [ -z "$generation" ] || [ "$generation" = "$(task_generation "$2")" ]
}

# Prints the reason and fails when the request cannot run.
validate_request() {
  local task="$1" key="$2" verb="$3" sha="$4" id
  id="$(result_id_of_key "$key")"
  is_valid_task_name "$task" || { echo "invalid task name"; return 1; }
  is_valid_request_id "$id" || { echo "invalid request id"; return 1; }
  [ "$key" = "$id" ] || [ "$key" = "$id.$task" ] || { echo "request id does not belong to the task"; return 1; }
  is_valid_sha "$sha" || { echo "invalid commit sha"; return 1; }
  [[ "$verb" =~ $VERB_PATTERN ]] || { echo "invalid verb"; return 1; }
  [ -n "$(task_project "$task")" ] || { echo "task has no profile record"; return 1; }
}

# A request that cannot run still gets a result, so that the waiting agent is released.
quarantine_request() {
  local file="$1" reason="$2" key
  key="$(request_key "$file")"
  warn "quarantining request $key: $reason"
  cleanup_request_objects "$key"
  with_queue_lock quarantine_request_locked "$file" "$reason" || warn "could not move request $key to quarantine"
}

# Queue lock held. Best effort: a full or unwritable results directory must not stop the runner.
quarantine_request_locked() {
  local file="$1" reason="$2" key place
  [ -e "$file" ] || return 0
  key="$(request_key "$file")"
  if ! record_rejection "$file" "$(result_id_of_key "$key")" "$reason"; then
    warn "could not write a result for request $key"
  fi
  # Into a folder reserved for it (see quarantine_place), never onto a name that may be there.
  place="$(quarantine_place "request-$key")" && mv -- "$file" "$place/request" 2>/dev/null || rm -f -- "$file" || true
}

# Writes the failure result that releases the waiting agent; a request that was cancelled still
# ends as cancelled. Runs in a subshell.
record_rejection() {
  (
    task="$(request_field "$1" task)"
    verb="$(request_field "$1" verb)"
    sha="$(request_field "$1" sha)"
    is_valid_task_name "$task" && is_valid_request_id "$2" && [[ "$verb" =~ $VERB_PATTERN ]] && is_valid_sha "$sha" || exit 0
    # A task that was removed gets no result: its results directory is not made again.
    [ -d "$(task_results_dir "$task")" ] || exit 0
    if request_is_cancelled "$1"; then write_cancelled_result_of "$1" 0; exit $?; fi
    echo "$QUARANTINE_LOG_MARKER$3" >>"$(task_results_dir "$task")/$2.log"
    write_result "$task" "$2" "$verb" "$sha" "$RUNNER_ERROR_EXIT_CODE" 0 0 "$(now_iso)" false false false \
      "$(unknown_checks_json "$QUARANTINED_REASON")"
  )
}

# Force-removes every container started for a request (install, checkout, copy, job), by label.
remove_request_containers() {
  local names
  if request_is_native "$1"; then end_native_job "$1"; return 0; fi
  names="$(dk ps -a --filter "label=agentbox.request=$1" --format '{{.Names}}' 2>/dev/null)" || return 0
  # shellcheck disable=SC2086
  [ -z "$names" ] || dk rm -f $names >/dev/null 2>&1 || true
}

cleanup_request_objects() {
  local id="$1"
  remove_request_containers "$id"
  if ! request_is_native "$id"; then
    dk rm -f "$(job_container_name "$id")" >/dev/null 2>&1 || true
    dk volume rm "$(checkout_volume "$id")" "$(job_deps_volume "$id")" >/dev/null 2>&1 || true
  fi
  rm -rf "${STAGE_DIR:?}/$id" "${STAGE_DIR:?}/$id.host"
}

run_request() {
  local file="$1" task="$2" id="$3" verb="$4" sha="$5" command="$6"
  local started start_epoch log exit_code=0 job=run_job
  ! task_is_on_mac "$task" || job=run_native_job
  started="$(now_iso)"
  start_epoch="$(now_epoch)"
  mkdir -p "$(task_results_dir "$task")"
  log="$(task_results_dir "$task")/$id.log"
  if ! with_queue_lock claim_request_locked "$file" "$started"; then finish_cancelled "$file"; return 0; fi
  echo "[box] $id: $verb on $sha" >"$log"
  logged "$log.done" "$MAX_LOG_BYTES" "$job" "$task" "$(request_key "$file")" "$sha" "$command" >>"$log" || exit_code=$?
  finish_request "$file" "$task" "$id" "$verb" "$sha" "$exit_code" "$started" "$(( $(now_epoch) - start_epoch ))" "$log"
}

finish_request() {
  local file="$1" task="$2" id="$3" verb="$4" sha="$5" exit_code="$6" started="$7" total="$8" log="$9"
  local seconds peak timed_out=false oom key extras
  key="$(request_key "$file")"
  seconds="$(host_number "$key" seconds "$total")"
  peak="$(host_number "$key" peak_bytes 0)"
  # A deadline is the runner's own measure: the job can choose its exit code, not the clock.
  [ "$exit_code" -eq "$TIMEOUT_EXIT_CODE" ] && [ "$seconds" -ge "$VERB_TIMEOUT_SECONDS" ] && timed_out=true
  oom="$(job_oom_killed "$key")"
  cleanup_request_objects "$key"
  if request_is_cancelled "$file"; then finish_cancelled "$file" "$total"; return 0; fi
  extras="$(result_extras_json "$task" "$verb" "$sha" "$log")"
  with_queue_lock end_request_locked "$file" "$total" \
    write_result "$task" "$id" "$verb" "$sha" "$exit_code" "$seconds" "$peak" "$started" "$timed_out" "$oom" false "$extras"
  if [ "$(result_field "$(task_results_dir "$task")/$id.json" cancelled 2>/dev/null)" != "false" ]; then
    warn "request $key was cancelled"
    return 0
  fi
  journal_event verify-done "$task" "$verb" "$sha" "$exit_code"
  warn "request $id finished: exit $exit_code in ${seconds}s"
}

# A request whose result was already published is never run again: it only leaves the queue.
finish_answered() {
  local file="$1" key
  key="$(request_key "$file")"
  cleanup_request_objects "$key"
  with_queue_lock end_request_locked "$file" 0 false
  warn "request $key already has its result; it is not run again"
}

# A cancelled request ends with a cancelled result for its commit and leaves the queue for good.
# usage: finish_cancelled <queue file> [seconds]
finish_cancelled() {
  local file="$1" key reason
  [ -f "$file" ] || return 0
  key="$(request_key "$file")"
  if ! reason="$(validate_request "$(request_field "$file" task)" "$key" "$(request_field "$file" verb)" "$(request_field "$file" sha)")"; then
    quarantine_request "$file" "$reason"
    return 0
  fi
  cleanup_request_objects "$key"
  with_queue_lock end_request_locked "$file" "${2:-0}" false
  warn "request $key was cancelled"
}

# ---- what the runner measures itself -------------------------------------------------------
# Nothing recorded about a job is produced inside its container, where the code of the commit
# runs: the seconds come from the runner's clock around the container run, the peak memory from
# readings taken through Docker, the exit code from Docker. They are kept in a host-private
# directory per request until the result is written.

request_host_dir() {
  echo "$STAGE_DIR/$1.host"
}

# host_number <key> <name> <default>: a number the runner recorded for the request, else the
# default (the seconds of the whole request, or 0 for a peak memory that is unknown).
host_number() {
  local value
  value="$(head -c "$HOST_NUMBER_MAX_DIGITS" "$(request_host_dir "$1")/$2" 2>/dev/null)" || value=""
  [[ "$value" =~ ^[0-9]+$ ]] && echo "$value" || echo "$3"
}

# Whole seconds of a clock that only moves forward (the wall clock can be set).
monotonic_seconds() {
  perl -MTime::HiRes=clock_gettime,CLOCK_MONOTONIC -e 'printf "%d\n", clock_gettime(CLOCK_MONOTONIC)'
}

# "812.3MiB / 6GiB", as docker stats prints it, to bytes; 0 when there is no reading.
memory_usage_bytes() {
  awk -v usage="${1%% *}" 'BEGIN {
    value = usage + 0
    if (usage ~ /GiB$/) value *= 1073741824
    else if (usage ~ /MiB$/) value *= 1048576
    else if (usage ~ /KiB$/) value *= 1024
    else if (usage !~ /[0-9]B$/) value = 0
    printf "%d", value }'
}

# Keeps the highest memory reading of a running container in a host file. The readings come
# from Docker, outside the container. It stops when the container is gone, or with the deadline.
# usage: sample_peak_memory <container> <file>
sample_peak_memory() {
  local name="$1" file="$2" tick usage bytes peak=0
  for ((tick = 0; tick * MEMORY_SAMPLE_SECONDS < VERB_TIMEOUT_SECONDS + DEADLINE_GRACE_SECONDS; tick++)); do
    usage="$(run_with_timeout "$STATS_TIMEOUT_SECONDS" docker stats --no-stream --format '{{.MemUsage}}' "$name" 2>/dev/null)" || usage=""
    bytes="$(memory_usage_bytes "$usage")"
    if [ "$bytes" -gt "$peak" ]; then peak="$bytes"; echo "$peak" >"$file"; fi
    if [ -z "$usage" ] && [ "$peak" -gt 0 ]; then return 0; fi
    sleep "$MEMORY_SAMPLE_SECONDS" 9>&-
  done
}

# Runs the command that runs a container and records, on the host, how long it took and the
# highest memory reading of the container meanwhile.
# usage: run_measured <key> <container> <command...>
run_measured() {
  local host_dir sampler started code=0
  host_dir="$(request_host_dir "$1")"
  mkdir -p "$host_dir"
  sample_peak_memory "$2" "$host_dir/peak_bytes" >/dev/null 2>&1 &
  sampler=$!
  started="$(monotonic_seconds)"
  "${@:3}" || code=$?
  echo "$(( $(monotonic_seconds) - started ))" >"$host_dir/seconds"
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  return "$code"
}

# job_exit_code <container> <status of the docker client>: the exit status of the job as Docker
# reports it for the container (128 plus the signal when a signal ended it). The client's
# status is used only when the container is gone, as after the runner's deadline removed it.
job_exit_code() {
  local state
  state="$(dk inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$1" 2>/dev/null)" || state=""
  if [[ "$state" =~ ^exited\ ([0-9]{1,3})$ ]]; then echo "${BASH_REMATCH[1]}"; else echo "$2"; fi
}

# bounded_log [limit]: copies standard input to standard output up to the limit (MAX_LOG_BYTES
# unless given), then keeps reading and drops the rest: whoever floods the log neither fills
# the disk nor stops, and a last line says so.
bounded_log() {
  perl "$BOX_ROOT/lib/bounded_log.pl" "${1:-$MAX_LOG_BYTES}" ${2:+"$2" "$LOG_DRAIN_SECONDS"}
}

# logged <done file> <limit> <command...>: runs the command with its output through the byte
# limit to standard output, and returns its exit code, or the reader's when the command
# succeeded and the reader did not. The reader of the output stops a moment
# after the command has ended, also when something the command left behind in another session
# still holds the pipe: nobody waits for that.
readonly LOG_DRAIN_SECONDS=2
logged() {
  local done_file="$1" limit="$2" status
  shift 2
  rm -f "$done_file"
  ! { code=0; "$@" || code=$?; : >"$done_file"; exit "$code"; } 2>&1 | bounded_log "$limit" "$done_file"
  status=("${PIPESTATUS[@]}")
  rm -f "$done_file"
  # A command that succeeded while the reader of its log failed did not succeed for box: what
  # it printed was not kept.
  [ "${status[0]}" -ne 0 ] || return "${status[1]:-0}"
  return "${status[0]}"
}

job_oom_killed() {
  local state
  if request_is_native "$1"; then echo false; return 0; fi
  state="$(dk inspect -f '{{.State.OOMKilled}}' "$(job_container_name "$1")" 2>/dev/null || true)"
  [ "$state" = "true" ] && echo true || echo false
}

# Runs a command that waits for a container, with a deadline. When it passes, the container is
# asked to go, and then the command itself is ended, whether or not Docker answered: the
# runner never waits for a Docker that stopped answering.
# usage: run_with_deadline <seconds> <container name> <command...>
run_with_deadline() {
  local seconds="$1" name="$2" child watchdog code=0 flag
  shift 2
  flag="$(mktemp "${TMPDIR:-/tmp}/box-deadline.XXXXXX")"
  rm -f "$flag"
  "$@" &
  child=$!
  end_at_deadline "$seconds" "$name" "$child" "$flag" >/dev/null 2>&1 &
  watchdog=$!
  wait "$child" || code=$?
  kill "$watchdog" 2>/dev/null || true
  wait "$watchdog" 2>/dev/null || true
  if [ -e "$flag" ]; then
    rm -f "$flag"
    echo "[box] deadline of ${seconds}s expired, container $name removed"
    return "$TIMEOUT_EXIT_CODE"
  fi
  return "$code"
}

# The watchdog of run_with_deadline: sleeps, then removes the container (bounded) and ends the
# waiting command, first asking, then killing. Its sleeps hold neither the runner's lock nor a
# task's (descriptors 9 and 5): a sleep may outlive the command that started it by a moment.
end_at_deadline() {
  local seconds="$1" name="$2" child="$3" flag="$4" tick
  for ((tick = 0; tick < seconds; tick++)); do sleep 1 9>&- 5>&-; done
  touch "$flag"
  dk rm -f "$name" || true
  kill -TERM "$child" 2>/dev/null || true
  sleep "$DEADLINE_KILL_SECONDS" 9>&- 5>&-
  kill -KILL "$child" 2>/dev/null || true
}

# ---- one job ----------------------------------------------------------------------------

# Dependencies (for a project that installs any), checkout, job. The checkout step has no
# network and sees the workspace read-only; the install step sees only package.json and the
# lockfile. A cancel is requested
# before every phase; every container carries the label agentbox.request=<key>, by which
# box cancel removes the one that is running.
run_job() {
  local task="$1" id="$2" sha="$3" command="$4" checkout_volume deps="" code=0 problem
  require_box_task "$task"
  require_task_disk_budget "$task" || return 1
  checkout_volume="$(checkout_volume "$id")"
  import_commit "$task" "$sha" || { echo "[box] the commit $sha could not be verified"; return 1; }
  problem="$(tree_problem "$task" "$sha")"
  if [ -n "$problem" ]; then echo "[box] the commit was not checked: $problem"; return "$REJECTED_EXIT_CODE"; fi
  if [ -n "$INSTALL" ]; then deps="$(prepare_dependencies "$task" "$id" "$sha")" || return 1; fi
  cancel_requested "$id" && return "$CANCELLED_EXIT_CODE"
  ensure_volume "$checkout_volume" "$task" || return 1
  run_checkout "$task" "$sha" "$checkout_volume" "$id" || return 1
  cancel_requested "$id" && return "$CANCELLED_EXIT_CODE"
  run_job_container "$task" "$id" "$command" "$checkout_volume" "$deps" || code=$?
  return "$code"
}

# Prints the name of the pristine dependency volume of the task for the commit, building it when
# new. Progress goes to stderr (the job log).
prepare_dependencies() {
  local task="$1" id="$2" sha="$3" stage files key volume
  stage="$STAGE_DIR/$id"
  mkdir -p "$stage"
  files="$(extract_package_files "$task" "$sha" "$stage")" || { echo "[box] cannot read package files of $sha" >&2; return 1; }
  cancel_requested "$id" && return "$CANCELLED_EXIT_CODE"
  key="$(dependency_key "$files")" || return 1
  volume="$(pristine_deps_volume "$task" "$key")"
  if ! deps_ready "$task" "$key" "$volume"; then
    build_pristine_deps "$key" "$volume" "$stage" "$task" "$id" >&2 || return 1
  fi
  echo "$volume"
}

# dependency_key <hash of package.json and the lockfile>: a whole SHA-256 (64 hex digits) that
# names everything the dependencies are built from: the two files (their exact bytes, see
# files_digest.sh), the install and rebuild commands of the project, and the image they are
# built in: its content id, its system and its architecture. The tools that do the install
# (node, npm) are part of the image, so another version of them is another image id. Each part
# goes in with its length before it. Dependencies built from anything else are other
# dependencies. Nothing of the digest is cut off: a short one can be made to collide, and a
# commit would then run on the dependencies of another. Fails when the image cannot be read.
dependency_key() {
  local LC_ALL=C image part key
  image="$(dk image inspect -f '{{.Id}} {{.Os}}/{{.Architecture}}' "$IMAGE" 2>/dev/null)" || return 1
  [[ "$image" =~ ^sha256:[0-9a-f]{64}\ [a-z0-9]+/[a-z0-9_]+$ ]] || return 1
  key="$(for part in "$1" "$INSTALL" "${REBUILD:-}" "$image"; do printf '%s\n%s\n' "${#part}" "$part"; done | sha256_of)"
  [[ "$key" =~ ^[0-9a-f]{64}$ ]] && echo "$key"
}

deps_marker() {
  echo "$DEPS_DIR/$1.$2.ok"
}

# usage: deps_ready <task> <key> <volume>: the marker of that key is there and holds the whole
# key, and the volume exists. (A volume and a marker named by the 12 digits of an older box
# have another name and are never taken for these.)
deps_ready() {
  [ "$(head -c 65 "$(deps_marker "$1" "$2")" 2>/dev/null)" = "$2" ] && volume_exists "$3"
}

build_pristine_deps() {
  local key="$1" volume="$2" stage="$3" task="$4" request="$5" name started=$SECONDS
  name="$(install_container_name "$task" "$key")"
  echo "[box] building pristine dependencies $volume"
  mkdir -p "$DEPS_DIR"
  rm -f "$(deps_marker "$task" "$key")"
  dk volume rm "$volume" >/dev/null 2>&1 || true
  dk rm -f "$name" >/dev/null 2>&1 || true
  ensure_volume "$volume" "$task" || return 1
  build_base_flags "${BOX_RUNNER_CPUS:-$DEFAULT_RUNNER_CPUS}" "${BOX_RUNNER_MEMORY:-$DEFAULT_RUNNER_MEMORY}" "$task"
  if run_with_deadline "$((INSTALL_TIMEOUT_SECONDS + DEADLINE_GRACE_SECONDS))" "$name" \
    docker run --rm "${BASE_FLAGS[@]}" --name "$name" --label agentbox.kind=install --label "agentbox.task=$task" --label "agentbox.request=$request" \
    -e "BOX_INSTALL=$INSTALL" -e "BOX_REBUILD=${REBUILD:-}" -e "BOX_INSTALL_TIMEOUT_SECONDS=$INSTALL_TIMEOUT_SECONDS" \
    -v "$stage:/stage:ro" -v "$volume:/ws/node_modules" -v "$RUNNER_SCRIPTS:/box:ro" \
    -w /ws "$IMAGE" bash /box/install.sh; then
    echo "$key" >"$(deps_marker "$task" "$key")"
    echo "[box] pristine dependencies ready in $((SECONDS - started))s"
  else
    dk volume rm "$volume" >/dev/null 2>&1 || true
    return 1
  fi
}

run_checkout() {
  run_with_timeout "$CHECKOUT_TIMEOUT_SECONDS" docker run "${HELPER_FLAGS[@]}" --user agent --label "agentbox.task=$1" --label "agentbox.request=$4" \
    -e "BOX_SPARSE=$(sparse_enabled)" -e "BOX_SUBDIR=$SUBDIR" \
    -v "$(task_verified_dir "$1"):/verified:ro" -v "$REPO:/trusted:ro" -v "$3:/ws" -v "$RUNNER_SCRIPTS:/box:ro" \
    "$IMAGE" bash /box/checkout.sh "$2"
}

# Prints the mount of the dependencies into the job: the pristine volume read-only, or a private
# copy that is removed with the job (BOX_DEPS_MODE=copy). Prints nothing for a project without
# dependencies.
dependency_mount() {
  local id="$1" deps="$2" copy
  [ -n "$deps" ] || return 0
  copy="$(job_deps_volume "$id")"
  if [ "$DEPS_MODE" = "copy" ]; then
    ensure_volume "$copy" "${id##*.}" >&2 || return 1
    run_with_timeout "$COPY_TIMEOUT_SECONDS" docker run "${HELPER_FLAGS[@]}" --label "agentbox.task=${id##*.}" --label "agentbox.request=$id" --user agent \
      -v "$deps:/from:ro" -v "$copy:/to" "$IMAGE" cp -a /from/. /to/ >&2 || return 1
    echo "$copy:/ws$(subdir_suffix)/node_modules"
  else
    echo "$deps:/ws$(subdir_suffix)/node_modules:ro"
  fi
}

run_job_container() {
  local task="$1" id="$2" command="$3" checkout_volume="$4" deps="$5" name mount code=0
  name="$(job_container_name "$id")"
  mount="$(dependency_mount "$id" "$deps")" || return 1
  cancel_requested "$id" && return "$CANCELLED_EXIT_CODE"
  build_base_flags "${BOX_RUNNER_CPUS:-$DEFAULT_RUNNER_CPUS}" "${BOX_RUNNER_MEMORY:-$DEFAULT_RUNNER_MEMORY}" "$task"
  run_measured "$id" "$name" run_with_deadline "$((VERB_TIMEOUT_SECONDS + DEADLINE_GRACE_SECONDS))" "$name" \
    docker run "${BASE_FLAGS[@]}" \
    --name "$name" --label agentbox.kind=job --label "agentbox.task=$task" --label "agentbox.request=$id" \
    -e "BOX_SUBDIR=$SUBDIR" -e "BOX_VERB_CMD=$command" -e "BOX_TIMEOUT_SECONDS=$VERB_TIMEOUT_SECONDS" \
    -v "$checkout_volume:/ws" ${mount:+-v "$mount"} -v "$RUNNER_SCRIPTS:/box:ro" \
    -w /ws "$IMAGE" bash /box/job.sh || code=$?
  return "$(job_exit_code "$name" "$code")"
}

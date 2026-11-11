# shellcheck shell=bash
# Play: the run command of a project (RUN), started for a lavoro on the Mac so that the person
# can look at the app the lavoro is building. It runs in the folder of the project inside the
# worktree, detached from box, in a process group of its own, with a range of ports reserved
# for the lavoro (SAKURA_PORT is the first; CONDUCTOR_PORT carries the same value), and its output
# in a bounded log of the data directory. Nothing runs at rest: Play runs only between start and
# stop, and box rm stops it.
#
# box play <task> start|stop|status [--json], box play <task> log.

readonly PLAY_DIR="$AGENTBOX_HOME/play"
readonly PLAY_FIRST_PORT=56000
readonly PLAY_LAST_PORT=65535
readonly PLAY_DEFAULT_PORTS=10
readonly PLAY_MAX_PORTS=100
readonly PLAY_MAX_LOG_BYTES="${BOX_PLAY_MAX_LOG_BYTES:-8388608}"
readonly PLAY_LOG_LINES=400
readonly PLAY_START_TICKS=50
readonly PLAY_SETTLE_SECONDS=0.5
readonly PLAY_STOP_TICKS=30
readonly PLAY_IN_BOX_EXIT_CODE=81
readonly PLAY_USAGE="usage: box play <task> start|stop|status [--json] | box play <task> log"
# native_run.pl with the command, its output through the byte limit into the log; the reader of
# the log stops a moment after the command has ended (the done file), whoever still holds the pipe.
# Arguments: native_run.pl, pid file, command, bounded_log.pl, limit, log, done file, drain
# seconds, perl, bash (the last two by absolute path: this runs inside the worktree).
readonly PLAY_PIPELINE='{ "$9" "$1" 0 "$2" "${10}" -c "$3"; : >"$7"; } 2>&1 | "$9" "$4" "$5" "$7" "$8" >"$6"'

project_ports() {
  echo "${PORTS:-$PLAY_DEFAULT_PORTS}"
}

project_run_mode() {
  echo "${RUN_MODE:-one}"
}

# Refresh only native preview settings from the current host-private project file.
refresh_play_settings() {
  local settings
  settings="$(
    load_project "$(task_project "$1")"
    printf '%s\n' "${RUN:-}" "$(project_ports)" "$(project_run_mode)"
  )" || return 1
  { IFS= read -r RUN; IFS= read -r PORTS; IFS= read -r RUN_MODE; } <<<"$settings"
}

cmd_play() {
  local task="${1:-}" action="${2:-}" json="${3:-}"
  [ "$#" -ge 2 ] && [ "$#" -le 3 ] || die "$PLAY_USAGE"
  require_existing_task "$task"
  if ! task_is_on_mac "$task"; then
    warn "Play in a box is not built yet (task '$task' is in a box)"
    exit "$PLAY_IN_BOX_EXIT_CODE"
  fi
  [ -z "$json" ] || { [ "$json" = "--json" ] && [ "$action" != "log" ]; } || die "$PLAY_USAGE"
  if [ "$action" = "start" ]; then begin_startup "$task" mac; fi
  require_checked_lavoro "$task"
  case "$action" in
    start) with_lock "$LOCK_DIR/play.lock" play_start "$task" || exit 1 ;;
    stop) with_lock "$LOCK_DIR/play.lock" play_stop "$task" || exit 1 ;;
    status) ;;
    log) play_log "$task"; return ;;
    *) die "$PLAY_USAGE" ;;
  esac
  if [ "$json" = "--json" ]; then play_status_json "$task"; else play_status_line "$task"; fi
}

# ---- state: host-private files of the data directory ------------------------------------------

play_file() {
  echo "$PLAY_DIR/$1.$2"
}

# Prints the process group of the Play of a task while it runs; fails otherwise.
play_group() {
  local group
  group="$(recorded_group "$(play_file "$1" pid)")" && kill -0 -- "-$group" 2>/dev/null && echo "$group"
}

# Prints "<first port> <count>" of the range reserved for a task; fails when it has none.
play_ports() {
  local first count
  [ -f "$(play_file "$1" ports)" ] || return 1
  read -r first count <"$(play_file "$1" ports)" || return 1
  [[ "$first" =~ ^[0-9]{4,5}$ ]] && [[ "$count" =~ ^[0-9]{1,3}$ ]] && echo "$first $count"
}

# Reserves the current port count, retaining an unchanged range between starts. A changed
# count is allocated afresh without overlapping any other task reservation.
reserve_play_ports() {
  local task="$1" count first file other_first other_count reservation moved=1
  count="$(project_ports)"
  reservation="$(play_file "$task" ports)"
  if read -r first other_count < <(play_ports "$task"); then
    [ "$other_count" != "$count" ] || return 0
  fi
  first="$PLAY_FIRST_PORT"
  while [ "$moved" -eq 1 ]; do
    moved=0
    for file in "$PLAY_DIR"/*.ports; do
      [ "$file" != "$reservation" ] && [ -f "$file" ] || continue
      read -r other_first other_count <"$file" || continue
      if [ "$first" -lt $((other_first + other_count)) ] && [ "$other_first" -lt $((first + count)) ]; then
        first=$((other_first + other_count))
        moved=1
      fi
    done
  done
  [ $((first + count - 1)) -le "$PLAY_LAST_PORT" ] || die "no range of $count ports is free for the Play of task '$task'"
  echo "$first $count" >"$(play_file "$task" ports)"
}

# ---- start, stop --------------------------------------------------------------------------------

play_start() {
  local task="$1" first count folder other launcher launcher_start pid_file log done_file
  refresh_play_settings "$task" || return 1
  [ -n "${RUN:-}" ] || die "the project of task '$task' has no run command (box project set $(task_project "$task") --run '<command>')"
  mkdir -p "$PLAY_DIR"
  ! play_group "$task" >/dev/null || return 0
  if [ "$(project_run_mode)" = "one" ]; then
    for other in $(project_tasks "$(task_project "$task")"); do
      [ "$other" = "$task" ] || end_play "$other"
    done
  fi
  confirm_startup "$task"
  reserve_play_ports "$task"
  read -r first count <<<"$(play_ports "$task")"
  folder="$(lavoro_folder "$task")"
  [ -d "$folder" ] || die "the folder of the project is not in the worktree of task '$task': $folder"
  rm -f "$(play_file "$task" pid)" "$(play_file "$task" done)"
  lavoro_environment "$task"
  pid_file="$(play_file "$task" pid)" log="$(play_file "$task" log)" done_file="$(play_file "$task" done)"
  (cd "$folder" && "$TRUSTED_ENV" "${LAVORO_ENVIRONMENT[@]}" "SAKURA_PORT=$first" "CONDUCTOR_PORT=$first" "SAKURA_PORTS=$count" \
    "$TRUSTED_PERL" -MPOSIX=setsid -e 'setsid(); exec @ARGV' "$TRUSTED_BASH" -c "$PLAY_PIPELINE" play "$NATIVE_RUN" "$pid_file" "$RUN" \
    "$BOX_ROOT/lib/bounded_log.pl" "$PLAY_MAX_LOG_BYTES" "$log" "$done_file" "$LOG_DRAIN_SECONDS" \
    "$TRUSTED_PERL" "$TRUSTED_BASH") </dev/null >/dev/null 2>&1 5>&- 6>&- 7>&- 8>&- 9>&- &
  launcher=$!
  launcher_start="$(process_start "$launcher")" || launcher_start=""
  now_iso >"$(play_file "$task" since)"
  if ! wait_for_play "$task"; then
    # A launch that did not come to run is ended before the lock is let go: the launcher (a
    # session of its own once it has started) and whatever group it recorded.
    end_failed_play_launcher "$launcher" "$launcher_start"
    end_play "$task"
    rm -f "$(play_file "$task" since)"
    die "the run command of task '$task' did not start or ended at once; its output: box play $task log"
  fi
  journal_event play-start "$task"
}

# A failed launch may have been reaped already; only its original process may be signalled.
end_failed_play_launcher() {
  [ -n "$2" ] && [ "$(process_start "$1")" = "$2" ] || return 0
  kill -KILL -- "-$1" 2>/dev/null || kill -KILL "$1" 2>/dev/null || true
  wait "$1" 2>/dev/null || true
}

# Waits until the Play of a task has started, then a moment more: a command that ends at once
# did not start a Play. Fails when it does not run then.
wait_for_play() {
  local tick
  for ((tick = 0; tick < PLAY_START_TICKS; tick++)); do
    [ ! -e "$(play_file "$1" pid)" ] || break
    sleep 0.1
  done
  sleep "$PLAY_SETTLE_SECONDS"
  play_group "$1" >/dev/null
}

play_stop() {
  end_play "$1"
  journal_event play-stop "$1"
}

# Ends the whole process group of the Play of a task: asked first, killed a moment later. Does
# nothing when it does not run.
end_play() {
  local task="$1" group tick
  group="$(play_group "$task")" || { rm -f "$(play_file "$task" pid)" "$(play_file "$task" since)"; return 0; }
  kill -TERM -- "-$group" 2>/dev/null || true
  for ((tick = 0; tick < PLAY_STOP_TICKS; tick++)); do
    kill -0 -- "-$group" 2>/dev/null || break
    sleep 0.1
  done
  kill_if_still_recorded "$(play_file "$task" pid)" "$group"
  rm -f "$(play_file "$task" pid)" "$(play_file "$task" since)"
}

# Stops the Play of a task and gives its ports back: for box rm.
forget_play() {
  end_play "$1"
  rm -f "$(play_file "$1" ports)" "$(play_file "$1" log)" "$(play_file "$1" done)"
}

# ---- status, log --------------------------------------------------------------------------------

play_status_json() {
  local task="$1" group="" first=0 count=0 since=""
  group="$(play_group "$task")" || group=""
  if play_ports "$task" >/dev/null; then read -r first count <<<"$(play_ports "$task")"; fi
  [ -z "$group" ] || since="$(head -c 20 "$(play_file "$task" since)" 2>/dev/null | tr -cd '0-9TZ:-')"
  jq -nc --arg task "$task" --arg group "$group" --argjson first "$first" --argjson count "$count" --arg since "$since" '
    { task: $task, running: ($group != ""), pid: (if $group == "" then null else ($group | tonumber) end),
      ports: [range($first; $first + $count)], since: (if $since == "" then null else $since end) }'
}

play_status_line() {
  local task="$1" group first count
  if group="$(play_group "$task")"; then
    read -r first count <<<"$(play_ports "$task")"
    echo "Play of task '$task' runs (process group $group, ports $first to $((first + count - 1)))"
  else
    echo "Play of task '$task' does not run"
  fi
}

play_log() {
  [ -f "$(play_file "$1" log)" ] || { warn "the Play of task '$1' has no output yet"; return "$NOT_FOUND_EXIT_CODE"; }
  tail -n "$PLAY_LOG_LINES" "$(play_file "$1" log)" | LC_ALL=C tr -cd '\n\t[:print:]'
}

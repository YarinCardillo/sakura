# shellcheck shell=bash
# Agent sessions: one container per task named agentbox-agent-<task>, started by box agent.

readonly AGENTS_DIR="$AGENTBOX_HOME/agents"
readonly ATTACH_LOG_LINES=2000
# Claude Code uses ctrl-p, ctrl-q and ctrl-] itself; ctrl-\ twice detaches without stopping it.
readonly DETACH_KEYS='ctrl-\,ctrl-\'
readonly NO_AGENT_EXIT_CODE=3

container_is_running() {
  [ "$(dk inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" = "true" ]
}

agent_is_running() {
  container_is_running "$(agent_container_name "$1")"
}

# Records account and start time in a host-private file (the container name alone does not say).
record_agent() {
  mkdir -p "$AGENTS_DIR"
  printf 'ACCOUNT=%s\nSTARTED=%s\nEXIT_RECORDED=0\n' "$2" "$(now_iso)" >"$AGENTS_DIR/$1.env"
}

agent_record_field() {
  sed -n "s/^$2=//p" "$AGENTS_DIR/$1.env" 2>/dev/null | head -1
}

# Journals the exit of an exited agent container once.
note_agent_exit_once() {
  local task="$1" exit_code="$2"
  [ "$(agent_record_field "$task" EXIT_RECORDED)" = "0" ] || return 0
  # (Rewritten through a second file: sed -i takes other arguments on macOS and on Linux.)
  sed 's/^EXIT_RECORDED=0$/EXIT_RECORDED=1/' "$AGENTS_DIR/$task.env" >"$AGENTS_DIR/$task.env.new" \
    && mv "$AGENTS_DIR/$task.env.new" "$AGENTS_DIR/$task.env"
  journal_event agent-exit "$task" "" "" "$exit_code" "$(agent_record_field "$task" ACCOUNT)"
}

# One front end per task, the terminal agent or a conversation: they share the session history
# of the task. This lock is taken before any check and kept by the process that took it until
# it ends: the whole session for box acp and for an attached box agent (the docker client it
# becomes keeps the descriptor), the start for a detached agent, whose container then says that
# it runs. Two starts at the same moment cannot both pass the checks.
hold_front_end_lock() {
  local task="$1"
  mkdir -p "$LOCK_DIR"
  exec 6>>"$LOCK_DIR/frontend-$task.lock"
  if perl -MFcntl=:flock -e 'open(my $f, ">&=", 6) or die "fd 6: $!"; flock($f, LOCK_EX | LOCK_NB) or exit 1'; then return 0; fi
  refuse_second_front_end "$task"
  die "another front end of task '$task' is starting; try again in a moment"
}

# Says which front end is open and ends, when one is.
refuse_second_front_end() {
  if agent_is_running "$1"; then die "an agent runs in the terminal for task '$1'; stop it first (box stop $1)"; fi
  if acp_is_running "$1"; then die "a conversation is open for task '$1'; close it first (box stop $1)"; fi
}

remove_exited_agent() {
  local name
  name="$(agent_container_name "$1")"
  if agent_is_running "$1"; then die "an agent is already running for task '$1' (box stop $1, or box attach $1)"; fi
  if acp_is_running "$1"; then die "a conversation is open for task '$1'; close it first (box stop $1)"; fi
  dk rm -f "$name" >/dev/null 2>&1 || true
}

# Starts the agent container. Arguments: task, account, mode (attached|detached), claude args...
start_agent() {
  local task="$1" account="$2" mode="$3" credential_files
  shift 3
  require_existing_task "$task"
  refuse_on_mac "$task" "box agent" "talk to its agent with box acp, or run claude in $(task_dir "$task") yourself"
  require_account_name "$account"
  begin_startup "$task" box
  load_task_project "$task"
  require_docker
  volume_exists "$(account_volume "$account")" || die "account '$account' has no login yet (box login $account)"
  credential_files="$(account_credential_files "$account")"
  grep -qx claude <<<"$credential_files" || die "account '$account' has no login yet (box login $account)"
  hold_front_end_lock "$task"
  remove_exited_agent "$task"
  ensure_task_volumes "$task"
  build_agent_flags "$task"
  AGENT_FLAGS+=(--log-driver json-file --log-opt max-size=8m --log-opt max-file=2)
  add_workspace_flags "$task" "$(agent_container_name "$task")"
  ensure_config_volume "$task" "$account"
  add_task_account_flags "$task" "$account" "$credential_files"
  confirm_startup "$task"
  record_agent "$task" "$account"
  if [ "$mode" = "detached" ]; then
    docker run -d -it "${AGENT_FLAGS[@]}" "$IMAGE" bash -c "$ACCOUNT_ENTRY" _ claude "$@" 5>&- >/dev/null
    journal_event agent-start "$task" "" "" "" "$account"
  else
    journal_event agent-start "$task" "" "" "" "$account"
    run_confirmed "$task" "$(agent_container_name "$task")" docker run -it "${AGENT_FLAGS[@]}" "$IMAGE" bash -c "$ACCOUNT_ENTRY" _ claude "$@"
  fi
}

# box attach <task>: the recent log, then a live attach. Closing the attach leaves the agent running.
cmd_attach() {
  local task="${1:-}" name
  require_existing_task "$task"
  refuse_on_mac "$task" "box attach" "there is no terminal agent of a box to attach to"
  require_docker
  name="$(agent_container_name "$task")"
  if ! agent_is_running "$task"; then
    note_exited_agent "$task"
    warn "no agent is running for task '$task'"
    return "$NO_AGENT_EXIT_CODE"
  fi
  dk logs --tail "$ATTACH_LOG_LINES" "$name" 2>&1
  exec docker attach --detach-keys "$DETACH_KEYS" "$name"
}

# Journals the exit of an exited agent container, when there is one.
note_exited_agent() {
  local exit_code
  exit_code="$(dk inspect -f '{{.State.ExitCode}}' "$(agent_container_name "$1")" 2>/dev/null)" || return 0
  [[ "$exit_code" =~ ^[0-9]+$ ]] && note_agent_exit_once "$1" "$exit_code"
  return 0
}

cmd_stop() {
  local task="${1:-}"
  require_existing_task "$task"
  if task_is_on_mac "$task"; then stop_on_mac "$task"; return; fi
  require_docker
  if acp_is_running "$task"; then dk stop "$(acp_container_name "$task")" >/dev/null; return 0; fi
  agent_is_running "$task" || die "no agent is running for task '$task'"
  dk stop "$(agent_container_name "$task")" >/dev/null
  note_exited_agent "$task"
}

# box shell <task>: a terminal for the person. For a lavoro on the Mac their own login shell in
# its folder (native_shell.sh); for a task in a box interactive bash in a one-off container.
cmd_shell() {
  local task="${1:-}" name
  require_existing_task "$task"
  if task_is_on_mac "$task"; then shell_on_mac "$task"; fi
  begin_startup "$task" box
  load_task_project "$task"
  require_docker
  ensure_task_volumes "$task"
  build_agent_flags "$task"
  name="$(workspace_container_name "$task" "$RANDOM")"
  add_workspace_flags "$task" "$name"
  confirm_startup "$task"
  run_confirmed "$task" "$name" docker run --rm -it "${AGENT_FLAGS[@]}" "$IMAGE" bash
}

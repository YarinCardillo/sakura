# shellcheck shell=bash
# box acp <task> <account>: the account's ACP adapter in the box of a task, for a client that
# speaks the Agent Client Protocol over stdin and stdout (the conversation view of Sakura).
#
# Standard output carries only messages of the adapter (newline-delimited JSON-RPC); box itself
# prints nothing there. The adapter's standard error goes to a host-private log. The container
# has the confinement of box agent, no TTY, and is removed when it ends. It ends when standard
# input closes or when box gets a signal, which it passes on.
#
# box is the boundary, not the client: lib/acp_proxy.pl stands between the two and passes only
# what a confined agent may say to a host program (see its header). The agent is never told that
# the client offers files or terminals, and its requests for them never reach the client.

readonly ACP_COMMAND="claude-agent-acp"
# The adapter drives the Claude Code CLI of the image instead of a copy of its own.
readonly ACP_CLAUDE_EXECUTABLE="/usr/local/bin/claude"
readonly ACP_LOG_DIR="$AGENTBOX_HOME/acp"
readonly ACP_LAST_LINE_LENGTH=200
readonly ACP_PROXY="$BOX_ROOT/lib/acp_proxy.pl"
# One JSON-RPC message is one line; a longer line ends the session (exit code 65).
readonly ACP_MAX_LINE_BYTES="${BOX_ACP_MAX_LINE_BYTES:-8388608}"
readonly ACP_OVERSIZED_EXIT_CODE=65
# Nothing is waited for without a limit: an agent still there this long after the client closed
# its side or a signal requested the end (whatever the agent still sends or reads), or a side that has not taken what waits for it for this long, ends the session (exit code 66).
readonly ACP_END_SECONDS="${BOX_ACP_END_SECONDS:-10}"
readonly ACP_STALL_SECONDS="${BOX_ACP_STALL_SECONDS:-120}"
readonly ACP_STALLED_EXIT_CODE=66
# What is kept of the adapter's standard error, and how much of its end is read for the last line.
readonly ACP_MAX_LOG_BYTES="${BOX_ACP_MAX_LOG_BYTES:-4194304}"
# Of that limit, the room kept for the line that says the log was cut and for the summary of
# what the proxy refused, which are written after the adapter's own words.
readonly ACP_LOG_RESERVE_BYTES=2560
readonly ACP_TAIL_BYTES=4096

acp_is_running() {
  container_is_running "$(acp_container_name "$1")"
}

# Exit codes: 0 the adapter ended cleanly after standard input closed; 1 box refused to start it
# (one line on stderr); 65 a line longer than ACP_MAX_LINE_BYTES came from either side and the
# session was ended; 66 a deadline ended it (the agent did not end after the conversation was closed, or a
# side stopped taking what was sent to it); anything else is the exit code of the adapter or of docker (125 to 127),
# 128 plus the signal number when a signal ended it, with a last line on stderr that says so.
cmd_acp() {
  local task="${1:-}" account="${2:-}"
  [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "usage: box acp <task> [<account>]"
  # The client's requests wait on descriptor 3 until the adapter runs; nothing before it reads them.
  exec 3<&0 </dev/null
  require_existing_task "$task"
  if task_is_on_mac "$task"; then native_acp "$task" "$account"; return; fi
  [ "$#" -eq 2 ] || die "usage: box acp <task> <account> (a task in a box needs its account named)"
  require_account_name "$account"
  begin_startup "$task" box
  load_task_project "$task"
  require_docker
  hold_front_end_lock "$task"
  refuse_second_front_end "$task"
  prepare_acp_flags "$task" "$account" >&2
  confirm_startup "$task"
  run_acp_container "$task"
}

# Fills AGENT_FLAGS like box agent does: the task's network, workspace, dependencies and
# configuration volume, and of the account only the credential files it has. An account that
# never logged in gets none, and the adapter then answers that authentication is required.
prepare_acp_flags() {
  local task="$1" account="$2" credential_files=""
  dk rm -f "$(acp_container_name "$task")" >/dev/null 2>&1 || true
  ensure_task_volumes "$task"
  build_agent_flags "$task"
  add_workspace_flags "$task" "$(acp_container_name "$task")"
  ensure_config_volume "$task" "$account"
  if volume_exists "$(account_volume "$account")"; then credential_files="$(account_credential_files "$account")"; fi
  add_task_account_flags "$task" "$account" "$credential_files"
  AGENT_FLAGS+=(--label agentbox.kind=acp -e "CLAUDE_CODE_EXECUTABLE=$ACP_CLAUDE_EXECUTABLE")
}

# start_log_reader <pipe> <log> <done file>: starts the reader that copies what the adapter
# writes to the pipe into the log, within the byte limit; its pid is in LOG_READER_PID. Opening
# a pipe waits for the other side, so the reader opens it itself, after the descriptors of box
# it inherited are closed (the client's input, the lifetime pipe, the lifecycle lock, the lock
# of the front end): were box killed before it starts the writer, no lock stays taken by a
# reader that waits. And it does not wait for ever: after LOG_RENDEZVOUS_SECONDS without a
# writer it ends by itself.
readonly LOG_RENDEZVOUS_SECONDS="${BOX_LOG_RENDEZVOUS_SECONDS:-30}"
start_log_reader() {
  "$TRUSTED_PERL" "$BOX_ROOT/lib/bounded_log.pl" "$((ACP_MAX_LOG_BYTES - ACP_LOG_RESERVE_BYTES))" "$3" "$LOG_DRAIN_SECONDS" \
    "$1" "$LOG_RENDEZVOUS_SECONDS" </dev/null 3<&- 4>&- 5>&- 6>&- >"$2" &
  LOG_READER_PID=$!
}

# Runs the adapter behind the proxy, with the client's input and output on the proxy's side. A
# signal to box is passed on to the proxy, then to the docker client, then to the adapter; the
# container is removed when the adapter ends, however it ends. Standard error reaches the log
# through a named pipe and the byte limit of bounded_log, so an adapter cannot fill the disk.
# What the proxy refused is counted at the end of the log.
run_acp_container() {
  local task="$1" log report pipe limiter child code=0 ended="$ACP_LOG_DIR/.$1.ended"
  mkdir -p "$ACP_LOG_DIR"
  log="$ACP_LOG_DIR/$task.log"
  report="$ACP_LOG_DIR/.$task.report"
  pipe="$ACP_LOG_DIR/.$task.stderr"
  rm -f "$report" "$pipe" "$ended"
  mkfifo -m 600 "$pipe" || die "could not create the log pipe of task '$task'"
  start_log_reader "$pipe" "$log" "$ended"
  limiter="$LOG_READER_PID"
  perl "$ACP_PROXY" "$ACP_MAX_LINE_BYTES" "$ACP_END_SECONDS" "$ACP_STALL_SECONDS" "$report" \
    docker run --rm -i "${AGENT_FLAGS[@]}" "$IMAGE" "$ACP_COMMAND" <&3 3<&- 5>&- 2>"$pipe" &
  child=$!
  exec 3<&-
  # From here a signal to box is passed on: also while the start is still being waited for.
  trap 'kill -TERM "$child" 2>/dev/null' TERM INT HUP
  wait_until_started "$child" acp_is_running "$task" || abandon_start "$task" "$child" TERM "$(acp_container_name "$task")"
  end_startup
  wait "$child" || code=$?
  while kill -0 "$child" 2>/dev/null; do code=0; wait "$child" || code=$?; done
  # The container is removed by name too: the docker client may be gone while it is still there.
  dk rm -f "$(acp_container_name "$task")" >/dev/null 2>&1 || true
  : >"$ended"
  wait "$limiter" 2>/dev/null || true
  rm -f "$pipe" "$ended"
  say_why_acp_ended "$task" "$code" "$log"
  append_acp_report "$report" "$log"
  return "$code"
}

# Adds the proxy's summary to the end of the log, within what is left of the log's limit.
append_acp_report() {
  local report="$1" log="$2" room
  [ -f "$report" ] || return 0
  room=$((ACP_MAX_LOG_BYTES - $(wc -c <"$log")))
  if [ "$room" -gt 0 ]; then head -c "$room" "$report" >>"$log"; fi
  rm -f "$report"
}

# The one line on stderr that a non-zero exit ends with.
say_why_acp_ended() {
  case "$2" in
    0) ;;
    "$ACP_OVERSIZED_EXIT_CODE") warn "a line of more than $ACP_MAX_LINE_BYTES bytes was sent; the conversation of task '$1' was ended" ;;
    "$ACP_STALLED_EXIT_CODE") warn "$(acp_stall_reason "$1" "$3")" ;;
    *) warn "the adapter of task '$1' ended with exit code $2: $(acp_last_words "$3")" ;;
  esac
}

# Which deadline ended the session, in the words of the proxy's last note in the log.
acp_stall_reason() {
  case "$(tail -c "$ACP_TAIL_BYTES" "$2" 2>/dev/null | grep -a '^\[box\] acp: the ' | tail -n 1)" in
    *"the client did not take"*) echo "the client of task '$1' did not take what the agent sent for $ACP_STALL_SECONDS s; the conversation was ended" ;;
    *"the agent did not take"*) echo "the agent of task '$1' did not take what the client sent for $ACP_STALL_SECONDS s; the conversation was ended" ;;
    *) echo "the agent of task '$1' did not end within $ACP_END_SECONDS s after the conversation was closed; the conversation was ended" ;;
  esac
}

# The last line of the log, read from a bounded piece of its end, as plain text of limited
# length (it is text to show, never to run).
acp_last_words() {
  tail -c "$ACP_TAIL_BYTES" "$1" 2>/dev/null | tail -n 1 | sanitize | cut -c1-"$ACP_LAST_LINE_LENGTH"
}

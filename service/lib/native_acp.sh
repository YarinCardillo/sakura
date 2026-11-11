# shellcheck shell=bash
# box acp <task> [<account>] for a lavoro on the Mac: the Claude ACP adapter, run natively in
# the worktree as the person, with CLAUDE_CONFIG_DIR set to the config directory of the account.
# The adapter is installed once, at a pinned version, under $AGENTBOX_HOME/tools (not fetched at
# every start). It talks to the client through the same filter as the conversation of a box
# (acp_proxy.pl: client capabilities stripped, the agent's requests limited, bounded lines, the
# deadlines), its standard error goes to the same bounded log, and one front end per lavoro is
# open at a time. The adapter and everything it starts live in one process group, which is
# ended when the channel closes: nothing keeps running afterwards.

# The same version as CLAUDE_AGENT_ACP_VERSION in image/Dockerfile: bump the two together.
readonly NATIVE_ACP_VERSION=0.103.0
readonly NATIVE_ACP_PACKAGE="@agentclientprotocol/claude-agent-acp"
readonly TOOLS_DIR="$AGENTBOX_HOME/tools"
readonly NATIVE_ACP_INSTALL_SECONDS="${BOX_INSTALL_TIMEOUT:-1800}"
# The adapter and its installation do not inherit the environment box was started in: a setting
# such as NODE_OPTIONS or npm_config_* would load code or change where packages come from. They
# get these variables, when set, plus the ones the person names in BOX_ADAPTER_ENV; the
# forbidden ones never pass, whoever names them.
readonly ADAPTER_ENVIRONMENT="HOME USER LOGNAME SHELL TMPDIR LANG LC_ALL LC_CTYPE TERM SSH_AUTH_SOCK"
readonly INSTALL_ENVIRONMENT="HOME USER LOGNAME TMPDIR LANG LC_ALL"
readonly FORBIDDEN_ENVIRONMENT='^(NODE_OPTIONS|NODE_PATH|[Nn][Pp][Mm]_[Cc][Oo][Nn][Ff][Ii][Gg]_.*)$'
readonly SYSTEM_PATH="/usr/bin:/bin:/usr/sbin:/sbin"

native_acp_directory() {
  echo "$TOOLS_DIR/claude-agent-acp-$NATIVE_ACP_VERSION"
}

native_acp_command() {
  echo "$(native_acp_directory)/node_modules/.bin/claude-agent-acp"
}

# The file that holds the process group of an open conversation.
conversation_pid_file() {
  echo "$ACP_LOG_DIR/$1.pid"
}

# clean_environment <names> [NAME=value...] -- <command...>: replaces this process by the command,
# with only the named variables that are set (and the names of BOX_ADAPTER_ENV) and the
# assignments given. Call it in a subshell.
clean_environment() {
  local names="$1" name assignments=()
  shift
  for name in $names ${BOX_ADAPTER_ENV:-}; do
    [[ "$name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] && [[ ! "$name" =~ $FORBIDDEN_ENVIRONMENT ]] && [ -n "${!name+set}" ] || continue
    assignments+=("$name=${!name}")
  done
  while [ "$1" != "--" ]; do assignments+=("$1"); shift; done
  shift
  exec "$TRUSTED_ENV" -i "${assignments[@]}" "$@"
}

# ---- the adapter, installed once --------------------------------------------------------------

# The file that names the node and the npm the adapter was installed with, as absolute paths.
native_acp_runtime() {
  echo "$(native_acp_directory)/box-runtime"
}

# Prints the node recorded at installation; the end when it is not there any more.
native_acp_node() {
  local node
  node="$(sed -n 's/^NODE=//p' "$(native_acp_runtime)" 2>/dev/null | head -n 1)"
  [[ "$node" == /* ]] && [ -x "$node" ] \
    || die "the node recorded for the Claude ACP adapter is gone; remove $(native_acp_directory) and start the conversation again to install it anew"
  echo "$node"
}

ensure_native_acp() {
  [ ! -x "$(native_acp_command)" ] || return 0
  trusted_program node >/dev/null && trusted_program npm >/dev/null \
    || die "node and npm are needed to install the Claude ACP adapter (brew install node)"
  mkdir -p "$TOOLS_DIR"
  with_lock "$LOCK_DIR/tools.lock" install_native_acp \
    || die "could not install the Claude ACP adapter $NATIVE_ACP_VERSION; what npm said is in $TOOLS_DIR/install.log"
}

# Installs into a folder of its own and renames it into place: the adapter is there whole or
# not at all. Called with the tools lock held, so two first conversations install it once. npm
# is the one found now, by its absolute path, started in the new folder (not where box was
# called), with the installation's list of variables and a path that begins where node is. The
# node and the npm that were used are recorded for every later start.
install_native_acp() {
  local staged node npm
  [ ! -x "$(native_acp_command)" ] || return 0
  node="$(trusted_program node)" && npm="$(trusted_program npm)" || return 1
  staged="$(mktemp -d "$TOOLS_DIR/.install.XXXXXX")" || return 1
  if (cd "$staged" && clean_environment "$INSTALL_ENVIRONMENT" "PATH=$(dirname "$node"):$SYSTEM_PATH" -- \
    "$TRUSTED_PERL" -e 'alarm shift; exec @ARGV or die "exec failed: $!\n"' "$NATIVE_ACP_INSTALL_SECONDS" "$npm" install --prefix "$staged" \
    --no-audit --no-fund --loglevel=error "$NATIVE_ACP_PACKAGE@$NATIVE_ACP_VERSION") </dev/null >"$TOOLS_DIR/install.log" 2>&1 \
    && [ -x "$staged/node_modules/.bin/claude-agent-acp" ] && printf 'NODE=%s\nNPM=%s\n' "$node" "$npm" >"$staged/box-runtime"; then
    rm -rf "$(native_acp_directory)"
    mv "$staged" "$(native_acp_directory)"
  else
    rm -rf "$staged"
    return 1
  fi
}

# ---- the conversation -------------------------------------------------------------------------

# One front end per lavoro. The lock is held by this process and by the supervisor of the
# session (the relay, which is started with the descriptor open): when box acp is killed
# outright the supervisor still holds it, and lets go only after it has ended the adapter. The
# adapter itself never gets the descriptor.
hold_conversation_lock() {
  mkdir -p "$LOCK_DIR"
  exec 6>>"$LOCK_DIR/frontend-$1.lock"
  perl -MFcntl=:flock -e 'open(my $f, ">&=", 6) or die "fd 6: $!"; flock($f, LOCK_EX | LOCK_NB) or exit 1' \
    || die "a conversation is open for task '$1'; close it first (box stop $1)"
}

conversation_is_open() {
  local lock="$LOCK_DIR/frontend-$1.lock"
  [ -f "$lock" ] || return 1
  ! perl -MFcntl=:flock -e 'open(my $f, "<", $ARGV[0]) or exit 1; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$lock"
}

# The config directory of the account of a conversation: the one named, or the project's.
conversation_account_directory() {
  local task="$1" account="$2" directory
  account="${account:-${ACCOUNT:-}}"
  [ -n "$account" ] || die "name the account of the conversation of task '$task' (box accounts --json lists them), or give its project one (box project set $(task_project "$task") --account <name>)"
  require_account_name "$account"
  directory="$(mac_account_directory "$account")" || die "'$account' is not an account of the Mac (the config of box names them, box accounts --json lists them)"
  [ -d "$directory" ] || die "the config directory of account '$account' does not exist (box accounts --json shows it with exists false)"
  echo "$directory"
}

# Prints the assignment that tells the Claude CLI which config directory to use, or nothing for
# the default one ($HOME/.claude). The CLI keeps the login of a directory under a name that
# depends on whether CLAUDE_CONFIG_DIR is set: with the variable set to the default directory
# it looks for another login than the one the person made without it, and finds none. So the
# default directory is named by leaving the variable out; any other directory (a login made
# with the variable set) by setting it. Both are compared as the same directory on disk, and
# nothing inside either is opened.
config_directory_assignment() {
  local directory="$1" own default
  # Links are followed here: what counts is the directory the CLI will end up in.
  own="$(file_identity -L "$directory" 2>/dev/null)" || own=""
  default="$(file_identity -L "${HOME:-/nonexistent}/.claude" 2>/dev/null)" || default=""
  if [ -n "$own" ] && [ "$own" = "$default" ]; then return 0; fi
  printf 'CLAUDE_CONFIG_DIR=%s\n' "$directory"
}

native_acp() {
  local task="$1" account="$2" directory
  begin_startup "$task" mac
  require_checked_lavoro "$task"
  directory="$(conversation_account_directory "$task" "$account")" || exit 1
  hold_conversation_lock "$task"
  ensure_native_acp
  confirm_startup "$task"
  run_native_acp "$task" "$directory"
}

# Runs the adapter behind the proxy, as run_acp_container does for a box: the client's input and
# output on the proxy's side, standard error through a named pipe into the bounded log, a signal
# to box passed on. The adapter runs under native_run.pl in a process group of its own; when the
# proxy has ended, whatever is left of that group is killed.
run_native_acp() {
  local task="$1" directory="$2" log report pipe pid_file limiter child code=0 node ended="$ACP_LOG_DIR/.$1.ended" life="$ACP_LOG_DIR/.$1.life"
  local config_assignment
  node="$(native_acp_node)" || exit 1
  config_assignment="$(config_directory_assignment "$directory")"
  mkdir -p "$ACP_LOG_DIR"
  log="$ACP_LOG_DIR/$task.log"
  report="$ACP_LOG_DIR/.$task.report"
  pipe="$ACP_LOG_DIR/.$task.stderr"
  pid_file="$(conversation_pid_file "$task")"
  rm -f "$report" "$pipe" "$pid_file" "$ended" "$life"
  mkfifo -m 600 "$pipe" "$life" || die "could not create the pipes of task '$task'"
  # The lifetime pipe: this process holds its writing end and nobody else does, so its end of
  # file tells the supervisor that this process is gone, however it went.
  exec 4<>"$life"
  start_log_reader "$pipe" "$log" "$ended"
  limiter="$LOG_READER_PID"
  (cd "$LAVORO_DIR" && clean_environment "$ADAPTER_ENVIRONMENT" "PATH=$PERSON_PATH" "PWD=$LAVORO_DIR" ${config_assignment:+"$config_assignment"} "BOX_ACP_LIFETIME_FD=4" -- "$TRUSTED_PERL" "$ACP_PROXY" "$ACP_MAX_LINE_BYTES" \
    "$ACP_END_SECONDS" "$ACP_STALL_SECONDS" "$report" "$TRUSTED_PERL" "$NATIVE_RUN" --keep-stdin 0 "$pid_file" "$node" "$(native_acp_command)") \
    <&3 3<&- 4<"$life" 5>&- 2>"$pipe" &
  child=$!
  exec 3<&-
  # From here a signal to box is passed on: also while the start is still being waited for.
  trap 'kill -TERM "$child" 2>/dev/null' TERM INT HUP
  wait_until_started "$child" test -e "$pid_file" || abandon_start "$task" "$child" TERM
  end_startup
  wait "$child" || code=$?
  while kill -0 "$child" 2>/dev/null; do code=0; wait "$child" || code=$?; done
  end_conversation_group "$pid_file"
  : >"$ended"
  wait "$limiter" 2>/dev/null || true
  exec 4>&-
  rm -f "$pipe" "$ended" "$life"
  say_why_acp_ended "$task" "$code" "$log"
  append_acp_report "$report" "$log"
  return "$code"
}

# Kills what is left of the process group of a conversation that has ended.
end_conversation_group() {
  local group
  if group="$(recorded_group "$1")"; then kill -KILL -- "-$group" 2>/dev/null || true; fi
  rm -f "$1"
}

# Ends the conversation of a task when one is open, and waits a moment for it: for box rm.
end_conversation() {
  local task="$1" group tick
  conversation_is_open "$task" || return 0
  group="$(recorded_group "$(conversation_pid_file "$task")")" || return 0
  kill -TERM -- "-$group" 2>/dev/null || true
  for ((tick = 0; tick < CONVERSATION_END_TICKS; tick++)); do
    conversation_is_open "$task" || return 0
    sleep 0.1
  done
  kill_if_still_recorded "$(conversation_pid_file "$task")" "$group"
}
readonly CONVERSATION_END_TICKS=50

# box stop <task> for a lavoro on the Mac: the open conversation is told to end.
stop_on_mac() {
  local task="$1" group
  conversation_is_open "$task" || die "no conversation is open for task '$task'"
  group="$(recorded_group "$(conversation_pid_file "$task")")" || die "the conversation of task '$task' is just starting or ending; try again in a moment"
  kill -TERM -- "-$group" 2>/dev/null || true
}

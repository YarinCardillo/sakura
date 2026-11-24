# shellcheck shell=bash
# Which runner runs the checks: the bash runner of this service (lib/runner.sh, the default) or
# the Restate engine, a whole second service kept under runners/engine (README: "Two runners").
# The choice is one key of the host-private config ($AGENTBOX_HOME/config): RUNNER=engine.
# Without the key every command is this service's own, unchanged. With it every command is handed
# to runners/engine/bin/box, with the PATH the person started box with, except `box config
# get|set RUNNER`, answered here in both cases, and `box status --json`, which says the choice
# (checkRunner) next to what the engine itself says.

readonly RUNNER_KEY=RUNNER
readonly BASH_RUNNER=bash
readonly ENGINE_RUNNER=engine
readonly ENGINE_ROOT="$BOX_ROOT/runners/engine"
readonly ENGINE_BOX="$ENGINE_ROOT/bin/box"

# Called first by main: answers `box config get|set RUNNER`, hands every other command to the
# engine when it is chosen, and returns for this service to run the command itself.
route_to_chosen_runner() {
  local runner
  if [ "${1:-}" = config ] && [ "${3:-}" = "$RUNNER_KEY" ]; then
    ensure_config
    cmd_runner_choice "${@:2}"
    exit 0
  fi
  runner="$(chosen_runner)" || exit 1
  [ "$runner" = "$ENGINE_RUNNER" ] || return 0
  if [ "${1:-}" = status ] && [ "${2:-}" = --json ]; then
    engine_status_json "${@:3}"
    exit 0
  fi
  PATH="$PERSON_PATH" exec "$ENGINE_BOX" "$@"
}

# The value of RUNNER in the config as it is written, nothing when the key is absent. The first
# line of the key counts.
configured_runner() {
  local line
  [ -f "$CONFIG_FILE" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in "$RUNNER_KEY="*) echo "${line#*=}"; return 0 ;; esac
  done <"$CONFIG_FILE"
}

# Prints bash or engine. A value box does not know stops every command but `box config set
# RUNNER`, which repairs it: running the checks with a runner nobody chose is worse.
chosen_runner() {
  local value
  value="$(configured_runner)"
  case "$value" in
    "" | "$BASH_RUNNER") echo "$BASH_RUNNER" ;;
    "$ENGINE_RUNNER") echo "$ENGINE_RUNNER" ;;
    *) die "the config ($CONFIG_FILE) names an unknown runner '$value': box config set RUNNER bash|engine" ;;
  esac
}

# box config get RUNNER | box config set RUNNER bash|engine
cmd_runner_choice() {
  case "${1:-}" in
    get)
      [ "$#" -eq 2 ] || die "usage: box config get RUNNER"
      chosen_runner
      ;;
    set)
      [ "$#" -eq 3 ] || die "usage: box config set RUNNER bash|engine"
      choose_runner "$3"
      ;;
    *) die "usage: box config get RUNNER | box config set RUNNER bash|engine" ;;
  esac
}

# The runner changes only while the one it replaces holds no check, so that no request is left
# behind in a queue that nothing reads any more.
choose_runner() {
  local wanted="$1" current
  case "$wanted" in
    "$BASH_RUNNER" | "$ENGINE_RUNNER") ;;
    *) die "unknown runner '$wanted' (bash or engine)" ;;
  esac
  current="$(configured_runner)"
  [ -n "$current" ] || current="$BASH_RUNNER"
  if [ "$current" = "$wanted" ]; then
    echo "checks already run with the $wanted runner"
    return 0
  fi
  case "$current" in
    "$BASH_RUNNER") leave_bash_runner ;;
    "$ENGINE_RUNNER") leave_engine ;;
  esac
  write_runner_choice "$wanted"
  echo "checks now run with the $wanted runner"
}

leave_bash_runner() {
  ! runner_is_running || die "the bash runner is running (pid $(runner_pid)): let its checks end, stop it (box runner --stop), then choose the engine"
  [ -z "$(list_requests)" ] || die "checks wait or run in the queue of the bash runner: let them end or cancel them (box cancel <task> <verb>), then choose the engine"
  ! bash_runner_starts_at_login || die "the bash runner starts at login: box runner --uninstall, then choose the engine"
}

# Whether the LaunchAgent of the bash runner starts this service's box.
bash_runner_starts_at_login() {
  local plist
  plist="$(launchd_plist_path)"
  [ -f "$plist" ] && grep -qF "<string>$BOX_ROOT/bin/box</string>" "$plist"
}

# The engine is asked only when it runs: one that is stopped holds no check that moves. One that
# runs is stopped here, so that nothing of it stays up once the bash runner is chosen.
leave_engine() {
  local said status
  said="$(engine_box engine status)" || die "the engine could not say whether it runs (box engine status)"
  case "$said" in *"launchd: installed"*) die "the engine starts at login: box engine uninstall, then choose the bash runner" ;; esac
  case "$said" in *": running"*) ;; *) return 0 ;; esac
  status="$(engine_box status --json)" || die "the engine could not say what it runs (box status --json)"
  jq -e '.runner.current == null and (.runner.queue | length) == 0' <<<"$status" >/dev/null \
    || die "a check runs or waits in the engine: let it end or cancel it (box cancel <task> <verb>), then choose the bash runner"
  engine_box engine stop >/dev/null || die "the engine did not stop (box engine status)"
}

engine_box() {
  PATH="$PERSON_PATH" "$ENGINE_BOX" "$@"
}

# Rewrites the config without the key, then adds RUNNER=engine when the engine is chosen: the
# bash runner leaves the file as it was before there was a choice.
write_runner_choice() {
  local tmp
  mkdir -p "$AGENTBOX_HOME"
  tmp="$(mktemp "$AGENTBOX_HOME/.config.XXXXXX")" || die "could not write the config"
  { grep -v "^$RUNNER_KEY=" "$CONFIG_FILE" 2>/dev/null || true; } >"$tmp"
  [ "$1" = "$BASH_RUNNER" ] || echo "$RUNNER_KEY=$1" >>"$tmp"
  mv "$tmp" "$CONFIG_FILE" || { rm -f "$tmp"; die "could not write the config"; }
}

# box status --json of the engine, with the choice added. Its exit code is the engine's.
engine_status_json() {
  local status code=0
  status="$(engine_box status --json "$@")" || code=$?
  [ "$code" -eq 0 ] || exit "$code"
  require_jq
  jq -c --argjson choice "$(check_runner_json "$ENGINE_RUNNER")" '. + {checkRunner: $choice}' <<<"$status"
}

# {runner, engine: {restateServer, node, setup}}: the runner chosen, and whether the two programs
# the engine needs are there, with the one line that says what to install (null when nothing is
# missing). engine is null, with a warning, when they could not be examined.
check_runner_json() {
  local engine
  engine="$(engine_prerequisites_json)" || { warn "what the engine needs could not be examined"; engine=null; }
  jq -nc --arg runner "$1" --argjson engine "$engine" '{runner: $runner, engine: $engine}'
}

# Asks the engine's own code (lib/engine_demand.sh) which programs it finds, in a shell of its
# own: the two services share names that cannot be defined twice in one shell. Starts nothing.
engine_prerequisites_json() {
  "$TRUSTED_ENV" BOX_ROOT="$ENGINE_ROOT" AGENTBOX_HOME="$AGENTBOX_HOME" "$TRUSTED_BASH" -c '
    . "$BOX_ROOT/lib/common.sh" && . "$BOX_ROOT/lib/project.sh" && . "$BOX_ROOT/lib/engine_demand.sh" || exit 1
    restate=false node=false
    if command -v "$(restate_program)" >/dev/null 2>&1; then restate=true; fi
    major="$("$TRUSTED_ENV" -u NODE_OPTIONS -u NODE_PATH "$(engine_node)" --version 2>/dev/null | sed -n "s/^v\([0-9][0-9]*\)\..*/\1/p")"
    if [[ "$major" =~ ^[0-9]+$ ]] && [ "$major" -ge "$ENGINE_NODE_MAJOR" ]; then node=true; fi
    jq -nc --argjson restate "$restate" --argjson node "$node" --arg setup "$(engine_setup_problem)" \
      "{restateServer: \$restate, node: \$node, setup: (if \$setup == \"\" then null else \$setup end)}"
  '
}

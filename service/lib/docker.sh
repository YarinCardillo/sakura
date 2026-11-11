# shellcheck shell=bash
# Docker access: timeouts, volumes and the confinement flags of every container.
# Only objects whose name starts with "agentbox" are ever created or touched.

readonly DOCKER_TIMEOUT="${BOX_DOCKER_TIMEOUT:-60}"
readonly VOLUME_INIT_TIMEOUT=120
readonly HOME_TMPFS_SIZE=128m
readonly AGENT_UID=1000
readonly NO_PROXY_HOSTS="localhost,127.0.0.1"
CONTAINER_RESOURCES=""

# docker with a timeout, for commands that return quickly.
dk() {
  run_with_timeout "$DOCKER_TIMEOUT" docker "$@"
}

require_docker() {
  dk info >/dev/null 2>&1 || die "Docker is not reachable. Start Docker Desktop and retry."
  prepare_container_resources || die "could not stage the container resources in the data directory"
}

# The app may carry box in a read-only bundle outside the VM's shared paths. Only the client
# reads that bundle; bind-mount sources are immutable copies under the app-selected data home.
prepare_container_resources() {
  local key
  [ -z "${CONTAINER_RESOURCES:-}" ] || return 0
  key="$(container_resource_archive | sha256_of)" || return 1
  [[ "$key" =~ ^[0-9a-f]{64}$ ]] || return 1
  CONTAINER_RESOURCES="$AGENTBOX_HOME/runtime/$key"
  with_lock "$LOCK_DIR/resources.lock" stage_container_resources "$CONTAINER_RESOURCES" || return 1
  RUNNER_SCRIPTS="$CONTAINER_RESOURCES/image/runner"
}

container_resource_archive() {
  tar --exclude __pycache__ --exclude '*.pyc' -cf - -C "$BOX_ROOT" image/runner rules proxy compose.yaml
}

stage_container_resources() {
  local destination="$1" temporary
  if [ -d "$destination" ]; then chmod -R a+rX,u+w "$destination"; return $?; fi
  umask 077
  mkdir -p "$AGENTBOX_HOME/runtime" || return 1
  temporary="$(mktemp -d "$AGENTBOX_HOME/runtime/.new.XXXXXX")" || return 1
  if container_resource_archive | tar -xf - -C "$temporary" && chmod -R a+rX,u+w "$temporary" && mv "$temporary" "$destination"; then return 0; fi
  rm -rf "$temporary"
  return 1
}

volume_exists() {
  dk volume inspect "$1" >/dev/null 2>&1
}

# The limits of a helper container: one that box starts for a small step of its own (the first
# mount of a volume, the checkout of a commit, a copy of the dependencies, a look at which
# files a volume holds). None of them needs the network or a capability, and what they work on
# can come from an agent (a commit whose tree is very large): each stays inside its own memory,
# with no extra swap, CPU and process limits, and is removed when it ends. The caller adds the
# deadline (run_with_timeout), the user and the mounts.
readonly HELPER_FLAGS=(
  --rm --network none --cap-drop ALL --security-opt no-new-privileges
  --log-driver none --read-only --tmpfs /tmp:size=64m,nr_inodes=65536,mode=1777
  --memory 1g --memory-swap 1g --cpus 1 --pids-limit 256
  --label agentbox=1
)

# Creates a named volume owned by the agent user. The first mount of an empty volume on
# /deps copies the ownership of the image directory, so later mounts at any path are writable.
# usage: ensure_volume <name> [task whose label it carries, so that box rm finds it]
ensure_volume() {
  local name="$1" task="${2:-}"
  [[ "$name" == $RESOURCE_PREFIX-* ]] || die "refusing to touch volume '$name' (not an $RESOURCE_PREFIX object)"
  volume_exists "$name" && return 0
  dk volume create --label agentbox=1 ${task:+--label "agentbox.task=$task"} "$name" >/dev/null
  run_with_timeout "$VOLUME_INIT_TIMEOUT" docker run "${HELPER_FLAGS[@]}" ${task:+--label "agentbox.task=$task"} -v "$name:/deps" "$IMAGE" true
}

# Creates the network of a task when missing and attaches the proxy to it (the proxy is the one
# thing every network has in common). Does not recreate or restart the proxy.
ensure_task_network() {
  local network
  network="$(task_network "$1")"
  if ! dk network inspect "$network" >/dev/null 2>&1; then
    dk network create --internal --label agentbox=1 "$network" >/dev/null || die "could not create the network $network"
  fi
  if ! dk network inspect "$network" -f '{{range .Containers}}{{.Name}} {{end}}' | grep -qw "$PROXY_CONTAINER"; then
    dk network connect "$network" "$PROXY_CONTAINER" >/dev/null \
      || die "could not attach the proxy to $network (is the proxy running? box up)"
  fi
}

# Detaches the proxy from the network of a task and removes the network.
remove_task_network() {
  local network
  network="$(task_network "$1")"
  dk network disconnect -f "$network" "$PROXY_CONTAINER" >/dev/null 2>&1 || true
  dk network rm "$network" >/dev/null 2>&1 || true
}

# Fills BASE_FLAGS: limits, dropped capabilities, the internal network and the proxy
# environment. Arguments: cpus, memory, task (its own network; without a task the shared
# network that only the proxy and task-less helpers use).
build_base_flags() {
  local cpus="$1" memory="$2" network="$NETWORK"
  if [ -n "${3:-}" ]; then
    require_box_task "$3"
    ensure_task_network "$3"
    network="$(task_network "$3")"
  fi
  BASE_FLAGS=(
    --init
    --log-driver none
    --tmpfs "/tmp:size=$TMPFS_SIZE,nr_inodes=65536,mode=1777"
    --tmpfs "/home/agent:size=$HOME_TMPFS_SIZE,nr_inodes=65536,uid=$AGENT_UID,gid=$AGENT_UID"
    --cpus "$cpus" --memory "$memory" --memory-swap "$memory"
    --pids-limit "$PIDS_LIMIT" --shm-size "$SHM_SIZE"
    --cap-drop ALL --security-opt no-new-privileges
    --network "$network" --user agent
    --label agentbox=1
    -e "HTTP_PROXY=$PROXY_URL" -e "HTTPS_PROXY=$PROXY_URL"
    -e "http_proxy=$PROXY_URL" -e "https_proxy=$PROXY_URL"
    -e "NO_PROXY=$NO_PROXY_HOSTS" -e "no_proxy=$NO_PROXY_HOSTS"
  )
}

# Fills AGENT_FLAGS: base flags with the agent limits, a read-only root filesystem and
# ephemeral tmpfs for /tmp and the home directory. Argument: task (optional, see build_base_flags).
build_agent_flags() {
  if [ -n "${1:-}" ]; then require_task_disk_budget "$1" || return 1; fi
  build_base_flags "${BOX_CPUS:-$DEFAULT_AGENT_CPUS}" "${BOX_MEMORY:-$DEFAULT_AGENT_MEMORY}" "${1:-}"
  AGENT_FLAGS=(
    "${BASE_FLAGS[@]}"
    --read-only
  )
}

# Appends to AGENT_FLAGS the mounts and identity of a task, the workspace at /work/<task>
# (Claude Code keys its history by directory). Needs the project loaded. Arguments: task, container name.
add_workspace_flags() {
  local task="$1" name="$2"
  require_box_task "$task"
  AGENT_FLAGS+=(
    --name "$name" --label agentbox.kind=ws --label "agentbox.task=$task"
    -e "BOX_TASK=$task" -e "BOX_WORKSPACE=/work/$task" -w "/work/$task"
    -v "$(task_dir "$task"):/work/$task"
    -v "$(drop_dir "$task"):/box/requests"
    -v "$(task_results_dir "$task"):/box/results:ro"
    -v "$(npm_cache_volume "$task"):/npm-cache"
    -v "${CONTAINER_RESOURCES:-$BOX_ROOT}/rules/agent.md:/etc/claude-code/CLAUDE.md:ro"
  )
  if [ -n "$INSTALL" ]; then AGENT_FLAGS+=(-v "$(deps_volume "$task"):/work/$task$(subdir_suffix)/node_modules"); fi
}

# The volumes a box of a task mounts: the npm cache, and the dependencies when the project
# installs any. Needs the project loaded.
ensure_task_volumes() {
  if [ -n "$INSTALL" ]; then ensure_volume "$(deps_volume "$1")" "$1"; fi
  ensure_volume "$(npm_cache_volume "$1")" "$1"
}

# Appends to AGENT_FLAGS the whole volume of an account, for the login and the status check, which
# belong to no task and run a fixed CLI command.
add_account_flags() {
  AGENT_FLAGS+=(
    -v "$(account_volume "$1"):/acct"
    -e CLAUDE_CONFIG_DIR=/acct/claude -e CODEX_HOME=/acct/codex
  )
}

# Creates the configuration volume of a task with the directories the CLIs write into, owned by the
# agent user (Docker would create the mount points of the credential files as root otherwise).
ensure_config_volume() {
  local task="$1" volume
  volume="$(config_volume "$1" "$2")"
  volume_exists "$volume" && return 0
  dk volume create --label agentbox=1 --label "agentbox.task=$task" "$volume" >/dev/null
  run_with_timeout "$VOLUME_INIT_TIMEOUT" docker run "${HELPER_FLAGS[@]}" --label "agentbox.task=$task" --user agent -v "$volume:/acct" "$IMAGE" mkdir -p /acct/claude /acct/codex
}

# Prints which credential files an account volume holds ("claude", "codex"), one per line. Only
# the existence of the files is tested, never their content.
account_credential_files() {
  run_with_timeout "$DOCKER_TIMEOUT" docker run "${HELPER_FLAGS[@]}" --user agent --read-only --label agentbox.kind=acctfiles \
    -v "$(account_volume "$1"):/acct:ro" "$IMAGE" \
    bash -c 'test -f /acct/claude/.credentials.json && echo claude; test -f /acct/codex/auth.json && echo codex; true'
}

# Appends to AGENT_FLAGS what a task's agent gets of an account: its own configuration volume at
# /acct and, as single files mounted over it, only the credentials the account has (the CLI
# refreshes them in place when a rename onto a mounted file fails, so every box of the account
# keeps the same tokens). Arguments: task, account, the output of account_credential_files.
add_task_account_flags() {
  local task="$1" account="$2" files="$3" volume
  volume="$(account_volume "$account")"
  AGENT_FLAGS+=(-v "$(config_volume "$task" "$account"):/acct" -e CLAUDE_CONFIG_DIR=/acct/claude -e CODEX_HOME=/acct/codex)
  if grep -qx claude <<<"$files"; then
    AGENT_FLAGS+=(--mount "type=volume,src=$volume,dst=/acct/claude/.credentials.json,volume-subpath=claude/.credentials.json")
  fi
  if grep -qx codex <<<"$files"; then
    AGENT_FLAGS+=(--mount "type=volume,src=$volume,dst=/acct/codex/auth.json,volume-subpath=codex/auth.json")
  fi
}

# "-it" on a terminal, "-i" otherwise.
interactive_flags() {
  if [ -t 0 ] && [ -t 1 ]; then echo "-it"; else echo "-i"; fi
}

# Storage is detected at operation boundaries, never claimed as a filesystem quota. The
# workspace and host-side channels, plus every task-owned volume, are examined read-only in
# a confined helper. Account volumes are not mounted and no file contents are opened.
readonly DEFAULT_DISK_BUDGET_MB=20480
readonly DISK_ENTRY_LIMIT=1000000

task_disk_json() (
  require_box_task "$1"
  bind_box_task "$1" || { warn "task '$1' is busy; storage was not read"; exit 1; }
  task_disk_json_locked "$1"
)

# Called under the task lifecycle lock; inspect its place again before preparing any mount.
task_disk_json_locked() {
  local task="$1" path volume volumes hostile=() trusted=() hostile_usage trusted_usage
  require_box_task "$task"
  for path in "$(task_dir "$task")" "$(drop_dir "$task")" \
    "$QUARANTINE_DIR/drop-$task."* "$QUARANTINE_DIR/request-"*".$task."*; do
    [ ! -d "$path" ] || hostile+=("$path")
  done
  for path in "$(task_results_dir "$task")" "$(task_verified_dir "$task")"; do
    [ ! -d "$path" ] || trusted+=("$path")
  done
  volumes="$(dk volume ls --format '{{.Name}}')" || return 1
  while IFS= read -r volume; do
    case "$volume" in
      "$RESOURCE_PREFIX-nm-$task" | "$NPM_CACHE_VOLUME-$task" | "$RESOURCE_PREFIX-cfg-$task."* | "$PRISTINE_DEPS_PREFIX.$task."* | "$CHECKOUT_VOLUME_PREFIX-"*".$task" | "$JOB_DEPS_VOLUME_PREFIX-"*".$task") ;;
      *) continue ;;
    esac
    [[ "$volume" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]*$ ]] || return 1
    trusted+=("$volume")
  done <<<"$volumes"
  hostile_usage="$(measure_disk_roots "$task" ${hostile[@]+"${hostile[@]}"})" || return 1
  trusted_usage="$(measure_disk_roots "$task" ${trusted[@]+"${trusted[@]}"})" || return 1
  jq -cne --argjson hostile "$hostile_usage" --argjson trusted "$trusted_usage" \
    --argjson limit "$(( ${DISK_BUDGET_MB:-$DEFAULT_DISK_BUDGET_MB} * 1048576 ))" --argjson entries "$DISK_ENTRY_LIMIT" '
    {bytes: ($hostile.bytes + $trusted.bytes), entries: ($hostile.entries + $trusted.entries)} |
    . + {limitBytes: $limit, entryLimit: $entries,
     overBudget: (.bytes > $limit or .entries > $entries), enforcement: "detection"}'
}

# Trusted stores and hostile workspace roots are measured in separate containers.
measure_disk_roots() {
  local task="$1" path index=0 mounts=() paths=() usage
  shift
  for path in "$@"; do
    mounts+=(-v "$path:/usage/$index:ro")
    paths+=("/usage/$index")
    index=$((index + 1))
  done
  [ "$index" -gt 0 ] || { printf '%s\n' '{"bytes":0,"entries":0}'; return 0; }
  build_sandbox_flags
  usage="$(run_with_timeout "$DOCKER_TIMEOUT" docker run "${SANDBOX_FLAGS[@]}" --label "agentbox.task=$task" \
    "${mounts[@]}" "$IMAGE" python3 -B -I /box/storage.py "$DISK_ENTRY_LIMIT" "${paths[@]}" 2>/dev/null)" || return 1
  jq -ce 'select(.bytes | type == "number" and . >= 0 and . == floor) |
    select(.entries | type == "number" and . >= 0 and . == floor) | {bytes, entries}' <<<"$usage"
}

require_task_disk_budget() {
  local usage
  require_box_task "$1"
  usage="$(task_disk_json "$1")" || { warn "task '$1': disk budget could not be measured; operation refused"; return 1; }
  if [ "$(jq -r '.overBudget' <<<"$usage")" = true ]; then
    warn "task '$1': disk budget exceeded ($(jq -r '.bytes' <<<"$usage") bytes, $(jq -r '.entries' <<<"$usage") entries); remove data or raise DISK_BUDGET_MB"
    return 1
  fi
}

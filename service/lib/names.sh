# shellcheck shell=bash
# Every Docker name box derives from a task, an account, a request key or a lockfile hash, in one
# place, so that the names can be proven distinct (tests/test_names_unique.sh walks them).
#
# The inputs: a task or an account name holds lowercase letters, digits and single hyphens; the
# key of a set of dependencies is a whole SHA-256, 64 hex digits; a request key is "<time>-<sha>-<verb>.<task>" (one dot; the
# whole sha of the commit, 40 hex digits: a shortened one can be made to collide).
# Why two different inputs never share a name, per Docker namespace:
# - Each kind has its own prefix, ending in its separator ("-" or "."). No prefix is the start of
#   another, so names of two kinds differ, whatever the task or account is called.
# - A kind with one variable part appends it unchanged.
# - The pristine dependencies of the runner use a dot, which no task name holds: "nm-<task>" can
#   never be "pristine.<task>.<key>". They belong to one task and one set of build inputs.
# - A name with two variable parts joins them with a dot, which neither part holds: the
#   configuration volume "cfg-<task>.<account>" and everything named after a request key.
# - A workspace container is "<task>-<suffix>" where the suffix (install, or digits) holds no
#   hyphen: the last hyphen splits the name in exactly one way.

readonly AGENT_CONTAINER_PREFIX="$RESOURCE_PREFIX-agent"
readonly ACP_CONTAINER_PREFIX="$RESOURCE_PREFIX-acp"
readonly WORKSPACE_CONTAINER_PREFIX="$RESOURCE_PREFIX-ws"
readonly JOB_CONTAINER_PREFIX="$RESOURCE_PREFIX-job"
readonly INSTALL_CONTAINER_PREFIX="$RESOURCE_PREFIX-install"
readonly ACCOUNT_CHECK_PREFIX="$RESOURCE_PREFIX-acctcheck"
readonly CHECKOUT_VOLUME_PREFIX="$RESOURCE_PREFIX-co"
readonly JOB_DEPS_VOLUME_PREFIX="$RESOURCE_PREFIX-nmjob"
readonly PRISTINE_DEPS_PREFIX="$RESOURCE_PREFIX-pristine"

# ---- volumes ----------------------------------------------------------------------------

# The dependencies a task's agent installs and may write.
deps_volume() {
  echo "$RESOURCE_PREFIX-nm-$1"
}

# The dependencies the runner builds for the jobs of one task from one set of inputs (64 hex
# digits, see dependency_key); jobs mount them read-only. Arguments: task, key.
pristine_deps_volume() {
  echo "$PRISTINE_DEPS_PREFIX.$1.$2"
}

# The npm cache volume of one task; nothing a box writes there is read by another box.
npm_cache_volume() {
  echo "$NPM_CACHE_VOLUME-$1"
}

account_volume() {
  echo "$RESOURCE_PREFIX-acct-$1"
}

# The configuration volume of one task for one account, mounted at /acct. It holds the CLI's
# settings, history and state; no other task sees it. The dot keeps (a-b, c) and (a, b-c) apart.
config_volume() {
  echo "$RESOURCE_PREFIX-cfg-$1.$2"
}

# The throwaway checkout of one request.
checkout_volume() {
  echo "$CHECKOUT_VOLUME_PREFIX-$1"
}

# The private copy of the dependencies of one request (BOX_DEPS_MODE=copy).
job_deps_volume() {
  echo "$JOB_DEPS_VOLUME_PREFIX-$1"
}

# ---- networks ---------------------------------------------------------------------------

# The internal network of one task. Only the proxy is attached to every task network, so a box
# cannot reach a port another box opens, by address or by name.
task_network() {
  echo "$NETWORK-$1"
}

# ---- containers -------------------------------------------------------------------------

agent_container_name() {
  echo "$AGENT_CONTAINER_PREFIX-$1"
}

# The container of the conversation of a task (box acp).
acp_container_name() {
  echo "$ACP_CONTAINER_PREFIX-$1"
}

# A one-off container with the workspace of a task. Arguments: task, suffix ("install" or digits).
workspace_container_name() {
  echo "$WORKSPACE_CONTAINER_PREFIX-$1-$2"
}

job_container_name() {
  echo "$JOB_CONTAINER_PREFIX-$1"
}

# The container that builds the pristine dependencies. Arguments: task, key.
install_container_name() {
  echo "$INSTALL_CONTAINER_PREFIX-$1.$2"
}

account_check_container_name() {
  echo "$ACCOUNT_CHECK_PREFIX-$1"
}

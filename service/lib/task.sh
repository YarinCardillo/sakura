# shellcheck shell=bash
# Task lifecycle: new, take, rm.
#
# work/<task> is written by the agent and is untrusted. After `box new` has created it, the
# host never runs git inside it (see lib/sandbox.sh).

readonly INSTALL_TIMEOUT_SECONDS="${BOX_INSTALL_TIMEOUT:-1800}"
readonly KILL_GRACE_SECONDS=30

readonly NEW_USAGE="usage: box new <task> <project> [--base <branch>] [--where mac|box]"

# box new <task> <project> [--base <branch>] [--where mac|box]: a lavoro of the project, started
# from the project's base branch or from the branch that is named, in the place the project
# names or the one that is requested. On the Mac it is a worktree of the repository (native.sh);
# in a box a workspace cloned from it, on its own branch box/<task>.
cmd_new() {
  local task="${1:-}" project="${2:-}"
  require_task_name "$task"
  [ -n "$project" ] && [[ "$project" != --* ]] || die "$NEW_USAGE"
  load_project "$project"
  shift 2
  parse_new_options "$project" "$@"
  hold_task_lock "$task"
  [ ! -e "$(task_dir "$task")" ] || die "task '$task' already exists"
  [ -z "$(task_requests "$task")" ] || die "a request of an earlier task '$task' is still in the queue; the runner removes it, then try again"
  if [ "$NEW_WHERE" = "mac" ]; then new_on_mac "$task" "$project" "$NEW_BASE"; else new_in_box "$task" "$project" "$NEW_BASE"; fi
  journal_event new "$task"
}

new_in_box() {
  local task="$1" project="$2" base="$3" base_sha
  require_docker
  base_sha="$(clone_workspace "$task" "$base")" || die "could not create the workspace of task '$task'"
  record_task "$task" "$project" "$base_sha" "$base" box
  write_task_project_snapshot "$task" || die "could not keep the project values of task '$task'"
  mkdir -p "$(drop_dir "$task")" "$(task_results_dir "$task")"
  install_dependencies "$task" || die "dependency install failed; the workspace is kept, remove it with: box rm $task --force"
  echo "task '$task' ready: $(task_dir "$task") (branch box/$task)"
}

# Sets NEW_BASE and NEW_WHERE from the options of box new: the project's base and place, or the
# local branch of the repository and the place that are named. Anything else is an error.
# Needs the project loaded.
parse_new_options() {
  local project="$1" seen=" "
  shift
  NEW_BASE="$BASE" NEW_WHERE="$(project_where)"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --base | --where) ;;
      --*) die "unknown option '$1' ($NEW_USAGE)" ;;
      *) die "$NEW_USAGE" ;;
    esac
    [ "$#" -ge 2 ] && [[ "$seen" != *" $1 "* ]] || die "$NEW_USAGE"
    seen="$seen$1 "
    if [ "$1" = "--base" ]; then NEW_BASE="$2"; else NEW_WHERE="$2"; fi
    shift 2
  done
  is_place "$NEW_WHERE" || die "'$NEW_WHERE' is not a place: a lavoro is on the mac or in a box ($NEW_USAGE)"
  [ -z "$(project_value_problem BASE "$NEW_BASE")" ] && has_local_branch "$REPO" "$NEW_BASE" \
    || die "'$NEW_BASE' is not a branch of the repository of project '$project' (box branches $project --json lists them)"
}

# box take <task>: the task branch reaches the source repository through a bundle written by
# a sandbox container, never by fetching from the workspace. The commit is read from the
# workspace once; the bundle is made for exactly that commit, from the verified store.
cmd_take() (
  local task="${1:-}" bundle bundle_dir problem
  require_existing_task "$task"
  refuse_on_mac "$task" "box take" "the branch is already in your repository"
  bind_box_task "$task" || exit 1
  load_task_project "$task"
  require_docker
  require_task_disk_budget "$task" || exit 1
  workspace_facts "$task"
  [ "$FACT_AHEAD" -gt 0 ] || die "task '$task' has no commits to take"
  # Before anything reaches the repository: a tree that is one thing on Linux and another on the
  # person's filesystem is not taken.
  problem="$(tree_problem "$task" "$FACT_HEAD")"
  [ -z "$problem" ] || die "task '$task' was not taken: $problem. Nothing was changed in $REPO"
  mkdir -p "$BUNDLES_DIR"
  bundle_dir="$(mktemp -d "$BUNDLES_DIR/$task.XXXXXX")" || die "could not reserve the bundle directory for task '$task'"
  trap 'rm -rf "$bundle_dir"' EXIT
  create_bundle "$task" "$FACT_HEAD" "$bundle_dir" || die "could not bundle the branch box/$task of task '$task'"
  bundle="$bundle_dir/$task.bundle"
  person_git -C "$REPO" bundle verify "$bundle" >/dev/null || die "the bundle of task '$task' did not verify"
  person_git -C "$REPO" -c transfer.fsckObjects=true -c fetch.fsckObjects=true fetch --quiet "$bundle" "refs/heads/box/$task:refs/heads/box/$task"
  require_taken_commit "$task" "$FACT_HEAD"
  journal_event take "$task" "" "$FACT_HEAD"
  echo "branch box/$task taken into $REPO"
  echo "$FACT_HEAD"
)

# What take reports is what arrived: the branch in the source repository must be at the commit
# that was read from the workspace and bundled, before anything is recorded.
require_taken_commit() {
  local taken
  taken="$(person_git -C "$REPO" rev-parse --verify --quiet "refs/heads/box/$1^{commit}" 2>/dev/null)" || taken=""
  [ "$taken" = "$2" ] || die "the branch box/$1 in $REPO is at ${taken:-no commit}, not at the commit that was read ($2); nothing was recorded"
}

# box rm <task> [--force]: refuses while commits are not taken or files are dirty. Once that is
# settled the intake of the task is cut off, its remaining requests are cancelled and the task
# goes. A workspace deleted by hand leaves only Docker objects, which rm cleans up.
cmd_rm() {
  local task="${1:-}" force="${2:-}"
  require_task_name "$task"
  hold_task_lock "$task"
  if task_is_on_mac "$task"; then rm_on_mac "$task" "$force"; return 0; fi
  require_docker
  if [ -d "$(task_dir "$task")" ] && [ "$force" != "--force" ]; then
    load_task_project "$task"
    refuse_if_unsafe_to_remove "$task"
  fi
  with_queue_lock retire_task_locked "$task"
  drain_task_requests "$task"
  remove_task_containers "$task"
  remove_task_network "$task"
  remove_task_config_volumes "$task"
  dk volume rm "$(deps_volume "$task")" "$(npm_cache_volume "$task")" >/dev/null 2>&1 || true
  rm -f "${DEPS_DIR:?}/$task."*.ok
  rm -rf "${WORK_DIR:?}/$task" "${DROP_DIR:?}/$task" "${RESULTS_DIR:?}/$task" "$(task_retiring_mark "$task")" \
    "${TASKS_DIR:?}/$task.env" "${TASKS_DIR:?}/$task.project" "${BUNDLES_DIR:?}/$task.bundle" "${VERIFIED_DIR:?}/$task.git" "${VERIFIED_DIR:?}/$task.store"
  journal_event rm "$task"
  echo "task '$task' removed"
}

# The point after which nothing can be queued for the task: inside the queue lock, where requests
# are published, the task is marked as being removed. The intake of drops and box verify refuse
# it from then on, so a box that keeps asking for checks cannot keep its task alive.
retire_task_locked() {
  mkdir -p "$TASKS_DIR"
  : >"$(task_retiring_mark "$1")"
}

# Cancels the requests the task still has (a running one is stopped and waited for). The set
# cannot grow any more. When the runner has not ended one in time the removal stops here and
# another box rm finishes it; the task stays marked meanwhile.
drain_task_requests() {
  local task="$1" key file
  while read -r key file; do
    cancel_request "$key" "$file" || true
  done < <(task_requests "$task")
  [ -z "$(task_requests "$task")" ] || die "a check of task '$task' is still ending; run box rm $task --force again in a moment"
}

# Succeeds when the source repository already holds the workspace HEAD on box/<task>.
# Needs workspace_facts to have run.
is_taken() {
  [ "$FACT_AHEAD" -eq 0 ] && return 0
  person_git -C "$REPO" merge-base --is-ancestor "$FACT_HEAD" "refs/heads/box/$1" 2>/dev/null
}

refuse_if_unsafe_to_remove() {
  local task="$1"
  workspace_facts "$task"
  [ "$FACT_DIRTY" -eq 0 ] || die "task '$task' has $FACT_DIRTY uncommitted file(s); commit them or use --force"
  is_taken "$task" || die "task '$task' has commits that were not taken (box take $task); use --force to discard them"
}

# Removes the volumes that carry the label of the task: its configuration volumes and the
# dependencies the runner built for it.
remove_task_config_volumes() {
  local volumes
  volumes="$(dk volume ls -q --filter "label=agentbox.task=$1")"
  # shellcheck disable=SC2086
  [ -z "$volumes" ] || dk volume rm $volumes >/dev/null 2>&1 || true
}

remove_task_containers() {
  local ids
  ids="$(dk ps -aq --filter "label=agentbox.task=$1")"
  [ -z "$ids" ] || echo "$ids" | xargs docker rm -f >/dev/null
}

# clone_workspace <task> <base branch>: clones the source repository at that branch (copying
# objects, no hardlinks and no alternates, so that nothing in the workspace can reach the
# source) and prints the base commit.
#
# This is the one moment the host runs git inside a workspace, and it is safe: the directory
# was created a moment ago by this very command from the person's repository, git clone copies
# no hooks and writes a fresh config, and no container has mounted it yet, so nothing the
# agent controls exists in it. With SPARSE=1 only the SUBDIR cone and the top-level files are
# checked out (history stays complete), which makes `git status` far cheaper.
# It is called in a command substitution, where a failing command does not end the shell: every
# step returns by itself. A folder that already exists is never touched: it may be the workspace
# of a task that won a race, already written by its agent.
clone_workspace() {
  local task="$1" base="$2" dir
  dir="$(task_dir "$task")"
  mkdir -p "$WORK_DIR"
  [ ! -e "$dir" ] || return 1
  person_git clone --local --no-hardlinks --no-checkout --quiet --single-branch --branch "$base" "$REPO" "$dir" >&2 || return 1
  person_git -C "$dir" remote remove origin || return 1
  if [ "$(sparse_enabled)" = "1" ]; then
    person_git -C "$dir" sparse-checkout init --cone || return 1
    person_git -C "$dir" sparse-checkout set -- "$SUBDIR" || return 1
  fi
  person_git -C "$dir" checkout --quiet -b "box/$task" || return 1
  person_git -C "$dir" rev-parse HEAD
}

# ---- the lifecycle lock of a task ---------------------------------------------------------------
# One lock file per task, on descriptor 5. box new, box rm and every command that starts
# something for the task hold it exclusively: from before the task is looked at until the
# command ends, or until what was started is running. A read of a workspace in a container
# (workspace_sandbox) holds it shared for as long as its docker client lives. So nothing can be
# mounted or started for a task while the task is being removed or made, and a task cannot be
# removed or replaced under a container that is being created for it.
readonly TASK_LOCK_PATIENCE_SECONDS="${BOX_TASK_LOCK_PATIENCE:-10}"
readonly TASK_READ_PATIENCE_SECONDS=30
readonly TASK_INTENT_SECONDS=15

task_lock_file() {
  echo "$LOCK_DIR/task-$1.lock"
}

# A file that says "a command wants this task for itself": reads that have not begun wait for
# it to go, so that reads which follow one another cannot keep a removal out for ever. It only
# counts while it is fresh.
task_intent_file() {
  echo "$LOCK_DIR/task-$1.intent"
}

# try_task_lock <EX|SH> <seconds>: takes the lock on descriptor 5, trying for that long.
try_task_lock() {
  perl -MFcntl=:flock -e 'open(my $f, ">&=", 5) or die "fd 5: $!";
    my $mode = ($ARGV[0] eq "SH" ? LOCK_SH : LOCK_EX) | LOCK_NB;
    my $end = time + $ARGV[1];
    until (flock($f, $mode)) { exit 1 if time >= $end; select(undef, undef, undef, 0.2) }' "$1" "$2"
}

hold_task_lock() {
  mkdir -p "$LOCK_DIR"
  exec 5>>"$(task_lock_file "$1")"
  : >"$(task_intent_file "$1")"
  if ! try_task_lock EX "$TASK_LOCK_PATIENCE_SECONDS"; then
    rm -f "$(task_intent_file "$1")"
    die "another box new or box rm of task '$1' is at work, or something of it is being started or read; try again in a moment"
  fi
  rm -f "$(task_intent_file "$1")"
  TASK_LOCK_HELD="$1"
}

# For a read of the workspace: shared, in the subshell that runs the docker client, which keeps
# the descriptor. A command that already holds the lock of the task reads under it.
hold_task_lock_shared() {
  local intent tick
  [ "${TASK_LOCK_HELD:-}" != "$1" ] || return 0
  mkdir -p "$LOCK_DIR"
  exec 5>>"$(task_lock_file "$1")"
  intent="$(task_intent_file "$1")"
  for ((tick = 0; tick < TASK_INTENT_SECONDS * 5; tick++)); do
    [ -e "$intent" ] && [ $(( $(now_epoch) - $(file_modified "$intent" 2>/dev/null || echo 0) )) -lt "$TASK_INTENT_SECONDS" ] || break
    sleep 0.2
  done
  try_task_lock SH "$TASK_READ_PATIENCE_SECONDS" || return 1
  TASK_LOCK_HELD="$1"
}

# Bind a reader to the generation observed before waiting for the lifecycle lock.
bind_box_task() {
  local generation
  generation="$(task_generation "$1")"
  hold_task_lock_shared "$1" || return 1
  require_box_task "$1"
  [ -n "$generation" ] && [ "$(task_generation "$1")" = "$generation" ] \
    || { warn "task '$1' was removed or created again; operation refused"; return 1; }
}

# ---- starting something for a task --------------------------------------------------------------
# Between the moment a command looks at a task and the moment it starts something for it (a
# conversation, Play, a container), box rm and box new could replace the task by one of the same
# name in the other place: a native start would land in the workspace of a box, the mounts
# prepared for a box on a worktree of the Mac. So a start holds the lifecycle lock from before
# it looks until what it starts is running, and checks once more, just before the start, that
# the task is the one it looked at.

# begin_startup <task> <place>: takes the lock and notes the place and the generation of the task.
begin_startup() {
  hold_task_lock "$1"
  [ -d "$(task_dir "$1")" ] && [ -f "$TASKS_DIR/$1.env" ] || die "task '$1' does not exist (box new $1)"
  STARTUP_WHERE="$(task_where "$1")"
  STARTUP_GENERATION="$(task_generation "$1")"
  [ "$STARTUP_WHERE" = "$2" ] || die "task '$1' is not $([ "$2" = mac ] && echo "on the Mac" || echo "in a box") any more; nothing was started"
}

# The end with one line unless the task is still the one begin_startup looked at.
confirm_startup() {
  [ -d "$(task_dir "$1")" ] && [ "$(task_where "$1")" = "$STARTUP_WHERE" ] && [ "$(task_generation "$1")" = "$STARTUP_GENERATION" ] \
    || die "task '$1' was removed or created again while this command was starting; nothing was started"
}

# What was started is running: box rm and box new may have the task again.
end_startup() {
  exec 5>&-
  TASK_LOCK_HELD=""
}

readonly START_TIMEOUT_SECONDS="${BOX_START_TIMEOUT:-60}"
readonly ABANDON_TICKS=100

container_exists() {
  [ -n "$(dk ps -aq --filter "name=^$1\$" 2>/dev/null)" ]
}

# wait_until_started <pid of what was launched> <command that succeeds once it runs...>: waits
# until what was launched is there, or its process has ended (it then has its own exit code to
# show). Fails when neither happened within START_TIMEOUT_SECONDS.
wait_until_started() {
  local child="$1" tick
  shift
  for ((tick = 0; tick < START_TIMEOUT_SECONDS * 5; tick++)); do
    if "$@" || ! kill -0 "$child" 2>/dev/null; then return 0; fi
    sleep 0.2
  done
  return 1
}

# abandon_start <task> <pid> <signal> [<container>]: a launch that did not register in time is
# ended and collected before the lock is let go (the end of this command): asked with the
# signal, killed when it does not go, its container removed should it have appeared after all.
abandon_start() {
  local task="$1" pid="$2" signal="$3" name="${4:-}" tick
  kill "-$signal" "$pid" 2>/dev/null || true
  for ((tick = 0; tick < ABANDON_TICKS; tick++)); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  [ -z "$name" ] || dk rm -f "$name" >/dev/null 2>&1 || true
  die "what was being started for task '$task' did not start within $START_TIMEOUT_SECONDS s; it was ended and nothing of it is left"
}

# run_confirmed <task> <container> <docker client command...>: runs a docker client that
# creates the container, with this terminal, and keeps the lifecycle lock until the container
# exists (or the client has ended). Returns the exit code of the client.
run_confirmed() {
  local task="$1" name="$2" child code=0
  shift 2
  "$@" <&0 5>&- &
  child=$!
  trap 'kill -TERM "$child" 2>/dev/null' TERM HUP
  trap : INT
  wait_until_started "$child" container_exists "$name" || abandon_start "$task" "$child" KILL "$name"
  end_startup
  wait "$child" || code=$?
  while kill -0 "$child" 2>/dev/null; do code=0; wait "$child" || code=$?; done
  return "$code"
}

# 1 when the profile asks for a sparse checkout: SPARSE=0 turns it off, it defaults to 1
# for a profile that names a SUBDIR.
sparse_enabled() {
  if [ -n "${SUBDIR:-}" ] && [ "${SPARSE:-1}" != "0" ]; then echo 1; else echo 0; fi
}

# The first install of a task, in its box. The project's command reaches the container as a
# value and runs whole under timeout (nothing in it can step outside), and the docker client
# that waits for it has a deadline on the host.
readonly INSTALL_PROGRAM='cd "$1" && exec timeout --kill-after="$2" "$3" bash -c "$BOX_INSTALL"'

install_dependencies() {
  local task="$1" name
  require_box_task "$task"
  [ -n "$INSTALL" ] || return 0
  name="$(workspace_container_name "$task" install)"
  ensure_task_volumes "$task"
  build_agent_flags "$task"
  add_workspace_flags "$task" "$name"
  run_with_deadline "$((INSTALL_TIMEOUT_SECONDS + DEADLINE_GRACE_SECONDS))" "$name" \
    docker run --rm "${AGENT_FLAGS[@]}" -e "BOX_INSTALL=$INSTALL" "$IMAGE" \
    bash -c "$INSTALL_PROGRAM" install "/work/$task$(subdir_suffix)" "$KILL_GRACE_SECONDS" "$INSTALL_TIMEOUT_SECONDS"
}

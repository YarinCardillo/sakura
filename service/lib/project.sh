# shellcheck shell=bash
# Projects: WHAT is worked on. A project names a repository on this Mac, an optional subfolder,
# the default base branch, the install command and the commands of the checks (verbs), the
# protected paths and the test file pattern. A request can only name a verb; the command comes
# from here.
#
# A project is a file of the data directory, $AGENTBOX_HOME/projects/<name>.env, written by box
# from the person's choices (box project add). It holds KEY=value lines and is read as data:
# nothing in it is evaluated, a key that is not known or a value that is not safe makes box
# refuse the project. Commands are text that only ever runs inside a container.

readonly PROJECTS_DIR="$AGENTBOX_HOME/projects"
# The profiles shipped with the code. They are not read as projects: on upgrade each becomes a
# project of the data directory, once (see import_shipped_projects).
readonly SHIPPED_PROFILES_DIR="${BOX_PROFILES_DIR:-$BOX_ROOT/profiles}"
readonly IMPORTED_PROFILES_FILE="$PROJECTS_DIR/.imported"
# Shipped profiles that become a project only when something names them (the live self-tests).
readonly ON_DEMAND_PROFILES="selftest"
readonly SHIPPED_HOME_TOKEN="@AGENTBOX_HOME@"
readonly BOX_MANAGED_DIRECTORIES="work tasks projects locks queue results bundles stage deps quarantine drop verified cache acp agents setup tools play trash shells runtime"

readonly VERB_PATTERN='^[a-z][a-z0-9-]*$'
readonly PROJECT_PATTERN='^[a-z0-9]+(-[a-z0-9]+)*$'
readonly PROJECT_KEYS="REPO BASE SUBDIR INSTALL REBUILD SPARSE PROTECTED TEST_FILES SAFE_OUTSIDE WHERE BRANCH_PREFIX SETUP ARCHIVE ACCOUNT RUN PORTS RUN_MODE DISK_BUDGET_MB"
# The two places of a lavoro: the Mac (a worktree of the person's repository, the agent runs as
# the person) or a box (a clone in a container). A project that does not say gets the default
# (BOX_DEFAULT_WHERE changes it; the tests of the box place do).
readonly PLACES="mac box"
readonly DEFAULT_WHERE="${BOX_DEFAULT_WHERE:-mac}"
readonly PROJECT_VERB_KEY_PATTERN='^VERB_[a-z][a-z0-9]*(_[a-z0-9]+)*$'
readonly PROJECT_SUBDIR_PATTERN='^[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)*$'
readonly PROJECT_BRANCH_PATTERN='^[A-Za-z0-9._][A-Za-z0-9._/-]*$'
readonly PROJECT_VALUE_MAX_LENGTH=2000

# ---- reading a project as data ------------------------------------------------------------

# A project must not inherit a setting or a verb from a project loaded before it.
reset_project_variables() {
  local var
  for var in $(compgen -A variable | grep '^VERB_[a-z]') $PROJECT_KEYS; do
    unset "$var"
  done
}

is_project_key() {
  [[ " $PROJECT_KEYS " == *" $1 "* ]] || [[ "$1" =~ $PROJECT_VERB_KEY_PATTERN ]]
}

is_place() {
  [[ " $PLACES " == *" ${1:-} "* ]]
}

# The place the lavori of the loaded project get when box new does not name one.
project_where() {
  echo "${WHERE:-$DEFAULT_WHERE}"
}

# Succeeds when a path has a "." or ".." component.
has_dot_component() {
  [[ "/$1/" == */./* ]] || [[ "/$1/" == */../* ]]
}

# project_value_problem <key> <value>: prints why the value is not safe; nothing when it is.
project_value_problem() {
  local key="$1" value="$2"
  if [ "${#value}" -gt "$PROJECT_VALUE_MAX_LENGTH" ]; then echo "$key is longer than $PROJECT_VALUE_MAX_LENGTH characters"; return 0; fi
  if [[ "$value" == *[[:cntrl:]]* ]]; then echo "$key holds a control character"; return 0; fi
  case "$key" in
    REPO)
      if [[ "$value" != /* ]] || has_dot_component "$value"; then echo "REPO must be an absolute path without . or .. parts"; fi ;;
    SUBDIR)
      if [[ "$value" == -* ]]; then
        echo "SUBDIR must not start with a hyphen"
      elif [ -n "$value" ] && { [[ ! "$value" =~ $PROJECT_SUBDIR_PATTERN ]] || has_dot_component "$value"; }; then
        echo "SUBDIR must be a path inside the repository (letters, digits, dot, hyphen, underscore, slash)"
      fi ;;
    BASE)
      if [[ ! "$value" =~ $PROJECT_BRANCH_PATTERN ]] || [[ "$value" == *..* ]]; then echo "BASE is not a branch name"; fi ;;
    SPARSE)
      if [ "$value" != "0" ] && [ "$value" != "1" ]; then echo "SPARSE must be 0 or 1"; fi ;;
    WHERE)
      is_place "$value" || echo "WHERE must be mac or box" ;;
    PORTS)
      if [[ ! "$value" =~ ^[1-9][0-9]{0,2}$ ]] || [ "$value" -gt 100 ]; then echo "PORTS must be a number from 1 to 100"; fi ;;
    DISK_BUDGET_MB)
      if [[ ! "$value" =~ ^[1-9][0-9]{0,6}$ ]] || [ "$value" -gt 1048576 ]; then echo "DISK_BUDGET_MB must be a number from 1 to 1048576"; fi ;;
    RUN_MODE)
      if [ "$value" != "one" ] && [ "$value" != "many" ]; then echo "RUN_MODE must be one or many"; fi ;;
    ACCOUNT)
      [[ "$value" =~ $ACCOUNT_NAME_PATTERN ]] || echo "ACCOUNT is not the name of an account (lowercase letters, digits and single hyphens)" ;;
    BRANCH_PREFIX)
      if [[ ! "$value" =~ $PROJECT_BRANCH_PATTERN ]] || [[ "$value" == *..* ]] || [[ "$value" == *//* ]]; then
        echo "BRANCH_PREFIX is not the start of a branch name (letters, digits, dot, hyphen, underscore, slash)"
      fi ;;
  esac
}

# Reads a project file into the project variables of this shell. Prints the reason and fails
# when a line is not KEY=value, a key is unknown or a value is not safe.
read_project_file() {
  local file="$1" line key value problem
  reset_project_variables
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in '' | '#'*) continue ;; esac
    if [[ "$line" != *=* ]]; then echo "a line is not KEY=value"; return 1; fi
    key="${line%%=*}"
    value="${line#*=}"
    if ! is_project_key "$key"; then echo "unknown key '$(printf '%s' "$key" | sanitize | cut -c1-40)'"; return 1; fi
    problem="$(project_value_problem "$key" "$value")"
    if [ -n "$problem" ]; then echo "$problem"; return 1; fi
    printf -v "$key" '%s' "$value"
  done <"$file"
  if [ -z "${REPO:-}" ]; then echo "REPO is missing"; return 1; fi
  if [ -z "${BASE:-}" ]; then echo "BASE is missing"; return 1; fi
  SUBDIR="${SUBDIR:-}"
  INSTALL="${INSTALL:-}"
}

# What is checked of a repository's git directory before git runs in it.
. "$(dirname "${BASH_SOURCE[0]}")/repository.sh"

# ---- where a repository of the person may be, and how box reads it ------------------------------
# box runs git on the host only in a repository of the person. The workspaces agents write live
# in the data directory, and a path can name a directory in many ways (another letter case on a
# file system that does not tell them apart, a link, another name of the same volume). So
# "inside the data directory" is decided by identity, never by comparing path strings: the
# device and inode of the folder and of each of its ancestors against those of the data
# directory and its parts.

# The physical path of a folder (links resolved), or nothing when it is not a folder.
physical_directory() {
  [ -d "$1" ] && (cd -P "$1" 2>/dev/null && pwd -P)
}

# folder_is_under <physical folder> <directories...>: succeeds when the folder is one of the
# directories or lies inside one, by device and inode. A directory that is a link stands for
# the folder it leads to. When the folder cannot be examined it counts as inside (the caller
# refuses it).
folder_is_under() {
  local folder="$1" wanted own identity dir paths=()
  shift
  wanted="$(file_identity -L "$@" 2>/dev/null)" || true
  [ -n "$wanted" ] || return 1
  dir="$folder"
  while :; do
    paths+=("$dir")
    [ "$dir" != "/" ] || break
    dir="${dir%/*}"
    dir="${dir:-/}"
  done
  own="$(file_identity "${paths[@]}" 2>/dev/null)" || true
  [ -n "$own" ] || return 0
  while IFS= read -r identity; do
    if grep -qxF "$identity" <<<"$wanted"; then return 0; fi
  done <<<"$own"
  return 1
}

# The parts of the data directory that box manages: the workspaces, which agents write, and the
# state of box.
managed_directories() {
  local name
  for name in $BOX_MANAGED_DIRECTORIES; do echo "$AGENTBOX_HOME/$name"; done
}

# The identities of the parts of the data directory that box manages, one per line (those that
# exist).
managed_identities() {
  local directories=() directory
  while IFS= read -r directory; do directories+=("$directory"); done < <(managed_directories)
  file_identity "${directories[@]}" 2>/dev/null || true
}

# Succeeds when a physical folder is in a part of the data directory that box manages. (The
# throwaway source of the self-tests lives in the data directory beside them and stays usable.)
is_in_managed_directory() {
  local directories=() directory
  while IFS= read -r directory; do directories+=("$directory"); done < <(managed_directories)
  folder_is_under "$1" "${directories[@]}"
}

# Succeeds when a physical folder is the data directory or anywhere inside it: what the person
# may not choose as a repository.
is_in_data_directory() {
  folder_is_under "$1" "$AGENTBOX_HOME" || is_in_managed_directory "$1"
}

owned_by_current_user() {
  [ "$(file_owner "$1" 2>/dev/null)" = "$(id -u)" ]
}

# git on a repository of the person, on the host. The repository is theirs and trusted, and
# still nothing its configuration names is given a chance to run: no file system monitor, no
# hooks, no pager, no external diff or helper protocol, no system configuration, and nothing of
# the caller's environment that points git elsewhere. Read-only commands, plus the clone of
# box new and the fetch of box take.
readonly PERSON_GIT_OPTIONS=(
  --no-pager --no-optional-locks -c core.fsmonitor=false -c core.hooksPath=/dev/null
  -c protocol.ext.allow=never -c diff.external= -c core.pager=cat -c core.sshCommand=false -c gc.auto=0
)
person_git() {
  person_environment git "${PERSON_GIT_OPTIONS[@]}" "$@"
}

# Runs a command with the environment person_git gives git: nothing of the caller's that points
# git elsewhere. For git itself and for a helper that is told to run git with those options.
person_environment() {
  "$TRUSTED_ENV" -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_EXTERNAL_DIFF -u GIT_SSH -u GIT_SSH_COMMAND \
    -u GIT_COMMON_DIR -u GIT_OBJECT_DIRECTORY -u GIT_ALTERNATE_OBJECT_DIRECTORIES -u GIT_NAMESPACE \
    -u GIT_CONFIG_COUNT -u GIT_CONFIG_PARAMETERS -u GIT_CEILING_DIRECTORIES \
    GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 GIT_OPTIONAL_LOCKS=0 GIT_PAGER=cat PAGER=cat "$@"
}

# Prints why the repository of the loaded project cannot be used; nothing when it can. Asked
# every time a project is loaded: a folder or anything in its .git can be replaced after it was
# added (see repository.sh for what is checked before git runs, and after).
project_repository_problem() {
  local physical
  physical="$(physical_directory "$REPO")" || { echo "its repository does not exist: $REPO"; return 0; }
  if is_in_managed_directory "$physical"; then echo "its repository is inside the data directory of box"; return 0; fi
  if ! owned_by_current_user "$physical"; then echo "its repository is not owned by you: $REPO"; return 0; fi
  checked_repository_at "$physical" || echo "$REPOSITORY_PROBLEM"
}

project_file() {
  echo "$PROJECTS_DIR/$1.env"
}

# Loads a project into the current shell (REPO, BASE, SUBDIR, INSTALL, VERB_*, ...). A shipped
# profile of that name becomes a project first, when it never was one.
load_project() {
  local name="${1:-}" file problem
  [[ "$name" =~ $PROJECT_PATTERN ]] || die "invalid project name '$name'"
  file="$(project_file "$name")"
  [ -f "$file" ] || import_shipped_project "$name" || true
  [ -f "$file" ] || die "unknown project '$name' (box projects --json lists them, box project add creates one)"
  problem="$(read_project_file "$file")" || die "project '$name' is refused: $problem ($file)"
  read_project_file "$file" >/dev/null
  problem="$(project_repository_problem)"
  [ -z "$problem" ] || die "project '$name' is refused: $problem"
}

# Prints the command of a verb, or nothing when the project does not name the verb.
verb_command() {
  local verb="$1" var
  [[ "$verb" =~ $VERB_PATTERN ]] || return 0
  var="VERB_${verb//-/_}"
  echo "${!var:-}"
}

# The verbs of the loaded project, sorted.
project_verbs() {
  compgen -A variable | grep '^VERB_[a-z]' | sed 's/^VERB_//; s/_/-/g' | sort
}

# "/<subfolder>" for a project in a subfolder, nothing for one at the root of its repository.
subdir_suffix() {
  echo "${SUBDIR:+/$SUBDIR}"
}

# ---- writing a project ----------------------------------------------------------------------

# Prints the loaded project as the lines of its file.
project_lines() {
  local key var
  for key in $PROJECT_KEYS; do
    [ -z "${!key:-}" ] || printf '%s=%s\n' "$key" "${!key}"
  done
  for var in $(compgen -A variable | grep '^VERB_[a-z]' | sort); do
    [ -z "${!var:-}" ] || printf '%s=%s\n' "$var" "${!var}"
  done
}

# write_project <name> <new|replace>: writes the loaded project whole. "new" never replaces a file.
write_project() {
  local name="$1" mode="$2" file tmp
  file="$(project_file "$name")"
  mkdir -p "$PROJECTS_DIR"
  tmp="$(mktemp "$PROJECTS_DIR/.new.XXXXXX")"
  project_lines >"$tmp"
  if [ "$mode" = "replace" ]; then mv "$tmp" "$file"; return 0; fi
  if ln "$tmp" "$file" 2>/dev/null; then rm -f "$tmp"; else rm -f "$tmp"; return 1; fi
}

# ---- upgrade: the shipped profiles become projects, once ------------------------------------

was_imported() {
  grep -qx "$1" "$IMPORTED_PROFILES_FILE" 2>/dev/null
}

# Makes the shipped profile of that name a project, unless it already was one once (the person
# may have removed or changed it since) or its repository is not on this Mac. Fails when
# nothing was imported.
import_shipped_project() {
  local name="$1" shipped="$SHIPPED_PROFILES_DIR/$1.env" staged content
  [ -f "$shipped" ] && ! was_imported "$name" && [ ! -f "$(project_file "$name")" ] || return 1
  mkdir -p "$PROJECTS_DIR"
  staged="$(mktemp "$PROJECTS_DIR/.shipped.XXXXXX")"
  content="$(cat "$shipped")"
  printf '%s\n' "${content//$SHIPPED_HOME_TOKEN/$AGENTBOX_HOME}" >"$staged"
  if read_project_file "$staged" >/dev/null && [ -z "$(project_repository_problem)" ] && write_project "$name" new; then
    echo "$name" >>"$IMPORTED_PROFILES_FILE"
    rm -f "$staged"
    return 0
  fi
  rm -f "$staged"
  return 1
}

# Called at the start of every command: the first time the data directory has no projects yet,
# the shipped profiles (all but the on-demand ones) become its first projects.
import_shipped_projects() {
  local shipped name
  [ ! -d "$PROJECTS_DIR" ] || return 0
  mkdir -p "$PROJECTS_DIR"
  for shipped in "$SHIPPED_PROFILES_DIR"/*.env; do
    [ -f "$shipped" ] || continue
    name="$(basename "$shipped" .env)"
    [[ "$name" =~ $PROJECT_PATTERN ]] && [[ " $ON_DEMAND_PROFILES " != *" $name "* ]] || continue
    (import_shipped_project "$name") || true
  done
}

# ---- task records ----------------------------------------------------------------------------

# Task records live outside the workspace on purpose: the agent can edit its workspace, it
# must not choose what the runner executes. Arguments: task, project, base sha, base branch,
# place and, for a lavoro on the Mac, the git directory of its worktree and the device and
# inode of its folder. The key of the project
# is still called PROFILE, as in the records of older tasks.
record_task() {
  mkdir -p "$TASKS_DIR"
  {
    printf 'PROFILE=%s\nBASE=%s\nBASE_SHA=%s\nGENERATION=%s\nWHERE=%s\n' "$2" "$4" "$3" "$(date +%s)$RANDOM$RANDOM" "$5"
    [ -z "${6:-}" ] || printf 'WORKTREE_GIT_DIR=%s\n' "$6"
    [ -z "${7:-}" ] || printf 'WORKTREE_IDENTITY=%s\n' "$7"
  } >"$TASKS_DIR/$1.env"
}

# The place of a task: mac or box. A task created before places existed is in a box.
task_where() {
  local where
  where="$(task_record_field "$1" WHERE)"
  is_place "$where" && echo "$where" || echo box
}

task_is_on_mac() {
  [ "$(task_where "$1")" = "mac" ]
}

readonly DOES_NOT_APPLY_EXIT_CODE=80

# The two places never mix (docs/interface.md, "The two places"). A lavoro on the Mac is the
# person's own worktree and is read with host git; nothing of it is ever mounted in a container
# or handed to the machinery of a box. Three guards keep it so, each ending with one line and
# the exit code 80.

# For a verb that only exists for a box. usage: refuse_on_mac <task> <verb> <why, and what to do instead>
refuse_on_mac() {
  task_is_on_mac "$1" || return 0
  warn "$2 does not apply to task '$1', which is on the Mac: $3"
  exit "$DOES_NOT_APPLY_EXIT_CODE"
}

# The other way round: for a verb that only exists for a lavoro on the Mac.
refuse_in_box() {
  ! task_is_on_mac "$1" || return 0
  warn "$2 does not apply to task '$1', which is in a box: $3"
  exit "$DOES_NOT_APPLY_EXIT_CODE"
}

# For a verb whose part for the Mac does not exist yet. usage: not_built_on_mac <task> <verb>
not_built_on_mac() {
  task_is_on_mac "$1" || return 0
  warn "$2 is not built yet for a lavoro on the Mac (task '$1'); nothing was started"
  exit "$DOES_NOT_APPLY_EXIT_CODE"
}

# The central guard, in every function that mounts a workspace or a drop directory, starts a
# job container or reads a workspace through the verified store: whatever verb led there.
require_box_task() {
  task_is_on_mac "$1" || return 0
  warn "task '$1' is on the Mac: nothing of it is ever mounted in a container or handed to the machinery of a box"
  exit "$DOES_NOT_APPLY_EXIT_CODE"
}

# What tells one incarnation of a task from a later task of the same name. Empty for a task
# created before tasks had one.
task_generation() {
  task_record_field "$1" GENERATION
}

# A task that box rm has begun to remove: nothing is queued for it any more.
task_retiring_mark() {
  echo "$TASKS_DIR/$1.retiring"
}

task_is_retiring() {
  [ -e "$(task_retiring_mark "$1")" ]
}

# The value of a field of a task record, or nothing. Read with what bash has built in: it is
# requested all the time, also by code that stands in a worktree.
task_record_field() {
  local line
  [ -f "$TASKS_DIR/$1.env" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    if [[ "$line" == "$2="* ]]; then
      echo "${line#"$2="}"
      return 0
    fi
  done <"$TASKS_DIR/$1.env"
}

task_project() {
  task_record_field "$1" PROFILE
}

task_base_sha() {
  task_record_field "$1" BASE_SHA
}

# The branch a task started from: its own record, or the project's base for a task created
# before tasks could choose one. Needs the project of the task loaded.
task_base_branch() {
  local branch
  branch="$(task_record_field "$1" BASE)"
  echo "${branch:-$BASE}"
}

# A task keeps the project values it was created with (its checks, protected paths, commands):
# they are copied beside its record when it is created and read from there ever after, so a
# later box project set changes what new tasks get. Native Play separately refreshes only
# RUN, PORTS and RUN_MODE; checks and other task commands retain their snapshot.
task_project_snapshot() {
  echo "$TASKS_DIR/$1.project"
}

# Writes the loaded project as the copy of a task, whole.
write_task_project_snapshot() {
  local snapshot tmp
  snapshot="$(task_project_snapshot "$1")"
  mkdir -p "$TASKS_DIR"
  tmp="$(mktemp "$TASKS_DIR/.snapshot.XXXXXX")" || return 1
  project_lines >"$tmp" && mv "$tmp" "$snapshot"
}

# Loads the project values of a task. A task created before tasks kept a copy gets one now, from
# the project as it is. The copy is read as data like a project and refused the same way.
load_task_project() {
  local task="$1" project snapshot problem
  project="$(task_project "$task")"
  [ -n "$project" ] || die "task '$task' has no project record in $TASKS_DIR"
  snapshot="$(task_project_snapshot "$task")"
  if [ ! -f "$snapshot" ]; then
    load_project "$project"
    write_task_project_snapshot "$task" || die "could not keep the project values of task '$task'"
    return 0
  fi
  problem="$(read_project_file "$snapshot")" || die "the project values of task '$task' are refused: $problem ($snapshot)"
  read_project_file "$snapshot" >/dev/null
  problem="$(project_repository_problem)"
  [ -z "$problem" ] || die "the project of task '$task' is refused: $problem"
}

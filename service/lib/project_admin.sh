# shellcheck shell=bash
# box project add | set | rm, box projects --json, box branches <project> --json: projects are
# made and changed here, from the person's choices, never by editing a file. Everything is
# validated before it is written; commands are stored as text and only ever run in a container.

# The options of add and set that put one value into one key of the project.
readonly PROJECT_VALUE_FLAGS="--subdir=SUBDIR --base=BASE --install=INSTALL --protected=PROTECTED --test-files=TEST_FILES --safe-outside=SAFE_OUTSIDE --where=WHERE --branch-prefix=BRANCH_PREFIX --setup=SETUP --archive=ARCHIVE --account=ACCOUNT --run=RUN --ports=PORTS --run-mode=RUN_MODE --disk-budget-mb=DISK_BUDGET_MB"
readonly PROJECT_FLAGS="--repo --check $(printf '%s\n' $PROJECT_VALUE_FLAGS | sed 's/=.*//' | tr '\n' ' ' | sed 's/ $//')"
readonly MAX_BRANCHES_LISTED=200

# box project <detect|add|set|rm> ...
cmd_project() {
  local action="${1:-}"
  [ "$#" -gt 0 ] && shift
  case "$action" in
    detect) cmd_project_detect "$@" ;;
    add) cmd_project_add "$@" ;;
    set) cmd_project_set "$@" ;;
    rm) cmd_project_rm "$@" ;;
    *) die "usage: box project detect <path> --json | add <name> --repo <path> [options] | set <name> [options] | rm <name>" ;;
  esac
}

# ---- the options of add and set ---------------------------------------------------------------

# Reads the options into FLAG_<KEY> variables (FLAG_REPO, FLAG_CHECKS and one per key of
# PROJECT_VALUE_FLAGS); FLAGS_GIVEN lists the ones that were named.
parse_project_flags() {
  local pair
  FLAGS_GIVEN="" FLAG_CHECKS=()
  while [ "$#" -gt 0 ]; do
    [[ " $PROJECT_FLAGS " == *" $1 "* ]] || die "unknown option '$1' (the options are: $PROJECT_FLAGS)"
    [ "$#" -ge 2 ] || die "$1 needs a value"
    case "$1" in
      --repo) FLAG_REPO="$2" ;;
      --check) FLAG_CHECKS+=("$2") ;;
      *) for pair in $PROJECT_VALUE_FLAGS; do
           if [ "${pair%%=*}" = "$1" ]; then printf -v "FLAG_${pair#*=}" '%s' "$2"; fi
         done ;;
    esac
    FLAGS_GIVEN="$FLAGS_GIVEN $1"
    shift 2
  done
}

flag_given() {
  [[ " $FLAGS_GIVEN " == *" $1 "* ]]
}

# Puts the options that were named into the project variables of this shell.
apply_project_flags() {
  local pair flag
  if flag_given --repo; then REPO="$(checked_repository "$FLAG_REPO")"; fi
  for pair in $PROJECT_VALUE_FLAGS; do
    flag="FLAG_${pair#*=}"
    if flag_given "${pair%%=*}"; then printf -v "${pair#*=}" '%s' "${!flag}"; fi
  done
  apply_check_flags
}

apply_check_flags() {
  local check verb seen=" "
  for check in ${FLAG_CHECKS[@]+"${FLAG_CHECKS[@]}"}; do
    verb="${check%%=*}"
    [[ "$check" == *=* ]] || die "a check is given as verb=command (an empty command removes the verb): '$check'"
    [[ "$verb" =~ $VERB_PATTERN ]] || die "'$verb' is not a verb (lowercase letters, digits and hyphens, starting with a letter)"
    [[ "$seen" != *" $verb "* ]] || die "the verb '$verb' is given twice"
    seen="$seen$verb "
    printf -v "VERB_${verb//-/_}" '%s' "${check#*=}"
  done
}

# The physical path of a folder that is the root of a repository of the person, or the end.
checked_repository() {
  local path="$1" dir root
  dir="$(physical_directory "$path")" || die "'$path' is not a folder"
  root="$(detect_repository_root "$dir")" || exit 1
  [ "$root" = "$dir" ] || die "'$dir' is not the root of its repository (the root is $root; the rest goes into --subdir)"
  echo "$dir"
}

# Checks the project in the variables of this shell as a whole; the end with one line otherwise.
require_valid_project() {
  local name="$1" key problem subfolder
  [ -n "${REPO:-}" ] && [ -n "${BASE:-}" ] || die "project '$name': the repository and the base branch are needed"
  for key in $PROJECT_KEYS $(compgen -A variable | grep '^VERB_[a-z]'); do
    [ -n "${!key:-}" ] || continue
    problem="$(project_value_problem "$key" "${!key}")"
    [ -z "$problem" ] || die "project '$name': $problem"
  done
  problem="$(project_repository_problem)"
  [ -z "$problem" ] || die "project '$name': $problem"
  [ "$(detect_repository_root "$(physical_directory "$REPO")")" = "$(physical_directory "$REPO")" ] || die "project '$name': $REPO is not the root of a repository of yours"
  if [ -n "${SUBDIR:-}" ]; then
    subfolder="$(physical_directory "$REPO/$SUBDIR")" && folder_is_under "$subfolder" "$REPO" \
      || die "project '$name': the subfolder '$SUBDIR' is not a folder inside $REPO"
  fi
  has_local_branch "$REPO" "$BASE" || die "project '$name': '$BASE' is not a branch of $REPO (box branches lists them)"
}

# The names of the tasks that use a project, one per line.
project_tasks() {
  local record
  for record in "$TASKS_DIR"/*.env; do
    if [ -f "$record" ] && grep -qx "PROFILE=$1" "$record"; then basename "$record" .env; fi
  done
}

# ---- add, set, rm ------------------------------------------------------------------------------

cmd_project_add() {
  local name="${1:-}"
  [[ "$name" =~ $PROJECT_PATTERN ]] || die "invalid project name '$name' (lowercase letters, digits and single hyphens)"
  shift
  [ ! -e "$(project_file "$name")" ] && [ ! -e "$SHIPPED_PROFILES_DIR/$name.env" ] || die "a project named '$name' already exists"
  parse_project_flags "$@"
  flag_given --repo || die "box project add needs --repo <path of the repository>"
  reset_project_variables
  SUBDIR="" INSTALL=""
  apply_project_flags
  flag_given --base || BASE="$(default_branch "$REPO" "$(person_git -C "$REPO" symbolic-ref --quiet --short HEAD 2>/dev/null || true)")"
  [ -n "${BASE:-}" ] || die "the repository $REPO has no branch yet"
  require_valid_project "$name"
  write_project "$name" new || die "a project named '$name' already exists"
  echo "project '$name' added: $REPO${SUBDIR:+ (folder $SUBDIR)}, base $BASE"
}

cmd_project_set() {
  local name="${1:-}" problem tasks
  [[ "$name" =~ $PROJECT_PATTERN ]] || die "invalid project name '$name'"
  shift
  [ -f "$(project_file "$name")" ] || die "unknown project '$name'"
  problem="$(read_project_file "$(project_file "$name")")" || die "project '$name' is refused: $problem"
  read_project_file "$(project_file "$name")" >/dev/null
  parse_project_flags "$@"
  [ -n "$FLAGS_GIVEN" ] || die "box project set needs at least one option ($PROJECT_FLAGS)"
  tasks="$(project_tasks "$name" | tr '\n' ' ')"
  if [ -n "$tasks" ] && { flag_given --repo || flag_given --subdir; }; then
    die "the repository and the subfolder of project '$name' cannot change while tasks use it: $tasks"
  fi
  apply_project_flags
  require_valid_project "$name"
  write_project "$name" replace
  echo "project '$name' changed"
}

cmd_project_rm() {
  local name="${1:-}" tasks
  [[ "$name" =~ $PROJECT_PATTERN ]] || die "invalid project name '$name'"
  [ -f "$(project_file "$name")" ] || die "unknown project '$name'"
  tasks="$(project_tasks "$name" | tr '\n' ' ')"
  [ -z "$tasks" ] || die "project '$name' is used by tasks: $tasks(remove them first: box rm <task>)"
  rm -f "$(project_file "$name")"
  echo "project '$name' removed"
}

# ---- lists ----------------------------------------------------------------------------------------

# One element of box projects --json. A file that is refused is listed with the reason.
project_json() {
  local name="$1" problem checks="[]" verb
  if ! problem="$(read_project_file "$(project_file "$name")")"; then
    jq -nc --arg name "$name" --arg error "$problem" '{name: $name, error: $error}'
    return 0
  fi
  read_project_file "$(project_file "$name")" >/dev/null
  for verb in $(project_verbs); do
    checks="$(jq -c --arg verb "$verb" --arg command "$(verb_command "$verb")" '. + [{verb: $verb, command: $command}]' <<<"$checks")"
  done
  jq -nc --arg name "$name" --arg repo "$REPO" --arg subdir "$SUBDIR" --arg base "$BASE" --arg install "$INSTALL" \
    --argjson checks "$checks" --arg protected "${PROTECTED:-}" --arg testFiles "${TEST_FILES:-}" --arg safeOutside "${SAFE_OUTSIDE:-}" --arg where "$(project_where)" \
    --arg branchPrefix "$(project_branch_prefix)" --arg setup "${SETUP:-}" --arg archive "${ARCHIVE:-}" --arg account "${ACCOUNT:-}" \
    --arg run "${RUN:-}" --argjson ports "$(project_ports)" --arg runMode "$(project_run_mode)" \
    --argjson tasks "$(project_tasks "$name" | jq -cR . | jq -cs .)" --arg error "$(project_repository_problem)" '
    def optional: if . == "" then null else . end;
    { name: $name, repo: $repo, subdir: $subdir, base: $base, where: $where, branchPrefix: $branchPrefix,
      account: ($account | optional), setup: ($setup | optional), archive: ($archive | optional),
      run: ($run | optional), ports: $ports, runMode: $runMode, install: ($install | optional), checks: $checks,
      protected: ($protected | optional), testFiles: ($testFiles | optional), safeOutside: ($safeOutside | optional), tasks: $tasks, error: ($error | optional) }'
}

# box projects --json (and box profiles --json, its older name): every project, sorted by name.
cmd_projects() {
  local file
  [ "${1:-}" = "--json" ] || die "usage: box projects --json"
  require_jq
  for file in "$PROJECTS_DIR"/*.env; do
    [ -f "$file" ] || continue
    [[ "$(basename "$file" .env)" =~ $PROJECT_PATTERN ]] && (project_json "$(basename "$file" .env)")
  done | jq -cs .
}

# box branches <project> --json: the local branches of the project's repository a task can start
# from, most recently committed first, and the project's default.
cmd_branches() {
  local name="${1:-}"
  [ "$#" -eq 2 ] && [ "$2" = "--json" ] || die "usage: box branches <project> --json"
  require_jq
  load_project "$name"
  person_git -C "$REPO" for-each-ref --sort=-committerdate --format='%(refname:short)%09%(objectname)%09%(committerdate:unix)' refs/heads \
    | head -n "$MAX_BRANCHES_LISTED" | jq -cR --arg project "$name" --arg default "$BASE" '
      split("\t") | select(length == 3 and (.[0] | test("^[A-Za-z0-9._][A-Za-z0-9._/-]*$")) and (.[0] | contains("..") | not))
      | {name: .[0], sha: .[1], time: (.[2] | tonumber | todate)}' \
    | jq -cs --arg project "$name" --arg default "$BASE" '{project: $project, default: $default, branches: .}'
}

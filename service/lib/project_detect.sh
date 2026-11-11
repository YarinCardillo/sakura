# shellcheck shell=bash
# box project detect <path> --json: proposes a project from a folder the person chose, and
# writes nothing. The folder is the person's own repository on this Mac: it is trusted and read
# with git on the host (person_git), which is why a folder of the data directory of box (the
# workspaces agents write) is refused, by identity, before git is run anywhere near it.

readonly DETECT_NOT_A_FOLDER_EXIT=3
readonly DETECT_NOT_A_REPOSITORY_EXIT=4
readonly DETECT_BARE_REPOSITORY_EXIT=5
readonly DETECT_INSIDE_DATA_DIRECTORY_EXIT=6
readonly DETECT_NOT_OWNED_EXIT=7
readonly DETECT_MAX_BRANCHES=30
readonly DETECT_REASON_LENGTH=120
# package.json scripts that are checks, in the order they are proposed.
readonly DETECT_CHECK_SCRIPTS="test build lint typecheck check"
# Files of a node project that decide how its checks run, when the folder has them.
readonly DETECT_NODE_PROTECTED="tsconfig*.json angular.json .npmrc vitest*.config.* vite.config.* jest.config.* karma.conf.* playwright.config.* eslint.config.* .eslintrc*"
readonly DETECT_NODE_TEST_FILES="*.spec.ts *.test.ts *.spec.tsx *.test.tsx *.spec.js *.test.js *.spec.jsx *.test.jsx"

# Ends with one line on stderr and the given exit code.
detect_fail() {
  local code="$1"
  shift
  warn "$*"
  exit "$code"
}

cmd_project_detect() {
  local path="${1:-}" dir root subdir
  [ "$#" -eq 2 ] && [ "$2" = "--json" ] || die "usage: box project detect <path> --json"
  require_jq
  dir="$(physical_directory "$path")" || detect_fail "$DETECT_NOT_A_FOLDER_EXIT" "'$path' is not a folder"
  root="$(detect_repository_root "$dir")" || exit "$?"
  subdir="$(person_git -C "$dir" rev-parse --show-prefix)"
  subdir="${subdir%/}"
  [ -z "$(project_value_problem SUBDIR "$subdir")" ] \
    || detect_fail 1 "the path of the folder inside its repository has characters box does not accept: $subdir"
  detect_proposal "$root" "$subdir"
}

# A folder of the data directory is never a project: git is not run there. The folder is
# compared by identity (see folder_is_under), not by its path.
refuse_data_directory() {
  if is_in_data_directory "$1"; then
    detect_fail "$DETECT_INSIDE_DATA_DIRECTORY_EXIT" "'$1' is inside the data directory of box ($AGENTBOX_HOME): choose your own copy of the repository"
  fi
}

# Prints the root of the working tree a physical folder belongs to, or ends with the documented
# code. Before git runs at all: the folder is not in the data directory, it belongs to the
# current user, and the repository git would find for it (found here in git's own order, see
# find_git_entry) passes every check of repository.sh. After: git used the directories that
# were checked, and the root it names is the folder or above it (a configuration cannot send
# box elsewhere).
detect_repository_root() {
  local dir="$1" root
  refuse_data_directory "$dir"
  owned_by_current_user "$dir" || detect_fail "$DETECT_NOT_OWNED_EXIT" "'$dir' is not owned by you"
  if ! find_git_entry "$dir"; then
    [ -z "$REPOSITORY_PROBLEM" ] || detect_fail "$DETECT_INSIDE_DATA_DIRECTORY_EXIT" "'$dir' cannot be used: $REPOSITORY_PROBLEM"
    detect_fail "$DETECT_NOT_A_REPOSITORY_EXIT" "'$dir' is not inside a git repository"
  fi
  owned_by_current_user "$GIT_ENTRY" || detect_fail "$DETECT_NOT_OWNED_EXIT" "the repository of '$dir' is not owned by you"
  check_git_entry "$GIT_ENTRY" && git_used_checked_directories "$dir" \
    || detect_fail "$DETECT_INSIDE_DATA_DIRECTORY_EXIT" "'$dir' cannot be used: $REPOSITORY_PROBLEM"
  if [ "$(person_git -C "$dir" rev-parse --is-bare-repository 2>/dev/null)" = "true" ]; then
    detect_fail "$DETECT_BARE_REPOSITORY_EXIT" "'$dir' is a bare repository: it has no folder to work in"
  fi
  [ "$(person_git -C "$dir" rev-parse --is-inside-work-tree 2>/dev/null)" = "true" ] \
    || detect_fail "$DETECT_NOT_A_REPOSITORY_EXIT" "'$dir' is not inside a git repository"
  root="$(physical_directory "$(person_git -C "$dir" rev-parse --show-toplevel)")" || root=""
  [ -n "$root" ] && folder_is_under "$dir" "$root" && ! is_in_data_directory "$root" \
    || detect_fail "$DETECT_NOT_A_REPOSITORY_EXIT" "the root git names for '$dir' is not that folder or one above it"
  echo "$root"
}

# ---- the proposal -----------------------------------------------------------------------------
# Lists of proposals are JSON arrays kept in these variables while the folder is examined.

detect_add() {
  printf -v "$1" '%s' "$(jq -c --argjson item "$2" '. + [$item]' <<<"${!1}")"
}

detect_note() {
  detect_add DETECT_NOTES "$(jq -nc --arg note "$1" '$note')"
}

# detect_check <verb> <command> <reason>: proposes a check unless the verb is already proposed.
detect_check() {
  jq -e --arg verb "$1" 'any(.[]; .verb == $verb)' <<<"$DETECT_CHECKS" >/dev/null && return 0
  detect_add DETECT_CHECKS "$(jq -nc --arg verb "$1" --arg command "$2" --arg reason "$3" '{verb: $verb, command: $command, reason: $reason}')"
}

detect_pattern() {
  detect_add "$1" "$(jq -nc --arg pattern "$2" --arg reason "$3" '{pattern: $pattern, reason: $reason}')"
}

detect_proposal() {
  local root="$1" subdir="$2" folder="$1${2:+/$2}"
  DETECT_INSTALL=null DETECT_CHECKS='[]' DETECT_PROTECTED='[]' DETECT_TEST_FILES='[]' DETECT_NOTES='[]'
  detect_node "$folder"
  detect_cargo "$folder"
  detect_make "$folder"
  [ "$DETECT_CHECKS" != "[]" ] || detect_note "nothing was recognised in this folder: add the commands of the checks by hand"
  jq -nc --arg repo "$root" --arg subdir "$subdir" --arg name "$(detect_name "$folder")" \
    --argjson branches "$(detect_branches "$root")" --argjson install "$DETECT_INSTALL" --argjson checks "$DETECT_CHECKS" \
    --argjson protected "$DETECT_PROTECTED" --argjson testFiles "$DETECT_TEST_FILES" --argjson notes "$DETECT_NOTES" '
    { repo: $repo, subdir: $subdir, name: $name } + $branches
    + { install: $install, checks: $checks, protected: $protected, testFiles: $testFiles, notes: $notes }'
}

# A project name from the folder's name: lowercase letters, digits and single hyphens, and not
# the name of a project that exists (a number is added).
detect_name() {
  local name candidate number=1
  name="$(basename "$1" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]\{1,\}/-/g; s/^-//; s/-$//')"
  candidate="${name:-project}"
  while [ -e "$(project_file "$candidate")" ] || [ -e "$SHIPPED_PROFILES_DIR/$candidate.env" ]; do
    number=$((number + 1))
    candidate="${name:-project}-$number"
  done
  echo "$candidate"
}

# {currentBranch, defaultBranch, branches}: the local branches, most recent first, at most
# DETECT_MAX_BRANCHES, those a project can name as its base.
detect_branches() {
  local root="$1" current default branches
  current="$(person_git -C "$root" symbolic-ref --quiet --short HEAD 2>/dev/null)" || current=""
  branches="$(local_branches "$root" | head -n "$DETECT_MAX_BRANCHES" | jq -cR . | jq -cs .)"
  default="$(default_branch "$root" "$current")"
  jq -nc --arg current "$current" --arg default "$default" --argjson branches "$branches" '
    { currentBranch: (if $current == "" then null else $current end),
      defaultBranch: (if $default == "" then null else $default end), branches: $branches }'
}

# The local branches of a repository whose names box accepts, most recently committed first.
local_branches() {
  local branch
  person_git -C "$1" for-each-ref --sort=-committerdate --format='%(refname:short)' refs/heads | while IFS= read -r branch; do
    if [ -z "$(project_value_problem BASE "$branch")" ]; then echo "$branch"; fi
  done
}

has_local_branch() {
  person_git -C "$1" rev-parse --verify --quiet "refs/heads/$2^{commit}" >/dev/null 2>&1
}

# The branch new work starts from when nobody says otherwise: the one the remote calls default
# when it exists here, else main or master, else the current one, else the most recent.
default_branch() {
  local root="$1" current="$2" candidate
  candidate="$(person_git -C "$root" symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null)" || candidate=""
  for candidate in "${candidate#origin/}" main master "$current"; do
    if [ -n "$candidate" ] && has_local_branch "$root" "$candidate"; then echo "$candidate"; return 0; fi
  done
  local_branches "$root" | head -n 1
}

# ---- what is recognised -------------------------------------------------------------------------

# The package manager a lockfile in the folder names: "<manager> <lockfile>", nothing without one.
node_package_manager() {
  local lockfile
  for lockfile in package-lock.json:npm pnpm-lock.yaml:pnpm yarn.lock:yarn bun.lock:bun bun.lockb:bun; do
    if [ -f "$1/${lockfile%%:*}" ]; then echo "${lockfile##*:} ${lockfile%%:*}"; return 0; fi
  done
}

# The command that runs a package.json script once with a package manager.
node_script_command() {
  local manager="$1" script="$2" body="$3" command
  if [ "$script" = "test" ] && [ "$manager" != "bun" ]; then command="$manager test"; else command="$manager run $script"; fi
  # ng test watches for changes unless told not to; a check must end.
  if [[ "$body" == *"ng test"* ]] && [[ "$body" != *"--watch=false"* ]] && [[ "$body" != *"--no-watch"* ]]; then
    command="$command -- --watch=false"
  fi
  echo "$command"
}

detect_node() {
  local folder="$1" manager="npm" lockfile="" found script body pattern count
  [ -f "$folder/package.json" ] || return 0
  if ! jq -e 'type == "object"' "$folder/package.json" >/dev/null 2>&1; then detect_note "package.json could not be read"; return 0; fi
  found="$(node_package_manager "$folder")"
  if [ -n "$found" ]; then manager="${found%% *}"; lockfile="${found##* }"; fi
  detect_node_install "$manager" "$lockfile"
  for script in $DETECT_CHECK_SCRIPTS; do
    body="$(jq -r --arg script "$script" '.scripts[$script] // empty | select(type == "string")' "$folder/package.json")"
    [ -n "$body" ] || continue
    detect_check "$script" "$(node_script_command "$manager" "$script" "$body")" \
      "script \"$script\" of package.json: $(printf '%s' "$body" | sanitize | cut -c1-"$DETECT_REASON_LENGTH")"
  done
  detect_pattern DETECT_PROTECTED "package.json" "names the scripts and the dependencies"
  [ -z "$lockfile" ] || detect_pattern DETECT_PROTECTED "$lockfile" "fixes the versions of the dependencies"
  for pattern in $DETECT_NODE_PROTECTED; do
    count="$({ compgen -G "$folder/$pattern" || true; } | wc -l | tr -d ' ')"
    [ "$count" -eq 0 ] || detect_pattern DETECT_PROTECTED "$pattern" "$count file(s) in the folder that decide how the checks run"
  done
  for pattern in $DETECT_NODE_TEST_FILES; do
    count="$(person_git -C "$folder" ls-files -- "$pattern" | wc -l | tr -d ' ')"
    [ "$count" -eq 0 ] || detect_pattern DETECT_TEST_FILES "**/$pattern" "$count tracked file(s) match"
  done
}

# The runner prepares dependencies from package.json and package-lock.json; with another
# lockfile, or none, no install command is proposed and a note says what that means.
detect_node_install() {
  local manager="$1" lockfile="$2"
  if [ "$lockfile" = "package-lock.json" ]; then
    DETECT_INSTALL="$(jq -nc '{command: "npm ci", reason: "package-lock.json is in the folder"}')"
  elif [ -n "$lockfile" ]; then
    detect_note "the lockfile is $lockfile ($manager): box prepares dependencies only from package-lock.json, so no install command is proposed and the checks would run without installed dependencies"
  else
    detect_note "package.json has no lockfile beside it: box prepares dependencies only from package-lock.json, so no install command is proposed"
  fi
}

detect_cargo() {
  local folder="$1"
  [ -f "$folder/Cargo.toml" ] || return 0
  detect_check test "cargo test" "Cargo.toml is in the folder"
  detect_check build "cargo build" "Cargo.toml is in the folder"
  detect_pattern DETECT_PROTECTED "Cargo.toml" "names the package and its dependencies"
  [ ! -f "$folder/Cargo.lock" ] || detect_pattern DETECT_PROTECTED "Cargo.lock" "fixes the versions of the dependencies"
  [ -z "$(compgen -G "$folder/rust-toolchain*")" ] || detect_pattern DETECT_PROTECTED "rust-toolchain*" "names the compiler"
  [ ! -d "$folder/.cargo" ] || detect_pattern DETECT_PROTECTED ".cargo/**" "configures cargo"
  [ ! -d "$folder/tests" ] || detect_pattern DETECT_TEST_FILES "tests/**" "the folder has a tests directory"
  detect_note "the image of the boxes has no cargo: these checks need an image that has the Rust toolchain"
}

detect_make() {
  local folder="$1" target
  [ -f "$folder/Makefile" ] || return 0
  for target in test check; do
    grep -q -E "^$target:" "$folder/Makefile" || continue
    detect_check "$target" "make $target" "target \"$target\" of the Makefile"
    detect_note "the image of the boxes has no make: the check \"make $target\" needs an image that has it"
  done
  jq -e 'any(.[]; .command | startswith("make "))' <<<"$DETECT_CHECKS" >/dev/null || return 0
  detect_pattern DETECT_PROTECTED "Makefile" "defines the targets of the checks"
}

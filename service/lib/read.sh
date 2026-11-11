# shellcheck shell=bash
# The read verbs of the app, the same for both places:
#   box tree <task> [--path <dir>] [--all] --json    one directory level
#   box file <task> <path> [--at <sha>]              the bytes of one file
#   box search <task> <text> --json                  file names, for quick open
#   box graph <project or task> --json [--limit <n>] the commits of the base and of the lavori
# tree, file and search are the same scripts of image/runner for both places: in a box they run
# in a workspace container (what they print is the agent's word, validated and only shown); for
# a lavoro on the Mac box runs them natively and gives them the git command (lavoro.py). graph
# reads the person's repository and the worktrees with host git, and a task in a box through
# the verified store.

readonly FILE_TOO_BIG_EXIT_CODE=82
readonly FILE_MAX_BYTES=2097152
readonly READ_PATH_MAX_LENGTH=4096
readonly SEARCH_MAX_LENGTH=200
readonly GRAPH_DEFAULT_LIMIT=100
readonly GRAPH_MAX_LIMIT=1000
readonly GRAPH_FORMAT='%H%x1f%P%x1f%ct%x1f%s'

readonly TREE_FILTER='
  def count: if type == "number" and . >= 0 and . == floor then . else null end;
  def name($n): select(type == "string") | gsub("[\u0000-\u001f\u007f]"; " ") | .[0:$n];
  { path: (.path | name(4096)),
    entries: [ (.entries // [])[:2000][] | {
      name: (.name | name(255)),
      kind: (.kind | select(. == "file" or . == "dir" or . == "link" or . == "other" or . == "deleted")),
      changed: (.changed == true), added: (.added | count), removed: (.removed | count) } ],
    truncated: (.truncated == true) }'
readonly SEARCH_FILTER='
  def name($n): select(type == "string") | gsub("[\u0000-\u001f\u007f]"; " ") | .[0:$n];
  { query: (.query | name(200)), matches: [ (.matches // [])[:200][] | name(4096) ], truncated: (.truncated == true) }'
# Commits as graph.py and "git log" with GRAPH_FORMAT give them, validated.
readonly GRAPH_COMMITS_FILTER='
  def sha: select(type == "string" and test("^[0-9a-f]{40}$"));
  [ .[:1000][] | { sha: (.sha | sha), parents: [ (.parents // [])[:16][] | sha ],
      subject: ((.subject // "") | tostring | gsub("[\u0001-\u001f\u007f]"; " ") | .[0:200]),
      time: (.time | select(type == "number") | todate) } ]'
readonly GRAPH_LOG_RECORDS='
  [ split("\n")[] | select(length > 0) | split("\u001f")
    | { sha: .[0], parents: (.[1] | split(" ") | map(select(length > 0))), time: (.[2] | tonumber), subject: (.[3:] | join(" ")) } ]'
# The answer of box graph: every commit once, the ones of the base first in rank, newest first.
readonly GRAPH_ANSWER='
  ($lanes | map(select(.head != null) | {key: .head, value: .branch}) | group_by(.key) | map({key: .[0].key, value: map(.value)}) | from_entries) as $tips
  | { project: $project, base: $base, limit: $limit,
      lanes: $lanes,
      commits: ( [ $groups[] | .lavoro as $lavoro | .commits[] | . + {lavoro: $lavoro} ]
        | group_by(.sha) | map(.[0] + {lavoro: (map(.lavoro) | if any(. == null) then null else .[0] end)})
        | map(. + {branches: ($tips[.sha] // [])}) | sort_by(.time) | reverse ) }'

# ---- the scripts, in the place of the task ---------------------------------------------------------

# read_script <task> <script name> <arguments...>: runs a read script of image/runner for the
# task: natively with the git command of the lavoro on the Mac, in a workspace container for a
# task in a box. Needs the project of the task loaded.
read_script() {
  local task="$1" script="$2" command
  shift 2
  if ! task_is_on_mac "$task"; then
    workspace_sandbox "$task" -- "/box/$script" "$@"
    return
  fi
  checked_lavoro "$task" || return 1
  command="$(printf '%s\n' git "${PERSON_GIT_OPTIONS[@]}" "--git-dir=$LAVORO_GIT_DIR" "--work-tree=$LAVORO_DIR")"
  BOX_SRC="$LAVORO_DIR" BOX_ROOT_IDENTITY="$LAVORO_IDENTITY" BOX_LAVORO_GIT="$command" person_environment python3 -B -E -s "$RUNNER_SCRIPTS/$script" "$@"
}

# Loads the project of a task and, for one in a box, asks for Docker; for one on the Mac checks
# its worktree. The end with one line otherwise.
require_readable_task() {
  require_existing_task "$1"
  require_jq
  if task_is_on_mac "$1"; then
    command -v python3 >/dev/null 2>&1 || die "python3 is needed to read a lavoro on the Mac"
    require_checked_lavoro "$1"
  else
    load_task_project "$1"
    require_docker
  fi
}

# A path given by the caller: text of a sane length without control characters.
require_plain_path() {
  [ "${#1}" -le "$READ_PATH_MAX_LENGTH" ] && [[ "$1" != *[[:cntrl:]]* ]] || die "the path is not a path box accepts"
}

# ---- tree, search -------------------------------------------------------------------------------

cmd_tree() {
  local task="${1:-}" path="" all=0 json=0 out
  [ "$#" -ge 1 ] || die "usage: box tree <task> [--path <dir>] [--all] --json"
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --path) [ "$#" -ge 2 ] || die "--path needs a directory"; path="$2"; shift ;;
      --all) all=1 ;;
      --json) json=1 ;;
      *) die "usage: box tree <task> [--path <dir>] [--all] --json" ;;
    esac
    shift
  done
  [ "$json" -eq 1 ] || die "usage: box tree <task> [--path <dir>] [--all] --json"
  require_plain_path "$path"
  require_readable_task "$task"
  out="$(sandbox_json "$TREE_FILTER" read_script "$task" tree.py "$(task_base_sha "$task")" "$path" "$all")" \
    || die "could not list '${path:-the root}' of task '$task' (is it a directory of the lavoro?)"
  echo "$out"
}

cmd_search() {
  local task="${1:-}" query="${2:-}" out
  [ "$#" -eq 3 ] && [ "$3" = "--json" ] || die "usage: box search <task> <text> --json"
  [ -n "${query// /}" ] && [ "${#query}" -le "$SEARCH_MAX_LENGTH" ] && [[ "$query" != *[[:cntrl:]]* ]] \
    || die "the text to search for is empty, longer than $SEARCH_MAX_LENGTH characters or not plain text"
  require_readable_task "$task"
  out="$(sandbox_json "$SEARCH_FILTER" read_script "$task" search.py "$query")" || die "could not search the files of task '$task'"
  echo "$out"
}

# ---- file -----------------------------------------------------------------------------------------

# box file <task> <path> [--at <sha>]: the bytes on standard output, or one line and an exit
# code: 82 too big, 3 no such file, 1 a path that leaves the lavoro or anything else.
cmd_file() {
  local task="${1:-}" path="${2:-}" sha="" code=0
  [ "$#" -eq 2 ] || { [ "$#" -eq 4 ] && [ "$3" = "--at" ]; } || die "usage: box file <task> <path> [--at <sha>]"
  if [ "$#" -eq 4 ]; then
    sha="$4"
    [[ "$sha" =~ $COMMIT_PATTERN ]] || die "invalid commit sha '$sha'"
  fi
  [ -n "$path" ] || die "usage: box file <task> <path> [--at <sha>]"
  require_plain_path "$path"
  require_readable_task "$task"
  read_script "$task" file.py "$path" ${sha:+"$sha"} 2>/dev/null | head -c "$FILE_MAX_BYTES" || code="${PIPESTATUS[0]}"
  case "$code" in
    0) ;;
    82) warn "the file is larger than 2 MiB: box file does not print it"; exit "$FILE_TOO_BIG_EXIT_CODE" ;;
    3) warn "no such file in task '$task'${sha:+ at $sha}"; exit "$NOT_FOUND_EXIT_CODE" ;;
    4) die "the path leaves the lavoro, or is a link that does: box file does not follow it" ;;
    *) die "could not read the file of task '$task'" ;;
  esac
}

# ---- graph ----------------------------------------------------------------------------------------

# box graph <project or task> --json [--limit <n>]: a project of that name wins over a task.
cmd_graph() {
  local name="${1:-}" limit="$GRAPH_DEFAULT_LIMIT" tasks project base
  [ "$#" -eq 2 ] || [ "$#" -eq 4 ] || die "usage: box graph <project or task> --json [--limit <n>]"
  [ "$2" = "--json" ] || die "usage: box graph <project or task> --json [--limit <n>]"
  if [ "$#" -eq 4 ]; then
    [ "$3" = "--limit" ] && [[ "$4" =~ ^[1-9][0-9]{0,3}$ ]] && [ "$4" -le "$GRAPH_MAX_LIMIT" ] || die "--limit takes a number from 1 to $GRAPH_MAX_LIMIT"
    limit="$4"
  fi
  require_jq
  if [[ "$name" =~ $PROJECT_PATTERN ]] && [ -f "$(project_file "$name")" ]; then
    load_project "$name"
    project="$name" base="$BASE" tasks="$(project_tasks "$name")"
  else
    require_existing_task "$name"
    load_task_project "$name"
    project="$(task_project "$name")" base="$(task_base_branch "$name")" tasks="$name"
  fi
  graph_json "$project" "$base" "$limit" $tasks
}

# The commits of a ref of the person's repository, newest first, as validated JSON.
repository_commits() {
  person_git -C "$REPO" log --max-count="$1" --format="$GRAPH_FORMAT" "$2" 2>/dev/null \
    | jq -Rs "$GRAPH_LOG_RECORDS" 2>/dev/null | jq -c "$GRAPH_COMMITS_FILTER" 2>/dev/null
}

# graph_json <project> <base branch> <limit> <tasks...>: needs the project loaded for the base.
graph_json() {
  local project="$1" base="$2" limit="$3" task lanes groups commits head lane
  shift 3
  commits="$(repository_commits "$limit" "refs/heads/$base")" && [ -n "$commits" ] || die "could not read the branch '$base' of project '$project'"
  head="$(jq -r '.[0].sha // ""' <<<"$commits")"
  lanes="$(jq -nc --arg branch "$base" --arg head "$head" '[{lavoro: null, branch: $branch, where: null, head: $head, base: null, readable: true}]')"
  groups="$(jq -nc --argjson commits "$commits" '[{lavoro: null, commits: $commits}]')"
  for task in "$@"; do
    lane="$(task_lane_json "$task")" || continue
    lanes="$(jq -c --argjson lane "$(jq -c '.lane' <<<"$lane")" '. + [$lane]' <<<"$lanes")"
    groups="$(jq -c --argjson group "$(jq -c '{lavoro: .lane.lavoro, commits}' <<<"$lane")" '. + [$group]' <<<"$groups")"
  done
  jq -nc --arg project "$project" --arg base "$base" --argjson limit "$limit" --argjson lanes "$lanes" --argjson groups "$groups" "$GRAPH_ANSWER"
}

# {lane, commits} of one task, in a subshell (it loads the project values of the task). A lavoro
# that cannot be read is a lane without commits, readable false.
task_lane_json() {
  (
    task="$1"
    load_task_project "$task" || exit 1
    where="$(task_where "$task")"
    commits="[]" head="" branch="box/$task"
    if [ "$where" = "mac" ]; then
      branch="$(lavoro_git "$task" rev-parse --abbrev-ref HEAD 2>/dev/null)" || branch=""
      commits="$(lavoro_git "$task" log --max-count="$NATIVE_MAX_COMMITS" --format="$GRAPH_FORMAT" "$(task_base_sha "$task")..HEAD" 2>/dev/null \
        | jq -Rs "$GRAPH_LOG_RECORDS" 2>/dev/null | jq -c "$GRAPH_COMMITS_FILTER" 2>/dev/null)" || commits=""
      head="$(lavoro_git "$task" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null)" || head=""
    elif head="$(workspace_verified_head "$task" 2>/dev/null)"; then
      commits="$(sandbox_json "$GRAPH_COMMITS_FILTER" verified_sandbox "$task" -- /box/graph.py "$(task_base_sha "$task")" "$head")" || commits=""
    fi
    jq -nc --arg lavoro "$task" --arg branch "$branch" --arg where "$where" --arg head "$head" --arg base "$(task_base_sha "$task")" \
      --argjson commits "${commits:-[]}" --argjson readable "$([ -n "$head" ] && [ -n "$commits" ] && echo true || echo false)" \
      '{lane: {lavoro: $lavoro, branch: $branch, where: $where, head: (if $head == "" then null else $head end), base: $base, readable: $readable}, commits: $commits}'
  )
}

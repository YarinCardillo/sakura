# shellcheck shell=bash
# Everything the host needs to know about an untrusted workspace is read by sandbox containers:
# no network, no capabilities, read-only root. The host never runs git inside work/<task>. What
# comes back is JSON written by a real encoder in the sandbox and re-validated by jq before it
# is used.
#
# git that reads the workspace can be made to run a program the agent chose (a clean filter
# named by its .git/config and .git/info/attributes runs during status and diff). So there are
# three kinds of container, and none of them holds both the workspace and a writable store:
# - workspace (workspace_sandbox): the workspace read-only at /src, nothing else. What it
#   prints is the agent's word: the head it names is used only once verified, the rest is shown.
# - import (import_commit): the verified store writable and the trusted source, no workspace.
#   Its only input is a pack, piped from a workspace container; every object is re-hashed.
# - verified (verified_sandbox): the verified store read-only and the trusted source, no
#   workspace. What decisions rest on is computed here, from commits the host names.

readonly SANDBOX_TIMEOUT_SECONDS=180
readonly MAX_SANDBOX_JSON_BYTES=1000000
# The most a pack of a task's commits may weigh on its way into the verified store.
readonly MAX_PACK_BYTES="${BOX_MAX_PACK_BYTES:-1073741824}"
readonly ZERO_SHA=0000000000000000000000000000000000000000
RUNNER_SCRIPTS="$BOX_ROOT/image/runner"
# The verified object store of each task: written only by import containers, one directory per
# task so that tasks never share objects. The workspace's own objects are never an alternate.
readonly VERIFIED_DIR="$AGENTBOX_HOME/verified"
readonly VERIFIED_MARKER_DIRECTORY="box-verified"

# Fills SANDBOX_FLAGS (everything except the mounts and the command).
build_sandbox_flags() {
  SANDBOX_FLAGS=(
    --rm --network none --user agent --cap-drop ALL --security-opt no-new-privileges
    --log-driver none --read-only --tmpfs "/tmp:size=64m,nr_inodes=65536,mode=1777" -e HOME=/tmp
    --memory 1g --memory-swap 1g --cpus 1 --pids-limit 256
    --label agentbox=1 --label agentbox.kind=sandbox
    -v "$RUNNER_SCRIPTS:/box:ro" -e PYTHONDONTWRITEBYTECODE=1
  )
}

# The source repository of a task's project (the person's trusted repository).
task_repo_path() {
  (load_task_project "$1" && echo "$REPO")
}

# The verified object store of a task. (A <task>.git beside it is the store of an older box,
# which workspace containers could write: it is not used any more.)
task_verified_dir() {
  echo "$VERIFIED_DIR/$1.store"
}

# workspace_sandbox <task> <extra docker flags...> -- <script and arguments>: runs a script of
# image/runner (/box/<name>) with the workspace of <task> read-only at /src and nothing else.
workspace_sandbox() {
  local task="$1" extra=()
  require_box_task "$task"
  shift
  while [ "$1" != "--" ]; do extra+=("$1"); shift; done
  shift
  build_sandbox_flags
  # The lifecycle lock of the task is held, shared, from before the task is looked at until
  # the docker client has ended: the task cannot be removed or replaced by one of the other
  # place between the look and the mount.
  (
    bind_box_task "$task" || { warn "task '$task' is being removed, made or started; its workspace was not read"; exit 1; }
    require_box_task "$task"
    [ -d "$(task_dir "$task")" ] || exit 1
    run_with_timeout "$SANDBOX_TIMEOUT_SECONDS" docker run "${SANDBOX_FLAGS[@]}" --label "agentbox.task=$task" \
      ${extra[@]+"${extra[@]}"} -v "$(task_dir "$task"):/src:ro" "$IMAGE" "$@"
  )
}

# verified_sandbox <task> <extra docker flags...> -- <script and arguments>: runs a script with
# the verified store of <task> read-only at /verified and the trusted source repository
# read-only at /trusted. The workspace is not mounted.
verified_sandbox() (
  local task="$1" extra=() repo
  require_box_task "$task"
  shift
  while [ "$1" != "--" ]; do extra+=("$1"); shift; done
  shift
  bind_box_task "$task" || return 1
  repo="$(task_repo_path "$task")" || return 1
  build_sandbox_flags
  run_with_timeout "$SANDBOX_TIMEOUT_SECONDS" docker run "${SANDBOX_FLAGS[@]}" --label "agentbox.task=$task" \
    ${extra[@]+"${extra[@]}"} -v "$(task_verified_dir "$task"):/verified:ro" -v "$repo:/trusted:ro" "$IMAGE" "$@"
)

# Succeeds when an import container has verified the commit for the task. The marker is a file
# of the store, which only import containers write.
commit_is_verified() {
  is_valid_sha "$2" && [ -f "$(task_verified_dir "$1")/$VERIFIED_MARKER_DIRECTORY/$2" ]
}

# import_commit <task> <sha>: makes a commit of the workspace a verified commit of the task,
# once. A workspace container packs the commits after the base; the pack, cut at
# MAX_PACK_BYTES, is piped into an import container, which re-hashes every object.
import_commit() (
  local task="$1" sha="$2" repo base verified
  bind_box_task "$task" || return 1
  commit_is_verified "$task" "$sha" && return 0
  is_valid_sha "$sha" || return 1
  repo="$(task_repo_path "$task")" || return 1
  base="$(task_base_sha "$task")"
  verified="$(task_verified_dir "$task")"
  mkdir -p "$verified"
  build_sandbox_flags
  workspace_sandbox "$task" -- /box/export_pack.py "$base" "$sha" 2>/dev/null | head -c "$MAX_PACK_BYTES" \
    | run_with_timeout "$SANDBOX_TIMEOUT_SECONDS" docker run -i "${SANDBOX_FLAGS[@]}" --label "agentbox.task=$task" \
      -v "$verified:/verified" -v "$repo:/trusted:ro" "$IMAGE" /box/verify_import.py "$base" "$sha" >/dev/null 2>&1 || return 1
  commit_is_verified "$task" "$sha"
)

# Strips everything but printable ASCII.
sanitize() {
  LC_ALL=C tr -cd '[:print:]'
}

require_jq() {
  command -v jq >/dev/null 2>&1 || die "jq is required (brew install jq)"
}

# jq filters for what a workspace container (facts.py) and a verified container
# (commit_facts.py) print about a task. Types and patterns are enforced here, strings are
# length-limited and stripped of control characters; if anything is off a filter prints nothing
# and the workspace counts as unreadable.
readonly WORKSPACE_FACTS_FILTER='
  def sha: select(type == "string" and test("^[0-9a-f]{40}$"));
  def count: select(type == "number" and . >= 0 and . == floor);
  {
    head: (.head | sha),
    branch: (.branch | select(type == "string" and test("^[A-Za-z0-9._/-]{1,100}$")) // "?"),
    dirtyFiles: (.dirtyFiles | count),
    filesChanged: (.filesChanged | count),
    insertions: (.insertions | count),
    deletions: (.deletions | count),
    key: (.key | if type == "string" and test("^head=[0-9a-f]{40}( (-|[0-9]+:[0-9]+)){4}$") then . else null end)
  }'
readonly COMMIT_FACTS_FILTER='
  def count: select(type == "number" and . >= 0 and . == floor);
  def text($n): (. // "") | tostring | gsub("[\u0001-\u001f\u007f]"; " ") | .[0:$n];
  def zulu: select(type == "string" and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"));
  { headSubject: (.headSubject | text(200)), headTime: (.headTime | zulu), ahead: (.ahead | count) }'
readonly FACTS_ORDER_FILTER='{head: $w.head, branch: $w.branch, headSubject: $c.headSubject, headTime: $c.headTime,
  ahead: $c.ahead, dirtyFiles: $w.dirtyFiles, filesChanged: $w.filesChanged, insertions: $w.insertions, deletions: $w.deletions}'

# jq filter for log.py output: an array of at most 200 validated commit objects.
readonly LOG_FILTER='
  def sha: select(type == "string" and test("^[0-9a-f]{40}$"));
  def count: select(type == "number" and . >= 0 and . == floor);
  def zulu: select(type == "string" and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"));
  [ .[:200][] | {
      sha: (.sha | sha),
      subject: ((.subject // "") | tostring | gsub("[\u0001-\u001f\u007f]"; " ") | .[0:200]),
      time: (.time | zulu),
      filesChanged: (.filesChanged | count), insertions: (.insertions | count), deletions: (.deletions | count)
  } ]'

# sandbox_json <filter> <command...>: the output of a sandbox command, cut and validated.
sandbox_json() {
  local filter="$1" raw json
  shift
  raw="$("$@" 2>/dev/null | head -c "$MAX_SANDBOX_JSON_BYTES")" || return 1
  json="$(printf '%s' "$raw" | jq -c "$filter" 2>/dev/null)" && [ -n "$json" ] || return 1
  echo "$json"
}

# read_workspace <task> [<cached key> <moment of the cached read>]: what a workspace container
# says about the workspace, validated: where HEAD is, what the working tree holds and the key
# for the facts cache. With a cached key the container answers {"unchanged":true} instead,
# without running git, when the workspace is as it was at that read.
read_workspace() {
  local task="$1"
  shift
  sandbox_json 'if .unchanged == true then {unchanged: true} else '"$WORKSPACE_FACTS_FILTER"' end' \
    workspace_sandbox "$task" -- /box/facts.py "$(task_base_sha "$task")" "$task" "$@"
}

# complete_facts <task> <what read_workspace printed>: the facts of a task as one JSON object.
# The commit the workspace named is verified first, and what is said about the commit comes
# from the verified store.
complete_facts() {
  local task="$1" worktree="$2" head commit
  head="$(jq -r '.head' <<<"$worktree")"
  import_commit "$task" "$head" || return 1
  commit="$(sandbox_json "$COMMIT_FACTS_FILTER" verified_sandbox "$task" -- /box/commit_facts.py "$(task_base_sha "$task")" "$head")" || return 1
  jq -nc --argjson w "$worktree" --argjson c "$commit" "$FACTS_ORDER_FILTER"
}

# Prints the validated facts of a workspace as one JSON object; fails when unreadable.
workspace_facts_json() {
  local task="$1" worktree
  require_jq
  worktree="$(read_workspace "$task")" || return 1
  complete_facts "$task" "$worktree"
}

# Prints the commit HEAD names in a workspace, once it is a verified commit of the task.
workspace_verified_head() {
  local task="$1" head
  head="$(workspace_sandbox "$task" -- /box/head.py 2>/dev/null | head -c 41 | sanitize)" || return 1
  import_commit "$task" "$head" || return 1
  echo "$head"
}

# Prints the validated commit list (JSON array) of a workspace; fails when unreadable.
workspace_log_json() {
  local task="$1" head
  require_jq
  head="$(workspace_verified_head "$task")" || return 1
  sandbox_json "$LOG_FILTER" verified_sandbox "$task" -- /box/log.py "$(task_base_sha "$task")" "$head"
}

# Fills FACT_HEAD, FACT_BRANCH, FACT_AHEAD and FACT_DIRTY: from a sandbox read of the workspace
# of a task in a box, with plain git from the worktree of one on the Mac.
workspace_facts() {
  local task="$1" json
  if task_is_on_mac "$task"; then
    json="$(native_facts_json "$task")" || die "could not read the worktree of task '$task'"
  else
    json="$(workspace_facts_json "$task")" || die "could not read the workspace of task '$task' (is Docker running?)"
  fi
  FACT_HEAD="$(jq -r '.head' <<<"$json")"
  FACT_BRANCH="$(jq -r '.branch' <<<"$json")"
  FACT_AHEAD="$(jq -r '.ahead' <<<"$json")"
  FACT_DIRTY="$(jq -r '.dirtyFiles' <<<"$json")"
}

# Copies package.json and the lockfile of a commit into a staging directory and prints the
# hash of the two files (validated: the 64 hex digits of a whole SHA-256).
extract_package_files() {
  local task="$1" sha="$2" stage="$3" output hash
  import_commit "$task" "$sha" || return 1
  output="$(verified_sandbox "$task" -v "$stage:/out" -- /box/extract.sh "$sha" "$SUBDIR" 2>/dev/null)" || return 1
  hash="$(printf '%s' "$output" | sed -n 's/^files_hash=//p' | sanitize)"
  [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || return 1
  echo "$hash"
}

# tree_problem <task> <sha>: prints why the tree of the commit cannot be checked out as it is on
# the filesystem of the person (names that differ only in letter case or composition, names a
# filesystem takes for .git: see image/runner/tree_check.py); nothing when it can. A tree that
# cannot be examined counts as one with a problem.
readonly TREE_UNEXAMINED="the paths of the commit could not be examined"
tree_problem() {
  local task="$1" sha="$2" answer
  import_commit "$task" "$sha" || { echo "$TREE_UNEXAMINED"; return 0; }
  answer="$(verified_sandbox "$task" -- /box/tree_check.py "$sha" 2>/dev/null | head -c 600 | sanitize)" || answer=""
  case "$answer" in
    ok) ;;
    "refused: "*) echo "${answer#refused: }" ;;
    *) echo "$TREE_UNEXAMINED" ;;
  esac
}

# create_bundle <task> <sha> <private directory>: writes one operation's bundle from the
# verified store. Only that operation can see or remove its output.
create_bundle() {
  local task="$1" sha="$2" directory="$3"
  import_commit "$task" "$sha" || return 1
  verified_sandbox "$task" -v "$directory:/out" -- /box/bundle.sh "$task" "$(task_base_sha "$task")" "$sha" >/dev/null
}

# shellcheck shell=bash
# What box reads of a lavoro on the Mac beyond its facts: its commits and its patch, with plain
# git (lavoro_git), in the shapes the same verbs print for a task in a box.

readonly EMPTY_TREE=4b825dc642cb6eb9a060e54bf8d69288fbee4904
readonly NATIVE_MAX_COMMITS=200
readonly NATIVE_MAX_UNTRACKED=1000
readonly NATIVE_MAX_DIFF_BYTES=8388608
readonly NATIVE_DIFF_OPTIONS=(--no-ext-diff --no-textconv --no-color -M)

# Turns "git log --numstat" records (one per commit, started by the record separator) into the
# array LOG_FILTER validates.
readonly NATIVE_LOG_RECORDS='
  [ split("\u001e")[1:][] | split("\n") as $lines | ($lines[0] | split("\u001f")) as $head
    | [ $lines[1:][] | select(test("^[0-9-]+\t[0-9-]+\t")) | split("\t") ] as $rows
    | { sha: $head[0], subject: ($head[2:] | join(" ")), time: ($head[1] | tonumber | todate),
        filesChanged: ($rows | length),
        insertions: ($rows | map(.[0] | tonumber? // 0) | add // 0),
        deletions: ($rows | map(.[1] | tonumber? // 0) | add // 0) } ]'

# Loads the project of a lavoro on the Mac and checks its worktree, or ends with the reason.
require_checked_lavoro() {
  load_task_project "$1"
  checked_lavoro "$1" || die "$LAVORO_PROBLEM"
}

# Prints the commits of a lavoro after its base, newest first, at most 200, as a JSON array.
native_log_json() {
  local task="$1"
  lavoro_git "$task" log --max-count="$NATIVE_MAX_COMMITS" --diff-merges=first-parent --no-ext-diff --no-textconv --numstat \
    --format='%x1e%H%x1f%ct%x1f%s' "$(task_base_sha "$task")..HEAD" 2>/dev/null \
    | jq -Rs "$NATIVE_LOG_RECORDS" 2>/dev/null | jq -c "$LOG_FILTER" 2>/dev/null
}

# git for a lavoro, started in its worktree (for the one command that takes paths of the file
# system); the git directory and the work tree are still named explicitly.
lavoro_git_in_tree() {
  local task="$1"
  shift
  local git
  checked_lavoro "$task" || return 1
  # Found before the change of folder, in the PATH box trusts.
  git="$(trusted_program git)" || return 1
  (cd "$LAVORO_DIR" && person_environment "$git" "${PERSON_GIT_OPTIONS[@]}" --git-dir="$LAVORO_GIT_DIR" --work-tree="$LAVORO_DIR" "$@")
}

# The patch of a lavoro against the commit it left the base at: what is committed, what is
# not, and every file git does not know yet as a new file (plain files and links only).
native_working_patch() {
  local task="$1" base merge_base path count=0
  base="$(task_base_sha "$task")"
  merge_base="$(lavoro_git "$task" merge-base "$base" HEAD 2>/dev/null)" || merge_base="$base"
  lavoro_git "$task" diff "${NATIVE_DIFF_OPTIONS[@]}" "$merge_base" || return 1
  while IFS= read -r -d '' path; do
    count=$((count + 1))
    [ "$count" -le "$NATIVE_MAX_UNTRACKED" ] || break
    [ -f "$LAVORO_DIR/$path" ] || [ -L "$LAVORO_DIR/$path" ] || continue
    lavoro_git_in_tree "$task" diff --no-index "${NATIVE_DIFF_OPTIONS[@]}" -- /dev/null "$path" || true
  done < <(lavoro_git "$task" ls-files --others --exclude-standard -z)
}

# The patch of one commit of the lavoro against its first parent.
native_commit_patch() {
  local task="$1" commit="$2" parent
  lavoro_git "$task" rev-list --max-count="$NATIVE_MAX_COMMITS" "$(task_base_sha "$task")..HEAD" 2>/dev/null | grep -qx "$commit" || return 1
  parent="$(lavoro_git "$task" rev-parse --verify --quiet "$commit^1" 2>/dev/null)" || parent="$EMPTY_TREE"
  lavoro_git "$task" diff "${NATIVE_DIFF_OPTIONS[@]}" "$parent" "$commit"
}

# box diff <task> [--commit <sha>] for a lavoro on the Mac: the patch, cut at its limit.
native_diff() {
  local task="$1" commit="$2" status
  require_checked_lavoro "$task"
  if [ -z "$commit" ]; then
    ! native_working_patch "$task" | bounded_log "$NATIVE_MAX_DIFF_BYTES"
    status=("${PIPESTATUS[@]}")
    [ "${status[0]}" -eq 0 ] || die "could not produce the diff of task '$task'"
    return 0
  fi
  ! native_commit_patch "$task" "$commit" | bounded_log "$NATIVE_MAX_DIFF_BYTES"
  status=("${PIPESTATUS[@]}")
  [ "${status[0]}" -eq 0 ] || die "could not produce the diff of task '$task' (is $commit one of the commits box log lists?)"
}

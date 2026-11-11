# shellcheck shell=bash
# A lavoro on the Mac: a git worktree of the project's repository at $AGENTBOX_HOME/work/<task>,
# on its own branch, with the agent running natively as the person. This is the person's own
# repository and the person's own agent: the machinery for untrusted workspaces (sandbox
# containers, the verified store, take) does not apply. box reads it with plain git, through the
# same hardened wrapper as the repository itself (person_git: no hooks, no pager, no external
# tools), and nothing is started for a read beyond git.
#
# The two places never mix: nothing here is called for a task in a box (lavoro_git refuses one),
# and nothing of the box machinery is called for a lavoro on the Mac (require_box_task).

readonly DEFAULT_BRANCH_PREFIX="sakura/"

# The prefix of the branches of the lavori of the loaded project.
project_branch_prefix() {
  echo "${BRANCH_PREFIX:-$DEFAULT_BRANCH_PREFIX}"
}

# The branch of a lavoro on the Mac. Needs the project loaded.
lavoro_branch() {
  echo "$(project_branch_prefix)$1"
}

# ---- the worktree, and how host git is pointed at it ---------------------------------------------
# Host git never finds the repository of a lavoro by looking in its worktree. It is given the
# git directory and the work tree explicitly, from trusted values: the folder of the lavoro and
# the git directory box recorded when it created the worktree (worktrees/<name> in the
# repository of the project, which is checked like the repository itself). Before every call the
# .git file of the worktree must still name exactly that directory.

# Sets LAVORO_PROBLEM and fails.
refuse_lavoro() {
  LAVORO_PROBLEM="$1"
  return 1
}

# Prints the path the .git file of a worktree names, read here without git: a plain file that
# holds the one line "gitdir: <path>" and nothing else.
worktree_pointer() {
  local file="$1/.git" line="" rest=""
  [ -f "$file" ] && [ ! -L "$file" ] || return 1
  { IFS= read -r -n "$GIT_POINTER_MAX_BYTES" line; IFS= read -r -n 1 rest; } <"$file" || true
  [ -z "$rest" ] && [[ "$line" == "gitdir: "?* ]] || return 1
  echo "${line#gitdir: }"
}

# checked_lavoro <task>: succeeds with LAVORO_DIR and LAVORO_GIT_DIR set when host git may be
# run for the lavoro; fails with LAVORO_PROBLEM set. Needs the project of the task loaded. The
# pointer is compared on every call; the recorded directory is examined once per process.
checked_lavoro() {
  local task="$1" dir recorded
  task_is_on_mac "$task" || { refuse_lavoro "task '$task' is in a box: host git never runs in its workspace"; return 1; }
  dir="$(task_dir "$task")"
  recorded="$(task_record_field "$task" WORKTREE_GIT_DIR)"
  [ -n "$recorded" ] || { refuse_lavoro "task '$task' has no recorded git directory"; return 1; }
  [ -d "$dir" ] && [ ! -L "$dir" ] || { refuse_lavoro "the folder of task '$task' is missing or is a link ($dir)"; return 1; }
  [ "$(worktree_pointer "$dir")" = "$recorded" ] \
    || { refuse_lavoro "the .git of the worktree of task '$task' does not name the git directory box recorded for it ($recorded)"; return 1; }
  if [[ " ${CHECKED_LAVORI:-} " != *" $task "* ]]; then
    check_recorded_git_dir "$recorded" || { refuse_lavoro "task '$task': $REPOSITORY_PROBLEM"; return 1; }
    check_worktree_identity "$task" "$dir" || { refuse_lavoro "the folder of task '$task' is not the folder box made for it ($dir)"; return 1; }
    CHECKED_LAVORI="${CHECKED_LAVORI:-} $task"
  fi
  LAVORO_DIR="$dir" LAVORO_GIT_DIR="$recorded"
}

# The folder of a lavoro is known by its device and inode, recorded when it was made: another
# folder under the same name is not the lavoro. Sets LAVORO_IDENTITY, which the read scripts
# compare with the folder they open. A lavoro made before the identity was recorded is known by
# the folder as it is at this check.
check_worktree_identity() {
  local task="$1" dir="$2" recorded
  LAVORO_IDENTITY="$(file_identity "$dir" 2>/dev/null)" && [ -n "$LAVORO_IDENTITY" ] || return 1
  recorded="$(task_record_field "$task" WORKTREE_IDENTITY)"
  [ -z "$recorded" ] || [ "$recorded" = "$LAVORO_IDENTITY" ]
}

# The recorded git directory must be a directory worktrees/<name> of the repository of the
# loaded project, plain like the rest of it, with that repository as its common directory.
check_recorded_git_dir() {
  local recorded="$1" repo common managed
  repo="$(physical_directory "$REPO")" || { refuse_repository "the repository of its project does not exist"; return 1; }
  check_git_entry "$repo/.git" || return 1
  common="$REPOSITORY_COMMON_DIR"
  [ -d "$recorded" ] && [ ! -L "$recorded" ] && same_directory "${recorded%/*}" "$common/worktrees" \
    || { refuse_repository "its git directory is not a worktree of the repository of its project"; return 1; }
  managed="$(managed_identities)"
  check_git_directory "$recorded" "$managed" || return 1
  [ -f "$recorded/HEAD" ] || { refuse_repository "its git directory has no HEAD"; return 1; }
  same_directory "$(pointed_directory "$recorded/commondir" "" "$recorded")" "$common" \
    || { refuse_repository "its git directory does not belong to the repository of its project"; return 1; }
  check_no_includes "$recorded"
}

# git for a lavoro on the Mac: never started in the worktree, always told where the git
# directory and the work tree are.
lavoro_git() {
  local task="$1"
  shift
  checked_lavoro "$task" || return 1
  person_git --git-dir="$LAVORO_GIT_DIR" --work-tree="$LAVORO_DIR" "$@"
}

# new_on_mac <task> <project> <base branch>: the worktree and the branch, from the base. A branch
# of that name that already exists is never taken over.
new_on_mac() {
  local task="$1" project="$2" base="$3" branch base_sha dir git_dir
  dir="$(task_dir "$task")"
  branch="$(lavoro_branch "$task")"
  person_git check-ref-format --branch "$branch" >/dev/null 2>&1 || die "'$branch' is not a branch name git accepts"
  ! has_local_branch "$REPO" "$branch" \
    || die "the branch $branch already exists in $REPO: choose another name for the lavoro, or remove that branch yourself"
  base_sha="$(person_git -C "$REPO" rev-parse --verify --quiet "refs/heads/$base^{commit}")" \
    || die "could not read the branch '$base' of $REPO"
  mkdir -p "$WORK_DIR"
  person_git -C "$REPO" worktree add --quiet -b "$branch" "$dir" "$base_sha" >&2 \
    || die "could not create the worktree of task '$task' in $dir"
  git_dir="$(worktree_pointer "$dir")" && check_recorded_git_dir "$git_dir" \
    || die "the worktree git made in $dir is not one box can use (${REPOSITORY_PROBLEM:-its .git is not a pointer}); remove it with: git -C $REPO worktree remove --force $dir"
  record_task "$task" "$project" "$base_sha" "$base" mac "$git_dir" "$(file_identity "$dir")"
  write_task_project_snapshot "$task" || die "could not keep the project values of task '$task'"
  mkdir -p "$(task_results_dir "$task")"
  run_setup "$task"
  echo "task '$task' ready: $dir (branch $branch, a worktree of $REPO)"
}

# ---- facts ------------------------------------------------------------------------------------

# The facts of a worktree from the output of a few git commands, in the shape of the facts of a
# box (see FACTS_ORDER_FILTER); every value is typed and bounded here.
readonly NATIVE_FACTS_FILTER='
  ($commit | split("\u001f")) as $c
  | ($numstat | split("\n") | map(select(length > 0) | split("\t"))) as $rows
  | def count: tonumber | select(. >= 0 and . == floor);
  { head: ($head | select(test("^[0-9a-f]{40}$"))),
    branch: ($branch | if test("^[A-Za-z0-9._/-]{1,100}$") then . else "?" end),
    headSubject: ($c[1:] | join(" ") | gsub("[\u0001-\u001f\u007f]"; " ") | .[0:200]),
    headTime: ($c[0] | tonumber | todate),
    ahead: ($ahead | count), dirtyFiles: ($dirty | count),
    filesChanged: ($rows | length),
    insertions: ($rows | map(.[0] | tonumber? // 0) | add // 0),
    deletions: ($rows | map(.[1] | tonumber? // 0) | add // 0) }'

# Prints the facts of a lavoro on the Mac as one JSON object; fails when the worktree cannot be
# read. The figures of the working tree are against the commit the branch left the base at.
native_facts_json() {
  local task="$1" base head merge_base
  checked_lavoro "$task" || return 1
  base="$(task_base_sha "$task")"
  head="$(lavoro_git "$task" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null)" || return 1
  merge_base="$(lavoro_git "$task" merge-base "$base" "$head" 2>/dev/null)" || merge_base="$base"
  jq -nc --arg head "$head" --arg branch "$(lavoro_git "$task" rev-parse --abbrev-ref HEAD 2>/dev/null)" \
    --arg commit "$(lavoro_git "$task" log -1 --format='%ct%x1f%s' "$head" 2>/dev/null)" \
    --arg ahead "$(lavoro_git "$task" rev-list --count "$base..$head" 2>/dev/null)" \
    --arg dirty "$(lavoro_git "$task" status --porcelain -z 2>/dev/null | tr -cd '\0' | wc -c | tr -d ' ')" \
    --arg numstat "$(lavoro_git "$task" diff --numstat --no-ext-diff --no-textconv "$merge_base" 2>/dev/null)" \
    "$NATIVE_FACTS_FILTER" 2>/dev/null
}

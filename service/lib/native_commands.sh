# shellcheck shell=bash
# The commands of a project that run natively for a lavoro on the Mac, as the person: setup
# (once, when the lavoro is created) and archive (before it is removed). Each runs in the folder
# of the project inside the worktree, in its own process group, with a deadline, its output in
# a bounded log of the data directory, and leaves no process behind (native_run.pl).

readonly NATIVE_RUN="$BOX_ROOT/lib/native_run.pl"
readonly SETUP_LOG_DIR="$AGENTBOX_HOME/setup"
readonly SETUP_MAX_LOG_BYTES=4194304
readonly SETUP_LOG_LINES=400

setup_log() {
  echo "$SETUP_LOG_DIR/$1.log"
}

archive_log() {
  echo "$SETUP_LOG_DIR/$1.archive.log"
}

# The folder of the project inside the worktree of a lavoro. Needs the project loaded.
lavoro_folder() {
  echo "$(task_dir "$1")$(subdir_suffix)"
}

# lavoro_environment <task>: fills LAVORO_ENVIRONMENT with what a command of the project gets:
# the person's own PATH, and what it may want to know about the lavoro (the CONDUCTOR_ names
# carry the same values, for scripts written for that tool). Called before box changes into
# the worktree: nothing is looked up or computed once it stands there.
lavoro_environment() {
  local task="$1" dir base
  dir="$(task_dir "$task")"
  base="$(task_base_branch "$task")"
  LAVORO_ENVIRONMENT=(
    "PATH=$PERSON_PATH" "SAKURA_LAVORO=$task" "SAKURA_WORKTREE=$dir" "SAKURA_REPO=$REPO" "SAKURA_BASE=$base"
    "CONDUCTOR_WORKSPACE_NAME=$task" "CONDUCTOR_WORKSPACE_PATH=$dir" "CONDUCTOR_ROOT_PATH=$REPO"
    "CONDUCTOR_DEFAULT_BRANCH=$base"
  )
}

# ---- records of process groups -------------------------------------------------------------------
# native_run.pl writes, for a group box may have to end later (Play, the conversation, a native
# check), a record of three lines: the id of the group, the boot of the machine and the start
# of its leader. A number alone says nothing after a crash or a reboot: it may have been given
# to another process since. So nothing is signalled through a record unless all three are what
# the machine says now; a record that does not match is discarded.

# Both are read as native_run.pl reads them: with programs named by their absolute paths (the
# app starts box with a PATH that has no /usr/sbin), from /proc where there are no such programs.
boot_time() {
  if [ -r /proc/sys/kernel/random/boot_id ]; then
    tr -d '[:space:]' </proc/sys/kernel/random/boot_id
  else
    /usr/sbin/sysctl -n kern.boottime 2>/dev/null | sed -n 's/^{ sec = \([0-9]*\),.*/\1/p'
  fi
}

process_start() {
  if [ -r "/proc/$1/stat" ]; then
    sed 's/^.*) //' "/proc/$1/stat" 2>/dev/null | cut -d' ' -f20
  else
    LC_ALL=C /bin/ps -p "$1" -o lstart= 2>/dev/null | sed 's/^ *//; s/ *$//'
  fi
}

# recorded_group <record>: prints the process group the record names when it still names the
# process it was written for; fails otherwise, and removes a record that does not match.
recorded_group() {
  local record="$1" group="" boot="" start=""
  [ -f "$record" ] || return 1
  { IFS= read -r group; IFS= read -r boot; IFS= read -r start; } <"$record" || true
  if [[ "$group" =~ ^[0-9]{1,10}$ ]] && [ "$group" -gt 1 ] && [ -n "$boot" ] && [ "$boot" = "$(boot_time)" ] \
    && [ -n "$start" ] && [ "$start" = "$(process_start "$group")" ]; then
    echo "$group"
    return 0
  fi
  rm -f "$record"
  return 1
}

# kill_if_still_recorded <record> <group>: the kill that follows an asking. Between the asking
# and now the group can have ended and its number gone to another process, so the record is
# compared with the machine again here, immediately before the signal: when it no longer names
# this group, nothing is killed (what is left of a group whose leader has gone is ended by its
# own supervisor, native_run.pl, which still holds the leader and so its number).
kill_if_still_recorded() {
  [ "$(recorded_group "$1")" = "$2" ] || return 0
  kill -KILL -- "-$2" 2>/dev/null || true
}

# run_native <task> <seconds> <pid file or -> <command text> [NAME=value...]: the command, run by
# bash in the folder of the project, with the deadline; its exit code. env, perl and bash are
# started by their absolute paths: the folder is the agent's and the PATH is the person's. (The
# command itself is the project's own and finds its programs as the person's shell would.) The locks of box are not
# passed on to it.
run_native() {
  local task="$1" seconds="$2" pid_file="$3" command="$4" folder
  shift 4
  checked_lavoro "$task" || { echo "[box] $LAVORO_PROBLEM"; return 1; }
  folder="$(lavoro_folder "$task")"
  [ -d "$folder" ] || { echo "[box] the folder of the project is not in the worktree: $folder"; return 1; }
  lavoro_environment "$task"
  (cd "$folder" && "$TRUSTED_ENV" "${LAVORO_ENVIRONMENT[@]}" "$@" "$TRUSTED_PERL" "$NATIVE_RUN" "$seconds" "$pid_file" "$TRUSTED_BASH" -c "$command") 5>&- 6>&- 7>&- 8>&- 9>&-
}

# logged_native <task> <command text> <log>: run_native with the install deadline and the output
# in the log, whole or cut at its limit. Returns the exit code of the command.
logged_native() {
  local task="$1" command="$2" log="$3"
  mkdir -p "$SETUP_LOG_DIR"
  logged "$log.done" "$SETUP_MAX_LOG_BYTES" run_native "$task" "$INSTALL_TIMEOUT_SECONDS" - "$command" >"$log"
}

# The setup command of the project, once, right after the worktree was made.
run_setup() {
  local task="$1" status=0
  [ -n "${SETUP:-}" ] || return 0
  logged_native "$task" "$SETUP" "$(setup_log "$task")" || status=$?
  [ "$status" -eq 0 ] || die "the setup command of task '$task' failed (exit $status$(deadline_note "$status")); the lavoro is kept. Its output: box log $task --setup. To remove the lavoro: box rm $task --force"
}

# The archive command of the project, before the worktree goes. Returns its exit code.
run_archive() {
  local task="$1"
  [ -n "${ARCHIVE:-}" ] || return 0
  logged_native "$task" "$ARCHIVE" "$(archive_log "$task")"
}

deadline_note() {
  [ "$1" -ne "$TIMEOUT_EXIT_CODE" ] || echo ": it did not end within $INSTALL_TIMEOUT_SECONDS s and was ended"
}

# box log <task> --setup: the end of the output of the setup command.
print_setup_log() {
  local task="$1"
  refuse_in_box "$task" "box log --setup" "a task in a box has no setup command (box new installs its dependencies)"
  [ -f "$(setup_log "$task")" ] || { warn "no setup command ran for task '$task'"; return "$NOT_FOUND_EXIT_CODE"; }
  tail -n "$SETUP_LOG_LINES" "$(setup_log "$task")" | LC_ALL=C tr -cd '\n\t[:print:]'
}

# ---- box rm -----------------------------------------------------------------------------------

# box rm of a lavoro on the Mac: refuses while work could be lost, runs the archive command,
# cuts off its requests, removes the worktree and the records. The branch is never deleted.
rm_on_mac() {
  local task="$1" force="$2" status=0
  if [ -e "$(task_dir "$task")" ] || [ -L "$(task_dir "$task")" ]; then
    require_own_worktree "$task"
    if [ "$force" != "--force" ]; then refuse_if_unsafe_to_remove_on_mac "$task"; fi
    settle_open_shells "$task" "$force"
    run_archive "$task" || status=$?
    [ "$status" -eq 0 ] || [ "$force" = "--force" ] || die "the archive command of task '$task' failed (exit $status$(deadline_note "$status")); the lavoro is kept. Its output is in $(archive_log "$task"). box rm $task --force removes it all the same"
    [ "$status" -eq 0 ] || warn "the archive command of task '$task' failed (exit $status); the lavoro is removed all the same"
  fi
  with_queue_lock retire_task_locked "$task"
  drain_task_requests "$task"
  with_lock "$LOCK_DIR/play.lock" forget_play "$task"
  end_conversation "$task"
  remove_worktree "$task"
  rm -rf "${RESULTS_DIR:?}/$task" "$(task_retiring_mark "$task")" "${TASKS_DIR:?}/$task.env" "${TASKS_DIR:?}/$task.project" \
    "$(setup_log "$task")" "$(archive_log "$task")" "${SHELLS_DIR:?}/$task".*
  journal_event rm "$task"
  echo "task '$task' removed (its branch stays in the repository)"
}

# box rm deletes a folder only when it is the worktree box made for the lavoro: its .git names
# the recorded git directory, which belongs to the project's repository (checked_lavoro). With
# --force too. Whatever else stands at the path is the person's and is left alone.
require_own_worktree() {
  local task="$1" problem
  if problem="$( (load_task_project "$task") 2>&1)"; then
    load_task_project "$task"
    checked_lavoro "$task" && return 0
    problem="$LAVORO_PROBLEM"
  fi
  die "box rm does not delete $(task_dir "$task"): it is not known to be the worktree of task '$task' (${problem#box: }). Move it away if it is not; box rm $task then removes the records"
}

# Uncommitted files, or commits that are in no remote branch and not in the base branch: the
# person would have to look for them on a branch nobody shows any more.
refuse_if_unsafe_to_remove_on_mac() {
  local task="$1" unpublished base
  workspace_facts "$task"
  [ "$FACT_DIRTY" -eq 0 ] || die "task '$task' has $FACT_DIRTY uncommitted file(s); commit them or use --force"
  base="$(task_base_branch "$task")"
  unpublished="$(lavoro_git "$task" rev-list --count HEAD --not --remotes "refs/heads/$base" 2>/dev/null)" \
    || die "could not tell whether the commits of task '$task' are pushed or merged (is '$base' still a branch?); use --force to remove it all the same"
  [ "$unpublished" -eq 0 ] || die "task '$task' has $unpublished commit(s) that are neither pushed nor merged into $base; push or merge them, or use --force (the branch stays in the repository either way)"
}

# Removes the worktree of a lavoro. Called last, after the archive command has run and the
# conversation and Play have been ended: whatever could still write has stopped. What stands at
# the path is looked at again now (require_own_worktree), then moved into a folder of box by
# its name (a rename moves a link, never what it leads to), examined there once more, and only
# then deleted. So the folder that is deleted is the one that was examined, whatever comes to
# stand at the path meanwhile. The entry in the person's repository goes by its own record:
# git prunes the worktree whose folder is gone; box never asks git to remove "the worktree at
# this path", which git would resolve through a link to another worktree. Nothing at the path
# (not even a link that leads nowhere) means the folder is gone and only the entry is pruned.
readonly TRASH_DIR="$AGENTBOX_HOME/trash"
remove_worktree() {
  local task="$1" dir trash
  dir="$(task_dir "$task")"
  if [ -e "$dir" ] || [ -L "$dir" ]; then
    CHECKED_LAVORI=""
    require_own_worktree "$task"
    mkdir -p "$TRASH_DIR"
    trash="$(mktemp -d "$TRASH_DIR/$task.XXXXXX")" || die "could not make a folder to remove the worktree of task '$task' through"
    mv "$dir" "$trash/worktree" || die "could not move the worktree of task '$task' away; nothing was deleted"
    if ! moved_is_own_worktree "$task" "$trash/worktree"; then
      mv "$trash/worktree" "$dir" 2>/dev/null || warn "what stood at $dir is now in $trash/worktree"
      rmdir "$trash" 2>/dev/null || true
      die "the folder of task '$task' changed while it was being removed; nothing was deleted and its records are kept"
    fi
    rm -rf "$trash"
  fi
  prune_worktrees "$task"
}

# Succeeds when a folder that was moved away is the worktree of the task: a real folder with the
# identity box checked, whose .git names the recorded git directory.
moved_is_own_worktree() {
  local task="$1" moved="$2"
  [ -d "$moved" ] && [ ! -L "$moved" ] && [ "$(file_identity "$moved" 2>/dev/null)" = "$LAVORO_IDENTITY" ] \
    && [ "$(worktree_pointer "$moved")" = "$(task_record_field "$task" WORKTREE_GIT_DIR)" ]
}

# Lets git drop the entries of worktrees whose folder is gone, in the repository of the task's
# project, when that project can still be read.
prune_worktrees() {
  if ! (load_task_project "$1") >/dev/null 2>&1; then
    warn "the project of task '$1' cannot be read: run 'git worktree prune' in its repository yourself"
    return 0
  fi
  load_task_project "$1"
  person_git -C "$REPO" worktree prune >/dev/null 2>&1 || true
}

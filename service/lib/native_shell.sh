# shellcheck shell=bash
# box shell <task> for a lavoro on the Mac: the person's own login shell, in the folder of the
# lavoro. It is not confined, like everything of the Mac place: it is the person's shell with
# the person's privileges. box does not stay beside it (the shell takes the place of the box
# process) and holds no lock while it is open: a terminal lasts as long as the person wants.
# What box keeps is a record of the shell (its process, the boot of the machine and its start,
# like the records of process groups), so that box rm knows a terminal is open in the lavoro:
# it refuses then, and with --force tells the shell to hang up first.

readonly SHELLS_DIR="$AGENTBOX_HOME/shells"
readonly SHELL_END_TICKS=15

# The shell to open: the person's ($SHELL) when it is an absolute path of a program, else the
# usual one of the system (zsh on macOS, bash elsewhere).
person_shell() {
  case "${SHELL:-}" in
    /*) if [ -x "$SHELL" ]; then echo "$SHELL"; return 0; fi ;;
  esac
  default_person_shell
}

# The lifecycle lock is held from before the lavoro is looked at until the record of the shell
# is written, then let go: the shell itself runs without it.
shell_on_mac() {
  local task="$1" folder shell
  begin_startup "$task" mac
  require_checked_lavoro "$task"
  folder="$(lavoro_folder "$task")"
  [ -d "$folder" ] || folder="$LAVORO_DIR"
  shell="$(person_shell)"
  confirm_startup "$task"
  record_shell "$task" || die "could not keep a record of the terminal of task '$task'; it was not opened"
  cd "$folder" || die "could not enter the folder of task '$task'"
  end_startup
  # The person's shell gets the PATH the person had, not the one box rebuilt for itself.
  export PATH="$PERSON_PATH"
  exec "$shell" -l
}

# The record of this process, which is about to become the shell: its id does not change when
# it does. Fails when the boot time or the start time cannot be read.
record_shell() {
  local boot start
  boot="$(boot_time)"
  start="$(process_start "$$")"
  [ -n "$boot" ] && [ -n "$start" ] || return 1
  mkdir -p "$SHELLS_DIR"
  printf '%s\n%s\n%s\n' "$$" "$boot" "$start" >"$SHELLS_DIR/$1.$$"
}

# Prints "<pid> <record>" for every terminal that is open in a lavoro. A record that no longer
# names its process (the terminal was closed, the machine restarted) is dropped on the way.
open_shells() {
  local record pid
  for record in "$SHELLS_DIR/$1".*; do
    [ -f "$record" ] || continue
    if pid="$(recorded_group "$record")"; then echo "$pid $record"; fi
  done
}

# For box rm: refuses while a terminal is open, or with --force tells each to hang up, waits a
# moment, and kills one that stays, only if its record still names it then.
settle_open_shells() {
  local task="$1" force="$2" pid record tick
  if [ "$force" != "--force" ]; then
    [ -z "$(open_shells "$task")" ] || die "a terminal is open in the folder of task '$task' (box shell); close it first, or use --force to end it"
    return 0
  fi
  while read -r pid record; do
    kill -HUP "$pid" 2>/dev/null || true
    for ((tick = 0; tick < SHELL_END_TICKS; tick++)); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.2
    done
    if [ "$(recorded_group "$record")" = "$pid" ]; then kill -KILL "$pid" 2>/dev/null || true; fi
  done < <(open_shells "$task")
}

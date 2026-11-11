# shellcheck shell=bash
# Shared constants and small helpers. Sourced by bin/box and the other libs.
# Written for bash 3.2 (the macOS system bash).

# bin/box names the data directory before it reads this file.
readonly AGENTBOX_HOME="${AGENTBOX_HOME:?the data directory is not named}"
readonly WORK_DIR="$AGENTBOX_HOME/work"
# Everything under $WORK_DIR/<task> is written by the agent and is untrusted. Everything the
# host trusts lives in the directories below, which no container can write.
readonly TASKS_DIR="$AGENTBOX_HOME/tasks"
readonly LOCK_DIR="$AGENTBOX_HOME/locks"
readonly QUEUE_DIR="$AGENTBOX_HOME/queue"
readonly RESULTS_DIR="$AGENTBOX_HOME/results"
readonly BUNDLES_DIR="$AGENTBOX_HOME/bundles"
readonly STAGE_DIR="$AGENTBOX_HOME/stage"
readonly DEPS_DIR="$AGENTBOX_HOME/deps"
readonly QUARANTINE_DIR="$AGENTBOX_HOME/quarantine"
# The only writable channel from an agent to the host: empty files named like verbs.
readonly DROP_DIR="$AGENTBOX_HOME/drop"

# The one place the image tag is named. A change to image/Dockerfile gets a new tag, so that the
# image of running boxes is never replaced under them.
readonly IMAGE="agentbox:4"
readonly NETWORK="agentbox-net"
readonly PROXY_CONTAINER="agentbox-proxy"
readonly PROXY_URL="http://$PROXY_CONTAINER:8888"
readonly NPM_CACHE_VOLUME="agentbox-npm-cache"
readonly RESOURCE_PREFIX="agentbox"

# The programs that supervise a command of the person, started by these absolute paths: they
# run with the person's own PATH (PERSON_PATH), which box does not choose, inside a worktree.
# With "." in that PATH a file of the worktree called env, perl or bash would otherwise run in
# place of the supervisor.
readonly TRUSTED_ENV=/usr/bin/env
readonly TRUSTED_PERL=/usr/bin/perl
readonly TRUSTED_BASH=/bin/bash

# The PATH the person had when box was started (bin/box keeps it before it rebuilds its own).
[ -n "${PERSON_PATH+set}" ] || PERSON_PATH="${PATH:-}"

# trusted_program <name>: prints the absolute path of a program, found in the PATH box rebuilt
# for itself at its start (bin/box: absolute entries only, outside the data directory, not
# writable by everybody). Fails when there is none, and, as a second layer under that PATH, when
# the program found lies in the data directory (by what its folder is, see folder_is_under).
# For a program whose path is recorded or is used after box has changed into a folder of a lavoro.
trusted_program() {
  local program
  program="$(command -v "$1" 2>/dev/null)" && [[ "$program" == /* ]] && [ -x "$program" ] && [ ! -d "$program" ] || return 1
  ! is_in_data_directory "$(physical_directory "${program%/*}")" || return 1
  echo "$program"
}

readonly TASK_NAME_PATTERN='^[a-z0-9]+(-[a-z0-9]+)*$'
readonly ACCOUNT_NAME_PATTERN='^[a-z0-9]+(-[a-z0-9]+)*$'

readonly DEFAULT_AGENT_CPUS=2
readonly DEFAULT_AGENT_MEMORY=3g
readonly DEFAULT_RUNNER_CPUS=4
readonly DEFAULT_RUNNER_MEMORY=6g
readonly PIDS_LIMIT=1024
readonly SHM_SIZE=256m
readonly TMPFS_SIZE=512m

# ---- what differs between macOS and Linux, one helper per difference ----------------------------
# box runs on both. The tools of macOS are the BSD ones, those of Linux the GNU ones, and a few
# of them take other options for the same thing. Each difference is in one function here and
# nowhere else. /usr/bin/stat is the system's own stat on both.
case "$(/usr/bin/uname -s)" in
  Darwin | *BSD) readonly BSD_TOOLS=1 ;;
  *) readonly BSD_TOOLS=0 ;;
esac

# file_stat <BSD format> <GNU format> [-L] <path>...: one line per path in the format of this
# system's stat; -L follows a link.
file_stat() {
  local bsd="$1" gnu="$2" follow=()
  shift 2
  if [ "${1:-}" = "-L" ]; then follow=(-L); shift; fi
  if [ "$BSD_TOOLS" -eq 1 ]; then
    /usr/bin/stat ${follow[@]+"${follow[@]}"} -f "$bsd" "$@"
  else
    /usr/bin/stat ${follow[@]+"${follow[@]}"} -c "$gnu" "$@"
  fi
}

# file_identity [-L] <path>...: "<device>:<inode>" of each path.
file_identity() {
  file_stat '%d:%i' '%d:%i' "$@"
}

# file_modified <path>: when the file was last changed, in seconds since 1970.
file_modified() {
  file_stat '%m' '%Y' "$@"
}

# file_owner <path>: the numeric id of the user the file belongs to.
file_owner() {
  file_stat '%u' '%u' "$@"
}

# file_kind_identity <path>...: "<kind>|<device>:<inode>" of each path, without following a
# link; the kind is directory, file, link or other, whatever this system's stat calls them.
file_kind_identity() {
  local kind identity
  file_stat '%HT|%d:%i' '%F|%d:%i' "$@" | while IFS='|' read -r kind identity; do
    case "$kind" in
      Directory | directory) kind=directory ;;
      "Regular File" | "regular file" | "regular empty file") kind=file ;;
      "Symbolic Link" | "symbolic link") kind=link ;;
      *) kind=other ;;
    esac
    echo "$kind|$identity"
  done
}

# epoch_to_zulu <seconds since 1970>: that moment as 2026-01-31T12:00:00Z.
epoch_to_zulu() {
  if [ "$BSD_TOOLS" -eq 1 ]; then date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ; else date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; fi
}

# The shell of a person who names none: zsh on macOS, bash elsewhere.
default_person_shell() {
  if [ "$BSD_TOOLS" -eq 1 ] && [ -x /bin/zsh ]; then echo /bin/zsh; else echo /bin/bash; fi
}

# sha256_of: the SHA-256 of standard input, 64 hex digits. (sha256sum where there is one, as on
# Linux; shasum, which macOS has, otherwise.)
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1; else shasum -a 256 | cut -d' ' -f1; fi
}

die() {
  echo "box: $*" >&2
  exit 1
}

warn() {
  echo "box: $*" >&2
}

# Succeeds when $1 is a valid task name.
is_valid_task_name() {
  [[ "${1:-}" =~ $TASK_NAME_PATTERN ]]
}

require_task_name() {
  is_valid_task_name "${1:-}" || die "invalid task name '${1:-}' (lowercase letters, digits and single hyphens)"
}

require_account_name() {
  [[ "${1:-}" =~ $ACCOUNT_NAME_PATTERN ]] || die "invalid account name '${1:-}' (lowercase letters, digits and single hyphens)"
}

require_existing_task() {
  require_task_name "${1:-}"
  [ -d "$WORK_DIR/$1" ] || die "task '$1' does not exist (box new $1)"
}

task_dir() {
  echo "$WORK_DIR/$1"
}

drop_dir() {
  echo "$DROP_DIR/$1"
}

task_results_dir() {
  echo "$RESULTS_DIR/$1"
}

# Seconds since epoch, overridable in tests.
now_epoch() {
  date +%s
}

now_iso() {
  date -u +%Y-%m-%dT%H:%M:%SZ
}

# with_lock <lock file> <command...>: runs the command in a subshell while holding an exclusive
# lock on the file; the lock is released when the subshell ends, however it ends. Not reentrant:
# a command that takes the same lock again waits for itself.
with_lock() {
  local lock_file="$1"
  shift
  mkdir -p "$(dirname "$lock_file")"
  (
    exec 7>>"$lock_file"
    perl -MFcntl=:flock -e 'open(my $f, ">&=", 7) or die "fd 7: $!"; flock($f, LOCK_EX) or die "lock: $!"' || exit 1
    "$@"
  )
}

# Runs a command and kills it after $1 seconds. macOS has no timeout(1) by default.
run_with_timeout() {
  local seconds="$1"
  shift
  perl -e 'alarm shift; exec @ARGV or die "exec failed: $!\n"' "$seconds" "$@"
}

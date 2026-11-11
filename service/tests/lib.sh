# shellcheck shell=bash
# Minimal test helpers. Each test file sources this, calls setup_sandbox, then asserts.

TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOX_ROOT="$(cd "$TESTS_DIR/.." && pwd)"
BOX="$BOX_ROOT/bin/box"
FAILURES=0

pass() {
  echo "  ok   $1"
}

fail() {
  echo "  FAIL $1"
  FAILURES=$((FAILURES + 1))
}

assert_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1 (expected '$2', got '$3')"; fi
}

assert_contains() {
  case "$3" in
    *"$2"*) pass "$1" ;;
    *) fail "$1 (expected to contain '$2', got '$3')" ;;
  esac
}

assert_file() {
  if [ -f "$2" ]; then pass "$1"; else fail "$1 (missing file $2)"; fi
}

assert_no_file() {
  if [ ! -e "$2" ]; then pass "$1"; else fail "$1 (unexpected $2)"; fi
}

# assert_status <name> <expected status> <command...>; the output is kept in $OUTPUT.
assert_status() {
  local name="$1" expected="$2" actual
  shift 2
  OUTPUT="$("$@" 2>&1)"
  actual=$?
  assert_eq "$name" "$expected" "$actual"
}

# still_alive <pid>: prints 0 as soon as the process is gone, 1 when it is still there after 15 s.
# (A process that has ended is listed until its parent has collected it, which takes a moment on
# a busy machine: what must not happen is that it keeps running.)
still_alive() {
  local tick
  for tick in $(seq 1 75); do
    ps -p "$1" -o pid= >/dev/null 2>&1 || { echo 0; return 0; }
    sleep 0.2
  done
  echo 1
}

# wait_for_started <task>: waits until a command that started something for the task has let go
# of its lifecycle lock (it does so a moment after what it started runs), at most 10 s.
wait_for_started() {
  local tick
  for tick in $(seq 1 50); do
    perl -MFcntl=:flock -e 'open(my $f, ">>", $ARGV[0]) or exit 1; exit(flock($f, LOCK_EX | LOCK_NB) ? 0 : 1)' "$AGENTBOX_HOME/locks/task-$1.lock" && return 0
    sleep 0.2
  done
  return 1
}

# Creates a throwaway sandbox: home, a source repository, a project for it and a fake docker.
# BOX_PROFILES_DIR (the shipped profiles that become projects on upgrade) is an empty directory.
# Program shims must live in trusted, private directories even when the caller has umask 000.
setup_sandbox() {
  umask 077
  TMP="$(mktemp -d "${TMPDIR:-/tmp}/box-test.XXXXXX")"
  trap 'rm -rf "$TMP"' EXIT
  export AGENTBOX_HOME="$TMP/${BOX_TEST_HOME_NAME:-home}"
  export BOX_PROFILES_DIR="$TMP/profiles"
  export FAKE_DOCKER_LOG="$TMP/docker.log"
  export PATH="$TESTS_DIR/fake-bin:$PATH"
  export BOX_RUNNER_POLL=1
  # The tests of the box place: a project that does not name a place gets a box. The tests of
  # the lavori on the Mac unset it.
  export BOX_DEFAULT_WHERE=box
  : >"$FAKE_DOCKER_LOG"
  make_source_repo
  make_project
  # The native adapter and its installation get an explicit list of variables; the switches of
  # the fake adapter, the fake node and the fake npm are named so that they still reach them.
  export BOX_ADAPTER_ENV="FAKE_ACP_LOGGED_IN FAKE_ACP_QUIET_REPLIES FAKE_ACP_IGNORE_EOF FAKE_ACP_IGNORE_TERM FAKE_ACP_EXIT FAKE_ACP_CRASH FAKE_ACP_STDERR_BYTES FAKE_ACP_CHILD_PID_FILE FAKE_ACP_REQUESTS FAKE_ACP_FLOOD_LINES FAKE_ACP_HUGE_BYTES FAKE_ACP_ENV_LOG FAKE_ACP_DETACHED_PID_FILE FAKE_NODE_LOG FAKE_NPM_LOG FAKE_NPM_FAIL FAKE_NPM_DELAY FAKE_NPM_ENV_LOG FAKE_NPM_ADAPTER"
  # No accounts of the Mac to begin with: the first run would list the logins of whoever runs
  # the tests (tests/test_mac_accounts.sh removes this file to see that first run).
  : >"$AGENTBOX_HOME/config"
}

make_source_repo() {
  SOURCE="$TMP/source"
  git init -q -b main "$SOURCE"
  mkdir -p "$SOURCE/sub"
  echo '{}' >"$SOURCE/sub/package-lock.json"
  echo '{}' >"$SOURCE/sub/package.json"
  echo 'existing' >"$SOURCE/sub/existing.spec.ts"
  echo 'other' >"$SOURCE/sub/other.spec.ts"
  git -C "$SOURCE" add -A
  git -C "$SOURCE" -c user.name=t -c user.email=t@example.com commit -q -m base
}

# The project every test starts with, as box writes one: plain KEY=value lines in the data directory.
make_project() {
  PROJECTS_DIR="$AGENTBOX_HOME/projects"
  mkdir -p "$PROJECTS_DIR"
  cat >"$PROJECTS_DIR/sandbox.env" <<PROJECT
REPO=$SOURCE
BASE=main
SUBDIR=sub
INSTALL=true
VERB_test=echo test
VERB_build=echo build
PROTECTED=package.json package-lock.json scripts/**
TEST_FILES=**/*.spec.ts
SAFE_OUTSIDE=change.txt
PROJECT
}

# ---- what differs between macOS and Linux, for the tests (the service has its own helpers in
# lib/common.sh) --------------------------------------------------------------------------------

# edit_in_place <sed expression> <file>: sed -i takes other arguments on the two systems.
edit_in_place() {
  sed "$1" "$2" >"$2.edited" && mv "$2.edited" "$2"
}

# What a process record holds about this machine and about a process: as box reads them
# (lib/native_commands.sh), from /proc where there is one, from sysctl and ps otherwise.
record_boot() {
  if [ -r /proc/sys/kernel/random/boot_id ]; then
    tr -d '[:space:]' </proc/sys/kernel/random/boot_id
  else
    /usr/sbin/sysctl -n kern.boottime | sed -n 's/^{ sec = \([0-9]*\),.*/\1/p'
  fi
}
record_start() {
  if [ -r "/proc/$1/stat" ]; then
    sed 's/^.*) //' "/proc/$1/stat" | cut -d' ' -f20
  else
    LC_ALL=C /bin/ps -p "$1" -o lstart= | sed 's/^ *//; s/ *$//'
  fi
}

# stat_of <BSD format> <GNU format> <path>: one value of this system's stat.
stat_of() {
  case "$(/usr/bin/uname -s)" in
    Darwin | *BSD) /usr/bin/stat -f "$1" "$3" ;;
    *) /usr/bin/stat -c "$2" "$3" ;;
  esac
}

# The shell box opens for a person who names none.
usual_shell() {
  case "$(/usr/bin/uname -s)" in Darwin) echo /bin/zsh ;; *) echo /bin/bash ;; esac
}

# commit_in_task <task> <message>
commit_in_task() {
  local dir="$AGENTBOX_HOME/work/$1"
  echo "$2" >>"$dir/change.txt"
  git -C "$dir" add -A
  git -C "$dir" -c user.name=t -c user.email=t@example.com commit -q -m "$2"
}

# drop_request <task> <verb>: what an agent does to ask for a verification.
drop_request() {
  : >"$AGENTBOX_HOME/drop/$1/$2"
}

# queue_directly <task> <verb> <time id> <sha>: a queue file as the host would write it.
# QUEUED_ID is the result id, QUEUED_KEY the request key (the queue file and job name).
queue_directly() {
  QUEUED_ID="$3-$4-$2"
  QUEUED_KEY="$QUEUED_ID.$1"
  mkdir -p "$AGENTBOX_HOME/queue"
  printf 'task=%s\nverb=%s\nsha=%s\nrequested=now\nstate=queued\n' "$1" "$2" "$4" \
    >"$AGENTBOX_HOME/queue/$QUEUED_KEY.req"
}

# heartbeat <task> [age seconds]: what a live runner keeps writing into the results directory.
heartbeat() {
  local directory="$AGENTBOX_HOME/results/$1" temporary
  mkdir -p "$directory"
  temporary="$(mktemp "$directory/.runner-alive.XXXXXX")" || return 1
  printf '%s\n' "$(( $(date +%s) - ${2:-0} ))" >"$temporary" && mv "$temporary" "$directory/.runner-alive"
}

workspace_head() {
  git -C "$AGENTBOX_HOME/work/$1" rev-parse HEAD
}

finish() {
  if [ "$FAILURES" -eq 0 ]; then echo "  all passed"; else echo "  $FAILURES failed"; fi
  [ "$FAILURES" -eq 0 ]
}

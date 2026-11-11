# shellcheck shell=bash
# Commands that start containers for agents: run, login, agent, verify.

# Creates the account config directories, then runs the given command.
readonly ACCOUNT_ENTRY='mkdir -p /acct/claude /acct/codex && exec "$@"'

# box run <task> -- <command...>: one-off container with the workspace.
cmd_run() {
  local task="${1:-}" name
  require_existing_task "$task"
  shift
  [ "${1:-}" = "--" ] && shift
  [ "$#" -gt 0 ] || die "usage: box run <task> -- <command...>"
  refuse_on_mac "$task" "box run" "its folder is $(task_dir "$task"), run the command there yourself"
  begin_startup "$task" box
  load_task_project "$task"
  require_docker
  ensure_task_volumes "$task"
  build_agent_flags "$task"
  name="$(workspace_container_name "$task" "$RANDOM")"
  add_workspace_flags "$task" "$name"
  confirm_startup "$task"
  run_confirmed "$task" "$name" docker run --rm "$(interactive_flags)" "${AGENT_FLAGS[@]}" "$IMAGE" "$@"
}

# box login <account>: `claude auth login` (interactive: the person pastes a code), then
# `claude auth status --json`, in one container whose config directory is the account volume.
# Succeeds only when that status says loggedIn.
cmd_login() {
  local account="${1:-}" code=0
  require_account_name "$account"
  require_docker
  ensure_volume "$(account_volume "$account")"
  build_agent_flags
  add_account_flags "$account"
  AGENT_FLAGS+=(--log-driver json-file --log-opt max-size=8m --log-opt max-file=2)
  docker run --rm "$(interactive_flags)" --label agentbox.kind=login "${AGENT_FLAGS[@]}" -v "$RUNNER_SCRIPTS:/box:ro" \
    -w /home/agent "$IMAGE" bash -c "$ACCOUNT_ENTRY" _ bash /box/login.sh || code=$?
  [ "$code" -eq 0 ] || die "$(login_failure_message "$code" "$account")"
}

# One plain sentence for an exit code of image/runner/login.sh (or of docker itself).
login_failure_message() {
  case "$1" in
    2) echo "the sign-in was not completed for account '$2'; run box login $2 again" ;;
    3) echo "the sign-in finished but account '$2' is still not logged in; run box login $2 again" ;;
    4) echo "could not read the login status of account '$2' after signing in" ;;
    5) echo "no connection to the sign-in service; is the proxy running (box up)?" ;;
    *) echo "the login container failed (exit $1); account '$2' is not logged in" ;;
  esac
}

# box agent <task> <account> [--detach] [-- claude args]: claude in the workspace with an account.
cmd_agent() {
  local task="${1:-}" account="${2:-}" mode=attached
  [ "$#" -ge 2 ] || die "usage: box agent <task> <account> [--detach] [-- <claude args>]"
  shift 2
  if [ "${1:-}" = "--detach" ]; then mode=detached; shift; fi
  [ "${1:-}" = "--" ] && shift
  start_agent "$task" "$account" "$mode" "$@"
}

# box verify <task> <verb>: asks the runner for a check of the commit the workspace is at, and
# waits for the result unless told not to. The host never asks the way an agent does (a file in
# the drop directory of the task): an agent writes that directory, and the host writes nothing
# there. The request goes through the queue of box, for the commit and the incarnation of the
# task that were read (a sandbox container reads the HEAD).
cmd_verify() {
  local task="${1:-}" verb="${2:-}" generation
  require_existing_task "$task"
  [ -n "$verb" ] || die "usage: box verify <task> <verb> [--no-wait]"
  if task_is_on_mac "$task"; then verify_on_mac "$task" "$verb" "${3:-}"; return; fi
  require_docker
  if [ "${3:-}" = "--no-wait" ]; then
    queue_without_waiting "$task" "$verb"
    return 0
  fi
  require_queueable "$task" "$verb"
  require_runner
  generation="$(task_generation "$task")"
  workspace_facts "$task"
  request_and_wait "$task" "$verb" "$FACT_HEAD" "$generation"
}

# Dies unless a request for that verb of that task can be queued.
require_queueable() {
  [[ "$2" =~ $VERB_PATTERN ]] || die "invalid verb '$2'"
  ! task_is_retiring "$1" || die "task '$1' is being removed (box rm $1 --force finishes it)"
  [ -n "$(task_verb_command "$1" "$2")" ] || die "verb '$2' is not in the profile of task '$1'"
}

# box verify <task> <verb> --no-wait: queue the request and return. The same commit and verb
# queued twice stays one request.
queue_without_waiting() {
  require_queueable "$1" "$2"
  queue_request "$1" "$2"
}

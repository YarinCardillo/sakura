# shellcheck shell=bash
# box cancel <task> <verb>: ends a queued or running request as a cancelled result.
#
# Cancel records its intent in the queue file. A request the live runner has claimed is ended by
# the runner alone (it asks before every phase and before it publishes); cancel only removes the
# containers of the request so that the phase in progress stops. A request nobody is working on
# (queued, or left running by a runner that is gone) is ended here.

readonly NOTHING_TO_CANCEL_EXIT_CODE=78
readonly CANCEL_WAIT_TICKS=60

# Cancels every request of the task for the verb. Exit 0 when something was cancelled, 78 when
# there was nothing to cancel.
cmd_cancel() {
  local task="${1:-}" verb="${2:-}" key file cancelled=0
  [ "$#" -eq 2 ] || die "usage: box cancel <task> <verb>"
  require_existing_task "$task"
  [[ "$verb" =~ $VERB_PATTERN ]] || die "invalid verb '$verb'"
  task_is_on_mac "$task" || require_docker
  while read -r key file; do
    [ "$(request_field "$file" task)" = "$task" ] && [ "$(request_field "$file" verb)" = "$verb" ] || continue
    if cancel_request "$key" "$file"; then cancelled=1; fi
  done < <(list_requests)
  [ "$cancelled" -eq 1 ] || { warn "nothing to cancel: no queued or running '$verb' for task '$task'"; return "$NOTHING_TO_CANCEL_EXIT_CODE"; }
}

# Fails when the request left the queue before the cancel could be recorded.
cancel_request() {
  local key="$1" file="$2" task verb sha reason outcome
  task="$(request_field "$file" task)"
  verb="$(request_field "$file" verb)"
  sha="$(request_field "$file" sha)"
  if ! reason="$(validate_request "$task" "$key" "$verb" "$sha")"; then
    quarantine_request "$file" "$reason"
    return 0
  fi
  outcome="$(with_queue_lock record_cancel_locked "$file")" || return 1
  if [ "$outcome" = "pending" ]; then stop_request_and_wait "$key" "$file"; else cleanup_request_objects "$key"; fi
  journal_event cancel "$task" "$verb" "$sha" "$CANCELLED_EXIT_CODE"
}

# Records the cancel (queue lock held). Prints "ended" when the request ended here, "pending"
# when the live runner has claimed it and must end it. Fails when it already left the queue.
record_cancel_locked() {
  local file="$1"
  [ -f "$file" ] || return 1
  mark_request_cancelled "$file" || return 1
  if [ "$(request_field "$file" state)" = "running" ] && runner_is_running; then
    echo pending
    return 0
  fi
  end_request_locked "$file" 0 false || return 1
  echo ended
}

# Removes the containers of a claimed request until the runner has ended it. The runner asks for
# the cancel before each phase, so a container that starts after one removal is caught by the next.
stop_request_and_wait() {
  local key="$1" file="$2" tick
  for ((tick = 0; tick < CANCEL_WAIT_TICKS; tick++)); do
    remove_request_containers "$key"
    [ -f "$file" ] || return 0
    sleep "$TICK_SECONDS"
  done
  warn "the runner has not ended request $key yet; it will end as cancelled"
}

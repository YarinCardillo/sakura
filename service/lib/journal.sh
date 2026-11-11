# shellcheck shell=bash
# Journal of what box did: $AGENTBOX_HOME/journal.jsonl, one JSON object per line, appended by
# box only, after the action succeeded. Lines are built by jq (a real encoder).

readonly JOURNAL_FILE="$AGENTBOX_HOME/journal.jsonl"
readonly JOURNAL_LOCK_FILE="$LOCK_DIR/journal.lock"
readonly JOURNAL_MAX_EVENTS=2000

# journal_event <event> <task> [verb] [sha] [exit code] [account]; empty arguments are omitted.
journal_event() {
  local line
  line="$(jq -nc --arg at "$(now_iso)" --arg event "$1" --arg task "$2" --arg verb "${3:-}" \
    --arg sha "${4:-}" --arg exit "${5:-}" --arg account "${6:-}" '
    { at: $at, event: $event, task: $task, verb: $verb, sha: $sha,
      exitCode: (if $exit == "" then null else ($exit | tonumber) end), account: $account }
    | with_entries(select(.value != "" and .value != null))')" || return 0
  mkdir -p "$LOCK_DIR" "$AGENTBOX_HOME"
  (
    exec 8>>"$JOURNAL_LOCK_FILE"
    perl -MFcntl=:flock -e 'open(my $f, ">&=", 8) or die; flock($f, LOCK_EX) or die'
    printf '%s\n' "$line" >>"$JOURNAL_FILE"
  )
}

# box journal --json [--since <time>]
cmd_journal() {
  local since=""
  require_jq
  [ "${1:-}" = "--json" ] && shift
  [ "${1:-}" = "--since" ] && since="${2:-}"
  if [ -n "$since" ] && ! [[ "$since" =~ ^[0-9T:.Z-]+$ ]]; then die "invalid --since time"; fi
  if [ ! -f "$JOURNAL_FILE" ]; then echo "[]"; return 0; fi
  tail -n "$JOURNAL_MAX_EVENTS" "$JOURNAL_FILE" \
    | jq -cR --arg since "$since" 'fromjson? | select(type == "object") | select($since == "" or .at > $since)' \
    | jq -cs '.'
}

# Time of the last journal event of a kind for a task, empty when there is none.
journal_last_time() {
  [ -f "$JOURNAL_FILE" ] || return 0
  jq -rR --arg event "$1" --arg task "$2" \
    'fromjson? | select(type == "object" and .event == $event and .task == $task) | .at' "$JOURNAL_FILE" | tail -n 1
}

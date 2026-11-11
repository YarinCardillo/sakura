# shellcheck shell=bash
# Verification requests and results.
#
# Channel in: $DROP_DIR/<task> is mounted writable in the agent container. A request is an
# EMPTY file whose NAME is the verb. The host reads names only, never contents.
# Queue: $QUEUE_DIR/<id>.req, host-private, key=value lines written by the host.
# Channel out: $RESULTS_DIR/<task> is written by the host and mounted read-only in the container.

# Result id: <second timestamp>-<sha>-<verb>, the name of a result file inside the results
# directory of one task. Request key: <result id>.<task>, the name of a queue file and the base
# of every Docker object of a job, so two tasks never share one. Task names hold no dot.
# The sha is the whole one, 40 characters. An agent can make two commits whose shas begin with
# the same twelve characters (about 2^24 tries), so a shortened sha is never what tells two
# requests or two results apart: with it, the second commit was taken for a request already
# queued and its waiter took the result of the first.
readonly REQUEST_ID_PATTERN='^[0-9]{8}T[0-9]{6}-[0-9a-f]{40}-[a-z][a-z0-9-]*$'
readonly SHA_PATTERN='^[0-9a-f]{40}$'
readonly SAFE_NAME_LENGTH=60
readonly CANCELLED_EXIT_CODE=130
readonly MAX_DROP_LIST_BYTES=100000
readonly DROP_KIND_PATTERN='^[a-z][a-z0-9-]* (empty|nonempty|other)$'
readonly QUEUE_LOCK_FILE="$LOCK_DIR/queue.lock"

# Every change of a queue file (claimed, cancelled, requeued, ended) happens inside this one
# lock, so two processes never rewrite a request from the same old content and exactly one of
# them publishes its terminal result. Functions named *_locked expect the lock to be held.
with_queue_lock() {
  with_lock "$QUEUE_LOCK_FILE" "$@"
}

is_valid_request_id() {
  [[ "${1:-}" =~ $REQUEST_ID_PATTERN ]]
}

is_valid_sha() {
  [[ "${1:-}" =~ $SHA_PATTERN ]]
}

# Path of the queue file of a request key.
queue_file() {
  echo "$QUEUE_DIR/$1.req"
}

# The request key of a queue file path.
request_key() {
  basename "$1" .req
}

# The result id inside a request key. A key without a task part (a file queued by an older
# box) is its own result id.
result_id_of_key() {
  echo "${1%.*}"
}

# The second timestamp of a new request. BOX_REQUEST_TIMESTAMP freezes it in tests.
request_timestamp() {
  echo "${BOX_REQUEST_TIMESTAMP:-$(date -u +%Y%m%dT%H%M%S)}"
}

# Prints the value of a key in a host-written queue file.
request_field() {
  sed -n "s/^$2=//p" "$1" | head -1
}

# Prints "<id> <path>" for every queued or running request, oldest first.
list_requests() {
  local file
  for file in "$QUEUE_DIR"/*.req; do
    if [ -f "$file" ]; then echo "$(basename "$file" .req) $file"; fi
  done | sort
}

# Prints "<key> <path>" for every request of a task.
task_requests() {
  local key file
  while read -r key file; do
    if [ "$(request_field "$file" task)" = "$1" ]; then echo "$key $file"; fi
  done < <(list_requests)
}

# Prints the path of the oldest queued request, nothing when there is none.
next_queued_request() {
  local id file
  while read -r id file; do
    [ "$(request_field "$file" state)" = "queued" ] && { echo "$file"; return 0; }
  done < <(list_requests)
  return 0
}

# Rewrites the state of a queue file atomically; extra "key=value" lines are appended. Call it
# with the queue lock held.
set_request_state() {
  local file="$1" state="$2" tmp line
  shift 2
  tmp="$file.tmp"
  { grep -v -e '^state=' -e '^pid=' -e '^started=' "$file" || true; } >"$tmp"
  { echo "state=$state"; for line in "$@"; do echo "$line"; done; } >>"$tmp"
  mv "$tmp" "$file"
}

# ---- intake: drop directories to queue --------------------------------------------------

# Turns the drop files of every task into queued requests.
ingest_drops() {
  local dir task
  for dir in "$DROP_DIR"/*/; do
    [ -d "$dir" ] || continue
    task="$(basename "$dir")"
    is_valid_task_name "$task" || continue
    [ -f "$TASKS_DIR/$task.env" ] && ! task_is_retiring "$task" || continue
    # A lavoro on the Mac has no drop directory; one made by hand is not read.
    ! task_is_on_mac "$task" || continue
    ingest_task_drops "$task"
  done
}

# The entries of a task's drop directory that the runner looks at in one time round: no more
# than the budget, and every entry counts against it, whatever it is. What is left waits for
# the next round, after the queue has been served, so a task that fills its drop directory
# takes the runner's time in slices and not all at once. INTAKE_LEFT says that a task reached
# its budget, so more may be waiting.
readonly DROP_INTAKE_BUDGET="${BOX_DROP_INTAKE_BUDGET:-25}"
INTAKE_LEFT=0

# drop_names <task>: at most DROP_INTAKE_BUDGET names of the task's drop directory, each ended
# by a NUL byte. The directory is read entry by entry and the reading stops at the budget: a
# directory an agent filled is never listed whole (a glob would expand all of it in the memory
# of the runner). Names only; nothing is opened or followed.
drop_names() {
  perl -e 'opendir(my $dir, $ARGV[0]) or exit 0; my $count = 0;
    while (defined(my $name = readdir $dir)) {
      next if $name eq "." || $name eq "..";
      print $name, "\0";
      last if ++$count >= $ARGV[1];
    }' "$(drop_dir "$1")" "$DROP_INTAKE_BUDGET"
}

ingest_task_drops() {
  local task="$1" entry name reason kinds="" names=() verbs=()
  while IFS= read -r -d '' name; do names+=("$name"); done < <(drop_names "$task")
  [ "${#names[@]}" -gt 0 ] || return 0
  [ "${#names[@]}" -lt "$DROP_INTAKE_BUDGET" ] || INTAKE_LEFT=1
  for name in "${names[@]}"; do
    if [[ "$name" =~ $VERB_PATTERN ]]; then verbs+=("$name"); fi
  done
  [ "${#verbs[@]}" -eq 0 ] || kinds="$(drop_kinds "$task" "${verbs[@]}")"
  for name in "${names[@]}"; do
    entry="$(drop_dir "$task")/$name"
    if ! reason="$(drop_rejection_reason "$task" "$name" "$kinds")"; then
      quarantine_entry "$task" "$entry" "$name" "$reason"
    elif (queue_request "$task" "$name"); then
      rm -f -- "$entry"
    else
      quarantine_entry "$task" "$entry" "$name" "the commit of the workspace could not be read"
    fi
  done
}

# drop_kinds <task> <names...>: "<name> empty|nonempty|other" for each of those entries of the
# task's drop directory (names that look like verbs, picked by the host within its budget). The
# box writes that directory, so what its entries are is read in a container that mounts nothing
# else, by lstat of the names it is given: it lists nothing. The first line says that the
# answer is there, also when no entry qualifies.
drop_kinds() {
  local task="$1"
  shift
  require_box_task "$task"
  echo "read"
  build_sandbox_flags
  run_with_timeout "$SANDBOX_TIMEOUT_SECONDS" docker run "${SANDBOX_FLAGS[@]}" -v "$(drop_dir "$task"):/drops:ro" \
    "$IMAGE" /box/drops.py "$@" 2>/dev/null | head -c "$MAX_DROP_LIST_BYTES" | grep -E "$DROP_KIND_PATTERN" || true
}

# A drop is accepted when its name is a verb pattern, it is an empty regular file (not a link)
# directly in the directory, and the profile of the task names that verb. Otherwise this prints
# the reason (host-written text) and fails. Arguments: task, name, the output of drop_kinds.
drop_rejection_reason() {
  local task="$1" name="$2" kinds="$3"
  if [[ ! "$name" =~ $VERB_PATTERN ]]; then echo "the drop name is not a verb"; return 1; fi
  if grep -qx "$name nonempty" <<<"$kinds"; then echo "the drop is not empty"; return 1; fi
  if ! grep -qx "$name empty" <<<"$kinds"; then echo "the drop is not a regular file"; return 1; fi
  if [ -z "$(task_verb_command "$task" "$name")" ]; then echo "verb '$name' is not in the profile of task '$task'"; return 1; fi
}

task_verb_command() {
  (load_task_project "$1" && verb_command "$2")
}

# quarantine_place <label>: makes a folder in the quarantine directory that is reserved for one
# entry, and prints it. mktemp makes the folder or fails: it never takes a name that is there
# already, a link included. What is then moved to a fixed name inside it lands in that folder
# and nowhere else, whatever an agent got into quarantine before.
quarantine_place() {
  mkdir -p "$QUARANTINE_DIR" && mktemp -d "$QUARANTINE_DIR/$1.XXXXXX"
}

# Moves anything unexpected out of the drop directory by rename, into a folder reserved for it.
# What it is gets looked at there, where no agent reaches it any more: a plain file is kept
# (never read); a link, a FIFO, a device or a directory is removed without being followed, and
# only its name and kind are written down. A waiting verify is told why through a marker in the
# results directory.
quarantine_entry() {
  local task="$1" entry="$2" name="$3" reason="$4" place kind="gone" safe_name
  safe_name="$(printf '%s' "$name" | sanitize | cut -c1-$SAFE_NAME_LENGTH)"
  if place="$(quarantine_place "drop-$task")" && mv -- "$entry" "$place/entry" 2>/dev/null; then
    kind="$(settle_quarantined "$place/entry")"
    printf 'time=%s\ntask=%s\nname=%s kind=%s\nreason=%s\n' "$(now_iso)" "$task" "$safe_name" "$kind" "$reason" >"$place/about"
  else
    rm -f -- "$entry" || true
  fi
  warn "quarantined drop entry '$safe_name' ($kind) of task '$task': $reason"
  record_drop_rejection "$task" "$name" "$reason" || warn "could not tell the waiting verify about task '$task'"
}

# settle_quarantined <moved entry>: prints the kind of what was moved into a quarantine place
# (file, link, fifo, directory, special) and removes it unless it is a plain file. The removal
# of a directory has a deadline: a tree too large to remove in that time stays where it is, in
# the reserved folder, where nothing is ever written again.
readonly QUARANTINE_REMOVE_SECONDS=20
settle_quarantined() {
  local moved="$1" kind
  if [ -L "$moved" ]; then kind=link
  elif [ -f "$moved" ]; then echo file; return 0
  elif [ -d "$moved" ]; then kind=directory
  elif [ -p "$moved" ]; then kind=fifo
  else kind=special; fi
  run_with_timeout "$QUARANTINE_REMOVE_SECONDS" rm -rf -- "$moved" 2>/dev/null || true
  echo "$kind"
}

# <results>/<task>/.rejected-<verb>: line 1 the epoch, line 2 the reason. Only for valid verb names.
record_drop_rejection() {
  local task="$1" verb="$2" reason="$3" dir
  [[ "$verb" =~ $VERB_PATTERN ]] || return 0
  dir="$(task_results_dir "$task")"
  mkdir -p "$dir" || return 1
  printf '%s\n%s\n' "$(now_epoch)" "$reason" >"$dir/.rejected-$verb.tmp" \
    && mv "$dir/.rejected-$verb.tmp" "$dir/.rejected-$verb"
}

# Takes the commit from a sandbox read of the workspace and writes the queue file. An
# identical queued request (same task, commit and verb) is not duplicated. Called in a subshell:
# a workspace that cannot be read must not stop the runner.
queue_request() {
  local task="$1" verb="$2" generation
  generation="$(task_generation "$task")"
  workspace_facts "$task"
  queue_commit "$task" "$verb" "$FACT_HEAD" "$generation" || return 0
}

# queue_commit <task> <verb> <sha> <generation>: queues the request for that commit of that
# incarnation of the task. An identical queued request is not duplicated. Fails when nothing
# was queued: the task is gone, is being removed, or was removed and made again since the
# generation was read.
queue_commit() {
  local task="$1" verb="$2" sha="$3" generation="$4" key
  [ -z "$(find_queued "$task" "$sha" "$verb")" ] || return 0
  key="$(request_timestamp)-$sha-$verb.$task"
  with_queue_lock publish_request "$key" "$task" "$verb" "$sha" "$generation" || return 1
  journal_event verify-queued "$task" "$verb" "$sha"
}

# Writes the queue file under a unique temporary name in the queue directory, then links it
# to its final name: the file appears whole or not at all, and an existing request is never
# replaced. Fails when the request already exists, or when the task has no record or is being
# removed, or is not the incarnation of the task the caller read (its generation, taken before
# the workspace was read: the task may have been removed and created again meanwhile). Call it
# with the queue lock held, so that a removal cannot begin between the check and the file.
publish_request() {
  local key="$1" task="$2" verb="$3" sha="$4" generation="${5:-}" tmp stamp
  [ -f "$TASKS_DIR/$task.env" ] && ! task_is_retiring "$task" || return 1
  [ "$(task_generation "$task")" = "$generation" ] || return 1
  [ -z "$(find_queued "$task" "$sha" "$verb")" ] || return 0
  stamp="${key%%-*}"
  while [ -e "$(task_results_dir "$task")/${key%.*}.json" ]; do
    stamp="$(next_request_timestamp "$stamp")" || return 1
    key="$stamp-$sha-$verb.$task"
  done
  mkdir -p "$QUEUE_DIR"
  tmp="$(mktemp "$QUEUE_DIR/.new.XXXXXX")"
  printf 'task=%s\nverb=%s\nsha=%s\nrequested=%s\ngeneration=%s\nstate=queued\n' "$task" "$verb" "$sha" "$(now_iso)" "$generation" >"$tmp"
  if ln "$tmp" "$(queue_file "$key")" 2>/dev/null; then rm -f "$tmp"; else rm -f "$tmp"; return 1; fi
}

# A result is immutable. A retry within one clock second must get a fresh id, rather than
# publishing a queue entry whose result already exists. Advance the id's logical timestamp;
# requested and finished still record real time. This runs while holding the queue lock.
next_request_timestamp() {
  perl -MTime::Local=timegm -MPOSIX=strftime -e '
    $ARGV[0] =~ /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})$/ or exit 1;
    my $time = timegm($6, $5, $4, $3, $2 - 1, $1);
    print strftime("%Y%m%dT%H%M%S", gmtime($time + 1)), "\n";
  ' "$1"
}

# Prints the key of the request of a task for a commit and verb, nothing when there is none.
find_queued() {
  local task="$1" sha="$2" verb="$3" id file
  while read -r id file; do
    [ "$(request_field "$file" sha)" = "$sha" ] && [ "$(request_field "$file" verb)" = "$verb" ] \
      && [ "$(request_field "$file" task)" = "$task" ] && { echo "$id"; return 0; }
  done < <(list_requests)
  return 0
}

# ---- cancel -----------------------------------------------------------------------------

# Adds the host-written line "cancelled=1" to a queue file, atomically. The runner keeps it
# when it rewrites the state, so a cancel survives a runner restart. Call it with the queue
# lock held. The mark is never taken back.
mark_request_cancelled() {
  local file="$1" tmp
  request_is_cancelled "$file" && return 0
  tmp="$(mktemp "$QUEUE_DIR/.new.XXXXXX")"
  { cat "$file"; echo "cancelled=1"; } >"$tmp" && mv "$tmp" "$file"
}

request_is_cancelled() {
  [ "$(request_field "$1" cancelled 2>/dev/null)" = "1" ]
}

# Succeeds when the request of a key was cancelled. The worker asks before every phase.
cancel_requested() {
  request_is_cancelled "$(queue_file "$1")"
}

# ---- state transitions (queue lock held) --------------------------------------------------

# Queued to running. Fails when the request was cancelled or already left the queue.
claim_request_locked() {
  local file="$1" started="$2"
  [ -f "$file" ] || return 1
  request_is_cancelled "$file" && return 1
  set_request_state "$file" running "pid=$$" "started=$started"
}

# The one place where a request ends: its result is published and its file leaves the queue. A
# request whose result was already published only leaves the queue (a result is final). A
# cancelled request gets the cancelled result, whatever else was computed for it; otherwise the
# given command writes the result. A request that already left the queue publishes nothing.
# usage: end_request_locked <queue file> <seconds> <command that writes the ordinary result...>
end_request_locked() {
  local file="$1" seconds="$2"
  shift 2
  [ -f "$file" ] || return 0
  if request_has_result "$file"; then
    :
  elif request_is_cancelled "$file"; then
    write_cancelled_result_of "$file" "$seconds" || return 1
  else
    "$@" || return 1
  fi
  rm -f "$file"
}

# Succeeds when the result of a request was already published: the result file of its id exists
# and names the same task, verb and commit.
request_has_result() {
  local file="$1" task result
  task="$(request_field "$file" task)"
  is_valid_task_name "$task" || return 1
  result="$(task_results_dir "$task")/$(result_id_of_key "$(request_key "$file")").json"
  [ -f "$result" ] && [ "$(result_field "$result" task)" = "$task" ] \
    && [ "$(result_field "$result" verb)" = "$(request_field "$file" verb)" ] \
    && [ "$(result_field "$result" sha)" = "$(request_field "$file" sha)" ]
}

# write_cancelled_result_of <queue file> <seconds>; the caller validated the fields of the file.
write_cancelled_result_of() {
  local file="$1" started
  started="$(request_field "$file" started)"
  write_cancelled_result "$(request_field "$file" task)" "$(result_id_of_key "$(request_key "$file")")" \
    "$(request_field "$file" verb)" "$(request_field "$file" sha)" "${started:-$(now_iso)}" "$2"
}

# write_cancelled_result <task> <id> <verb> <sha> <started> <seconds>
write_cancelled_result() {
  mkdir -p "$(task_results_dir "$1")"
  echo "[box] cancelled" >>"$(task_results_dir "$1")/$2.log"
  write_result "$1" "$2" "$3" "$4" "$CANCELLED_EXIT_CODE" "$6" 0 "$5" false false true
}

# ---- results ----------------------------------------------------------------------------

# write_result <task> <id> <verb> <sha> <exit_code> <seconds> <peak_bytes> <started> <timed_out> <oom_killed> [cancelled] [extra fields as a JSON object]
# Every field is produced by the runner (validated names, numbers, timestamps). "where" is the
# place the check ran in: a result of the Mac is not evidence against the agent, which runs as
# the person there, and whoever reads a result must be able to tell. Every result
# carries the checks fields (changesChecks, checksTouched, checksReason, testsTouched, tests):
# result_checks_json fills in the conservative values for whatever the extras do not say. A result
# is final: the file is linked into place and an existing result is never replaced (fails then).
write_result() {
  local dir tmp
  dir="$(task_results_dir "$1")"
  mkdir -p "$dir"
  tmp="$dir/.$2.$$.$RANDOM.tmp"
  cat >"$tmp" <<JSON
{
  "id": "$2",
  "task": "$1",
  "where": "$(task_where "$1")",
  "verb": "$3",
  "sha": "$4",
  "exit_code": $5,
  "seconds": $6,
  "peak_memory_bytes": $7,
  "started": "$8",
  "finished": "$(now_iso)",
  "timed_out": $9,
  "oom_killed": ${10},
  "cancelled": ${11:-false}
}
JSON
  merge_result_extras "$tmp" "$(result_checks_json "${11:-false}" "${12:-}")" || { rm -f "$tmp" "$tmp.merged"; return 1; }
  if ! ln "$tmp" "$dir/$2.json" 2>/dev/null; then
    rm -f "$tmp"
    warn "result $2 of task '$1' already exists and is not replaced"
    return 1
  fi
  rm -f "$tmp"
}

# Adds the fields of a JSON object to a result file, keeping one field per line.
merge_result_extras() {
  jq --argjson extra "$2" '. + $extra' "$1" >"$1.merged" && mv "$1.merged" "$1"
}

# Prints a field of a result file, without quotes.
result_field() {
  sed -n "s/^ *\"$2\": *\"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}$/\1/p" "$1" | head -1
}

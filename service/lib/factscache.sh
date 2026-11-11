# shellcheck shell=bash
# A cache of the facts of each workspace (the reads behind `box facts` and `box status`), so
# that a call does not run git over a workspace in which nothing changed. It is for display
# only: take, verify, rm and the runner always read fresh. --fresh bypasses it; so does an entry
# older than a safety limit.
#
# The host reads no path of a workspace for it, not even a change time: the agent writes those
# paths. One workspace container answers, before it runs any git, whether the workspace is as it
# was at the cached read (image/runner/facts.py and workspace_key.py: the branch head read as
# bytes through checked descriptors, the change time and size of the files that move with the
# commits, and no file of the working tree changed since). The host adds what it knows without
# the workspace: the base recorded for the task and the state of its agent container.

readonly FACTS_CACHE_DIR="$AGENTBOX_HOME/cache/facts"
readonly FACTS_CACHE_MAX_AGE="${BOX_FACTS_CACHE_MAX_AGE:-600}"
FACTS_FRESH=0

# The part of an entry's key that the host knows without the workspace.
facts_host_key() {
  local task="$1" state
  state="$(dk inspect -f '{{.State.Status}} {{.State.StartedAt}}' "$(agent_container_name "$task")" 2>/dev/null)" || state=none
  echo "base=$(task_base_sha "$task") agent=$state"
}

# This moment in nanoseconds since the epoch (to the microsecond, rounded down).
now_nanoseconds() {
  perl -MTime::HiRes=gettimeofday -e 'my ($seconds, $micro) = gettimeofday; printf "%d%06d000\n", $seconds, $micro'
}

facts_cache_lock() {
  echo "$LOCK_DIR/facts-$1.lock"
}

# An entry is one file: the facts, the key of the workspace they were read under
# (cacheWorkspaceKey), the host's part of the key (cacheHostKey) and the moment the read began
# (cacheReference). It is renamed into place whole, so one read of the file gives one
# publication. Prints the entry when it may still be asked about; fails otherwise.
facts_cache_entry() {
  local task="$1" entry="$FACTS_CACHE_DIR/$1.json"
  [ "$FACTS_FRESH" != "1" ] && [ -f "$entry" ] || return 1
  [ $(( $(now_epoch) - $(file_modified "$entry") )) -lt "$FACTS_CACHE_MAX_AGE" ] || return 1
  jq -ce --arg host "$(facts_host_key "$task")" 'select(type == "object" and .cacheHostKey == $host
    and (.cacheWorkspaceKey | type) == "string" and (.cacheReference | type) == "string")' "$entry" 2>/dev/null
}

# The facts of an entry as they are shown: without what the cache keeps for itself, and only when
# the head they name is the head of the key they were read under.
facts_cache_shown() {
  jq -ce '(.cacheWorkspaceKey | capture("^head=(?<head>[0-9a-f]{40}) ").head) as $head
    | select(.head == $head) | del(.cacheWorkspaceKey, .cacheHostKey, .cacheReference)' <<<"$1" 2>/dev/null
}

# facts_cache_write <task> <facts> <workspace key> <host key> <moment the read began>: publishes
# facts read through the sandbox. The entry is written whole under a name of its own.
facts_cache_write() {
  local task="$1" json="$2" tmp
  mkdir -p "$FACTS_CACHE_DIR"
  tmp="$(mktemp "$FACTS_CACHE_DIR/.$task.XXXXXX")" || return 1
  if jq -ce --arg workspace "$3" --arg host "$4" --arg reference "$5" 'select(type == "object")
      | . + {cacheWorkspaceKey: $workspace, cacheHostKey: $host, cacheReference: $reference}' <<<"$json" >"$tmp" \
    && with_lock "$(facts_cache_lock "$task")" facts_cache_publish_locked "$task" "$4" "$5" "$tmp"; then
    return 0
  fi
  rm -f "$tmp"
  return 1
}

# Renames a prepared entry into place unless it went stale while it was read: the host's part of
# the key must still hold, and an entry of a read that began later stays. (Whether the workspace
# moved during the read is checked by the container that read it, which then gives no key.)
facts_cache_publish_locked() {
  local task="$1" host="$2" reference="$3" tmp="$4" entry="$FACTS_CACHE_DIR/$1.json" published
  [ "$(facts_host_key "$task")" = "$host" ] || return 1
  published="$(jq -r '.cacheReference // "0"' "$entry" 2>/dev/null)" || published=0
  [[ "$published" =~ ^[0-9]+$ ]] || published=0
  if [ "${#published}" -gt "${#reference}" ] || { [ "${#published}" -eq "${#reference}" ] && [[ "$published" > "$reference" ]]; }; then
    return 1
  fi
  mv "$tmp" "$entry"
}

# The validated facts of a workspace: the cached ones when a workspace container says that
# nothing changed since they were read, else a full read, which is cached when it succeeded and
# the workspace has a key.
cached_workspace_facts_json() {
  local task="$1" entry worktree facts key host reference
  require_jq
  reference="$(now_nanoseconds)"
  host="$(facts_host_key "$task")"
  if entry="$(facts_cache_entry "$task")"; then
    worktree="$(read_workspace "$task" "$(jq -r .cacheWorkspaceKey <<<"$entry")" "$(jq -r .cacheReference <<<"$entry")")" || return 1
    if [ "$(jq -r '.unchanged // false' <<<"$worktree")" = "true" ]; then
      facts="$(facts_cache_shown "$entry")" && { echo "$facts"; return 0; }
      worktree="$(read_workspace "$task")" || return 1
    fi
  else
    worktree="$(read_workspace "$task")" || return 1
  fi
  facts="$(complete_facts "$task" "$worktree")" || return 1
  key="$(jq -r '.key // empty' <<<"$worktree")"
  [ -z "$key" ] || facts_cache_write "$task" "$facts" "$key" "$host" "$reference" || true
  echo "$facts"
}

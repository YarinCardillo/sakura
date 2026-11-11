#!/bin/bash
# Publishing a facts cache entry: the facts, the key they were read under and the moment the
# read began are one file, written under a name of its own and published inside the task's
# lock, and only while they are still valid. Reads that finish together cannot mix their parts.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
CACHE="$AGENTBOX_HOME/cache/facts"
ENTRY="$CACHE/t1.json"
HEAD_SHA="$(workspace_head t1)"
readonly WRITERS=12
readonly WORKSPACE_KEY="head=$HEAD_SHA 1:1 2:2 - 3:3"

# in_box <shell code> [arguments]: runs code with the libraries of box loaded.
in_box() {
  AGENTBOX_HOME="$AGENTBOX_HOME" BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names docker project sandbox journal task request checks factscache session; do . "$BOX_ROOT/lib/$lib.sh"; done
    code="$1"; shift
    eval "$code"' _ "$@" 2>/dev/null
}

# moment <number>: the moment a read began, in nanoseconds, later for a larger number.
moment() {
  printf '17000000000000000%02d' "$1"
}

# write_entry <marker> <moment> [host key]: one read that publishes.
write_entry() {
  in_box 'facts_cache_write t1 "{\"head\":\"$3\",\"marker\":\"$1\"}" "$4" "${5:-$(facts_host_key t1)}" "$2"' \
    "$1" "$2" "$HEAD_SHA" "$WORKSPACE_KEY" "${3:-}"
}

echo " reads that finish at the same moment publish one whole entry, the newest read"
for round in 1 2 3; do
  rm -f "$ENTRY"
  for writer in $(seq 1 "$WRITERS"); do
    write_entry "$writer" "$(moment "$writer")" &
  done
  wait
  assert_eq "round $round: the entry is the newest read" "$WRITERS" "$(jq -r .marker "$ENTRY")"
  assert_eq "round $round: with the moment of that read" "$(moment "$WRITERS")" "$(jq -r .cacheReference "$ENTRY")"
  assert_eq "round $round: and the key it was read under" "$WORKSPACE_KEY" "$(jq -r .cacheWorkspaceKey "$ENTRY")"
done

echo " an older read never replaces a newer entry"
write_entry old "$(moment 1)"
assert_eq "the newer entry stays" "$WRITERS" "$(jq -r .marker "$ENTRY")"
write_entry newer "$(moment 50)"
assert_eq "a newer read replaces it" "newer" "$(jq -r .marker "$ENTRY")"

echo " a read whose key no longer holds is not published"
write_entry stale "$(moment 60)" "base=$(printf '0%.0s' {1..40}) agent=running since yesterday"
assert_eq "the entry is unchanged" "newer" "$(jq -r .marker "$ENTRY")"

echo " nothing else is left in the cache directory"
assert_eq "one file per task" "t1.json" "$(/bin/ls -A "$CACHE" | tr '\n' ' ' | sed 's/ $//')"

echo " the entry is served by box facts without what the cache keeps for itself"
: >"$FAKE_DOCKER_LOG"
rm -f "$ENTRY"
FIRST="$("$BOX" facts t1 --json 2>/dev/null)"
SECOND="$("$BOX" facts t1 --json 2>/dev/null)"
assert_eq "one full read for two calls" "1" "$(grep -c -e '/box/commit_facts.py' "$FAKE_DOCKER_LOG")"
assert_eq "the same answer" "$FIRST" "$SECOND"
assert_eq "no field of the cache is part of the answer" "0" "$(jq '[keys[] | select(startswith("cache"))] | length' <<<"$SECOND")"
finish

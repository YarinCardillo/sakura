#!/bin/bash
# Each take owns its output directory, including failed and overlapping takes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 bundle
"$BOX" facts t1 --json >/dev/null
: >"$FAKE_DOCKER_LOG"
"$BOX" take t1 >"$TMP/take-one" 2>"$TMP/take-one.err" &
ONE=$!
"$BOX" take t1 >"$TMP/take-two" 2>"$TMP/take-two.err" &
TWO=$!
wait "$ONE"; wait "$TWO"
PATHS="$(grep '/box/bundle.sh ' "$FAKE_DOCKER_LOG" | sed -n 's/.*-v \(.*\):\/out .*/\1/p')"
assert_eq 'both operations made a bundle' 2 "$(wc -l <<<"$PATHS" | tr -d ' ')"
assert_eq 'the output directories are distinct' 2 "$(sort -u <<<"$PATHS" | wc -l | tr -d ' ')"
while IFS= read -r directory; do
  assert_eq 'no operation mounts the shared bundles root' 1 "$([ "$directory" != "$AGENTBOX_HOME/bundles" ] && echo 1 || echo 0)"
  assert_no_file 'the operation removes its directory' "$directory"
done <<<"$PATHS"
assert_eq 'no bundle is retained' 0 "$(find "$AGENTBOX_HOME/bundles" -name '*.bundle' | wc -l | tr -d ' ')"

# Refuse the fetch after the real bundle was created, to exercise the error cleanup.
assert_status 'a failed import ends unsuccessfully' 1 env BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker project sandbox accounts native journal task; do . "$BOX_ROOT/lib/$lib.sh"; done
  person_git() { return 1; }
  cmd_take t1'
assert_eq 'failure also leaves no bundle' 0 "$(find "$AGENTBOX_HOME/bundles" -name '*.bundle' | wc -l | tr -d ' ')"
finish

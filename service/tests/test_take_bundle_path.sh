#!/bin/bash
# Each take owns its output directory, including failed and overlapping takes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 bundle
"$BOX" facts t1 --json >/dev/null
: >"$FAKE_DOCKER_LOG"
export FAKE_BUNDLE_MOUNTS="$TMP/bundle-mounts"
: >"$FAKE_BUNDLE_MOUNTS"
"$BOX" take t1 >"$TMP/take-one" 2>"$TMP/take-one.err" &
ONE=$!
"$BOX" take t1 >"$TMP/take-two" 2>"$TMP/take-two.err" &
TWO=$!
wait "$ONE"; wait "$TWO"
PATHS=()
while IFS= read -r -d '' directory; do PATHS+=("$directory"); done <"$FAKE_BUNDLE_MOUNTS"
assert_eq 'both operations made a bundle' 2 "${#PATHS[@]}"
UNIQUE="$(printf '%s\0' "${PATHS[@]}" | python3 -c 'import sys; print(len(set(sys.stdin.buffer.read().split(b"\0")) - {b""}))')"
assert_eq 'the output directories are distinct' 2 "$UNIQUE"
for directory in "${PATHS[@]}"; do
  assert_eq 'no operation mounts the shared bundles root' 1 "$([ "$directory" != "$AGENTBOX_HOME/bundles" ] && echo 1 || echo 0)"
  assert_no_file 'the operation removes its directory' "$directory"
done
assert_eq 'no bundle is retained' 0 "$(find "$AGENTBOX_HOME/bundles" -name '*.bundle' | wc -l | tr -d ' ')"

# Refuse the fetch after the real bundle was created, to exercise the error cleanup.
assert_status 'a failed import ends unsuccessfully' 1 env BOX_ROOT="$BOX_ROOT" bash -c '
  set -eo pipefail
  for lib in common names docker project sandbox accounts native journal task request; do . "$BOX_ROOT/lib/$lib.sh"; done
  eval "$(declare -f person_git | sed "1s/person_git/original_person_git/")"
  person_git() {
    case " $* " in *" fetch "*) return 1 ;; esac
    original_person_git "$@"
  }
  cmd_take t1'
assert_eq 'the refused fetch really followed a completed bundle' 3 "$(tr -cd '\0' <"$FAKE_BUNDLE_MOUNTS" | wc -c | tr -d ' ')"
assert_eq 'failure also leaves no bundle' 0 "$(find "$AGENTBOX_HOME/bundles" -name '*.bundle' | wc -l | tr -d ' ')"
finish

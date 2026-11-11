#!/bin/bash
# The carried service works from read-only resources and keeps its writes in a data home with spaces.
set -u
. "$(dirname "$0")/lib.sh"
setup_sandbox

cleanup_bundle() {
  case "${runtime:-}" in
    engine) "$BOX" engine stop >/dev/null 2>&1 || true ;;
    runner) "$BOX" runner --stop >/dev/null 2>&1 || true ;;
  esac
  if declare -F cleanup_sandbox >/dev/null; then cleanup_sandbox; fi
  chmod -R u+w "$TMP" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup_bundle EXIT

mkdir -p "$TMP/Application Support" "$TMP/person"
mv "$AGENTBOX_HOME" "$TMP/Application Support/Sakura"
export AGENTBOX_HOME="$TMP/Application Support/Sakura"
export HOME="$TMP/person"
export BOX_DEFAULT_WHERE=mac
carried="$TMP/Sakura.app/Contents/Resources/service"
mkdir -p "$(dirname "$carried")"
cp -R "$BOX_ROOT" "$carried"
chmod -R a-w "$carried"
BOX="$carried/bin/box"

assert_status "a task is created from read-only bundled resources" 0 "$BOX" new bundled sandbox --where mac
assert_status "tasks can be read with a spaced data home" 0 "$BOX" tasks --json
assert_contains "the bundled task is listed" 'bundled' "$OUTPUT"
if [ -f "$carried/engine/package.json" ]; then
  runtime=engine
  start_engine
else
  runtime=runner
  assert_status "the bundled runner starts" 0 "$BOX" runner --daemon
fi
assert_status "a check runs from read-only bundled resources" 0 "$BOX" verify bundled test
assert_status "the bundled result is readable" 0 "$BOX" facts bundled --json
OUTPUT="$(printf '%s\n' "$OUTPUT" | jq .)"
assert_contains "the check succeeds" '"exitCode": 0' "$OUTPUT"
assert_file "task data lives outside the resources" "$AGENTBOX_HOME/tasks/bundled.env"
assert_status "the bundled task is removed" 0 "$BOX" rm bundled

exit "$FAILURES"

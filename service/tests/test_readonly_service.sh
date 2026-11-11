#!/bin/bash
# A shipped service is read-only. The app supplies a fresh data home, never its own bundle.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
SERVICE="$TMP/service"
mkdir -p "$SERVICE" "$TMP/person"
for item in bin lib image rules profiles proxy launchd compose.yaml; do cp -R "$BOX_ROOT/$item" "$SERVICE/"; done
find "$SERVICE" -name __pycache__ -type d -prune -exec rm -rf {} +
chmod -R a-w "$SERVICE"
trap 'chmod -R u+w "$SERVICE"; rm -rf "$TMP"' EXIT
export HOME="$TMP/person"
export AGENTBOX_HOME="$TMP/fresh-data"
export BOX_PROFILES_DIR="$TMP/empty-profiles"
mkdir -p "$BOX_PROFILES_DIR"
BOX="$SERVICE/bin/box"
assert_status 'first read on an empty home' 0 "$BOX" projects --json
assert_eq 'there are no imported projects' '[]' "$OUTPUT"
assert_no_file 'no legacy work directory was needed' "$HOME/agentbox-home"
assert_status 'a project is registered without setup steps' 0 "$BOX" project add app --repo "$SOURCE" --subdir sub --check 'test=printf "1 passing\n"' --setup 'touch ready'
assert_status 'a native lavoro starts from the bundle' 0 "$BOX" new m1 app
assert_file 'setup ran in the lavoro' "$AGENTBOX_HOME/work/m1/sub/ready"
rm "$AGENTBOX_HOME/work/m1/sub/ready"
assert_status 'facts from a read-only service' 0 "$BOX" facts m1 --json
assert_eq 'facts identify the native place' mac "$(jq -r '.where' <<<"$OUTPUT")"
assert_status 'read files from a read-only service' 0 "$BOX" tree m1 --json
assert_status 'queue a native check' 0 "$BOX" verify m1 test --no-wait
assert_status 'run the check with no Docker' 0 "$BOX" runner --once
RESULT="$(find "$AGENTBOX_HOME/results/m1" -name '*.json' | head -1)"
assert_eq 'the check succeeds' 0 "$(jq -r '.exit_code' "$RESULT")"
assert_eq 'native work used no Docker' '' "$(cat "$FAKE_DOCKER_LOG")"
assert_no_file 'Python did not write into the bundle' "$SERVICE/image/runner/__pycache__"
assert_status 'native cleanup works from the bundle' 0 "$BOX" rm m1 --force

# Bind-mount sources used by boxes must live in data home as well. App bundles may be
# outside the directories shared by the Docker VM. The fake Docker records mount sources.
assert_status 'a box project can be made from the bundle' 0 "$BOX" project set app --where box
assert_status 'a box can start from the bundle' 0 "$BOX" new b1 app
assert_status 'and run' 0 "$BOX" run b1 -- true
assert_status 'queue a box check from the bundle' 0 "$BOX" verify b1 test --no-wait
assert_status 'the runner can use bundled resources' 0 "$BOX" runner --once
RUNNER="$(find "$AGENTBOX_HOME/runtime" -type d -path '*/image/runner' | head -1)"
for path in "$RUNNER" "$RUNNER/facts.py"; do
  MODE="$(stat_of %Lp %a "$path")"
  assert_eq 'public runner resources are readable and executable by the container uid' 5 "$(( 8#$MODE & 5 ))"
done
# An inherited internal path must not defeat the app-selected data home.
CONTAINER_RESOURCES="$SERVICE" "$BOX" run b1 -- true >/dev/null 2>&1
assert_eq 'no bind mount comes from the service bundle'  0 "$(grep '^run ' "$FAKE_DOCKER_LOG" | grep -c -e "-v $SERVICE/")"
finish

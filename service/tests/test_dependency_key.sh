#!/bin/bash
# The dependencies the runner builds once and mounts into every job of a task are named by a
# key. The key is a whole SHA-256 over everything they are built from, each part with its
# length before it: the exact bytes of package.json and of the lockfile, the install and the
# rebuild command, and the image (its content id, system and architecture). A short key can be
# made to collide, and a commit would then run on the dependencies of another commit.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly HEX64='^[0-9a-f]{64}$'
key() {
  BOX_ROOT="$BOX_ROOT" INSTALL="${INSTALL-npm ci}" REBUILD="${REBUILD-}" bash -c '
    for lib in common names docker project project_detect project_admin sandbox accounts native native_commands native_read native_checks native_shell journal task request checks factscache cancel launchd runner; do . "$BOX_ROOT/lib/$lib.sh"; done
    INSTALL="$2" REBUILD="$3"
    dependency_key "$1"' _ "$1" "${INSTALL-npm ci}" "${REBUILD-}" 2>/dev/null
}
readonly FILES_ONE="$(printf 'a%.0s' {1..64})" FILES_TWO="$(printf 'b%.0s' {1..64})"

echo " the digest of the package files"
digest() {
  PATH="$FAKE_BIN:$PATH" "$BOX_ROOT/image/runner/files_digest.sh" "$@"
}
printf 'ab' >"$TMP/one"; printf 'c' >"$TMP/two"; printf 'a' >"$TMP/three"; printf 'bc' >"$TMP/four"
assert_eq "is a whole SHA-256" "1" "$([[ "$(digest "$TMP/one" "$TMP/two")" =~ $HEX64 ]] && echo 1 || echo 0)"
assert_eq "the same bytes split another way between the two files are another digest" "1" "$([ "$(digest "$TMP/one" "$TMP/two")" != "$(digest "$TMP/three" "$TMP/four")" ] && echo 1 || echo 0)"
assert_eq "the same files give the same digest" "$(digest "$TMP/one" "$TMP/two")" "$(digest "$TMP/one" "$TMP/two")"

echo " the key"
BASE_KEY="$(key "$FILES_ONE")"
assert_eq "is a whole SHA-256" "1" "$([[ "$BASE_KEY" =~ $HEX64 ]] && echo 1 || echo 0)"
assert_eq "the same inputs give the same key" "$BASE_KEY" "$(key "$FILES_ONE")"
differs() {
  assert_eq "$1" "1" "$([[ "$2" =~ $HEX64 ]] && [ "$2" != "$BASE_KEY" ] && echo 1 || echo 0)"
}
differs "other package files: another key" "$(key "$FILES_TWO")"
differs "another install command" "$(INSTALL='npm install' key "$FILES_ONE")"
differs "another rebuild command" "$(REBUILD='npm rebuild' key "$FILES_ONE")"
differs "another image" "$(FAKE_IMAGE_ID="$(printf '2%.0s' {1..64})" key "$FILES_ONE")"
differs "another architecture" "$(FAKE_IMAGE_PLATFORM=linux/amd64 key "$FILES_ONE")"
differs "the same text split another way between the two commands" "$(INSTALL='npm' REBUILD=' ci' key "$FILES_ONE")"
assert_status "an image that cannot be read: no key, so nothing is reused" 1 env FAKE_IMAGE_UNREADABLE=1 bash -c "$(declare -f key); FILES='$FILES_ONE'; BOX_ROOT='$BOX_ROOT'; key \"\$FILES\" >/dev/null"

echo " a check builds its dependencies under the whole key and reuses them only under it"
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 work
run_test() {
  "$BOX" verify t1 test --no-wait 2>/dev/null
  "$BOX" runner --once >/dev/null 2>&1
}
: >"$FAKE_DOCKER_LOG"
run_test
MARKER="$(/bin/ls "$AGENTBOX_HOME"/deps/t1.*.ok 2>/dev/null | head -1)"
KEY="$(basename "$MARKER" .ok)"; KEY="${KEY#t1.}"
assert_eq "the marker is named by a whole key" "1" "$([[ "$KEY" =~ $HEX64 ]] && echo 1 || echo 0)"
assert_eq "and holds it" "$KEY" "$(cat "$MARKER")"
assert_eq "the volume of the job is named by it" "1" "$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG" | grep -c -e "agentbox-pristine\.t1\.$KEY:")"
assert_eq "(the dependencies were built once)" "1" "$(grep -c -e '--name agentbox-install-t1\.' "$FAKE_DOCKER_LOG")"
commit_in_task t1 more
run_test
assert_eq "the same inputs: built once, reused" "1" "$(grep -c -e '--name agentbox-install-t1\.' "$FAKE_DOCKER_LOG")"
echo "something else" >"$MARKER"
commit_in_task t1 again
run_test
assert_eq "a marker that does not hold the whole key is not believed: built again" "2" "$(grep -c -e '--name agentbox-install-t1\.' "$FAKE_DOCKER_LOG")"
: >"$AGENTBOX_HOME/deps/t1.aaaaaaaaaaaa.ok"
commit_in_task t1 "another image"
FAKE_IMAGE_ID="$(printf '3%.0s' {1..64})" run_test
assert_eq "another image: other dependencies, built for it" "3" "$(grep -c -e '--name agentbox-install-t1\.' "$FAKE_DOCKER_LOG")"
assert_eq "a marker with the 12 digits of an older box names no volume that is used" "0" "$(grep -c -e 'agentbox-pristine\.t1\.aaaaaaaaaaaa[:. ]' "$FAKE_DOCKER_LOG")"
finish

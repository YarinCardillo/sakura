#!/bin/bash
# An installed image with the old verification protocol must not suppress the new build.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_EXISTING_IMAGES=agentbox:3
"$BOX" up >/dev/null
assert_eq 'an image with the space-safe verification reader is built' 1 "$(grep -c '^build ' "$FAKE_DOCKER_LOG")"
assert_eq 'the previous installed image is left intact' 0 "$(grep '^build ' "$FAKE_DOCKER_LOG" | grep -c -e '-t agentbox:3 ')"
: >"$FAKE_DOCKER_LOG"
"$BOX" up >/dev/null
assert_eq 'the compatible built image is reused' 0 "$(grep -c '^build ' "$FAKE_DOCKER_LOG")"
finish

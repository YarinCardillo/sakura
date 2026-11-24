#!/bin/bash
# box up --check: one quick answer, as JSON, to "can a box be used now?", for an app that must
# know before it starts anything in a box. It asks Docker three things and starts nothing:
# whether Docker answers, whether the image is there, whether the proxy runs. "ready" is all
# three. After box up the answer is ready.
. "$(dirname "$0")/lib.sh"
setup_sandbox

# check: the answer of box up --check, in $CHECK; its status in $STATUS.
check() {
  STATUS=0
  CHECK="$("$BOX" up --check 2>/dev/null)" || STATUS=$?
}
field() {
  jq -r ".$1" <<<"$CHECK"
}

echo " Docker that does not answer"
FAKE_DOCKER_DOWN=1 check
assert_eq "the check itself succeeds" "0" "$STATUS"
assert_eq "valid JSON" "0" "$(jq -e . <<<"$CHECK" >/dev/null 2>&1; echo $?)"
assert_eq "docker false" "false" "$(field docker)"
assert_eq "not ready" "false" "$(field ready)"

echo " a machine that never ran box up"
export FAKE_EXISTING_IMAGES=""
: >"$FAKE_DOCKER_LOG"
check
assert_eq "docker answers" "true" "$(field docker)"
assert_eq "no image" "false" "$(field image)"
assert_eq "no proxy" "false" "$(field proxy)"
assert_eq "not ready" "false" "$(field ready)"
assert_eq "nothing was built or started" "0" "$(grep -c -e '^build' -e 'compose' "$FAKE_DOCKER_LOG")"

echo " after box up"
"$BOX" up >/dev/null 2>&1
check
assert_eq "the image is there" "true" "$(field image)"
assert_eq "the proxy runs" "true" "$(field proxy)"
assert_eq "ready" "true" "$(field ready)"

echo " a proxy that stopped"
rm -f "$FAKE_DOCKER_LOG.proxy"
check
assert_eq "not ready" "false" "$(field ready)"
assert_eq "the image is still there" "true" "$(field image)"

echo " the usage names it"
assert_contains "in the help" "up --check" "$("$BOX" help)"
finish

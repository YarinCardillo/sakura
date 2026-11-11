#!/bin/bash
# The first install of a task (box new) has a deadline the project's install command cannot
# leave: the command reaches the container as a value and runs whole under timeout, and the
# docker client that waits for it is ended on the host when the deadline passes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
readonly ESCAPING_INSTALL="true'; sleep 999999; #"
readonly PATIENCE_SECONDS=40

echo " an install command with a quote in it stays one value, under the timeout"
edit_in_place "s|^INSTALL=.*|INSTALL=$ESCAPING_INSTALL|" "$AGENTBOX_HOME/projects/sandbox.env"
assert_status "box new" 0 "$BOX" new t1 sandbox
CALL="$(grep -e '--name agentbox-ws-t1-install' "$FAKE_DOCKER_LOG" | head -1)"
assert_contains "the command is passed as a value of the environment" "-e BOX_INSTALL=$ESCAPING_INSTALL" "$CALL"
PROGRAM="${CALL#* agentbox:* }"
assert_contains "the program runs that value under timeout" 'timeout --kill-after="$2" "$3" bash -c "$BOX_INSTALL"' "$PROGRAM"
assert_eq "and holds none of the command's text" "0" "$(grep -c -e 'sleep 999999' <<<"$PROGRAM")"

echo " an install container that never ends: box new ends at its deadline"
STARTED=$SECONDS
FAKE_WS_INSTALL_HANG=1 FAKE_HANG_SECONDS=60 BOX_INSTALL_TIMEOUT=1 BOX_DEADLINE_GRACE=1 perl -e 'alarm shift; exec @ARGV' "$PATIENCE_SECONDS" \
  "$BOX" new t2 sandbox >"$TMP/new.out" 2>"$TMP/new.err"
assert_eq "it fails (after $((SECONDS - STARTED)) s)" "1" "$?"
assert_contains "and says so in one line" "box: dependency install failed" "$(tail -n 1 "$TMP/new.err")"
assert_contains "the container was asked to go" "rm -f agentbox-ws-t2-install" "$(grep -e '^rm ' "$FAKE_DOCKER_LOG")"
finish

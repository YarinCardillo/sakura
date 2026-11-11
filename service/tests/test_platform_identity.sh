#!/bin/bash
# Inherited OSTYPE cannot select another system's stat, date or default shell.
. "$(dirname "$0")/lib.sh"
setup_sandbox
case "$(/usr/bin/uname -s)" in Darwin) SPOOF=linux-gnu; EXPECTED_SHELL=/bin/zsh ;; *) SPOOF=darwin; EXPECTED_SHELL=/bin/bash ;; esac
assert_status 'task creation ignores inherited OSTYPE' 0 env OSTYPE="$SPOOF" "$BOX" new t1 sandbox
assert_status 'facts ignore inherited OSTYPE' 0 env OSTYPE="$SPOOF" "$BOX" facts t1 --json
ACTUAL="$(OSTYPE="$SPOOF" BOX_ROOT="$BOX_ROOT" bash -c '
  . "$BOX_ROOT/lib/common.sh"
  file_identity "$AGENTBOX_HOME"
  epoch_to_zulu 0
  default_person_shell
' 2>"$TMP/platform.err")"
assert_contains 'the host stat identity is used' "$(stat_of '%d:%i' '%d:%i' "$AGENTBOX_HOME")" "$ACTUAL"
assert_contains 'the host date syntax is used' '1970-01-01T00:00:00Z' "$ACTUAL"
assert_contains 'the host default shell is used' "$EXPECTED_SHELL" "$ACTUAL"
assert_eq 'no incompatible system-tool errors' '' "$(cat "$TMP/platform.err")"
finish

#!/bin/bash
# box login: runs `claude auth login` and then `claude auth status --json` in the same container
# with the account volume as the config directory; exit 0 only when the status says loggedIn.
. "$(dirname "$0")/lib.sh"
setup_sandbox
echo " the container runs the dedicated commands with the account volume"
"$BOX" login personal >/dev/null 2>&1; STATUS=$?
LOGIN_CALL="$(grep -e 'agentbox.kind=login' "$FAKE_DOCKER_LOG" | tail -1)"
assert_eq "exit 0 when the container says logged in" "0" "$STATUS"
assert_contains "account volume is mounted" "-v agentbox-acct-personal:/acct" "$LOGIN_CALL"
assert_contains "config directory is the account volume" "CLAUDE_CONFIG_DIR=/acct/claude" "$LOGIN_CALL"
assert_contains "it runs the login script, not the program" "bash /box/login.sh" "$LOGIN_CALL"
assert_contains "the scripts are mounted read-only" "image/runner:/box:ro" "$LOGIN_CALL"
assert_eq "the whole program is not started" "0" "$(grep -c -e ' _ claude$' <<<"$LOGIN_CALL")"
assert_contains "the proxy is the only network" "--network agentbox-net" "$LOGIN_CALL"

echo " failures end with one line that starts with box: and a non-zero exit"
check_failure() {
  local code="$1" words="$2"
  FAKE_LOGIN_EXIT="$code" "$BOX" login personal >"$TMP/out" 2>"$TMP/err"; STATUS=$?
  assert_eq "container exit $code gives a non-zero exit" "1" "$([ "$STATUS" -ne 0 ] && echo 1 || echo 0)"
  assert_contains "last line starts with box: ($code)" "box: " "$(tail -n 1 "$TMP/err")"
  assert_contains "last line says what failed ($code)" "$words" "$(tail -n 1 "$TMP/err")"
}
check_failure 2 "sign-in was not completed"
check_failure 3 "not logged in"
check_failure 4 "could not read the login status"
check_failure 5 "no connection"
check_failure 125 "login container"

echo " the in-container script maps what claude reports to those exit codes"
BIN="$TMP/loginbin"; mkdir -p "$BIN"
cat >"$BIN/claude" <<'FAKE'
#!/bin/bash
case "$2" in
  login) exit "${FAKE_CLAUDE_LOGIN_EXIT:-0}" ;;
  status) [ -n "${FAKE_CLAUDE_STATUS_EXIT:-}" ] && exit "$FAKE_CLAUDE_STATUS_EXIT"; if [ -n "${FAKE_CLAUDE_STATUS:-}" ]; then echo "$FAKE_CLAUDE_STATUS"; else echo "{\"loggedIn\": true, \"authMethod\": \"claude.ai\"}"; fi ;;
esac
FAKE
cat >"$BIN/curl" <<'FAKE'
#!/bin/bash
exit "${FAKE_CURL_EXIT:-0}"
FAKE
chmod +x "$BIN/claude" "$BIN/curl"
run_script() { PATH="$BIN:$PATH" bash "$BOX_ROOT/image/runner/login.sh" </dev/null >"$TMP/script.out" 2>&1; }
assert_contains "the script runs claude auth login (subscription by default)" "claude auth login" "$(grep -v '^#' "$BOX_ROOT/image/runner/login.sh")"
assert_eq "the script never asks for the console" "0" "$(grep -c -e '--console' "$BOX_ROOT/image/runner/login.sh")"
run_script; assert_eq "logged in: 0" "0" "$?"
FAKE_CLAUDE_STATUS='{"loggedIn": false}' run_script; assert_eq "status says not logged in: 3" "3" "$?"
FAKE_CLAUDE_STATUS='not json' run_script; assert_eq "unreadable status: 4" "4" "$?"
FAKE_CLAUDE_STATUS_EXIT=1 run_script; assert_eq "status fails: 4" "4" "$?"
FAKE_CLAUDE_LOGIN_EXIT=1 run_script; assert_eq "login fails with a connection: 2" "2" "$?"
FAKE_CLAUDE_LOGIN_EXIT=1 FAKE_CURL_EXIT=28 run_script; assert_eq "login fails without a connection: 5" "5" "$?"
finish

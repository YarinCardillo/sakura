#!/bin/bash
# box acp <task> [<account>] for a lavoro on the Mac: the same adapter, run natively in the
# worktree with the account's config directory, installed once under tools/, behind the same
# filter and deadlines as the conversation of a box, one front end per lavoro, and nothing left
# running when the channel closes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log"
: >"$FAKE_NPM_LOG"
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$TMP/login-a" "$TMP/login-b"
printf 'ACCOUNT_personal=%s\nACCOUNT_work=%s\nACCOUNT_gone=%s\n' "$TMP/login-a" "$TMP/login-b" "$TMP/no-such-login" >"$AGENTBOX_HOME/config"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
ACP_LOG="$AGENTBOX_HOME/acp/m1.log"
ADAPTER_VERSION="$(sed -n 's/^ARG CLAUDE_AGENT_ACP_VERSION=//p' "$BOX_ROOT/image/Dockerfile")"
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true}}}'
readonly NEW_SESSION='{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/x","mcpServers":[]}}'
readonly STALLED_EXIT_CODE=66
readonly PATIENCE_SECONDS=40

# wait_bounded <pid>: its exit code in ENDED, or "never" after killing it.
wait_bounded() {
  local second
  for second in $(seq 1 "$PATIENCE_SECONDS"); do
    if ! kill -0 "$1" 2>/dev/null; then wait "$1"; ENDED=$?; return; fi
    sleep 1
  done
  kill -KILL "$1" 2>/dev/null
  ENDED="never"
}

is_alive() {
  ps -p "$1" -o pid= 2>/dev/null | wc -l | tr -d ' '
}

echo " a conversation: the adapter runs in the worktree with the account's config directory"
: >"$FAKE_DOCKER_LOG"
printf '%s\n%s\n' "$INITIALIZE" "$NEW_SESSION" | FAKE_ACP_LOGGED_IN=1 "$BOX" acp m1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "exit 0 after stdin closed" "0" "$?"
assert_eq "two answers, one per line, all JSON-RPC" "2 2" "$(wc -l <"$TMP/acp.out" | tr -d ' ') $(jq -c 'select(.jsonrpc == "2.0")' "$TMP/acp.out" | wc -l | tr -d ' ')"
assert_eq "the session is opened" "fake-session" "$(jq -r 'select(.id == 2) | .result.sessionId' "$TMP/acp.out")"
assert_eq "nothing on stderr after a clean end" "" "$(cat "$TMP/acp.err")"
assert_contains "it ran in the worktree" "fake adapter started in $W" "$(cat "$ACP_LOG")"
assert_contains "with the config directory of the account" "fake adapter config $TMP/login-a" "$(cat "$ACP_LOG")"
assert_eq "Docker was not requested anything" "" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "no pipe and no pid file are left" "0" "$(find "$AGENTBOX_HOME/acp" -type p -o -name '*.pid' | wc -l | tr -d ' ')"

echo " the adapter is installed once, pinned, under tools/"
assert_eq "the version is the one of the image" "1" "$(grep -c "NATIVE_ACP_VERSION=$ADAPTER_VERSION\$" "$BOX_ROOT/lib/native_acp.sh")"
assert_eq "npm was asked once" "1" "$(grep -c '^install ' "$FAKE_NPM_LOG")"
assert_contains "for that version of the adapter" "@agentclientprotocol/claude-agent-acp@$ADAPTER_VERSION" "$(cat "$FAKE_NPM_LOG")"
assert_file "it is under tools/, named with its version" "$AGENTBOX_HOME/tools/claude-agent-acp-$ADAPTER_VERSION/node_modules/.bin/claude-agent-acp"
printf '%s\n' "$INITIALIZE" | "$BOX" acp m1 work >"$TMP/acp.out" 2>/dev/null
assert_eq "a second conversation starts without npm" "1" "$(grep -c '^install ' "$FAKE_NPM_LOG")"
assert_contains "and with the other account's directory" "fake adapter config $TMP/login-b" "$(cat "$ACP_LOG")"

echo " box is the boundary here too"
assert_eq "the client's capabilities for files and terminals never reach the agent" "null" "$(jq -c '.result._meta.received | (.fs // null)' "$TMP/acp.out")"
{ printf '%s\n' '{"jsonrpc":"2.0","id":5,"method":"test/agent_talks"}'; sleep 3; } | FAKE_ACP_QUIET_REPLIES=1 "$BOX" acp m1 personal >"$TMP/acp.out" 2>/dev/null
assert_eq "of what a hostile agent sends, the client gets the permission request, the update and the answer" \
  "session/request_permission session/update response" "$(jq -r '.method // "response"' "$TMP/acp.out" | tr '\n' ' ' | sed 's/ $//')"
assert_contains "the refusals are counted in the log" "3 request(s) of the agent refused" "$(tail -n 1 "$ACP_LOG")"

echo " the account: named, or the project's, and it must be one of the Mac"
assert_status "no account anywhere" 1 "$BOX" acp m1
assert_contains "it says how to name one" "box project set" "$OUTPUT"
assert_status "an account the config does not name" 1 "$BOX" acp m1 nobody
assert_contains "it says so" "'nobody' is not an account of the Mac" "$OUTPUT"
assert_status "an account whose directory is gone" 1 "$BOX" acp m1 gone
assert_contains "it says so" "does not exist" "$OUTPUT"
assert_eq "without printing where that directory is" "0" "$(grep -c -e "$TMP/no-such-login" <<<"$OUTPUT")"
assert_eq "nothing on stdout for a refusal" "" "$("$BOX" acp m1 nobody 2>/dev/null)"
"$BOX" project set app --account work >/dev/null
"$BOX" new m2 app >/dev/null
printf '%s\n' "$INITIALIZE" | "$BOX" acp m2 >/dev/null 2>&1
assert_eq "a lavoro of a project with an account needs none named" "0" "$?"
assert_contains "it used the project's" "fake adapter config $TMP/login-b" "$(cat "$AGENTBOX_HOME/acp/m2.log")"

echo " one front end per lavoro"
mkfifo "$TMP/stdin"
FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" "$BOX" acp m1 personal <"$TMP/stdin" >"$TMP/open.out" 2>"$TMP/open.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 40); do [ -s "$TMP/child.pid" ] && break; sleep 0.5; done
assert_eq "(the conversation is open, and the adapter started a program of its own)" "1" "$(is_alive "$(cat "$TMP/child.pid")")"
wait_for_started m1
assert_status "a second box acp is refused" 1 "$BOX" acp m1 personal
assert_contains "it says why" "a conversation is open for task 'm1'" "$OUTPUT"
printf '%s\n' "$INITIALIZE" >&5
exec 5>&-
wait_bounded "$SESSION"
assert_eq "the open one ends with 0 when its stdin closes" "0" "$ENDED"
assert_eq "and answered what it was sent" "1" "$(jq -r '.result.protocolVersion' "$TMP/open.out")"
sleep 1
assert_eq "what the adapter started is not left behind" "0" "$(still_alive "$(cat "$TMP/child.pid")")"

echo " an agent that does not end after the channel closed is ended, with everything it started"
rm -f "$TMP/child.pid"
printf '%s\n' "$INITIALIZE" | FAKE_ACP_IGNORE_EOF=1 FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" BOX_ACP_END_SECONDS=2 "$BOX" acp m1 personal >"$TMP/acp.out" 2>"$TMP/acp.err" &
wait_bounded $!
assert_eq "the deadline's exit code" "$STALLED_EXIT_CODE" "$ENDED"
assert_contains "the last line says why" "box: the agent of task 'm1' did not end" "$(tail -n 1 "$TMP/acp.err")"
sleep 1
assert_eq "nothing of it is left" "0 0" "$(still_alive "$(cat "$TMP/child.pid")") $(pgrep -f "$AGENTBOX_HOME/tools/claude-agent-acp-$ADAPTER_VERSION" | wc -l | tr -d ' ')"

echo " box stop ends an open conversation; a signal does too"
rm -f "$TMP/child.pid"
FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" "$BOX" acp m1 personal <"$TMP/stdin" >/dev/null 2>"$TMP/stop.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 40); do [ -s "$TMP/child.pid" ] && break; sleep 0.5; done
assert_status "box stop" 0 "$BOX" stop m1
wait_bounded "$SESSION"
exec 5>&-
assert_eq "the conversation ended as terminated" "143" "$ENDED"
assert_contains "with one line that says so" "box: " "$(tail -n 1 "$TMP/stop.err")"
sleep 1
assert_eq "nothing of it is left" "0" "$(still_alive "$(cat "$TMP/child.pid")")"
assert_status "box stop with nothing open" 1 "$BOX" stop m1
assert_contains "it says so" "no conversation is open for task 'm1'" "$OUTPUT"
rm -f "$TMP/child.pid"
FAKE_ACP_CHILD_PID_FILE="$TMP/child.pid" "$BOX" acp m1 personal <"$TMP/stdin" >/dev/null 2>"$TMP/term.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 40); do [ -s "$TMP/child.pid" ] && break; sleep 0.5; done
kill -TERM "$SESSION"
wait_bounded "$SESSION"
exec 5>&-
assert_eq "a signal: the exit code of a terminated adapter" "143" "$ENDED"
sleep 1
assert_eq "nothing of it is left" "0" "$(still_alive "$(cat "$TMP/child.pid")")"

echo " an install that fails: one line, nothing on stdout, and the next start tries again"
rm -rf "$AGENTBOX_HOME/tools"
OUT="$(printf '%s\n' "$INITIALIZE" | FAKE_NPM_FAIL=1 "$BOX" acp m1 personal 2>"$TMP/acp.err")"
assert_eq "exit 1" "1" "$?"
assert_eq "nothing on stdout" "" "$OUT"
assert_contains "one line that names the log" "box: could not install the Claude ACP adapter" "$(cat "$TMP/acp.err")"
assert_eq "nothing half installed is left" "0" "$(/bin/ls -A "$AGENTBOX_HOME/tools" | grep -c -v '^install.log$')"
printf '%s\n' "$INITIALIZE" | "$BOX" acp m1 personal >"$TMP/acp.out" 2>/dev/null
assert_eq "the next start installs and answers" "0 1" "$? $(jq -r '.result.protocolVersion' "$TMP/acp.out")"

echo " the app is told where the lavoro is"
assert_eq "facts carry its folder" "$W" "$("$BOX" facts m1 --json | jq -r .folder)"
"$BOX" new b1 app --where box >/dev/null 2>&1
assert_eq "a task in a box has none on the Mac" "null" "$("$BOX" facts b1 --json | jq -r .folder)"
assert_status "a task in a box still needs its account named" 1 "$BOX" acp b1
finish

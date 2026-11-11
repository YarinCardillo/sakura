#!/bin/bash
# box acp <task> <account>: the account's ACP adapter in the box of the task, with stdin and
# stdout passed through untouched (newline-delimited JSON-RPC, no TTY), the same confinement as
# box agent, one front end per task at a time, and an end when stdin closes.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_ACCOUNTS="personal"
"$BOX" new t1 sandbox >/dev/null 2>&1
ACP_LOG="$AGENTBOX_HOME/acp/t1.log"
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'
readonly NEW_SESSION='{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/work/t1","mcpServers":[]}}'
readonly AUTH_REQUIRED_CODE=-32000

# acp_session <task> <account>: sends the two requests, closes stdin; stdout, stderr and status are kept.
acp_session() {
  : >"$FAKE_DOCKER_LOG"
  printf '%s\n%s\n' "$INITIALIZE" "$NEW_SESSION" | "$BOX" acp "$1" "$2" >"$TMP/acp.out" 2>"$TMP/acp.err"
  ACP_STATUS=$?
  ACP_CALL="$(grep -e '--name agentbox-acp-' "$FAKE_DOCKER_LOG" | head -1)"
}

echo " a session: only the adapter's JSON lines on stdout, an end when stdin closes"
acp_session t1 personal
assert_eq "exit 0 after stdin closed" "0" "$ACP_STATUS"
assert_eq "two answers, one per line" "2" "$(wc -l <"$TMP/acp.out" | tr -d ' ')"
assert_eq "every line of stdout is JSON-RPC" "2" "$(jq -c 'select(.jsonrpc == "2.0")' "$TMP/acp.out" | wc -l | tr -d ' ')"
assert_eq "initialize is answered" "1" "$(jq -r 'select(.id == 1) | .result.protocolVersion' "$TMP/acp.out")"
assert_eq "an account that is not signed in gets the authentication-required answer" "$AUTH_REQUIRED_CODE" "$(jq -r 'select(.id == 2) | .error.code' "$TMP/acp.out")"
assert_eq "nothing on stderr after a clean end" "" "$(cat "$TMP/acp.err")"
assert_contains "the adapter's stderr is in the host-private log" "fake adapter started" "$(cat "$ACP_LOG")"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"

echo " the container: stdin without a TTY, removed at the end, confined like box agent"
assert_contains "interactive" " -i " "$ACP_CALL"
assert_eq "no TTY" "0" "$(grep -c -e ' -it ' -e ' -t ' -e ' --tty' <<<"$ACP_CALL")"
assert_contains "removed when it ends" "run --rm " "$ACP_CALL"
assert_contains "the task's own network" "--network agentbox-net-t1" "$ACP_CALL"
assert_contains "agent limits" "--cpus 2 --memory 3g" "$ACP_CALL"
assert_contains "no capabilities" "--cap-drop ALL --security-opt no-new-privileges" "$ACP_CALL"
assert_contains "read-only root" "--read-only" "$ACP_CALL"
assert_contains "the workspace is the working directory" "-w /work/t1" "$ACP_CALL"
assert_contains "the task's own configuration volume" "-v agentbox-cfg-t1.personal:/acct" "$ACP_CALL"
assert_contains "the config directory" "CLAUDE_CONFIG_DIR=/acct/claude" "$ACP_CALL"
assert_contains "only the credential file of the account" "src=agentbox-acct-personal,dst=/acct/claude/.credentials.json,volume-subpath=claude/.credentials.json" "$ACP_CALL"
assert_eq "the account volume is not mounted whole" "0" "$(grep -c -e '-v agentbox-acct-personal' <<<"$ACP_CALL")"
assert_contains "labelled with the task" "agentbox.task=t1" "$ACP_CALL"
assert_contains "the adapter is the command" " claude-agent-acp" "$ACP_CALL"
assert_contains "of the image named by the one constant" " $(sed -n 's/^readonly IMAGE="\(.*\)"$/\1/p' "$BOX_ROOT/lib/common.sh") claude-agent-acp" "$ACP_CALL"

echo " an account without a login starts all the same, without the credential file"
export FAKE_ACCOUNT_FILES=""
acp_session t1 personal
assert_eq "exit 0" "0" "$ACP_STATUS"
assert_eq "the session request ends in the adapter's answer" "$AUTH_REQUIRED_CODE" "$(jq -r 'select(.id == 2) | .error.code' "$TMP/acp.out")"
assert_eq "no credential file is mounted" "0" "$(grep -c -e 'credentials.json' <<<"$ACP_CALL")"
assert_contains "its own configuration volume still is" "-v agentbox-cfg-t1.personal:/acct" "$ACP_CALL"
unset FAKE_ACCOUNT_FILES
FAKE_ACCOUNTS="" acp_session t1 nobody
assert_eq "an account that was never created: exit 0, the adapter answers" "0 $AUTH_REQUIRED_CODE" "$ACP_STATUS $(jq -r 'select(.id == 2) | .error.code' "$TMP/acp.out")"
assert_eq "its volume is not created" "0" "$(grep -c -e 'volume create.*agentbox-acct-nobody' "$FAKE_DOCKER_LOG")"

echo " a signed-in account opens a session"
FAKE_ACP_LOGGED_IN=1 acp_session t1 personal
assert_eq "session/new answers with a session" "fake-session" "$(jq -r 'select(.id == 2) | .result.sessionId' "$TMP/acp.out")"

echo " box is the boundary: the client's capabilities for files and terminals never reach the agent"
readonly GREEDY_INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true,"auth":{"terminal":true},"session":{"notices":{}},"madeUp":{"x":1}},"clientInfo":{"name":"careless","version":"1"}}}'
: >"$FAKE_DOCKER_LOG"
printf '%s\n' "$GREEDY_INITIALIZE" | "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "exit 0" "0" "$?"
assert_eq "the agent was told only what box knows to be harmless" '{"session":{"notices":{}}}' "$(jq -cS '.result._meta.received' "$TMP/acp.out")"
printf '%s\n' "$INITIALIZE" | "$BOX" acp t1 personal >"$TMP/acp.out" 2>/dev/null
assert_eq "a client that names no capabilities is passed on as it is" "null" "$(jq -c '.result._meta.received' "$TMP/acp.out")"

echo " what the agent may send to the client: responses, session updates, permission requests"
{ printf '%s\n' '{"jsonrpc":"2.0","id":7,"method":"test/agent_talks"}'; sleep 3; } | "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "exit 0" "0" "$?"
assert_eq "every line of stdout is a JSON-RPC message" "$(wc -l <"$TMP/acp.out" | tr -d ' ')" "$(jq -c 'select(.jsonrpc == "2.0")' "$TMP/acp.out" | wc -l | tr -d ' ')"
assert_eq "the request for a file of the client is not passed on" "0" "$(grep -c -e 'fs/read_text_file' -e '/etc/passwd' "$TMP/acp.out")"
assert_eq "nor the request for a terminal" "0" "$(grep -c 'terminal/create' "$TMP/acp.out")"
assert_eq "nor a request hidden behind a duplicate key" "0" "$(grep -c -e 'fs/write_text_file' -e 'dup-1' "$TMP/acp.out")"
assert_eq "nor a notification box does not know" "0" "$(grep -c '_auth/status_update' "$TMP/acp.out")"
assert_eq "nor a line that is not JSON" "0" "$(grep -c 'this is not json' "$TMP/acp.out")"
assert_eq "the permission request passes" "perm-1" "$(jq -r 'select(.method == "session/request_permission") | .id' "$TMP/acp.out")"
assert_eq "the update passes with the same meaning" \
  '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"content":{"text":"café is \"quoted\" 1.5 [1,2]","type":"text"},"sessionUpdate":"agent_message_chunk"}}}' \
  "$(jq -cS 'select(.method == "session/update")' "$TMP/acp.out")"
assert_eq "the response passes" '{"id":7,"jsonrpc":"2.0","result":{}}' "$(jq -cS 'select(.id == 7)' "$TMP/acp.out")"
assert_eq "three messages reached the client" "3" "$(wc -l <"$TMP/acp.out" | tr -d ' ')"
assert_contains "the agent got an error for the file request" 'reply "fs-1" "error -32601"' "$(cat "$ACP_LOG")"
assert_contains "and for the terminal request" 'reply "term-1" "error -32601"' "$(cat "$ACP_LOG")"
assert_contains "and for the hidden one" 'reply "dup-1" "error -32601"' "$(cat "$ACP_LOG")"
assert_contains "each refusal is in the host-private log" "refused the agent's request fs/read_text_file" "$(cat "$ACP_LOG")"
assert_contains "and they are counted there" "3 request(s) of the agent refused (fs/read_text_file: 1, fs/write_text_file: 1, terminal/create: 1), 2 other message(s) of the agent dropped" "$(cat "$ACP_LOG")"
assert_eq "nothing of this on the client's stderr" "" "$(cat "$TMP/acp.err")"

echo " a line longer than the limit ends the session, whoever sends it"
{ printf '%s\n' '{"jsonrpc":"2.0","id":8,"method":"test/huge_line"}'; sleep 5; } | "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "a 10 MB line from the agent: its own exit code" "65" "$?"
assert_eq "nothing of it reached the client" "0" "$(wc -c <"$TMP/acp.out" | tr -d ' ')"
assert_contains "the last line says why" "box: " "$(tail -n 1 "$TMP/acp.err")"
assert_contains "in words" "a line of more than" "$(tail -n 1 "$TMP/acp.err")"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"
{ printf '{"jsonrpc":"2.0","id":9,"method":"initialize","params":{"pad":"'; head -c 10485760 /dev/zero | tr '\0' 'x'; printf '"}}\n'; sleep 3; } | "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "a 10 MB line from the client: the same exit code" "65" "$?"
assert_eq "the agent never saw it (no answer)" "0" "$(wc -c <"$TMP/acp.out" | tr -d ' ')"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"
BOX_ACP_MAX_LINE_BYTES=100 "$BOX" acp t1 personal <<<"$GREEDY_INITIALIZE" >"$TMP/acp.out" 2>/dev/null
assert_eq "the limit can be set" "65" "$?"

echo " an adapter that fails: its exit code, and a last line on stderr that says why"
FAKE_ACP_CRASH=1 acp_session t1 personal
assert_eq "the adapter's exit code" "3" "$ACP_STATUS"
assert_eq "nothing on stdout" "" "$(cat "$TMP/acp.out")"
assert_contains "the last line starts with box:" "box: " "$(tail -n 1 "$TMP/acp.err")"
assert_contains "and carries the adapter's last words" "cannot start" "$(tail -n 1 "$TMP/acp.err")"
assert_eq "one line only" "1" "$(wc -l <"$TMP/acp.err" | tr -d ' ')"

echo " an adapter that floods its stderr: the log stays at its limit and box still ends with its one line"
readonly FLOOD_BYTES=209715200
readonly ACP_LOG_LIMIT=4194304
readonly LOG_RESERVE=2560
# The client's input ends at once and the flood takes a while on a busy machine: this is about
# the log, so the end deadline is kept out of its way.
BOX_ACP_END_SECONDS=300 FAKE_ACP_STDERR_BYTES="$FLOOD_BYTES" FAKE_ACP_CRASH=1 acp_session t1 personal
assert_eq "the adapter's exit code" "3" "$ACP_STATUS"
LOG_SIZE="$(wc -c <"$ACP_LOG" | tr -d ' ')"
assert_eq "200 MB of stderr leave a log within the limit (got $LOG_SIZE bytes)" "1" "$([ "$LOG_SIZE" -le "$ACP_LOG_LIMIT" ] && echo 1 || echo 0)"
assert_contains "the log says it was cut, at the limit less the room kept for the last lines" "[box] log cut at $((ACP_LOG_LIMIT - LOG_RESERVE)) bytes" "$(tail -c 200 "$ACP_LOG")"
assert_eq "one line on stderr" "1" "$(wc -l <"$TMP/acp.err" | tr -d ' ')"
assert_contains "which starts with box:" "box: the adapter of task 't1' ended with exit code 3" "$(cat "$TMP/acp.err")"
assert_eq "and is short" "1" "$([ "$(wc -c <"$TMP/acp.err")" -le 400 ] && echo 1 || echo 0)"
assert_eq "no pipe is left in the log directory" "0" "$(find "$AGENTBOX_HOME/acp" -type p | wc -l | tr -d ' ')"

echo " an agent that does not end after its input closed is ended"
readonly STALLED_EXIT_CODE=66
readonly PATIENCE_SECONDS=60
STARTED=$SECONDS
: >"$FAKE_DOCKER_LOG"
printf '%s\n' "$INITIALIZE" | FAKE_ACP_IGNORE_EOF=1 BOX_ACP_END_SECONDS=2 "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "its own exit code" "$STALLED_EXIT_CODE" "$?"
assert_eq "well within a minute (took $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -le "$PATIENCE_SECONDS" ] && echo 1 || echo 0)"
assert_eq "what it had answered was delivered" "1" "$(jq -r '.result.protocolVersion' "$TMP/acp.out")"
assert_contains "the last line says why" "box: the agent of task 't1' did not end" "$(tail -n 1 "$TMP/acp.err")"
assert_contains "the container is removed by name" "rm -f agentbox-acp-t1" "$(cat "$FAKE_DOCKER_LOG")"
assert_eq "and is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"

echo " a client that stops reading does not hold the session open for ever"
STARTED=$SECONDS
{ printf '%s\n' '{"jsonrpc":"2.0","id":12,"method":"test/flood"}'; sleep "$PATIENCE_SECONDS"; } \
  | BOX_ACP_STALL_SECONDS=2 "$BOX" acp t1 personal 2>"$TMP/acp.err" | sleep "$PATIENCE_SECONDS" &
PIPELINE=$!
for _ in $(seq 1 "$PATIENCE_SECONDS"); do grep -q '^box: ' "$TMP/acp.err" 2>/dev/null && break; sleep 1; done
assert_contains "box ends the session and says why" "box: the client of task 't1' did not take" "$(tail -n 1 "$TMP/acp.err")"
assert_eq "long before the client moved (after $((SECONDS - STARTED)) s)" "1" "$([ $((SECONDS - STARTED)) -lt "$PATIENCE_SECONDS" ] && echo 1 || echo 0)"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"
kill "$PIPELINE" $(pgrep -P "$PIPELINE") 2>/dev/null
wait "$PIPELINE" 2>/dev/null

echo " refusals under many names: the log keeps its limit and the count is not lost"
readonly SMALL_LOG_LIMIT=8192
readonly MANY_REQUESTS=400
{ printf '%s\n' '{"jsonrpc":"2.0","id":11,"method":"test/many_requests"}'; sleep 25; } \
  | FAKE_ACP_REQUESTS="$MANY_REQUESTS" FAKE_ACP_QUIET_REPLIES=1 BOX_ACP_MAX_LOG_BYTES="$SMALL_LOG_LIMIT" "$BOX" acp t1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "exit 0" "0" "$?"
assert_eq "only the response reached the client" "1" "$(wc -l <"$TMP/acp.out" | tr -d ' ')"
LOG_SIZE="$(wc -c <"$ACP_LOG" | tr -d ' ')"
assert_eq "the whole log, summary included, is within its limit (got $LOG_SIZE bytes)" "1" "$([ "$LOG_SIZE" -le "$SMALL_LOG_LIMIT" ] && echo 1 || echo 0)"
SUMMARY="$(tail -n 1 "$ACP_LOG")"
assert_contains "the last line counts every refusal" "$MANY_REQUESTS request(s) of the agent refused" "$SUMMARY"
assert_eq "it names at most 20 methods" "20" "$(grep -o 'made/up-[0-9]*' <<<"$SUMMARY" | wc -l | tr -d ' ')"
assert_contains "and counts the rest" "$((MANY_REQUESTS - 20)) under other names" "$SUMMARY"
assert_eq "the summary itself is short" "1" "$([ "${#SUMMARY}" -le 2000 ] && echo 1 || echo 0)"

echo " refusals are one box: line on stderr and nothing on stdout"
assert_status "a task that does not exist" 1 "$BOX" acp nope personal
assert_contains "it says so" "box: task 'nope' does not exist" "$OUTPUT"
assert_status "an invalid account name" 1 "$BOX" acp t1 "Bad Name"
assert_status "missing arguments" 1 "$BOX" acp t1
assert_eq "nothing on stdout" "" "$("$BOX" acp nope personal 2>/dev/null)"

echo " one front end per task: not while the terminal agent runs"
"$BOX" agent t1 personal --detach >/dev/null 2>&1
: >"$FAKE_DOCKER_LOG"
OUT="$(printf '%s\n' "$INITIALIZE" | "$BOX" acp t1 personal 2>"$TMP/acp.err")"
assert_eq "refused" "1" "$?"
assert_eq "nothing on stdout" "" "$OUT"
assert_contains "it says which front end is open" "box: an agent runs in the terminal for task 't1'" "$(cat "$TMP/acp.err")"
assert_eq "no adapter was started" "0" "$(grep -c -e '--name agentbox-acp-' "$FAKE_DOCKER_LOG")"
"$BOX" stop t1 >/dev/null 2>&1

echo " and the reverse: no terminal agent, and no second conversation, while one is open"
mkfifo "$TMP/stdin"
"$BOX" acp t1 personal <"$TMP/stdin" >"$TMP/open.out" 2>"$TMP/open.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 20); do docker inspect -f '{{.State.Running}}' agentbox-acp-t1 2>/dev/null | grep -q true && break; sleep 0.5; done
wait_for_started t1
assert_eq "(the conversation is open)" "true" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 2>/dev/null)"
assert_status "box agent is refused" 1 "$BOX" agent t1 personal --detach
assert_contains "it says why" "a conversation is open for task 't1'" "$OUTPUT"
assert_status "a second box acp is refused" 1 "$BOX" acp t1 personal
assert_contains "it says why" "a conversation is open for task 't1'" "$OUTPUT"
printf '%s\n' "$INITIALIZE" >&5
exec 5>&-
wait "$SESSION"
assert_eq "the open conversation ends with 0 when its stdin closes" "0" "$?"
assert_eq "and answered what it was sent" "1" "$(jq -r '.result.protocolVersion' "$TMP/open.out")"

echo " two front ends started at the same moment: exactly one opens"
mkfifo "$TMP/stdin-a" "$TMP/stdin-b"
"$BOX" acp t1 personal <"$TMP/stdin-a" >/dev/null 2>"$TMP/race-a.err" &
FIRST=$!
"$BOX" acp t1 personal <"$TMP/stdin-b" >/dev/null 2>"$TMP/race-b.err" &
SECOND=$!
exec 5>"$TMP/stdin-a" 6>"$TMP/stdin-b"
sleep 6
OPEN=0
for session in "$FIRST" "$SECOND"; do if kill -0 "$session" 2>/dev/null; then OPEN=$((OPEN + 1)); fi; done
assert_eq "one conversation is open" "1" "$OPEN"
assert_eq "the other was refused with one box: line" "1" "$(cat "$TMP/race-a.err" "$TMP/race-b.err" | grep -c '^box: ')"
exec 5>&- 6>&-
wait "$FIRST"; FIRST_STATUS=$?
wait "$SECOND"; SECOND_STATUS=$?
assert_eq "exit codes: one clean end, one refusal" "0 1" "$(printf '%s\n' "$FIRST_STATUS" "$SECOND_STATUS" | sort | tr '\n' ' ' | sed 's/ $//')"
"$BOX" acp t1 personal <"$TMP/stdin-a" >/dev/null 2>"$TMP/race-a.err" &
FIRST=$!
"$BOX" agent t1 personal --detach >/dev/null 2>"$TMP/race-b.err"
AGENT_STATUS=$?
exec 5>"$TMP/stdin-a"
sleep 4
CONVERSATION_OPEN=0
if kill -0 "$FIRST" 2>/dev/null; then CONVERSATION_OPEN=1; fi
AGENT_RUNS="$(docker inspect -f '{{.State.Running}}' agentbox-agent-t1 2>/dev/null | grep -c true)"
assert_eq "a conversation and a terminal agent started together: one of the two" "1" "$((CONVERSATION_OPEN + AGENT_RUNS))"
exec 5>&-
wait "$FIRST" 2>/dev/null
"$BOX" stop t1 >/dev/null 2>&1

echo " a signal ends the adapter and the container"
"$BOX" acp t1 personal <"$TMP/stdin" >/dev/null 2>"$TMP/term.err" &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 20); do docker inspect -f '{{.State.Running}}' agentbox-acp-t1 2>/dev/null | grep -q true && break; sleep 0.5; done
wait_for_started t1
kill -TERM "$SESSION"
wait "$SESSION"
TERM_STATUS=$?
exec 5>&-
assert_eq "exit code of a terminated adapter" "143" "$TERM_STATUS"
assert_eq "the container is gone" "1" "$(docker inspect -f '{{.State.Running}}' agentbox-acp-t1 >/dev/null 2>&1; echo $?)"
assert_contains "the last line says why" "box: " "$(tail -n 1 "$TMP/term.err")"

echo " box stop ends an open conversation"
"$BOX" acp t1 personal <"$TMP/stdin" >/dev/null 2>&1 &
SESSION=$!
exec 5>"$TMP/stdin"
for _ in $(seq 1 20); do docker inspect -f '{{.State.Running}}' agentbox-acp-t1 2>/dev/null | grep -q true && break; sleep 0.5; done
wait_for_started t1
assert_status "box stop" 0 "$BOX" stop t1
assert_contains "it stopped the conversation's container" "stop agentbox-acp-t1" "$(cat "$FAKE_DOCKER_LOG")"
exec 5>&-
wait "$SESSION" 2>/dev/null
finish

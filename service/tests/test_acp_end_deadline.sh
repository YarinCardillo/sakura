#!/bin/bash
# The end of a conversation has one deadline. It starts when the client closes its side, and
# an agent that is still there when it passes is told to end at that moment. Writing out what
# the agent said last does not get a deadline of its own after it: a client that reads slowly,
# with an agent that keeps talking, cannot keep the session (and the lock of the front end)
# for the stall allowance on top.
. "$(dirname "$0")/lib.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/box-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
FAILURES=0
readonly END_SECONDS=2 STALL_SECONDS=40

# An agent that ignores the end of its input and keeps sending notifications the relay passes on.
cat >"$TMP/agent" <<AGENT
#!/usr/bin/perl
\$| = 1;
open(my \$pid, '>', '$TMP/agent.pid'); print \$pid \$\$; close \$pid;
my \$line = '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","text":"' . ('x' x 2000) . '"}}';
while (1) { print \$line, "\n"; }
AGENT
chmod +x "$TMP/agent"

echo " the client closes its side and reads slowly; the agent goes on talking"
mkfifo "$TMP/client"
# The client: it opens its end and reads nothing for a long time.
(exec 3<"$TMP/client"; sleep 60) &
SLOW_CLIENT=$!
STARTED="$(date +%s)"
perl "$BOX_ROOT/lib/acp_proxy.pl" 1000000 "$END_SECONDS" "$STALL_SECONDS" "$TMP/report" "$TMP/agent" </dev/null >"$TMP/client" 2>"$TMP/relay.err" &
RELAY=$!
for _ in $(seq 1 50); do [ -s "$TMP/agent.pid" ] && break; sleep 0.2; done
AGENT_PID="$(cat "$TMP/agent.pid")"
assert_eq "(the agent runs)" "1" "$(ps -p "$AGENT_PID" -o pid= | wc -l | tr -d ' ')"
# Well inside the stall allowance, a little after the end deadline: the agent must be gone.
for _ in $(seq 1 50); do ps -p "$AGENT_PID" -o pid= >/dev/null 2>&1 || break; sleep 0.2; done
GONE_AFTER=$(( $(date +%s) - STARTED ))
assert_eq "the agent was ended near the end deadline of $END_SECONDS s, not after $STALL_SECONDS more (it took $GONE_AFTER s)" "1" "$([ "$GONE_AFTER" -le 9 ] && echo 1 || echo 0)"
assert_eq "the relay is gone too" "0" "$(still_alive "$RELAY")"
wait "$RELAY" 2>/dev/null
assert_eq "its exit code says the session was ended" "66" "$?"
assert_contains "and why" "did not end within $END_SECONDS s" "$(cat "$TMP/relay.err")"
kill "$SLOW_CLIENT" "$AGENT_PID" 2>/dev/null
wait "$SLOW_CLIENT" 2>/dev/null
finish

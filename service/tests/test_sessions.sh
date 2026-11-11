#!/bin/bash
# Agent sessions and the runner daemon (fake docker).
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
export BOX_STDIN_IS_NOT_A_TTY=1

echo " detached agent"
: >"$FAKE_DOCKER_LOG"
assert_status "start" 0 "$BOX" agent t1 personal --detach -- --model x
CALL="$(grep '^run ' "$FAKE_DOCKER_LOG" | tail -1)"
assert_contains "detached with a tty" "run -d -it" "$CALL"
assert_contains "named after the task" "--name agentbox-agent-t1" "$CALL"
assert_contains "workspace mounted at /work/<task>" "$AGENTBOX_HOME/work/t1:/work/t1" "$CALL"
assert_contains "working directory is the clone" "-w /work/t1" "$CALL"
assert_contains "dependencies under the clone" ":/work/t1/sub/node_modules" "$CALL"
assert_contains "drop directory" "$AGENTBOX_HOME/drop/t1:/box/requests" "$CALL"
assert_contains "results read-only" "$AGENTBOX_HOME/results/t1:/box/results:ro" "$CALL"
assert_contains "account credentials come from the account volume" "src=agentbox-acct-personal,dst=/acct/claude/.credentials.json" "$CALL"
assert_contains "no network but the internal one" "--network agentbox-net" "$CALL"
assert_contains "claude args are passed" "claude --model x" "$CALL"
assert_status "a second start refuses" 1 "$BOX" agent t1 personal --detach
assert_contains "  says why" "already running" "$OUTPUT"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "agent running in status" "running" "$(jq -r '.tasks[0].agent.state' <<<"$OUT")"
assert_eq "agent account" "personal" "$(jq -r '.tasks[0].agent.account' <<<"$OUT")"
assert_eq "agent container" "agentbox-agent-t1" "$(jq -r '.tasks[0].agent.container' <<<"$OUT")"
assert_eq "exit code null while running" "null" "$(jq -r '.tasks[0].agent.exitCode' <<<"$OUT")"

echo " attach and stop"
: >"$FAKE_DOCKER_LOG"
assert_status "attach works on a running agent" 0 "$BOX" attach t1
assert_contains "recent log first" "logs --tail 2000 agentbox-agent-t1" "$(cat "$FAKE_DOCKER_LOG")"
assert_contains "detach keys that Claude Code does not use" 'attach --detach-keys ctrl-\,ctrl-\ agentbox-agent-t1' "$(cat "$FAKE_DOCKER_LOG")"
assert_status "stop" 0 "$BOX" stop t1
assert_contains "docker stop by name" "stop agentbox-agent-t1" "$(cat "$FAKE_DOCKER_LOG")"
assert_status "attach on a stopped agent exits 3" 3 "$BOX" attach t1
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "exited" "exited" "$(jq -r '.tasks[0].agent.state' <<<"$OUT")"
assert_eq "exit code" "137" "$(jq -r '.tasks[0].agent.exitCode' <<<"$OUT")"
"$BOX" status --json >/dev/null 2>&1
assert_eq "agent-exit journaled once" "1" "$("$BOX" journal --json | jq '[.[] | select(.event == "agent-exit")] | length')"
assert_eq "agent-start journaled" "personal" "$("$BOX" journal --json | jq -r '[.[] | select(.event == "agent-start")][0].account')"
assert_status "stop with nothing running fails" 1 "$BOX" stop t1

echo " restart removes the exited container"
: >"$FAKE_DOCKER_LOG"
assert_status "start again" 0 "$BOX" agent t1 personal --detach
assert_contains "old container removed" "rm -f agentbox-agent-t1" "$(cat "$FAKE_DOCKER_LOG")"

echo " shell"
: >"$FAKE_DOCKER_LOG"
"$BOX" shell t1 >/dev/null 2>&1 </dev/null
assert_contains "one-off interactive bash" "--rm -it" "$(grep '^run ' "$FAKE_DOCKER_LOG" | tail -1)"
assert_contains "in the clone" "-w /work/t1" "$(grep '^run ' "$FAKE_DOCKER_LOG" | tail -1)"

echo " runner daemon"
assert_status "start" 0 "$BOX" runner --daemon
assert_contains "status sees it" "running" "$("$BOX" status --json | jq -r .runner.state)"
assert_eq "pid is in the lock file" "1" "$(head -1 "$AGENTBOX_HOME/locks/runner.lock" | grep -c '^[0-9][0-9]*$')"
assert_status "a second start refuses" 1 "$BOX" runner --daemon
assert_file "log file" "$AGENTBOX_HOME/runner.log"
assert_status "stop" 0 "$BOX" runner --stop
assert_eq "status sees it stopped" "stopped" "$("$BOX" status --json | jq -r .runner.state)"
assert_status "stop again fails" 1 "$BOX" runner --stop
finish

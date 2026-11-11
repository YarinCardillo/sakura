#!/bin/bash
# Docker must not keep an unbounded second copy of supervised output.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export FAKE_ACCOUNTS=main
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 storage
: >"$FAKE_DOCKER_LOG"
drop_request t1 test
"$BOX" runner --once >/dev/null 2>&1
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n' | "$BOX" acp t1 main >/dev/null 2>&1
"$BOX" accounts --json >/dev/null
"$BOX" login main >/dev/null
"$BOX" agent t1 main --detach >/dev/null

for kind in job install sandbox acctfiles acp acctcheck; do
  lines="$(grep '^run ' "$FAKE_DOCKER_LOG" | grep -e "agentbox.kind=$kind ")"
  assert_eq "$kind is exercised" 1 "$([ -n "$lines" ] && echo 1 || echo 0)"
  [ -n "$lines" ] || continue
  while IFS= read -r line; do
    assert_contains "$kind disables Docker's duplicate log" '--log-driver none' "$line"
    assert_contains "$kind bounds tmpfs bytes" '--tmpfs /tmp:size=' "$line"
    assert_contains "$kind bounds tmpfs inodes" 'nr_inodes=' "$line"
  done <<<"$lines"
done
line="$(grep '^run ' "$FAKE_DOCKER_LOG" | grep -- '--name agentbox-agent-t1 ' | head -1)"
for flag in '--log-driver json-file' '--log-opt max-size=8m' '--log-opt max-file=2'; do
  assert_contains "detached agent: $flag" "$flag" "$line"
done
line="$(grep '^run ' "$FAKE_DOCKER_LOG" | grep 'agentbox.kind=login ' | head -1)"
for flag in '--log-driver json-file' '--log-opt max-size=8m' '--log-opt max-file=2'; do
  assert_contains "sign-in: $flag" "$flag" "$line"
done
for line in "$(cat "$BOX_ROOT/compose.yaml")"; do
  assert_contains 'proxy has rotated logs' 'max-size:' "$line"
  assert_contains 'proxy limits retained files' 'max-file:' "$line"
done
assert_contains 'proxy avoids routine request logs' 'LogLevel Warning' "$(cat "$BOX_ROOT/proxy/tinyproxy.conf")"
finish

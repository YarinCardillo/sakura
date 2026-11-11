#!/bin/bash
# Boxes do not share a network or an npm cache: one internal network and one cache volume per
# task, the proxy attached to each network, both removed with the task.
. "$(dirname "$0")/lib.sh"
setup_sandbox
PROXY=agentbox-proxy

echo " box new creates the network and the cache of the task"
"$BOX" new zz1 sandbox >/dev/null 2>&1
LOG="$(cat "$FAKE_DOCKER_LOG")"
assert_contains "an internal network for the task" "network create --internal --label agentbox=1 agentbox-net-zz1" "$LOG"
assert_contains "the proxy is attached to it" "network connect agentbox-net-zz1 $PROXY" "$LOG"
assert_contains "the install runs on it" "--network agentbox-net-zz1" "$(grep -e 'agentbox-ws-zz1-install' "$FAKE_DOCKER_LOG" | head -1)"
assert_contains "the cache volume is the task's" "-v agentbox-npm-cache-zz1:/npm-cache" "$LOG"
assert_eq "the shared cache volume is gone" "0" "$(grep -c -e '-v agentbox-npm-cache:/npm-cache' "$FAKE_DOCKER_LOG")"
assert_eq "no task container is on the shared network" "0" "$(grep -c -e '--network agentbox-net ' "$FAKE_DOCKER_LOG")"

echo " a second task gets its own"
"$BOX" new zz2 sandbox >/dev/null 2>&1
assert_contains "second network" "network create --internal --label agentbox=1 agentbox-net-zz2" "$(cat "$FAKE_DOCKER_LOG")"
SECOND_INSTALL="$(grep -e 'agentbox-ws-zz2-install' "$FAKE_DOCKER_LOG" | head -1)"
assert_contains "the second install is on its own network" "--network agentbox-net-zz2" "$SECOND_INSTALL"
assert_contains "and its own cache" "-v agentbox-npm-cache-zz2:/npm-cache" "$SECOND_INSTALL"
assert_eq "it does not touch the first task's" "0" "$(grep -c -e 'zz1' <<<"$SECOND_INSTALL" | sed 's/^[1-9].*/1/' | grep -c '^[1-9]')"

echo " runner jobs run on the network of their task"
export BOX_VERIFY_POLL=1
"$BOX" verify zz2 test --no-wait 2>/dev/null
: >"$FAKE_DOCKER_LOG"
"$BOX" runner --once 2>/dev/null
JOB="$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG" | head -1)"
assert_contains "job network" "--network agentbox-net-zz2" "$JOB"

echo " agents and shells use the task's network"
"$BOX" agent zz1 acct --detach >/dev/null 2>&1
"$BOX" shell zz1 >/dev/null 2>&1 </dev/null
assert_contains "shell" "--network agentbox-net-zz1" "$(grep -e '^run .*agentbox-ws-zz1' "$FAKE_DOCKER_LOG" | tail -1)"

echo " removing the task removes its network and cache"
: >"$FAKE_DOCKER_LOG"
"$BOX" rm zz1 --force >/dev/null 2>&1
LOG="$(cat "$FAKE_DOCKER_LOG")"
assert_contains "proxy detached" "network disconnect -f agentbox-net-zz1 $PROXY" "$LOG"
assert_contains "network removed" "network rm agentbox-net-zz1" "$LOG"
assert_contains "cache removed" "agentbox-npm-cache-zz1" "$(grep -e '^volume rm' "$FAKE_DOCKER_LOG")"
assert_eq "the other task's network stays" "0" "$(grep -c 'agentbox-net-zz2' <<<"$LOG")"
finish

#!/bin/bash
# Every Docker name box derives from a task, an account, a request or a lockfile hash is unique:
# two different inputs never give one name, and no task or account name can produce the name of
# another kind of object (a task called lock-<hash> must not own the runner's dependencies).
. "$(dirname "$0")/lib.sh"
setup_sandbox

export HASHES="4fed23771f16 aaaaaaaaaaaa"
export WORDS="a b c a-b b-c a-b-c 1 a-1 lock lock-4fed23771f16 lock-aaaaaaaaaaaa 4fed23771f16 cache-a npm-cache-a
  job-a nmjob-a co-a install a-install install-4fed23771f16 net egress proxy sampler acct-a acctcheck-a cfg-a agent-a
  ws-a ws-a-install acp-a pristine pristine-a nm-a"

# Prints "<namespace> <name> <kind>(<inputs>)" for every name the name functions give.
walk_names() {
  BOX_ROOT="$BOX_ROOT" bash -c '
    for lib in common names; do . "$BOX_ROOT/lib/$lib.sh"; done
    echo "network $NETWORK fixed(net)"
    echo "network agentbox-egress fixed(egress)"
    echo "container $PROXY_CONTAINER fixed(proxy)"
    echo "container agentbox-sampler fixed(sampler)"
    for word in $WORDS; do
      echo "volume $(deps_volume "$word") deps($word)"
      echo "volume $(npm_cache_volume "$word") cache($word)"
      echo "volume $(account_volume "$word") account($word)"
      echo "network $(task_network "$word") network($word)"
      echo "container $(agent_container_name "$word") agent($word)"
      echo "container $(acp_container_name "$word") acp($word)"
      echo "container $(account_check_container_name "$word") acctcheck($word)"
      for hash in $HASHES; do
        echo "volume $(pristine_deps_volume "$word" "$hash") pristine($word,$hash)"
        echo "container $(install_container_name "$word" "$hash") install($word,$hash)"
      done
      for account in $WORDS; do
        echo "volume $(config_volume "$word" "$account") config($word,$account)"
      done
      for suffix in install 1 12345; do
        echo "container $(workspace_container_name "$word" "$suffix") ws($word,$suffix)"
      done
      for id in 20260101T000000-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test 20260101T000000-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test-a; do
        for key in "$id" "$id.$word"; do
          echo "volume $(checkout_volume "$key") checkout($key)"
          echo "volume $(job_deps_volume "$key") jobdeps($key)"
          echo "container $(job_container_name "$key") job($key)"
        done
      done
    done' | sort -u
}

echo " every word used below is a valid task and account name"
for word in $WORDS; do
  [[ "$word" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]] || fail "'$word' is not a valid name"
done
pass "adversarial names are valid names"

echo " no two inputs give the same Docker name"
NAMES="$(walk_names)"
assert_eq "the walk produced names" "1" "$([ "$(wc -l <<<"$NAMES")" -gt 300 ] && echo 1 || echo 0)"
CLASHES="$(awk '{ key = $1 " " $2; if (key in seen && seen[key] != $3) print key ": " seen[key] " and " $3; seen[key] = $3 }' <<<"$NAMES")"
assert_eq "no clash within volumes, networks or containers" "" "$CLASHES"

echo " two tasks never share a configuration volume, whatever the account is called"
"$BOX" new a-b sandbox >/dev/null 2>&1
"$BOX" new a sandbox >/dev/null 2>&1
FAKE_ACCOUNTS="c b-c" "$BOX" agent a-b c --detach >/dev/null 2>&1
FAKE_ACCOUNTS="c b-c" "$BOX" agent a b-c --detach >/dev/null 2>&1
FIRST_CONFIG="$(grep -e '--name agentbox-agent-a-b ' "$FAKE_DOCKER_LOG" | grep -o -e '-v [^ ]*:/acct' | head -1)"
SECOND_CONFIG="$(grep -e '--name agentbox-agent-a ' "$FAKE_DOCKER_LOG" | grep -o -e '-v [^ ]*:/acct' | head -1)"
assert_contains "task a-b with account c has a configuration volume" "agentbox-cfg-" "$FIRST_CONFIG"
assert_contains "task a with account b-c has one too" "agentbox-cfg-" "$SECOND_CONFIG"
if [ "$FIRST_CONFIG" != "$SECOND_CONFIG" ]; then pass "they are two volumes"; else fail "both tasks mount $FIRST_CONFIG"; fi

echo " a task named like the runner's dependency volume gets other volumes"
LOCK_HASH="$(shasum -a 256 "$SOURCE/sub/package-lock.json" | cut -c1-12)"
EVIL="lock-$LOCK_HASH"
"$BOX" new "$EVIL" sandbox >/dev/null 2>&1
"$BOX" new t1 sandbox >/dev/null 2>&1
"$BOX" verify t1 test --no-wait 2>/dev/null
"$BOX" runner --once 2>/dev/null
AGENT_DEPS="$(grep -o -e "-v [^ ]*:/work/$EVIL/sub/node_modules" "$FAKE_DOCKER_LOG" | head -1 | sed 's/^-v //; s/:.*//')"
RUNNER_DEPS="$(grep -e '--name agentbox-job-' "$FAKE_DOCKER_LOG" | grep -o -e '-v [^ ]*:/ws/sub/node_modules:ro' | head -1 | sed 's/^-v //; s/:.*//')"
assert_contains "the task's writable dependencies are named after the task" "$EVIL" "$AGENT_DEPS"
assert_contains "the job mounted the pristine dependencies of its own task" "agentbox-pristine.t1." "$RUNNER_DEPS"
if [ "$AGENT_DEPS" != "$RUNNER_DEPS" ]; then pass "they are two volumes"; else fail "the task writes the runner's dependencies ($AGENT_DEPS)"; fi
finish

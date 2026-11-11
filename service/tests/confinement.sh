#!/bin/bash
# Live confinement checks, run from real agent containers (needs Docker, `box up` and a task).
# usage: tests/confinement.sh <task> [<attack task 1> <attack task 2>]
# The two optional tasks use the "selftest" profile and enable the host-side attack checks;
# the runner must be running (box runner --daemon).
# "must fail" checks pass when the command fails inside the container; "must work" checks
# pass when it succeeds. Exit status is the number of checks that did not hold.
set -u

readonly TASK="${1:?usage: confinement.sh <task> [<attack task 1> <attack task 2>]}"
readonly ATTACK_ONE="${2:-}"
readonly ATTACK_TWO="${3:-}"
# The data directory, as bin/box names it when AGENTBOX_HOME does not.
if [ -z "${AGENTBOX_HOME:-}" ]; then
  case "${OSTYPE:-}" in
    darwin*) AGENTBOX_HOME="$HOME/Library/Application Support/Sakura" ;;
    *) AGENTBOX_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/sakura" ;;
  esac
fi
readonly AGENTBOX_HOME
readonly ATTACK_WAIT_SECONDS=240
BOX="$(cd "$(dirname "$0")/.." && pwd)/bin/box"
readonly BOX
# The image tag is named in one place.
IMAGE="$(sed -n 's/^readonly IMAGE="\(.*\)"$/\1/p' "$(dirname "$BOX")/../lib/common.sh")"
readonly IMAGE
readonly CHECK_TIMEOUT_SECONDS=90
readonly NO_PROXY_ENV="env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy"
readonly FALLBACK_HOST_PORT=22
# The most a job of a few seconds may be measured at on a busy machine.
readonly SLOW_JOB_SECONDS=300
UNEXPECTED=0

# A TCP port the Mac is really listening on, so that "unreachable" cannot mean "nothing there".
host_listening_port() {
  lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | awk 'NR > 1 {n = split($9, a, ":"); print a[n]; exit}'
}

# check <must fail|must work> <name> <command in the container>
check() {
  local expectation="$1" name="$2" command="$3" status
  perl -e 'alarm shift; exec @ARGV' "$CHECK_TIMEOUT_SECONDS" "$BOX" run "$TASK" -- bash -c "$command" >/dev/null 2>&1 </dev/null
  status=$?
  if [ "$expectation" = "must fail" ] && [ "$status" -ne 0 ]; then
    echo "PASS  $name (failed as required, status $status)"
  elif [ "$expectation" = "must work" ] && [ "$status" -eq 0 ]; then
    echo "PASS  $name (worked)"
  else
    echo "FAIL  $name ($expectation, status $status)"
    UNEXPECTED=$((UNEXPECTED + 1))
  fi
}

HOST_PORT="$(host_listening_port)"
HOST_PORT="${HOST_PORT:-$FALLBACK_HOST_PORT}"
echo "confinement checks on task $TASK (host port used for host.docker.internal: $HOST_PORT)"

check "must fail" "host not on the allow-list, through the proxy" "curl -fsS -m 20 -o /dev/null https://example.com/"
check "must fail" "allowed host without the proxy" "$NO_PROXY_ENV curl -fsS -m 10 -o /dev/null https://registry.npmjs.org/"
check "must fail" "arbitrary host without the proxy" "curl --noproxy '*' -fsS -m 10 -o /dev/null https://example.com/"
check "must fail" "IP literal through the proxy (https)" "curl -fsS -m 20 -o /dev/null https://1.1.1.1/"
check "must fail" "IP literal through the proxy (http)" "curl -fsS -m 20 -o /dev/null http://1.1.1.1/"
check "must fail" "port 22 through the proxy" "curl -fsS -m 20 -o /dev/null https://github.com:22/"
# An ordinary HTTP request names its own port; only tunnels to 443 pass. The 403 is the proxy's
# own answer: it refused before it opened any connection.
check "must fail" "ordinary HTTP to an allowed name on port 22, through the proxy" "curl -fsS -m 20 -o /dev/null http://github.com:22/"
check "must fail" "ordinary HTTP to an allowed name on port 80, through the proxy" "curl -fsS -m 20 -o /dev/null http://registry.npmjs.org/"
check "must work" "the proxy answers such a request with 403 itself" "test \"\$(curl -sS -m 20 -o /dev/null -w '%{http_code}' http://github.com:8080/)\" = 403"
check "must fail" "host.docker.internal without the proxy" "curl --noproxy '*' -sS -m 10 -o /dev/null http://host.docker.internal:$HOST_PORT/"
check "must fail" "host.docker.internal through the proxy" "curl -fsS -m 20 -o /dev/null http://host.docker.internal:$HOST_PORT/"
check "must fail" "overlay network address without the proxy" "curl --noproxy '*' -sS -m 10 -o /dev/null http://100.100.100.100/"
check "must fail" "overlay network address through the proxy" "curl -fsS -m 20 -o /dev/null http://100.100.100.100/"
check "must fail" "reading /Users" "ls /Users"
check "must fail" "the Docker socket" "test -S /var/run/docker.sock"
check "must fail" "a docker client" "command -v docker"
check "must fail" "writing /etc" "touch /etc/agentbox-write-test"
check "must fail" "writing /usr/local" "touch /usr/local/agentbox-write-test"
check "must fail" "writing /opt" "touch /opt/agentbox-write-test"
check "must fail" "writing / " "touch /agentbox-write-test"
check "must work" "npm view of a package" "npm view left-pad version"
check "must work" "git ls-remote of a public GitHub repository" "git ls-remote https://github.com/octocat/Hello-World.git HEAD"
# The hosts the agent CLI needs to sign in and to answer. A reply from the origin, whatever its status,
# proves the tunnel; a refusal by the proxy makes curl fail.
for host in api.anthropic.com platform.claude.com claude.ai; do
  check "must work" "tunnel to $host" "curl -sS -m 20 -o /dev/null https://$host/"
done
check "must work" "writing inside the workspace" "touch ./confinement-write-test && rm ./confinement-write-test"

# ---- host-side attacks (the workspace is hostile) --------------------------------------

# expect <name> <condition command...>: passes when the command succeeds on the host.
expect() {
  local name="$1"
  shift
  if "$@"; then echo "PASS  $name"; else echo "FAIL  $name"; UNEXPECTED=$((UNEXPECTED + 1)); fi
}

drop_is_empty() {
  [ -z "$(/bin/ls -A "$AGENTBOX_HOME/drop/$1")" ]
}

wait_until() {
  local waited=0
  until "$@"; do
    sleep 2
    waited=$((waited + 2))
    [ "$waited" -ge "$ATTACK_WAIT_SECONDS" ] && return 1
  done
  return 0
}

attack_symlink_in_drop() {
  local victim="$AGENTBOX_HOME/victim-$$.txt"
  echo "keep me" >"$victim"
  "$BOX" run "$ATTACK_ONE" -- ln -s "$victim" /box/requests/test >/dev/null 2>&1 </dev/null
  expect "drop symlink to a host path: picked up by the runner and moved aside" wait_until drop_is_empty "$ATTACK_ONE"
  expect "  the host file behind the symlink is untouched" test "$(cat "$victim")" = "keep me"
  expect "  nothing was queued for it" test -z "$(grep -l "task=$ATTACK_ONE" "$AGENTBOX_HOME"/queue/*.req 2>/dev/null)"
  rm -f "$victim"
}

attack_git_config() {
  local marker="$AGENTBOX_HOME/fsmonitor-marker-$$"
  rm -f "$marker"
  "$BOX" run "$ATTACK_ONE" -- bash -c "git config core.fsmonitor 'touch $marker'; git config core.pager 'touch $marker'; \
    git config diff.external 'touch $marker'; git config alias.status '!touch $marker'; \
    printf '#!/bin/sh\ntouch $marker\n' >.git/hooks/post-checkout; chmod +x .git/hooks/post-checkout; \
    echo x >attack.txt; git add attack.txt; git -c user.name=a -c user.email=a@example.com commit -q -m attack; \
    git config filter.evil.clean 'sh -c \"touch /verified/poisoned /trusted/poisoned $marker-filter; cat\"'; \
    echo '* filter=evil' >.git/info/attributes; echo edited >>attack.txt" >/dev/null 2>&1 </dev/null
  "$BOX" status >/dev/null 2>&1
  "$BOX" status --json >/dev/null 2>&1
  "$BOX" facts "$ATTACK_ONE" --json >/dev/null 2>&1
  "$BOX" log "$ATTACK_ONE" --json >/dev/null 2>&1
  "$BOX" diff "$ATTACK_ONE" >/dev/null 2>&1
  "$BOX" take "$ATTACK_ONE" >/dev/null 2>&1
  expect "git config with command-executing keys: status, facts, log, diff and take ran, no marker on the host" test ! -e "$marker"
  expect "  a clean filter of the workspace left nothing in the verified store, the source or on the host" \
    test -z "$(find "$AGENTBOX_HOME/verified" "$AGENTBOX_HOME/selftest-source" -maxdepth 2 -name poisoned 2>/dev/null)$(/bin/ls "$marker-filter" 2>/dev/null)"
  expect "  take still worked (branch box/$ATTACK_ONE reached the throwaway source)" \
    git -C "$AGENTBOX_HOME/selftest-source" rev-parse --verify --quiet "refs/heads/box/$ATTACK_ONE"
}

# A job that writes into node_modules, then a job of another task sees pristine dependencies.
attack_dependency_write() {
  "$BOX" verify "$ATTACK_ONE" poison >/dev/null 2>&1 </dev/null
  echo "      poison job exit status: $? (nonzero is the read-only mount refusing the write, zero is a private copy)"
  "$BOX" verify "$ATTACK_TWO" probe >/dev/null 2>&1 </dev/null
  expect "dependencies seen by the next job of another task are pristine" test "$?" -eq 0
  expect "  the pristine volume has no poison marker" docker run --rm --network none --user agent --cap-drop ALL \
    -v "agentbox-pristine.$(basename "$(/bin/ls "$AGENTBOX_HOME"/deps/*.ok | head -1)" .ok):/d:ro" "$IMAGE" test ! -e /d/poison-marker
}

# A port opened in one task's container is not reachable from another task's container, by address
# or by name, while the proxy still is.
has_listener_container() {
  [ -n "$(docker ps --filter "label=agentbox.task=$ATTACK_ONE" --format '{{.Names}}')" ]
}

attack_network_between_tasks() {
  local listener address
  perl -e 'alarm shift; exec @ARGV' 120 "$BOX" run "$ATTACK_ONE" -- python3 -m http.server 8099 --bind 0.0.0.0 >/dev/null 2>&1 </dev/null &
  wait_until has_listener_container
  sleep 3
  listener="$(docker ps --filter "label=agentbox.task=$ATTACK_ONE" --format '{{.Names}}' | head -1)"
  address="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}' "$listener" | awk '{print $1}')"
  expect "a listener of task one answers inside task one's own network (the test is meaningful)" \
    "$BOX" run "$ATTACK_ONE" -- curl --noproxy '*' -fsS -m 5 -o /dev/null "http://$address:8099/"
  expect "task two cannot reach task one's listener by address" \
    test "$("$BOX" run "$ATTACK_TWO" -- curl --noproxy '*' -sS -m 5 -o /dev/null -w '%{http_code}' "http://$address:8099/" 2>/dev/null </dev/null)" != "200"
  expect "task two cannot reach it by container name" \
    test "$("$BOX" run "$ATTACK_TWO" -- curl --noproxy '*' -sS -m 5 -o /dev/null -w '%{http_code}' "http://$listener:8099/" 2>/dev/null </dev/null)" != "200"
  expect "the proxy still answers task two" \
    "$BOX" run "$ATTACK_TWO" -- curl -fsS -m 20 -o /dev/null https://registry.npmjs.org/
  expect "the two tasks do not share a network" \
    test "$(docker network inspect "agentbox-net-$ATTACK_ONE" -f '{{range .Containers}}{{.Name}} {{end}}' | grep -c "agentbox-ws-$ATTACK_TWO")" -eq 0
  docker rm -f "$listener" >/dev/null 2>&1
}

# The newest result file of a task for a verb.
latest_result() {
  /bin/ls -t "$AGENTBOX_HOME/results/$1"/*-"$2".json 2>/dev/null | head -1
}

# result_number_between <result file> <field> <low> <high>
result_number_between() {
  local value
  value="$(jq -r ".$2" "$1" 2>/dev/null)"
  [[ "$value" =~ ^[0-9]+$ ]] && [ "$value" -ge "$3" ] && [ "$value" -le "$4" ]
}

# What the runner records about a job is its own: a job cannot choose its seconds, its memory
# or its exit status by printing records, leaving a record file, killing the wrapper or leaving
# a process behind.
attack_job_values() {
  local result
  "$BOX" verify "$ATTACK_ONE" forge >/dev/null 2>&1 </dev/null
  result="$(latest_result "$ATTACK_ONE" forge)"
  expect "forged records in the log and in the container: the job ran and exited 0" test "$(jq -r .exit_code "$result")" = "0"
  expect "  seconds are the runner's measure (12 to $SLOW_JOB_SECONDS), not the forged 999999" result_number_between "$result" seconds 12 "$SLOW_JOB_SECONDS"
  expect "  peak memory is a reading through Docker (at least 1 MiB), not the forged 1" result_number_between "$result" peak_memory_bytes 1048576 68719476736
  "$BOX" verify "$ATTACK_ONE" killwrapper >/dev/null 2>&1 </dev/null
  result="$(latest_result "$ATTACK_ONE" killwrapper)"
  expect "a job that kills the wrapper: the container's exit status by signal (137)" test "$(jq -r .exit_code "$result")" = "137"
  expect "  seconds are the runner's measure" result_number_between "$result" seconds 3 "$SLOW_JOB_SECONDS"
  "$BOX" verify "$ATTACK_ONE" linger >/dev/null 2>&1 </dev/null
  result="$(latest_result "$ATTACK_ONE" linger)"
  expect "a job that leaves a process behind: exit 0 and the time of the verb, not of the process" test "$(jq -r .exit_code "$result")" = "0"
  expect "  seconds are the runner's measure" result_number_between "$result" seconds 3 "$SLOW_JOB_SECONDS"
  expect "  no job container is left running" test -z "$(docker ps -q --filter "label=agentbox.task=$ATTACK_ONE" --filter label=agentbox.kind=job)"
}

if [ -n "$ATTACK_ONE" ] && [ -n "$ATTACK_TWO" ]; then
  attack_job_values
  attack_network_between_tasks
  attack_symlink_in_drop
  attack_git_config
  attack_dependency_write
fi

echo "$UNEXPECTED check(s) did not hold"
exit "$UNEXPECTED"

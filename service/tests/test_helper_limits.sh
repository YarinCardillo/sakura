#!/bin/bash
# Every container box starts has limits. The small helper containers (the checkout of a
# commit an agent made, the copy of the dependencies, the first mount of a volume, the look at
# which credential files an account has) all get one policy: memory with no extra swap, CPU, a
# limit on processes, no capabilities, no network, and a deadline. A commit can be small and
# still check out into a very large tree: the checkout must fail inside its own limits.
. "$(dirname "$0")/lib.sh"
setup_sandbox
export BOX_DEPS_MODE=copy
"$BOX" new t1 sandbox >/dev/null
commit_in_task t1 work
: >"$FAKE_DOCKER_LOG"
drop_request t1 test
"$BOX" runner --once >/dev/null 2>&1
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}\n' | "$BOX" acp t1 main >/dev/null 2>&1

# has_policy <what> <a pattern that picks the docker run line>
has_policy() {
  local line
  line="$(grep -e '^run ' "$FAKE_DOCKER_LOG" | grep -e "$2" | head -1)"
  assert_eq "$1: started" "1" "$([ -n "$line" ] && echo 1 || echo 0)"
  for flag in "--memory 1g" "--memory-swap 1g" "--cpus 1" "--pids-limit 256" "--cap-drop ALL" "--security-opt no-new-privileges" "--network none" "--rm" "--label agentbox=1"; do
    assert_eq "  $flag" "1" "$(grep -c -e " $flag " <<<" $line ")"
  done
}

echo " the helper containers of one check and one conversation"
has_policy "the checkout" '/box/checkout\.sh'
has_policy "the copy of the dependencies" ' cp -a /from/\. /to/'
has_policy "the folders of a configuration volume" 'mkdir -p /acct/claude /acct/codex'
has_policy "the look at the credential files of an account" 'agentbox.kind=acctfiles'

echo " the first mount of a volume"
# (The docker of the tests says that every such volume exists already: the function is called.)
: >"$FAKE_DOCKER_LOG"
BOX_ROOT="$BOX_ROOT" bash -c '
  for lib in common names docker; do . "$BOX_ROOT/lib/$lib.sh"; done
  volume_exists() { return 1; }
  ensure_volume agentbox-deps-zz' >/dev/null 2>&1
has_policy "it" ':/deps [^ ]* true$'

echo " no container is started without one of the four sets of limits"
# Every docker run in the sources of box, with the lines that continue it, names the flags of an
# agent, of a job, of a sandbox reader or of a helper.
UNLIMITED="$(cat "$BOX_ROOT"/lib/*.sh "$BOX_ROOT/bin/box" | awk '
  /docker run/ && !/^ *#/ { statement = $0; while (statement ~ /\\$/ && (getline next_line) > 0) statement = statement next_line
    if (statement !~ /AGENT_FLAGS|BASE_FLAGS|SANDBOX_FLAGS|HELPER_FLAGS/) print statement }')"
assert_eq "none ($UNLIMITED)" "" "$UNLIMITED"
finish

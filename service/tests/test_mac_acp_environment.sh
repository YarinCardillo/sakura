#!/bin/bash
# The native adapter and its installation do not inherit the environment box was started in.
# node and npm are found once, when the adapter is installed, and recorded as absolute paths;
# both run with an explicit list of variables (never NODE_OPTIONS, NODE_PATH or npm_config_*),
# and npm runs in a folder of tools/, not where box was called and not in a worktree.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log" FAKE_NPM_ENV_LOG="$TMP/npm.env" FAKE_ACP_ENV_LOG="$TMP/adapter.env" FAKE_NODE_LOG="$TMP/node.log"
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
mkdir -p "$TMP/login"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'
echo 'require("fs").writeFileSync(process.env.MARKER, "preloaded")' >"$W/preload.cjs"
TOOL="$AGENTBOX_HOME/tools/claude-agent-acp-$(sed -n 's/^ARG CLAUDE_AGENT_ACP_VERSION=//p' "$BOX_ROOT/image/Dockerfile")"

has() {
  grep -c -e "^$2=" "$1"
}

echo " a conversation started with settings that redirect node and npm in the environment"
cd "$W" || exit 1
printf '%s\n' "$INITIALIZE" | NODE_OPTIONS="--require ./preload.cjs" NODE_PATH="$W" npm_config_registry="http://registry.invalid" \
  NPM_CONFIG_PREFIX="$W" MARKER="$TMP/preloaded" SOME_SECRET="s3cret" SSH_AUTH_SOCK="$TMP/agent.sock" EXTRA_ONE="one" \
  BOX_ADAPTER_ENV="$BOX_ADAPTER_ENV EXTRA_ONE NODE_OPTIONS npm_config_registry" "$BOX" acp m1 personal >"$TMP/acp.out" 2>"$TMP/acp.err"
assert_eq "it works" "0 1" "$? $(jq -r '.result.protocolVersion' "$TMP/acp.out")"
cd "$TMP" || exit 1
assert_eq "npm did not get them" "0 0 0 0 0" "$(has "$FAKE_NPM_ENV_LOG" NODE_OPTIONS) $(has "$FAKE_NPM_ENV_LOG" NODE_PATH) $(has "$FAKE_NPM_ENV_LOG" npm_config_registry) $(has "$FAKE_NPM_ENV_LOG" NPM_CONFIG_PREFIX) $(has "$FAKE_NPM_ENV_LOG" SOME_SECRET)"
assert_contains "npm ran in a folder of tools/" "$AGENTBOX_HOME/tools/.install." "$(head -n 1 "$FAKE_NPM_ENV_LOG")"
assert_eq "with a path that starts where node is" "$TESTS_DIR/fake-bin" "$(sed -n 's/^PATH=//p' "$FAKE_NPM_ENV_LOG" | cut -d: -f1)"
assert_eq "the adapter did not get them either" "0 0 0 0 0" "$(has "$FAKE_ACP_ENV_LOG" NODE_OPTIONS) $(has "$FAKE_ACP_ENV_LOG" NODE_PATH) $(has "$FAKE_ACP_ENV_LOG" npm_config_registry) $(has "$FAKE_ACP_ENV_LOG" NPM_CONFIG_PREFIX) $(has "$FAKE_ACP_ENV_LOG" SOME_SECRET)"
assert_eq "it got what an agent of the person needs" "1 1 1 1" "$(has "$FAKE_ACP_ENV_LOG" HOME) $(has "$FAKE_ACP_ENV_LOG" PATH) $(has "$FAKE_ACP_ENV_LOG" CLAUDE_CONFIG_DIR) $(has "$FAKE_ACP_ENV_LOG" SSH_AUTH_SOCK)"
assert_eq "and a variable the person named in BOX_ADAPTER_ENV" "one" "$(sed -n 's/^EXTRA_ONE=//p' "$FAKE_ACP_ENV_LOG")"
assert_no_file "nothing of the worktree was loaded before the adapter" "$TMP/preloaded"

echo " node and npm are recorded once, as absolute paths, and the adapter is started with that node"
assert_eq "the record" "NODE=$TESTS_DIR/fake-bin/node NPM=$TESTS_DIR/fake-bin/npm" "$(tr '\n' ' ' <"$TOOL/box-runtime" | sed 's/ $//')"
assert_contains "the adapter was started by the recorded node" "$TESTS_DIR/fake-bin/node $TOOL/node_modules/.bin/claude-agent-acp" "$(cat "$FAKE_NODE_LOG")"
mkdir -p "$TMP/other-bin"
printf '#!/bin/bash\ntouch %q\nexec /bin/bash "$@"\n' "$TMP/other-node-ran" >"$TMP/other-bin/node"
chmod +x "$TMP/other-bin/node"
printf '%s\n' "$INITIALIZE" | PATH="$TMP/other-bin:$PATH" "$BOX" acp m1 personal >"$TMP/acp.out" 2>/dev/null
assert_eq "a later conversation works" "1" "$(jq -r '.result.protocolVersion' "$TMP/acp.out")"
assert_no_file "another node that came first in the path later is not used" "$TMP/other-node-ran"
assert_eq "npm was not asked again" "1" "$(grep -c '^install ' "$FAKE_NPM_LOG")"
edit_in_place "s|^NODE=.*|NODE=$TMP/gone/node|" "$TOOL/box-runtime"
OUT="$(printf '%s\n' "$INITIALIZE" | "$BOX" acp m1 personal 2>"$TMP/acp.err")"
assert_eq "a recorded node that is gone: exit 1, nothing on stdout" "1 " "$? $OUT"
assert_contains "one line says what to do" "box: the node recorded for the Claude ACP adapter is gone" "$(cat "$TMP/acp.err")"
finish

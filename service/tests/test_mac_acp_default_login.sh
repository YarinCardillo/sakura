#!/bin/bash
# The Claude CLI keeps the login of the default config directory ($HOME/.claude) under a name
# that depends on whether CLAUDE_CONFIG_DIR is set: with the variable set to that very directory
# it looks for another login and finds none. So for an account whose directory is the default
# one the adapter is started without CLAUDE_CONFIG_DIR; for any other directory (a login that
# was made with the variable set) it is started with it.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log" FAKE_ACP_ENV_LOG="$TMP/adapter.env"
export HOME="$TMP/person"
mkdir -p "$HOME/.claude" "$HOME/.claude-work"
ln -s "$HOME/.claude" "$TMP/another-name"
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
PHYSICAL_HOME="$(cd -P "$HOME" && pwd -P)"
printf 'ACCOUNT_personal=%s\nACCOUNT_work=%s\nACCOUNT_link=%s\nACCOUNT_spelled=%s\n' \
  "$HOME/.claude" "$HOME/.claude-work" "$TMP/another-name" "$PHYSICAL_HOME/.claude" >"$AGENTBOX_HOME/config"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
"$BOX" new m1 app >/dev/null
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'

# conversation <account>: opens one; the environment of the adapter is in FAKE_ACP_ENV_LOG.
conversation() {
  rm -f "$FAKE_ACP_ENV_LOG"
  printf '%s\n' "$INITIALIZE" | "$BOX" acp m1 "$1" >"$TMP/acp.out" 2>"$TMP/acp.err"
}

echo " the account of the default config directory"
conversation personal
assert_eq "the conversation opens" "1" "$(jq -r '.result.protocolVersion' "$TMP/acp.out")"
assert_eq "the adapter is started without CLAUDE_CONFIG_DIR" "0" "$(grep -c '^CLAUDE_CONFIG_DIR=' "$FAKE_ACP_ENV_LOG")"
assert_eq "it has the person's home, where the CLI finds the login by itself" "$HOME" "$(sed -n 's/^HOME=//p' "$FAKE_ACP_ENV_LOG")"
CLAUDE_CONFIG_DIR="$HOME/.claude" conversation personal
assert_eq "also when box itself was started with the variable set" "0" "$(grep -c '^CLAUDE_CONFIG_DIR=' "$FAKE_ACP_ENV_LOG")"

echo " the same directory under another name or spelling"
for account in link spelled; do
  conversation "$account"
  assert_eq "$account: no CLAUDE_CONFIG_DIR either" "0" "$(grep -c '^CLAUDE_CONFIG_DIR=' "$FAKE_ACP_ENV_LOG")"
done

echo " an account of another directory"
conversation work
assert_eq "the adapter is started with CLAUDE_CONFIG_DIR set to it" "$HOME/.claude-work" "$(sed -n 's/^CLAUDE_CONFIG_DIR=//p' "$FAKE_ACP_ENV_LOG")"
finish

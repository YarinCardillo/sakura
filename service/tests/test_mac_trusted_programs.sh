#!/bin/bash
# box looks its programs up only in trusted directories. A person whose PATH has "." in it (or a
# relative entry, or a folder inside the data directory of box) must not have a file of a
# worktree that happens to be called env, sed, git or anything else box uses run in the place of
# that program: not after box has changed into the worktree, and not when box itself is started
# from inside it. The names are not picked by hand: every word of the sources of box that names
# a program of this machine gets a file of that name in the worktree.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_NPM_LOG="$TMP/npm.log" FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"

echo " every program name the sources of box use"
readonly SYSTEM_DIRECTORIES="/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"
NAMES="$(cat "$BOX_ROOT/bin/box" "$BOX_ROOT"/lib/*.sh "$BOX_ROOT"/lib/*.pl "$BOX_ROOT"/lib/*.py "$BOX_ROOT"/image/runner/*.py "$BOX_ROOT"/image/runner/*.sh "$BOX_ROOT/image/verify" \
  | tr -c 'A-Za-z0-9_.+-' '\n' | sort -u | while IFS= read -r word; do
    case "$word" in ('' | . | .. | -*) continue ;; esac
    if PATH="$SYSTEM_DIRECTORIES" type -P "$word" >/dev/null 2>&1; then echo "$word"; fi
  done)"
COUNT="$(wc -l <<<"$NAMES" | tr -d ' ')"
assert_eq "(the list is found by reading the sources: $COUNT names)" "1" "$([ "$COUNT" -ge 40 ] && echo 1 || echo 0)"
for name in env perl bash git sed head jq stat python3 ps tr cut grep mktemp date sleep; do
  assert_eq "  it has $name" "1" "$(grep -c -x -e "$name" <<<"$NAMES")"
done
for name in $NAMES; do
  for place in "$REPO" "$REPO/sub"; do
    printf '#!/bin/sh\necho "$0 $*" >>%q/planted-ran-%s\nexit 97\n' "$TMP" "$name" >"$place/$name"
    chmod +x "$place/$name"
  done
done
git -C "$REPO" add -A && git -C "$REPO" -c user.name=t -c user.email=t@example.com commit -q -m "files named like programs"
planted() {
  /bin/ls "$TMP" | grep -c '^planted-ran-'
}
which_ran() {
  /bin/ls "$TMP" | sed -n 's/^planted-ran-//p' | tr '\n' ' '
}

# An adapter and a shell that use nothing but what bash has built in: what they would look up
# themselves, with the person's PATH, is the person's own business and not what is tested here.
cat >"$TMP/adapter" <<'ADAPTER'
#!/bin/bash
while IFS= read -r line; do
  case "$line" in *'"method":"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}' ;; esac
done
ADAPTER
printf '#!/bin/bash\necho "shell in $PWD" >%q/shell.said\nexit 0\n' "$TMP" >"$TMP/shell"
chmod +x "$TMP/adapter" "$TMP/shell"
export FAKE_NPM_ADAPTER="$TMP/adapter" SHELL="$TMP/shell"
mkdir -p "$TMP/login"
echo "ACCOUNT_personal=$TMP/login" >"$AGENTBOX_HOME/config"
cd "$TMP" || exit 1
# The PATH box is started with: "." first, a relative entry, and the worktree itself. (The test
# keeps its own PATH: what it runs to look at the outcome must not be the planted files.)
HOSTILE_PATH=".:bin:$AGENTBOX_HOME/work/m1:$AGENTBOX_HOME/work/m1/sub:$PATH"
box() {
  PATH="$HOSTILE_PATH" "$BOX" "$@"
}
readonly INITIALIZE='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'
W="$AGENTBOX_HOME/work/m1"

echo " a lavoro whose worktree holds a file for each of them, a PATH that begins with . and names the worktree"
assert_status "project add" 0 box project add app --repo "$REPO" --subdir sub --setup 'echo setup-ran' --check "test=echo checked" --run 'exec /bin/sleep 300'
assert_status "box new, with its setup command" 0 box new m1 app
assert_contains "the setup command ran" "setup-ran" "$(box log m1 --setup)"
assert_eq "none of the files ran ($(which_ran))" "0" "$(planted)"

echo " every verb that handles the lavoro"
printf '%s\n' "$INITIALIZE" | box acp m1 personal >"$TMP/acp.out" 2>/dev/null
assert_eq "acp: the conversation answers" '{"id":1,"jsonrpc":"2.0","result":{"protocolVersion":1}}' "$(jq -c -S . "$TMP/acp.out")"
assert_eq "play start" "true" "$(box play m1 start --json | jq -r .running)"
assert_status "play stop" 0 box play m1 stop
box verify m1 test --no-wait >/dev/null
box runner --once >/dev/null 2>&1
assert_eq "verify: the check ran and passed" "0" "$(jq -r .exit_code "$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-test.json | head -1)")"
assert_status "shell" 0 box shell m1
assert_eq "  it was opened in the folder" "shell in $W/sub" "$(cat "$TMP/shell.said")"
assert_eq "tree" "1" "$(box tree m1 --path sub --json | jq -r '[.entries[] | select(.name == "package.json")] | length')"
assert_eq "file" "{}" "$(box file m1 sub/package.json)"
assert_eq "search" '["sub/package.json"]' "$(box search m1 package.json --json | jq -c .matches)"
assert_eq "graph" "m1" "$(box graph m1 --json | jq -r '.lanes[1].lavoro')"
echo "new" >"$W/sub/untracked.txt"
assert_contains "diff, with a file git does not know yet" "+new" "$(box diff m1)"
assert_status "log" 0 box log m1 --json
assert_eq "facts and status" "mac mac" "$(box facts m1 --json | jq -r .where) $(box status --json | jq -r '.tasks[0].where')"
assert_eq "none of the files ran ($(which_ran))" "0" "$(planted)"

echo " box started from inside the worktree, as from its terminal"
assert_eq "facts" "mac" "$( (cd "$W/sub" && box facts m1 --json) | jq -r .where)"
assert_eq "tree" "sub" "$( (cd "$W" && box tree m1 --path sub --json) | jq -r .path)"
assert_eq "play status" "false" "$( (cd "$W/sub" && box play m1 status --json) | jq -r .running)"
assert_eq "none of the files ran ($(which_ran))" "0" "$(planted)"

echo " rm"
assert_status "box rm --force" 0 box rm m1 --force
assert_eq "none of the files ran ($(which_ran))" "0" "$(planted)"
finish

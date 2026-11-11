#!/bin/bash
# A check of a lavoro on the Mac runs on the worktree as it is, and the agent can edit files
# while it runs. A result is published as a result of its commit only when the worktree is,
# after the check, what it was before: the same head, the same content of the tracked files,
# nothing new that git does not ignore. Otherwise the result is a refusal with the reason, never
# a pass, whatever the command returned.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
echo "*.out" >"$REPO/.gitignore"
git -C "$REPO" add -A && git -C "$REPO" -c user.name=t -c user.email=t@example.com commit -q -m "an ignore list"
"$BOX" project add app --repo "$REPO" --subdir sub --protected "package.json" --test-files "**/*.spec.ts" \
  --check "clean=echo fine" \
  --check "edit=echo edited >>existing.spec.ts; echo passed-on-the-edit" \
  --check "revert=cp existing.spec.ts \"$TMP/kept\"; echo edited >>existing.spec.ts; cp \"$TMP/kept\" existing.spec.ts; echo reverted" \
  --check "commit=echo more >>existing.spec.ts; git add -A; git -c user.name=t -c user.email=t@example.com commit -q -m during; echo committed" \
  --check "untracked=echo out >left-behind.txt; echo left" \
  --check "ignored=echo out >result.out; echo wrote-ignored" \
  --check "hide=git update-index --assume-unchanged existing.spec.ts; echo edited >>existing.spec.ts; echo hid-the-edit" \
  --check "ran=echo ran >>\"$TMP/ran\"; echo did-run" >/dev/null
"$BOX" new m1 app >/dev/null
W="$AGENTBOX_HOME/work/m1"
readonly REASON="the worktree changed while the check ran"

# check <verb>: queues and runs it; RESULT is its result file.
check() {
  "$BOX" verify m1 "$1" --no-wait >/dev/null 2>&1
  "$BOX" runner --once >/dev/null 2>&1
  RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-"$1".json 2>/dev/null | head -1)"
}
restore() {
  git -C "$W" reset -q --hard "$1"
  git -C "$W" clean -fdq
}
HEAD_SHA="$(git -C "$W" rev-parse HEAD)"

echo " a check that leaves the worktree as it found it"
check clean
assert_eq "passes for its commit" "0 $HEAD_SHA" "$(jq -r '"\(.exit_code) \(.sha)"' "$RESULT")"
check ignored
assert_eq "also when it writes files git ignores" "0" "$(jq -r .exit_code "$RESULT")"

echo " tracked files edited while the check ran"
check edit
assert_contains "(the command itself passed)" "passed-on-the-edit" "$(cat "${RESULT%.json}.log")"
assert_eq "the result is not a pass" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
assert_eq "facts do not show a pass for the head" "2" "$("$BOX" facts m1 --json | jq -r '.results.edit.exitCode')"
restore "$HEAD_SHA"

echo " the head moved while the check ran"
check commit
assert_eq "the result is not a pass" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
assert_eq "it is recorded for the commit that was requested, not the new one" "$HEAD_SHA" "$(jq -r .sha "$RESULT")"
restore "$HEAD_SHA"

echo " a file git does not ignore left behind"
check untracked
assert_eq "the result is not a pass" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
restore "$HEAD_SHA"

echo " an edit the index is told not to look at"
check hide
assert_contains "(the command passed, after it told git to assume the file unchanged and edited it)" "hid-the-edit" "$(cat "${RESULT%.json}.log")"
assert_eq "the result is not a pass: the state is read from the head, not from the index of the worktree" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
git -C "$W" update-index --no-assume-unchanged sub/existing.spec.ts
restore "$HEAD_SHA"
for bit in assume-unchanged skip-worktree; do
  "$BOX" verify m1 ran --no-wait >/dev/null 2>&1
  git -C "$W" update-index "--$bit" sub/existing.spec.ts
  rm -f "$TMP/ran"
  "$BOX" runner --once >/dev/null 2>&1
  RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-ran.json | head -1)"
  assert_eq "an index with a $bit entry: the check is refused" "2" "$(jq -r .exit_code "$RESULT")"
  assert_contains "  its log says why" "hides files from git" "$(cat "${RESULT%.json}.log")"
  assert_no_file "  the command did not run" "$TMP/ran"
  assert_status "  box verify refuses it too" 1 "$BOX" verify m1 ran --no-wait
  assert_contains "  with one box: line that says why" "box: the index of the worktree of task 'm1' hides files from git" "$OUTPUT"
  git -C "$W" update-index "--no-$bit" sub/existing.spec.ts
done

# A git that does something at a chosen step of the snapshot: the test's way into the moments
# between the steps. SHIM_AT is the git command to act at, SHIM_CALL which of its calls (1, 2..),
# SHIM_DO what to do: "edit-before" or "edit-after" (a tracked file is edited before or after
# the real git ran), or "fail" (the step fails).
REAL_GIT="$(command -v git)"
mkdir -p "$TMP/shim"
cat >"$TMP/shim/git" <<SHIM
#!/bin/bash
if [ -n "\${SHIM_AT:-}" ] && [[ " \$* " == *" \$SHIM_AT "* ]]; then
  count=\$(( \$(cat "$TMP/shim.count" 2>/dev/null || echo 0) + 1 ))
  echo "\$count" >"$TMP/shim.count"
  if [ "\$count" = "\${SHIM_CALL:-1}" ]; then
    case "\$SHIM_DO" in
      fail) exit 1 ;;
      edit-before) echo "slipped in" >>"$W/sub/existing.spec.ts" ;;
      edit-after) "$REAL_GIT" "\$@"; status=\$?; echo "slipped in" >>"$W/sub/existing.spec.ts"; exit \$status ;;
    esac
  fi
fi
exec "$REAL_GIT" "\$@"
SHIM
chmod +x "$TMP/shim/git"
# shimmed_check <step> <call> <what>: queues the check "ran" and runs it with the shim in the path.
shimmed_check() {
  "$BOX" verify m1 ran --no-wait >/dev/null 2>&1
  rm -f "$TMP/ran" "$TMP/shim.count"
  PATH="$TMP/shim:$PATH" SHIM_AT="$1" SHIM_CALL="$2" SHIM_DO="$3" "$BOX" runner --once >/dev/null 2>&1
  RESULT="$(/bin/ls -t "$AGENTBOX_HOME/results/m1"/*-ran.json | head -1)"
}

echo " an edit that slips in while the state before the check is being read"
shimmed_check read-tree 1 edit-before
assert_eq "it is part of that state, which is then not the commit: refused" "2" "$(jq -r .exit_code "$RESULT")"
assert_contains "as uncommitted work" "commit first: a check is for a commit" "$(cat "${RESULT%.json}.log")"
assert_no_file "the command did not run" "$TMP/ran"
restore "$HEAD_SHA"

echo " an edit that slips in between the state that was read and the start of the command"
shimmed_check write-tree 1 edit-after
assert_eq "the command ran on it, and the result is not a pass" "1 2" "$(wc -l <"$TMP/ran" | tr -d ' ') $(jq -r .exit_code "$RESULT")"
assert_contains "its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
restore "$HEAD_SHA"

echo " a state that cannot be read never counts as unchanged"
for step in read-tree add write-tree; do
  shimmed_check "$step" 1 fail
  assert_eq "$step fails before the check: refused" "2" "$(jq -r .exit_code "$RESULT")"
  assert_contains "  its log says why" "the state of the worktree cannot be read" "$(cat "${RESULT%.json}.log")"
  assert_no_file "  the command did not run" "$TMP/ran"
  shimmed_check "$step" 2 fail
  assert_eq "$step fails after the check: the result is not a pass" "1 2" "$(wc -l <"$TMP/ran" | tr -d ' ') $(jq -r .exit_code "$RESULT")"
  assert_contains "  its log says why" "$REASON" "$(cat "${RESULT%.json}.log")"
done

echo " an edit that is undone before the check ends is not seen (the limit of this check)"
check revert
assert_eq "the result is the command's" "0" "$(jq -r .exit_code "$RESULT")"
assert_eq "comparing left the worktree and its index alone" "" "$(git -C "$W" status --porcelain)"
finish

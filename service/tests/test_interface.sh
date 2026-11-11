#!/bin/bash
# Machine interface (docs/interface.md): JSON reports, diff, log, journal, sessions, daemon.
. "$(dirname "$0")/lib.sh"
setup_sandbox
"$BOX" new t1 sandbox >/dev/null
WORKSPACE="$AGENTBOX_HOME/work/t1"
export BOX_VERIFY_POLL=1

echo " status --json"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "valid JSON" "0" "$(jq -e . <<<"$OUT" >/dev/null 2>&1; echo $?)"
assert_eq "runner stopped" "stopped" "$(jq -r .runner.state <<<"$OUT")"
assert_eq "no current request" "null" "$(jq -c .runner.current <<<"$OUT")"
assert_eq "one task" "t1" "$(jq -r '.tasks[0].name' <<<"$OUT")"
assert_eq "profile" "sandbox" "$(jq -r '.tasks[0].profile' <<<"$OUT")"
assert_eq "base" "main" "$(jq -r '.tasks[0].base' <<<"$OUT")"
assert_eq "readable" "true" "$(jq -r '.tasks[0].readable' <<<"$OUT")"
assert_eq "branch" "box/t1" "$(jq -r '.tasks[0].branch' <<<"$OUT")"
assert_eq "head is the workspace head" "$(workspace_head t1)" "$(jq -r '.tasks[0].head' <<<"$OUT")"
assert_eq "head subject" "base" "$(jq -r '.tasks[0].headSubject' <<<"$OUT")"
assert_eq "ahead 0" "0" "$(jq -r '.tasks[0].ahead' <<<"$OUT")"
assert_eq "taken is null" "null" "$(jq -c '.tasks[0].taken' <<<"$OUT")"
assert_eq "agent is null" "null" "$(jq -c '.tasks[0].agent' <<<"$OUT")"
assert_eq "no results yet" "{}" "$(jq -c '.tasks[0].results' <<<"$OUT")"
assert_eq "baseSha is 40 hex" "1" "$(jq -r '.tasks[0].baseSha | test("^[0-9a-f]{40}$") | if . then 1 else 0 end' <<<"$OUT")"

echo " hostile text stays data"
SUBJECT=$'evil "}],"x":{"y":1} $(touch '"$TMP"'/pwned) `touch '"$TMP"'/pwned2` \033[31mred\ttab'
echo one >>"$WORKSPACE/sub/a.txt"
git -C "$WORKSPACE" add -A
git -C "$WORKSPACE" -c user.name=t -c user.email=t@example.com commit -q -m "$SUBJECT"
echo dirty >"$WORKSPACE/untracked.txt"
echo more >>"$WORKSPACE/sub/a.txt"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "still valid JSON" "0" "$(jq -e . <<<"$OUT" >/dev/null 2>&1; echo $?)"
assert_eq "no top-level injection" "null" "$(jq -c '.x' <<<"$OUT")"
assert_contains "subject is data" 'evil "}],"x":{"y":1}' "$(jq -r '.tasks[0].headSubject' <<<"$OUT")"
assert_eq "no control character in the subject" "0" "$(jq -r '.tasks[0].headSubject' <<<"$OUT" | LC_ALL=C tr -d '\n[:print:]' | wc -c | tr -d ' ')"
assert_no_file "nothing was evaluated" "$TMP/pwned"
assert_no_file "nothing was evaluated (backticks)" "$TMP/pwned2"
assert_eq "ahead 1" "1" "$(jq -r '.tasks[0].ahead' <<<"$OUT")"
assert_eq "dirty files (modified + untracked)" "2" "$(jq -r '.tasks[0].dirtyFiles' <<<"$OUT")"
assert_eq "files changed vs merge base (tracked)" "1" "$(jq -r '.tasks[0].filesChanged' <<<"$OUT")"
assert_eq "insertions" "2" "$(jq -r '.tasks[0].insertions' <<<"$OUT")"
assert_eq "deletions" "0" "$(jq -r '.tasks[0].deletions' <<<"$OUT")"
git -C "$WORKSPACE" checkout -q -- sub/a.txt
rm "$WORKSPACE/untracked.txt"

echo " facts --json is one element of tasks"
assert_eq "same object" "$("$BOX" status --json 2>/dev/null | jq -c '.tasks[0]')" "$("$BOX" facts t1 --json 2>/dev/null)"

echo " an unreadable workspace"
"$BOX" new t2 sandbox >/dev/null
echo garbage >"$AGENTBOX_HOME/work/t2/.git/HEAD"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "valid JSON" "0" "$(jq -e . <<<"$OUT" >/dev/null 2>&1; echo $?)"
assert_eq "readable false" "false" "$(jq -r '.tasks[] | select(.name == "t2") | .readable' <<<"$OUT")"
assert_eq "git fields null" "null" "$(jq -c '.tasks[] | select(.name == "t2") | .head' <<<"$OUT")"
assert_eq "other task still reported" "true" "$(jq -r '.tasks[] | select(.name == "t1") | .readable' <<<"$OUT")"
"$BOX" rm t2 --force >/dev/null

echo " log --json"
OUT="$("$BOX" log t1 --json 2>/dev/null)"
assert_eq "valid JSON array" "array" "$(jq -r type <<<"$OUT")"
assert_eq "one commit" "1" "$(jq length <<<"$OUT")"
assert_eq "commit sha" "$(workspace_head t1)" "$(jq -r '.[0].sha' <<<"$OUT")"
assert_eq "commit stats" "1 1 0" "$(jq -r '.[0] | "\(.filesChanged) \(.insertions) \(.deletions)"' <<<"$OUT")"
assert_eq "time is UTC" "1" "$(jq -r '.[0].time | test("Z$") | if . then 1 else 0 end' <<<"$OUT")"

echo " diff"
echo changed >>"$WORKSPACE/sub/a.txt"
echo fresh >"$WORKSPACE/new-file.txt"
mkfifo "$WORKSPACE/pipe"
ln -s /etc/hostname "$WORKSPACE/link"
head -c 500000 /dev/urandom | base64 >"$WORKSPACE/big.txt"
PATCH="$("$BOX" diff t1 2>/dev/null)"
assert_contains "tracked change" "+changed" "$PATCH"
assert_contains "untracked file as a new file" "+++ b/new-file.txt" "$PATCH"
assert_contains "oversized file is replaced" "[patch omitted:" "$PATCH"
assert_eq "its content is not in the patch" "0" "$(grep -c 'AAAA' <<<"$(grep -A3 'big.txt' <<<"$PATCH")")"
assert_contains "symlink appears as a patch" "link" "$PATCH"
assert_eq "the fifo was skipped" "0" "$(grep -c 'pipe' <<<"$PATCH")"
rm -f "$WORKSPACE/pipe" "$WORKSPACE/big.txt" "$WORKSPACE/new-file.txt" "$WORKSPACE/link"
git -C "$WORKSPACE" checkout -q -- sub/a.txt
PATCH="$("$BOX" diff t1 --commit "$(workspace_head t1)" 2>/dev/null)"
assert_contains "commit patch" "+one" "$PATCH"
assert_status "a sha that is not a listed commit fails" 1 "$BOX" diff t1 --commit "$(printf 'a%.0s' {1..40})"
assert_status "an invalid sha fails" 1 "$BOX" diff t1 --commit "not-a-sha"

echo " verify --no-wait, result-log, journal"
assert_status "no result yet: exit 3" 3 "$BOX" result-log t1 test
assert_status "queue returns at once" 0 "$BOX" verify t1 test --no-wait
assert_status "same commit and verb again" 0 "$BOX" verify t1 test --no-wait
assert_eq "still one request" "1" "$(/bin/ls "$AGENTBOX_HOME/queue" | wc -l | tr -d ' ')"
assert_status "unknown verb refused" 1 "$BOX" verify t1 deploy --no-wait
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "queue shows it" "test" "$(jq -r '.runner.queue[0].verb' <<<"$OUT")"
assert_eq "queue sha is the head" "$(workspace_head t1)" "$(jq -r '.runner.queue[0].sha' <<<"$OUT")"
FAKE_JOB_SUMMARY='      Tests  3 passed (3)' "$BOX" runner --once 2>/dev/null
assert_status "result-log prints the log" 0 "$BOX" result-log t1 test
assert_contains "log content" "[box]" "$OUTPUT"
OUT="$("$BOX" status --json 2>/dev/null)"
assert_eq "result in status" "0" "$(jq -r '.tasks[0].results.test.exitCode' <<<"$OUT")"
assert_eq "result is for the head" "true" "$(jq -r '.tasks[0].results.test.isHead' <<<"$OUT")"
assert_eq "result seconds are a number the runner measured" "1" "$([ "$(jq -r '.tasks[0].results.test.seconds' <<<"$OUT")" -le 60 ] && echo 1 || echo 0)"
assert_eq "results carry checksReason (null when the checks were computed)" "true null" "$(jq -r '.tasks[0].results.test | "\(has("checksReason")) \(.checksReason)"' <<<"$OUT")"
JOURNAL="$("$BOX" journal --json 2>/dev/null)"
assert_eq "journal events in order" "new verify-queued verify-done" "$(jq -r '[.[].event] | join(" ")' <<<"$JOURNAL" | sed 's/new new/new/; s/new t2.*//' | awk '{print $1, $(NF-1), $NF}')"
assert_eq "verify-done carries the exit code" "0" "$(jq -r '[.[] | select(.event == "verify-done")][0].exitCode' <<<"$JOURNAL")"
assert_eq "since filter" "0" "$("$BOX" journal --json --since 2999-01-01T00:00:00Z | jq length)"

echo " profiles and accounts"
assert_eq "profiles" '["sandbox"]' "$("$BOX" profiles --json | jq -c '[.[].name]')"
assert_eq "no accounts" '[]' "$("$BOX" accounts --json)"

echo " take prints the sha last and journals it"
OUT="$("$BOX" take t1 2>/dev/null)"
assert_eq "last line is the taken sha" "$(workspace_head t1)" "$(echo "$OUT" | tail -n 1)"
assert_eq "status shows taken" "$(workspace_head t1)" "$("$BOX" status --json | jq -r '.tasks[0].taken.sha')"
finish

#!/bin/bash
# box rm of a lavoro on the Mac deletes a folder only when it is the worktree box made for the
# lavoro (its .git names the recorded git directory, which belongs to the project's
# repository), also with --force. Anything else that has come to stand at that path is left
# alone, with one line that says so. A folder that is gone is cleaned up through the records
# and the repository only. Nothing is ever deleted by its path alone.
. "$(dirname "$0")/lib.sh"
setup_sandbox
unset BOX_DEFAULT_WHERE
export FAKE_DOCKER_DOWN=1
REPO="$TMP/repo"
git clone -q "$SOURCE" "$REPO"
"$BOX" project add app --repo "$REPO" --subdir sub >/dev/null
WORK="$AGENTBOX_HOME/work"

# refused <task> <what stands at the path>: box rm, plain and forced, leaves it and the records.
refused() {
  local task="$1" option
  for option in "" "--force"; do
    # shellcheck disable=SC2086
    OUTPUT="$("$BOX" rm "$task" $option 2>&1)"
    assert_eq "$2: box rm $option is refused" "1" "$?"
    assert_eq "  with one box: line" "1 1" "$(grep -c '^box: ' <<<"$OUTPUT") $(wc -l <<<"$OUTPUT" | tr -d ' ')"
  done
  assert_file "  the record is kept" "$AGENTBOX_HOME/tasks/$task.env"
}

echo " another folder stands where the worktree was"
"$BOX" new m1 app >/dev/null
mv "$WORK/m1" "$TMP/m1-moved"
mkdir -p "$WORK/m1/precious" && echo "keep me" >"$WORK/m1/precious/file.txt"
refused m1 "a plain folder"
assert_eq "  the folder and what is in it are untouched" "keep me" "$(cat "$WORK/m1/precious/file.txt")"
rm -rf "$WORK/m1"
git clone -q "$SOURCE" "$WORK/m1"
echo "also mine" >"$WORK/m1/note.txt"
refused m1 "another repository"
assert_eq "  it is untouched" "also mine" "$(cat "$WORK/m1/note.txt")"
rm -rf "$WORK/m1"
mkdir -p "$TMP/elsewhere" && echo "far away" >"$TMP/elsewhere/file.txt"
ln -s "$TMP/elsewhere" "$WORK/m1"
refused m1 "a link to a folder elsewhere"
assert_eq "  the folder it names is untouched, and the link too" "far away 1" "$(cat "$TMP/elsewhere/file.txt") $([ -L "$WORK/m1" ] && echo 1 || echo 0)"
rm "$WORK/m1"

echo " the folder is gone: the records and the entry in the repository are cleaned up"
assert_status "box rm" 0 "$BOX" rm m1
assert_no_file "the record is gone" "$AGENTBOX_HOME/tasks/m1.env"
assert_eq "the repository no longer lists the worktree" "0" "$(git -C "$REPO" worktree list | grep -c '/work/m1')"
assert_eq "the branch stays" "1" "$(git -C "$REPO" branch --list sakura/m1 | wc -l | tr -d ' ')"

echo " the project cannot be read any more (its repository was moved away)"
"$BOX" new m2 app >/dev/null
echo "work" >"$WORK/m2/sub/work.txt"
mv "$REPO" "$TMP/repo-moved"
refused m2 "a worktree whose repository is gone"
assert_eq "  the folder is untouched" "work" "$(cat "$WORK/m2/sub/work.txt")"
mv "$WORK/m2" "$TMP/m2-moved"
assert_status "once the person has moved the folder away, box rm removes the records" 0 "$BOX" rm m2
assert_no_file "the record is gone" "$AGENTBOX_HOME/tasks/m2.env"
mv "$TMP/repo-moved" "$REPO"

echo " the folder is swapped for a link to another worktree while the archive command runs"
"$BOX" new other app >/dev/null
echo "the other lavoro" >"$WORK/other/sub/kept.txt"
"$BOX" project add swap --repo "$REPO" --subdir sub \
  --archive "mv \"\$SAKURA_WORKTREE\" \"$TMP/aside\" && ln -s \"$WORK/other\" \"\$SAKURA_WORKTREE\"" >/dev/null
# (A lavoro of its own for each form: the branch of a removed lavoro stays and its name is taken.)
for pair in ":m4" "--force:m5"; do
  option="${pair%%:*}" task="${pair#*:}"
  "$BOX" new "$task" swap >/dev/null
  # shellcheck disable=SC2086
  OUTPUT="$("$BOX" rm "$task" $option 2>&1)"
  assert_eq "box rm $option: refused" "1" "$?"
  assert_contains "  one line says that the folder changed" "box: " "$(tail -n 1 <<<"$OUTPUT")"
  assert_eq "  the other worktree is untouched and still registered" "the other lavoro 1" "$(cat "$WORK/other/sub/kept.txt" 2>/dev/null) $(git -C "$REPO" worktree list | grep -c '/work/other')"
  assert_file "  the record of the lavoro is kept" "$AGENTBOX_HOME/tasks/$task.env"
  assert_eq "  what stands at its path is still the link" "1" "$([ -L "$WORK/$task" ] && echo 1 || echo 0)"
  rm "$WORK/$task"
  ln -s "$TMP/nowhere" "$WORK/$task"
  assert_status "  a link that leads nowhere is not taken for a folder that is gone" 1 "$BOX" rm "$task" --force
  assert_file "  the record is still kept" "$AGENTBOX_HOME/tasks/$task.env"
  rm "$WORK/$task" && mv "$TMP/aside" "$WORK/$task"
  "$BOX" project set swap --archive "" >/dev/null
  edit_in_place '/^ARCHIVE=/d' "$AGENTBOX_HOME/tasks/$task.project"
  assert_status "  with its own folder back (and no such archive command) it is removed" 0 "$BOX" rm "$task" --force
  "$BOX" project set swap --archive "mv \"\$SAKURA_WORKTREE\" \"$TMP/aside\" && ln -s \"$WORK/other\" \"\$SAKURA_WORKTREE\"" >/dev/null
done
"$BOX" rm other --force >/dev/null

echo " the worktree box made is removed as before, also with files that are not committed"
"$BOX" new m3 app >/dev/null
echo "loose" >"$WORK/m3/sub/loose.txt"
assert_status "box rm --force" 0 "$BOX" rm m3 --force
assert_no_file "the worktree is gone" "$WORK/m3"
finish

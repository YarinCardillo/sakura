# shellcheck shell=bash
# What box checks about a repository before git runs in it on the host. git reads more than the
# folder it is started in: .git can be a file that names another directory, a linked worktree
# names a common directory, an object store can borrow from others, entries can be links, and
# the configuration can include other files. A repository that does any of this beyond a linked
# worktree is refused, by the simple rule each time, never by following where it leads:
# - the git directory and the common directory are directories of the person, outside the parts
#   of the data directory that box manages (by identity, see folder_is_under);
# - none of GIT_CHECKED_ENTRIES in them is a symbolic link or anything but the plain file or
#   directory git writes there;
# - objects/info/alternates does not exist;
# - the repository's own configuration includes nothing;
# - a folder that can be read as two repositories is refused;
# - after git ran, the directories it used are the ones that were checked.
# Everything fails closed: what cannot be read or is not expected is a refusal with a reason.
# Sourced by project.sh; uses its identity helpers and person_git.

readonly GIT_POINTER_MAX_BYTES=4096
# The entries of a git directory that decide what git reads, and what each must be when present.
readonly GIT_CHECKED_FILES="config config.worktree HEAD packed-refs commondir gitdir"
readonly GIT_CHECKED_DIRECTORIES="objects objects/info refs info worktrees"
readonly GIT_INCLUDE_PATTERN='\[[[:space:]]*include'

# Sets REPOSITORY_PROBLEM and fails: the one way a check of this file refuses.
refuse_repository() {
  REPOSITORY_PROBLEM="$1"
  return 1
}

# Succeeds when a folder looks like a git directory itself (the layout of a bare repository).
has_bare_layout() {
  [ -e "$1/HEAD" ] && [ -e "$1/objects" ] && [ -e "$1/refs" ]
}

has_git_entry() {
  [ -e "$1/.git" ] || [ -L "$1/.git" ]
}

# find_git_entry <physical folder>: sets GIT_ENTRY to what git itself would use for that folder,
# found in git's own order without running it: at every level from the folder up, a .git entry
# first, then the bare layout. Fails with REPOSITORY_PROBLEM set when one level offers both, and
# with it empty when there is no repository at all.
find_git_entry() {
  local dir="$1"
  GIT_ENTRY="" REPOSITORY_PROBLEM=""
  while :; do
    if has_git_entry "$dir"; then
      ! has_bare_layout "$dir" || { refuse_repository "it can be read as two repositories (a .git and a bare repository in $dir)"; return 1; }
      GIT_ENTRY="$dir/.git"
      return 0
    fi
    if has_bare_layout "$dir"; then GIT_ENTRY="$dir"; return 0; fi
    [ "$dir" != "/" ] || return 1
    dir="${dir%/*}"
    dir="${dir:-/}"
  done
}

# Prints the directory a pointer file names (the .git file of a linked worktree, commondir), or
# nothing: the file must be a plain small file, and what it names a directory.
# usage: pointed_directory <file> <prefix of its first line> <directory relative paths start from>
pointed_directory() {
  local file="$1" prefix="$2" from="$3" target
  [ -f "$file" ] && [ ! -L "$file" ] && [ -r "$file" ] || return 1
  [ "$(wc -c <"$file")" -le "$GIT_POINTER_MAX_BYTES" ] || return 1
  target="$(head -n 1 "$file")"
  [ -n "$prefix" ] && [[ "$target" != "$prefix"* ]] && return 1
  target="${target#"$prefix"}"
  [ -n "$target" ] || return 1
  [[ "$target" == /* ]] || target="$from/$target"
  physical_directory "$target"
}

# Sets REPOSITORY_GIT_DIR to the git directory a .git entry (or a bare repository) stands for.
resolve_git_entry() {
  local entry="$1" target
  if [ -L "$entry" ]; then
    target="$(physical_directory "$entry")" || target=""
    if [ -n "$target" ] && is_in_managed_directory "$target"; then refuse_repository "its git metadata is inside the data directory of box"; return 1; fi
    refuse_repository "its .git is a symbolic link"
    return 1
  fi
  if [ -d "$entry" ]; then
    REPOSITORY_GIT_DIR="$(physical_directory "$entry")" || true
  else
    REPOSITORY_GIT_DIR="$(pointed_directory "$entry" "gitdir: " "$(dirname "$entry")")" || true
  fi
  [ -n "$REPOSITORY_GIT_DIR" ] || refuse_repository "its .git is neither a git directory nor a pointer to one"
}

# check_git_entry <the .git of a working tree, or a bare repository>: succeeds when git may be
# run with that metadata, with REPOSITORY_GIT_DIR and REPOSITORY_COMMON_DIR set to the physical
# directories that were checked; fails with REPOSITORY_PROBLEM set.
check_git_entry() {
  local managed
  REPOSITORY_GIT_DIR="" REPOSITORY_COMMON_DIR="" REPOSITORY_PROBLEM=""
  resolve_git_entry "$1" || return 1
  REPOSITORY_COMMON_DIR="$REPOSITORY_GIT_DIR"
  if [ -e "$REPOSITORY_GIT_DIR/commondir" ] || [ -L "$REPOSITORY_GIT_DIR/commondir" ]; then
    REPOSITORY_COMMON_DIR="$(pointed_directory "$REPOSITORY_GIT_DIR/commondir" "" "$REPOSITORY_GIT_DIR")" \
      || { refuse_repository "the common git directory it names cannot be read"; return 1; }
  fi
  managed="$(managed_identities)"
  check_git_directory "$REPOSITORY_GIT_DIR" "$managed" || return 1
  [ "$REPOSITORY_COMMON_DIR" = "$REPOSITORY_GIT_DIR" ] || check_git_directory "$REPOSITORY_COMMON_DIR" "$managed" || return 1
  [ -f "$REPOSITORY_GIT_DIR/HEAD" ] || { refuse_repository "its git directory has no HEAD"; return 1; }
  [ -d "$REPOSITORY_COMMON_DIR/objects" ] && [ -d "$REPOSITORY_COMMON_DIR/refs" ] \
    || { refuse_repository "its git directory has no objects or no refs"; return 1; }
  check_no_alternates "$REPOSITORY_COMMON_DIR" && check_no_includes "$REPOSITORY_GIT_DIR" "$REPOSITORY_COMMON_DIR"
}

# One directory of git metadata: the person's, outside what box manages, with plain entries.
# usage: check_git_directory <physical directory> <device:inode of each managed directory>
check_git_directory() {
  local dir="$1" managed="$2" name kind identity index=0 paths=() names=()
  ! is_in_managed_directory "$dir" || { refuse_repository "its git metadata is inside the data directory of box"; return 1; }
  owned_by_current_user "$dir" || { refuse_repository "its git metadata is not owned by you"; return 1; }
  for name in $GIT_CHECKED_FILES $GIT_CHECKED_DIRECTORIES; do
    if [ -e "$dir/$name" ] || [ -L "$dir/$name" ]; then paths+=("$dir/$name"); names+=("$name"); fi
  done
  [ "${#paths[@]}" -gt 0 ] || return 0
  while IFS='|' read -r kind identity; do
    check_git_entry_kind "${names[index]}" "$kind" "$identity" "$managed" || return 1
    index=$((index + 1))
  done < <(file_kind_identity "${paths[@]}" 2>/dev/null)
  [ "$index" -eq "${#paths[@]}" ] || refuse_repository "its git directory cannot be examined"
}

# One entry of a git directory: the plain file or directory git writes there, never a link, and
# not itself a directory box manages.
check_git_entry_kind() {
  local name="$1" kind="$2" identity="$3" managed="$4" wanted="directory"
  case " $GIT_CHECKED_FILES " in *" $name "*) wanted="file" ;; esac
  if [ "$kind" = "link" ]; then refuse_repository "its git directory has a symbolic link in it ($name)"; return 1; fi
  [ "$kind" = "$wanted" ] || { refuse_repository "its git directory has a $name that is not a plain file or directory"; return 1; }
  ! grep -qxF "$identity" <<<"$managed" || refuse_repository "its git directory leads into the data directory of box ($name)"
}

# Borrowed objects are refused whatever the file says: it is not read.
check_no_alternates() {
  [ ! -e "$1/objects/info/alternates" ] && [ ! -L "$1/objects/info/alternates" ] && return 0
  refuse_repository "it borrows git objects from another repository (objects/info/alternates): run 'git repack -a -d' in it, then remove that file"
}

# The repository's own configuration (and a worktree's) names no other file to read.
check_no_includes() {
  local dir file
  for dir in "$@"; do
    for file in "$dir/config" "$dir/config.worktree"; do
      [ -e "$file" ] || continue
      [ -r "$file" ] || { refuse_repository "its git configuration cannot be read (${file##*/})"; return 1; }
      if grep -a -q -i -E -e "$GIT_INCLUDE_PATTERN" "$file"; then
        refuse_repository "its git configuration includes other files (include or includeIf in ${file##*/}): remove that section"
        return 1
      fi
    done
  done
}

same_directory() {
  local first second
  first="$(file_identity "$1" 2>/dev/null)" && second="$(file_identity "$2" 2>/dev/null)" && [ "$first" = "$second" ]
}

# git_used_checked_directories <folder>: asks git, run in that folder, which git directory and
# common directory it uses, and succeeds only when they are the ones check_git_entry checked.
git_used_checked_directories() {
  local folder="$1" answer used_git_dir used_common_dir
  answer="$(person_git -C "$folder" rev-parse --absolute-git-dir --git-common-dir 2>/dev/null)" \
    || { refuse_repository "git does not read it as a repository"; return 1; }
  used_git_dir="$(sed -n 1p <<<"$answer")"
  used_common_dir="$(sed -n 2p <<<"$answer")"
  [[ "$used_common_dir" == /* ]] || used_common_dir="$folder/$used_common_dir"
  same_directory "$used_git_dir" "$REPOSITORY_GIT_DIR" && same_directory "$used_common_dir" "$REPOSITORY_COMMON_DIR" \
    || refuse_repository "git reads another git directory for it than the one box checked"
}

# checked_repository_at <physical root of a working tree>: the checks before git, then git's own
# word. Fails with REPOSITORY_PROBLEM set.
checked_repository_at() {
  local root="$1"
  has_git_entry "$root" || { refuse_repository "its repository has no .git: $root"; return 1; }
  ! has_bare_layout "$root" || { refuse_repository "it can be read as two repositories (a .git and a bare repository in $root)"; return 1; }
  check_git_entry "$root/.git" && git_used_checked_directories "$root"
}

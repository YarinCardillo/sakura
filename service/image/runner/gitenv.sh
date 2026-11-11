# shellcheck shell=bash
# Sourced by the sandbox and runner scripts that run git.
# box_harden_git: nothing from the environment or from system or global config reaches git, replace
# objects and grafts are off. BOX_GIT_OPTIONS holds the options every call must carry: no hooks,
# fsmonitor or external diff, and none of the auxiliary files an object store can carry
# (commit-graph, multi-pack-index, bitmaps), which git trusts blindly.

readonly BOX_GIT_OPTIONS=(
  --no-replace-objects --no-optional-locks
  -c safe.directory='*' -c core.fsmonitor=false -c core.hooksPath=/dev/null
  -c core.attributesFile=/dev/null -c diff.external= -c core.pager=cat
  -c core.commitGraph=false -c core.multiPackIndex=false -c pack.useBitmaps=false -c gc.auto=0 -c advice.graftFileDeprecated=false
)

box_harden_git() {
  local keep_path="$PATH" name
  while read -r name; do unset "$name"; done < <(env | sed -n 's/^\(GIT_[A-Za-z_]*\)=.*/\1/p')
  export PATH="$keep_path" HOME="${TMPDIR:-/tmp}"
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_GLOBAL=/dev/null
  export GIT_NO_REPLACE_OBJECTS=1 GIT_GRAFT_FILE=/dev/null GIT_ATTR_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
}

# git on the verified store (read-only in every container but the import one).
box_verified_git() {
  git "${BOX_GIT_OPTIONS[@]}" --git-dir="${BOX_VERIFIED:-/verified}" "$@"
}

# Succeeds when the import container has verified the commit and the store holds it.
box_require_verified() {
  [[ "$1" =~ ^[0-9a-f]{40}$ ]] && [ -f "${BOX_VERIFIED:-/verified}/box-verified/$1" ] \
    && box_verified_git cat-file -e "$1^{commit}" || { echo "$1 is not a verified commit of the task" >&2; return 1; }
}

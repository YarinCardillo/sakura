"""Helpers shared by the sandbox scripts. Nothing here evaluates workspace text: git is always
called with an argument list, never through a shell.

There are three kinds of sandbox container and a script runs in exactly one of them:
- workspace: the untrusted workspace read-only at /src and nothing else. git reads a repository
  whose configuration the agent writes (a clean filter can run a program of its choosing), so
  such a container may be the agent's by the time it answers. Use git().
- verified: the verified store read-only at /verified and the trusted source at /trusted, no
  workspace. Everything a decision rests on is computed here. Use verified_git().
- import: the verified store writable, the trusted source, no workspace. Its only input is a
  pack on standard input. Use import_pack()."""
import os
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

SRC = os.environ.get("BOX_SRC", "/src")
TRUSTED = os.environ.get("BOX_TRUSTED", "/trusted")
VERIFIED = os.environ.get("BOX_VERIFIED", "/verified")
GIT_TIMEOUT_SECONDS = 120
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
MAX_LOG_COMMITS = 200
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")
MARKER_DIRECTORY = "box-verified"
# Fixed options for every git call: no replace objects, no grafts, no system or global
# configuration, no hooks, no fsmonitor, no external diff, and none of the auxiliary files an
# object store can carry (commit-graph, multi-pack-index, bitmaps), which git trusts blindly.
HARDENED_OPTIONS = [
    "--no-replace-objects", "--no-optional-locks",
    "-c", "safe.directory=*", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
    "-c", "core.attributesFile=/dev/null", "-c", "diff.external=", "-c", "core.pager=cat",
    "-c", "core.commitGraph=false", "-c", "core.multiPackIndex=false", "-c", "pack.useBitmaps=false",
    "-c", "gc.auto=0", "-c", "advice.graftFileDeprecated=false",
]
GIT = ["git"] + HARDENED_OPTIONS + ["-C", SRC]
VERIFIED_GIT = ["git", "--git-dir", VERIFIED] + HARDENED_OPTIONS


def hardened_env():
    """The environment of every git call: nothing is inherited except PATH."""
    return {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": tempfile.gettempdir(), "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": "/dev/null", "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_NO_REPLACE_OBJECTS": "1", "GIT_GRAFT_FILE": "/dev/null", "GIT_ATTR_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0",
    }


def _run(command, stdin=None):
    return subprocess.run(command, input=stdin, capture_output=True, text=True, errors="replace",
                          timeout=GIT_TIMEOUT_SECONDS, check=True, env=hardened_env())


def git(*args):
    """Runs git in the workspace and returns stdout as text. Raises on a non-zero exit. Only in a
    workspace container, for what only the workspace knows (HEAD, branch, the working tree)."""
    return _run(GIT + list(args)).stdout


def verified_git(*args):
    """Runs git in the verified store and returns stdout as text. The store is read-only here."""
    return _run(VERIFIED_GIT + list(args)).stdout


def full_sha(sha):
    """The argument when it is a 40-hex id, else fail."""
    if not FULL_SHA.match(sha):
        fail("not a full commit sha: %r" % sha[:50])
    return sha


def _marker(sha):
    return os.path.join(VERIFIED, MARKER_DIRECTORY, sha)


def require_verified(sha):
    """The commit, when the import container has verified it and the store holds it; else fail.
    A branch, tag or replace object cannot stand in for it: the argument must be the 40-hex id
    of a commit object."""
    full_sha(sha)
    try:
        found = verified_git("rev-parse", "--verify", "--quiet", sha + "^{commit}").strip()
    except subprocess.CalledProcessError:
        found = ""
    if found != sha or not os.path.exists(_marker(sha)):
        fail("%s is not a verified commit of the task" % sha)
    return sha


def require_trusted_commit(base):
    """The base, when it is a commit of the trusted source repository; else fail."""
    full_sha(base)
    try:
        _run(["git", "--git-dir", os.path.dirname(_trusted_objects())] + HARDENED_OPTIONS + ["cat-file", "-e", base + "^{commit}"])
    except subprocess.CalledProcessError:
        fail("the base commit is not in the trusted repository")
    return base


def _trusted_objects():
    for candidate in (os.path.join(TRUSTED, ".git", "objects"), os.path.join(TRUSTED, "objects")):
        if os.path.isdir(candidate):
            return candidate
    fail("the trusted repository is not mounted")


def _prepare_store():
    """Creates the verified store on first use. Its only link to the outside is an alternates
    entry for the objects of the trusted source repository; the workspace is never one."""
    if not os.path.isdir(os.path.join(VERIFIED, "objects")):
        os.makedirs(VERIFIED, exist_ok=True)
        subprocess.run(["git", "init", "--bare", "--quiet", "--template=", VERIFIED],
                       check=True, env=hardened_env(), timeout=GIT_TIMEOUT_SECONDS)
    alternates = os.path.join(VERIFIED, "objects", "info", "alternates")
    os.makedirs(os.path.dirname(alternates), exist_ok=True)
    with open(alternates, "w") as handle:
        handle.write(_trusted_objects() + "\n")


def import_pack(base, head, pack):
    """Import container only. Puts the commit `head` into the verified store from a pack (an open
    file) that a workspace container made: index-pack --strict computes the hash of every object itself
    and checks the links, and the walk below fails when an object between the base and the head is
    missing. `base` must be a commit of the trusted source repository. Fails on anything wrong."""
    full_sha(head)
    require_trusted_commit(base)
    _prepare_store()
    indexed = subprocess.run(VERIFIED_GIT + ["index-pack", "--stdin", "--strict", "--fix-thin"],
                             stdin=pack, capture_output=True, timeout=GIT_TIMEOUT_SECONDS, env=hardened_env())
    if indexed.returncode != 0:
        fail("the commits after the base failed verification")
    try:
        verified_git("rev-list", "--objects", "--quiet", head, "--not", base)
    except subprocess.CalledProcessError:
        fail("the commits after the base are incomplete or damaged")
    os.makedirs(os.path.dirname(_marker(head)), exist_ok=True)
    open(_marker(head), "w").close()


def workspace_head():
    """The commit HEAD names in the workspace (a ref lookup; the commit is verified elsewhere)."""
    sha = git("rev-parse", "HEAD").strip()
    if not FULL_SHA.match(sha):
        fail("HEAD of the workspace is not a commit id")
    return sha


def utc(iso_time):
    """Converts a git ISO 8601 time to UTC with a trailing Z."""
    parsed = datetime.fromisoformat(iso_time.strip())
    return parsed.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def numstat(*diff_args, reader):
    """Returns (files, insertions, deletions) of a git diff --numstat. `reader` is git (the working
    tree, in a workspace container) or verified_git (commits only)."""
    files = insertions = deletions = 0
    for line in reader("diff", "--numstat", "--no-ext-diff", "--no-textconv", *diff_args).splitlines():
        added, removed, _path = line.split("\t", 2)
        files += 1
        insertions += int(added) if added.isdigit() else 0
        deletions += int(removed) if removed.isdigit() else 0
    return files, insertions, deletions


def workspace_merge_base(base_sha):
    """Workspace container only: the commit the task branched from as the workspace's own
    repository tells it, or the recorded base. For figures about the working tree, to show."""
    try:
        return full_sha(git("merge-base", base_sha, "HEAD").strip())
    except subprocess.CalledProcessError:
        return base_sha


def commits_ahead(base, head):
    """Shas of the verified commits after the base, newest first, at most MAX_LOG_COMMITS."""
    require_verified(head)
    return verified_git("rev-list", "--max-count=%d" % MAX_LOG_COMMITS, "%s..%s" % (base, head)).split()


def parent_or_empty_tree(sha):
    try:
        return verified_git("rev-parse", "--verify", "--quiet", sha + "^1").strip()
    except subprocess.CalledProcessError:
        return EMPTY_TREE


def fail(message, code=1):
    sys.stderr.write(message + "\n")
    sys.exit(code)

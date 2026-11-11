"""How the read scripts (tree, file, search) reach the files and the git of a lavoro.

The same scripts serve both places. In a box they run in a workspace container: the untrusted
workspace read-only at /src, git through boxgit (hardened, nothing inherited); what they print
is the agent's word and is only shown. For a lavoro on the Mac box runs them natively and names
the git command itself in BOX_LAVORO_GIT (one argument per line: git, the person's hardened
options, --git-dir and --work-tree), with the person's environment as box cleaned it. A script
never builds that command on its own.

Files are reached through directory descriptors only (open_inside): no path is resolved by name
and then opened by name, so nothing that is swapped in between can lead a read out of the
lavoro."""
import os
import stat
import subprocess

import boxgit

ROOT = boxgit.SRC
MAX_FILE_BYTES = 2 * 1024 * 1024
EXIT_NOT_FOUND = 3
EXIT_OUTSIDE = 4
EXIT_TOO_BIG = 82
MAX_LINKS = 16
# Never through a link, never waiting (a FIFO opens at once and is then seen not to be a file).
_OPEN_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
_NATIVE = os.environ.get("BOX_LAVORO_GIT")
# The device and inode of the folder box checked for the lavoro ("<device>:<inode>"), when it
# has one on record: the folder that is opened must be that one.
_ROOT_IDENTITY = os.environ.get("BOX_ROOT_IDENTITY")


def _command():
    return _NATIVE.split("\n") if _NATIVE else boxgit.GIT


def _environment():
    return dict(os.environ) if _NATIVE else boxgit.hardened_env()


def git(*args, check=True, stdin=None):
    """Runs git for the lavoro and returns its standard output as bytes."""
    done = subprocess.run(_command() + list(args), input=stdin, capture_output=True, timeout=boxgit.GIT_TIMEOUT_SECONDS,
                          check=check, env=_environment())
    return done.stdout


def text(raw):
    """Bytes of a path as text for JSON: what is not UTF-8 is replaced."""
    return raw.decode("utf-8", "replace")


def merge_base(base):
    """The commit the lavoro left its base at, or the recorded base."""
    try:
        found = git("merge-base", base, "HEAD").decode().strip()
        return boxgit.full_sha(found)
    except subprocess.CalledProcessError:
        return base


def plain_parts(path):
    """The components of a path the caller gave: relative, without "..", not the git metadata."""
    parts = [part for part in path.split("/") if part not in ("", ".")]
    if path.startswith("/") or ".." in parts or (parts and parts[0] == ".git"):
        boxgit.fail("the path is not a path inside the lavoro", EXIT_OUTSIDE)
    return parts


def open_root():
    """Opens the folder of the lavoro and returns its descriptor. It is opened by its name below
    its parent (the work directory of box, or the root of the container), never through a link,
    and must be the folder box checked: a path that was swapped for a link, or for another
    folder, after the check is not walked."""
    parent_path, name = os.path.split(ROOT.rstrip("/"))
    try:
        parent = os.open(parent_path or "/", os.O_RDONLY | os.O_DIRECTORY)
        try:
            root = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
        finally:
            os.close(parent)
    except OSError:
        boxgit.fail("the folder of the lavoro cannot be opened", EXIT_OUTSIDE)
    if _ROOT_IDENTITY:
        status = os.fstat(root)
        if "%d:%d" % (status.st_dev, status.st_ino) != _ROOT_IDENTITY:
            boxgit.fail("the folder of the lavoro is not the one that was checked", EXIT_OUTSIDE)
    return root


def _step(name, parent, want_directory):
    """Opens one component below a directory descriptor without following a link. Returns the
    descriptor, or the target of the link when the component is one."""
    flags = _OPEN_FLAGS | (os.O_DIRECTORY if want_directory else 0)
    try:
        return os.open(name, flags, dir_fd=parent), None
    except OSError:
        try:
            is_link = stat.S_ISLNK(os.lstat(name, dir_fd=parent).st_mode)
        except OSError:
            boxgit.fail("no such path in the lavoro", EXIT_NOT_FOUND)
        if not is_link:
            boxgit.fail("no such path in the lavoro", EXIT_NOT_FOUND)
        return None, os.readlink(name, dir_fd=parent)


def open_inside(path, directory=False):
    """Opens a path of the lavoro and returns its descriptor. The path is walked from the root
    of the lavoro one component at a time, each opened relative to the descriptor of its parent
    and never through a link: a link is read and walked the same way, so one that is absolute,
    climbs above the root or reaches the git metadata ends the walk, and a component that is
    replaced while the walk goes on cannot lead it elsewhere. The caller examines the
    descriptor (fstat) before it reads: nothing here says what kind of file was opened."""
    pending = plain_parts(path)
    stack = [open_root()]
    links = 0
    while pending:
        name = pending.pop(0)
        if name in ("", "."):
            continue
        if name == "..":
            if len(stack) == 1:
                boxgit.fail("the path leaves the lavoro", EXIT_OUTSIDE)
            os.close(stack.pop())
            continue
        if len(stack) == 1 and name == ".git":
            boxgit.fail("the path leaves the lavoro", EXIT_OUTSIDE)
        descriptor, target = _step(name, stack[-1], bool(pending) or directory)
        if descriptor is not None:
            stack.append(descriptor)
            continue
        links += 1
        if target.startswith("/") or links > MAX_LINKS:
            boxgit.fail("the path leaves the lavoro through a link", EXIT_OUTSIDE)
        pending = target.split("/") + pending
    for descriptor in stack[:-1]:
        os.close(descriptor)
    return stack[-1]


def read_bounded(descriptor):
    """At most MAX_FILE_BYTES + 1 bytes of an open file, however long it is or becomes."""
    chunks, left = [], MAX_FILE_BYTES + 1
    while left > 0:
        chunk = os.read(descriptor, min(left, 1 << 16))
        if not chunk:
            break
        chunks.append(chunk)
        left -= len(chunk)
    return b"".join(chunks)


def count_lines(directory, name):
    """The lines of a regular file of an open directory, or nothing: for a file that is not a
    regular file, is over the limit or cannot be opened. The file is opened below the
    descriptor and never through a link."""
    try:
        descriptor = os.open(name, _OPEN_FLAGS, dir_fd=directory)
    except OSError:
        return None
    try:
        status = os.fstat(descriptor)
        if not stat.S_ISREG(status.st_mode) or status.st_size > MAX_FILE_BYTES:
            return None
        data = read_bounded(descriptor)
    finally:
        os.close(descriptor)
    if len(data) > MAX_FILE_BYTES:
        return None
    return data.count(b"\n") + (1 if data and not data.endswith(b"\n") else 0)

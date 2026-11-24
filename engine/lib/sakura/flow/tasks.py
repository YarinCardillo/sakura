"""Starting tasks and checking the preconditions of running them."""

from __future__ import annotations

import hashlib
import os
import re
from dataclasses import dataclass
from pathlib import Path

from sakura import beneath, profile, repository
from sakura.flow import git_config, gitutil, roles, store
from sakura.flow.config import FlowConfig

DEFAULT_ORIGIN = "defaults"
TEXT_REF_PREFIX = "text:"
TEXT_REF_HASH_LENGTH = 12
TEXT_LABEL_WORDS = 6
# The title of a lane is the first line of its task text, cut here (a title is a line in a list, not a description).
MAX_TITLE_CHARACTERS = 120
# The state key of the lane's title.
TITLE_KEY = "title"
# A task description, not a data set: far above any real one (a few kilobytes), small enough that reading it can
# never strain the engine's memory, and a sparse or ever-growing file is refused by its size.
MAX_TASK_FILE_BYTES = 1 << 20
# The state key of the SHA-256 of the task's input as the engine stored it (`input.md` in the task folder).
INPUT_DIGEST_KEY = "input_sha256"
# Set on a task a detached start created, until the first driver run consumes it: only that run may be the start's continuation.
PENDING_START_KEY = "pending_start"
INBOX_START_KEY = "inbox_start"
TRACK_START_KEY = "track_start"
TRACK_START_PREFIX = "track-"
TRACK_START_ID = re.compile(rf"{TRACK_START_PREFIX}[0-9a-f]{{{store.MAX_TASK_ID_LENGTH - len(TRACK_START_PREFIX)}}}")
TRACK_BRANCH_HASH_LENGTH = 16
PROPOSAL_CONTEXT_KEY = "proposal_context"



# The line `sakura flow start` prints when it made a lane, followed by the lane's id: what the cockpit reads back.
CREATED_LANE_PREFIX = "lane "


def created_lane_id(output: str) -> str:
    """Return the lane id reported by flow start, or an empty string."""
    return next((line.removeprefix(CREATED_LANE_PREFIX).strip()
                 for line in output.splitlines() if line.startswith(CREATED_LANE_PREFIX)), "")


class TaskStartError(ValueError):
    """Raised when a task cannot be started or run as configured."""


@dataclass(frozen=True)
class CapturedTaskFile:
    """The bytes of a task file read once, and their SHA-256: what is validated, hashed and copied."""

    data: bytes
    sha256: str


@dataclass(frozen=True)
class TaskInput:
    """Where a task comes from: `ref` names it, `text` is its description when it came from the command line, and
    `captured` the task file read once when the task is created (None for another source or a missing file)."""

    ref: str
    text: str | None
    label: str | None
    description: str
    captured: CapturedTaskFile | None = None

    def input_bytes(self) -> bytes | None:
        """What the engine stores as the task's input when the task is created: the captured file or the text."""
        if self.captured is not None:
            return self.captured.data
        return (self.text + "\n").encode() if self.text is not None else None


def resolve_input(repo: Path, cfg: FlowConfig, ref: str | None, message: str | None) -> TaskInput:
    """Decide whether the task is a file (or issue or page) or text typed on the command line, and say which.

    With a file source, a word that is not an existing file but has whitespace in it is the task text; one word
    that is not a file stays a file reference, so a typo in a path is reported instead of becoming a task.
    """
    if ref and message:
        raise TaskStartError("give the task as either a file or the task text (-m), not both")
    if not ref and not message:
        raise TaskStartError("give the task as a file or as its text: sakura flow start <file> or sakura flow start -m \"text\"")
    source = cfg.values["spec"]["source"]
    text = message if message else None
    if text is None and source == "file" and len(ref.split()) > 1 and not (repo / ref).is_file():
        text = ref
    if text is not None:
        return _text_input(text)
    captured = _capture_at_start(repo, ref) if source == "file" else None
    kind = "file" if source == "file" else f"{source} reference"
    return TaskInput(ref, None, None, f"task {kind} {ref}", captured)


def split_title(text: str) -> tuple[str, str]:
    """The title and the description of a task text: the first line (spaces collapsed, cut at MAX_TITLE_CHARACTERS) and
    what follows it after the blank line, or an empty description for a one line text."""
    first, _, rest = text.strip().partition("\n")
    return " ".join(first.split())[:MAX_TITLE_CHARACTERS], rest.strip()


def capture_task_file(repo: Path, ref: str) -> CapturedTaskFile:
    """Read the task file `ref`, a path below the repository, once and without following any link.

    The repository is the person's and its files are untrusted: a task file that is a link to a secret, lies behind
    a linked folder, has a second name or leaves the repository is refused, not read; so is one over
    `MAX_TASK_FILE_BYTES` or one that changed while it was read. The caller validates, hashes and copies the returned
    bytes and never reads the file again. Raises FileNotFoundError when it does not exist and TaskStartError for a
    refusal.
    """
    relative = ref
    if os.path.isabs(ref):
        relative = _below(repo, Path(ref))
    try:
        data = beneath.read_capped(repo, relative, MAX_TASK_FILE_BYTES)
    except FileNotFoundError:
        raise
    except beneath.FileTooLarge as error:
        raise TaskStartError(f"refusing the task file {ref}: it is larger than {MAX_TASK_FILE_BYTES >> 20} MiB "
                             f"({error}); a task description is short, put the long material in the repository "
                             "and name it in the task") from error
    except beneath.FileChanged as error:
        raise TaskStartError(f"refusing the task file {ref}: it changed while it was read; {error}. Let whatever "
                             "writes it finish and start again") from error
    except (OSError, ValueError) as error:
        raise TaskStartError(f"refusing the task file {ref}: it must be a plain file inside the repository, not a "
                             f"link, behind a link, a second name of another file or outside it ({error})") from error
    return CapturedTaskFile(data, hashlib.sha256(data).hexdigest())


def _below(repo: Path, path: Path) -> str:
    for base in (repo, Path(os.path.realpath(repo))):
        try:
            return path.relative_to(base).as_posix()
        except ValueError:
            continue
    raise TaskStartError(f"refusing the task file {path}: it is outside the repository {repo}")


def _capture_at_start(repo: Path, ref: str) -> CapturedTaskFile | None:
    """The one capture of the task file, refused as `capture_task_file` refuses; None for a missing file (intake
    reports it by name)."""
    try:
        return capture_task_file(repo, ref)
    except FileNotFoundError:
        return None


def _text_input(text: str) -> TaskInput:
    words = text.split()
    if not words:
        raise TaskStartError("the task text is empty")
    digest = hashlib.sha256(text.encode()).hexdigest()[:TEXT_REF_HASH_LENGTH]
    return TaskInput(f"{TEXT_REF_PREFIX}{digest}", text, " ".join(words[:TEXT_LABEL_WORDS]),
                     f"task text from the command line ({len(words)} words)")


def known_tasks(repo: Path) -> dict[str, str]:
    """Map every existing task id to its ref; an unreadable task maps to an empty ref."""
    root = store.flow_root(repo)
    if not root.is_dir():
        return {}
    found = {}
    for directory in sorted(path for path in root.iterdir() if path.is_dir()):
        try:
            found[directory.name] = store.TaskStore(repo, directory.name).read_state()["ref"]
        except (store.StateError, KeyError):
            found[directory.name] = ""
    return found


def check_codex_trust(cfg: FlowConfig, repo: Path) -> None:
    """Refuse to run when a Codex role lacks recorded hook trust."""
    for role, value in cfg.values["roles"].items():
        problem = roles.codex_trust_problem(roles.parse_role(value), role, repo)
        if problem:
            raise TaskStartError(problem)


def check_git_config(repo: Path) -> None:
    """Refuse to start while the repository's local Git config names a program the engine's Git would run."""
    try:
        found = git_config.executable_entries(repository.common_dir(repo), repo)
    except git_config.GitConfigError as error:
        raise TaskStartError(str(error)) from error
    if found:
        raise TaskStartError("the repository's local Git config names programs the engine's Git would run: "
                             f"{'; '.join(found)}. Remove them (git config --unset <key>, with --worktree or "
                             "--file for a config.worktree) and start again")


def _track_branch_slug(task_id: str, title: str) -> str:
    suffix = "--" + task_id.removeprefix(TRACK_START_PREFIX)[:TRACK_BRANCH_HASH_LENGTH]
    slug = store.task_id_for(store.DEFAULT_TASK_ID, {}, label=title)
    return slug[:store.MAX_TASK_ID_LENGTH - len(suffix)].rstrip("-") + suffix


def initial_state(repo: Path, cfg: FlowConfig, task_id: str, ref: str, overrides: dict, urgent: bool = True,
                  *, track_title: str | None = None) -> dict:
    """Return the state of a new task, pinned to the integration branch tip.

    A task that is not `urgent` (started with `--when-cheap`) waits for the night when the week's pace is high.
    A reserved Track start supplies `track_title` for its branch while keeping its task id.
    """
    base_branch = integration_branch(repo)
    values = cfg.values
    branch_slug = task_id if track_title is None else _track_branch_slug(task_id, track_title)
    return {
        "task": task_id, "ref": ref, "phase": "intake", "status": "running", "reason": "", "history": [],
        "preset": values["preset"], "dial": values["dial"], "familiarity": values["familiarity"],
        "overrides": overrides, "base_branch": base_branch, "base_commit": _resolve(repo, base_branch),
        "push_url": _origin_url(repo),
        "git_config_sha256": git_config.digest(repository.common_dir(repo)),
        "branch": values["workspace"]["branch"].replace("{task}", branch_slug),
        "approvals": {}, "completed": {}, "active_run": None, "active_runs": [], "active_stacks": [], "rounds": {}, "contract_sha256": "",
        "plan_sha256": "", "plan_review_findings": [],
        "protected_paths": [], "workspaces": {}, "build": {}, "pr": {}, "urgent": urgent,
    }


def integration_branch(repo: Path) -> str:
    """The branch a task starts from and its pull request targets.

    A profile layer that names one decides. Otherwise the repository's own: what `origin/HEAD` points at, else the
    branch checked out now, else the profile default (`main`), so a repository on `master` or `trunk` starts with
    no profile file.
    """
    result = profile.load(repo)
    configured = result.values["integration_branch"]
    if result.origins["integration_branch"] != DEFAULT_ORIGIN:
        return configured
    return profile.remote_default_branch(repo) or _checked_out_branch(repo) or configured


def _checked_out_branch(repo: Path) -> str:
    done = gitutil.run(repo, "symbolic-ref", "--short", "HEAD")
    return done.stdout.strip() if done.returncode == 0 else ""


def _origin_url(repo: Path) -> str:
    """The push URL of the person's `origin` (pushurl and pushInsteadOf apply), read once when the task starts; the push goes to it, never to a later config."""
    done = gitutil.run(repo, "remote", "get-url", "--push", "origin", person_config=True)
    url = done.stdout.strip() if done.returncode == 0 else ""
    if gitutil.is_command_transport(url):
        raise TaskStartError(f"origin's push URL uses the ext:: transport, which runs a program: {url!r}; "
                             "point it at a real remote and start again")
    return url


def _resolve(repo: Path, branch: str) -> str:
    done = gitutil.run(repo, "rev-parse", "--verify", f"{branch}^{{commit}}")
    if done.returncode:
        raise TaskStartError(f"integration branch {branch} does not exist: {done.stderr.strip()}. Check out the branch "
                             "tasks should start from, or set integration_branch in ~/.config/sakura/profile.toml")
    return done.stdout.strip()

"""Contract commands the engine runs itself: who approved them, the cage they run in, and their receipts.

A contract row's `flow` is written by the planner, an agent, so the engine never runs it with its own rights.
Every contract command runs under the cage's Seatbelt profile (workspace writes only, no engine home, no
secrets, loopback network to the registered app port and the host's approved dependent ports only, and the system's
ephemeral port range too once the person approved the project's own sockets). Before the first run the command must be approved: either
it is on the host's list (`checks.approved` in the personal or project config: an exact command or a strict
prefix such as `npm test`, matched on plain words with no shell syntax, and then run without a shell), or the
person approves the digest of the commands that are not (`sakura flow approve <task> checks`). A changed
command changes the digest and asks again. With `--remember` the person's approval also adds each plain-words
command, exactly as written, to the project's remembered list (`checks.remembered`, kept in the engine home): later
contracts that use that very command run without asking. Unlike a `checks.approved` entry, a remembered command is
not a prefix, so adding an argument asks again.

The same goes for every command a repository's own config supplies (`workspace.setup`, `app.start`, `app.stop` and
the `tests.*` commands in the shared or local layer): it runs in the checks cage, with the scrubbed environment, and
only once it is approved the same way (see `flow/repository_commands.py`). Commands of the personal or project layer,
or a flag, are the host's own: they need no approval, and still run in the same cage with the scrubbed environment,
since they execute the workspace's content.

A check row passes only on a receipt the engine wrote after running it: row, contract digest, revision, exit
status and the digest of its log.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import time
import tomllib
from datetime import datetime, timezone
from pathlib import Path

from sakura import beneath, engine_home, layers
from sakura.flow import cage, contract, controls, task_repository
from sakura.flow.config import REPOSITORY_LAYERS, FlowConfig, PresetSteps

APPROVAL_GATE = "checks"
PENDING_KEY = "checks_pending"
RECEIPTS_KEY = "receipts"
# Receipts of the evaluator's `run_row` runs live apart from the engine's own check receipts: neither replaces the other.
EVALUATOR_RECEIPTS_KEY = "evaluator_receipts"
# Receipts of the engine's own run of every calibration row on its base build (`flow/calibration.py`).
CALIBRATION_RECEIPTS_KEY = "calibration_receipts"
ROW_RECEIPTS_DIR = "row-receipts"
# The config keys that hold commands the engine runs: from a repository layer they are untrusted like a contract row.
COMMAND_KEYS = ("workspace.setup", "app.start", "app.stop", "tests.focused", "tests.full", "tests.e2e")
# Anything a shell would read as more than plain words: such a command is never matched by the host list.
SHELL_MARKS = frozenset(";&|<>`$()\\\n*?[]{}~!#'\"")


class ChecksNeedApproval(Exception):
    """The contract has commands neither on the host list nor approved by the person; carries them."""

    def __init__(self, commands: list[str], digest: str) -> None:
        super().__init__(f"{len(commands)} contract commands wait for the person's approval")
        self.commands = commands
        self.digest = digest


def commands(rows: list[contract.Row], steps: PresetSteps) -> list[str]:
    """The distinct commands the engine runs for these rows (owner rows carry instructions, not commands)."""
    return sorted({row.flow for row in contract.proven_by(rows, steps, *contract.ENGINE_RUN_PROVERS)})


def on_list(command: str, approved: list[str], remembered: list[str] = ()) -> bool:
    """True when `command` is plain words that extend, word by word, an `approved` entry or equal a remembered one."""
    if SHELL_MARKS & set(command):
        return False
    words = command.split()
    return any(words[:len(entry.split())] == entry.split() for entry in approved if entry.split()) \
        or any(words == entry.split() for entry in remembered)


def unapproved_commands(commands_: list[str], approved: list[str], remembered: list[str] = ()) -> list[str]:
    """The commands among `commands_` that are neither on the host list nor remembered."""
    return [command for command in dict.fromkeys(commands_) if not on_list(command, approved, remembered)]


def repository_commands(config: FlowConfig) -> list[str]:
    """The commands a repository layer (shared or local) supplies: untrusted, so they need the same approval as a row."""
    return _commands_where(config, lambda layer: layer in REPOSITORY_LAYERS)


def host_commands(config: FlowConfig) -> list[str]:
    """The commands only host layers supply (personal, project, preset defaults, flags): the host's own words."""
    return _commands_where(config, lambda layer: layer not in REPOSITORY_LAYERS)


def _commands_where(config: FlowConfig, from_layer) -> list[str]:
    found = []
    for key in COMMAND_KEYS:
        if from_layer(config.origins.get(key, "defaults")):
            value = layers.flatten(config.values).get(key, [])
            found += [item for item in (value if isinstance(value, list) else [value]) if isinstance(item, str) and item.strip()]
    return list(dict.fromkeys(found))


def unapproved(rows: list[contract.Row], steps: PresetSteps, approved: list[str],
               remembered: list[str] = (), extra: list[str] = ()) -> list[str]:
    """The commands that need the person: not on the host list and not remembered.

    `extra` are commands that are not contract rows, such as the repository's own configured commands.
    """
    return [command for command in sorted({*commands(rows, steps), *extra})
            if not on_list(command, approved, remembered)]


def pending_summary(pending: list[str], rows: list[contract.Row], steps: PresetSteps) -> str:
    """How many commands wait and how many contract rows they cover, as the person reads it: `9 commands for 10 rows`.

    Rows that share a command count once as a command and each as a row; commands that no row runs (the repository's
    own `workspace.setup` or `app.start`) are named apart.
    """
    run_rows = contract.proven_by(rows, steps, *contract.ENGINE_RUN_PROVERS)
    covered = sum(1 for row in run_rows if row.flow in pending)
    from_config = sum(1 for command in pending if command not in {row.flow for row in run_rows})
    text = _counted(len(pending), "command")
    if covered:
        text += f" for {_counted(covered, 'row')}"
    if from_config and covered:
        text += f", {from_config} of them from the repository's config"
    return text


def _counted(number: int, noun: str) -> str:
    return f"{number} {noun}{'' if number == 1 else 's'}"


def digest(pending: list[str]) -> str:
    """The digest the person approves: every command not on the list, in order."""
    return hashlib.sha256(json.dumps(sorted(pending)).encode()).hexdigest()


def granted(state: dict, flows: str | None = None) -> list[str]:
    """The commands the person approved at the `checks` gate. With `flows` (the digest of the flow files the commands
    would run now), an approval that records the flows it was given for counts only when they are these."""
    approval = state.get("approvals", {}).get(APPROVAL_GATE, {})
    if flows is not None and "flows" in approval and approval["flows"] != flows:
        return []
    return approval.get("commands", [])


def require_approved(state: dict, rows: list[contract.Row], steps: PresetSteps, approved: list[str],
                     remembered: list[str] = (), extra: list[str] = (), flows: str | None = None) -> None:
    """Raise ChecksNeedApproval unless every command is on the host list, remembered or approved by the person (for the
    flows digest `flows`, when given)."""
    pending = unapproved(rows, steps, approved, remembered, extra)
    if set(pending) - set(granted(state, flows)):
        raise ChecksNeedApproval(pending, digest(pending))


def argv(command: str, approved: list[str], remembered: list[str] = ()) -> list[str]:
    """A listed or remembered command runs as its words, without a shell; an approved one as the shell text approved."""
    return command.split() if on_list(command, approved, remembered) else ["sh", "-c", command]


def remember(repo: Path, pending: list[str]) -> tuple[list[str], list[str]]:
    """Add the plain-words commands of `pending` to the repository's remembered list; returns (remembered, skipped).

    A command with shell syntax is skipped: it could never be matched without a shell, so the person approves it
    again per task. A command with a control character is skipped too, as the config refuses it (contract loading
    already refuses such a row). The file is written whole and moved into place, so a crash leaves the old list.
    """
    keep = [" ".join(command.split()) for command in pending if _rememberable(command)]
    skipped = [command for command in pending if not _rememberable(command)]
    path = engine_home.remembered_checks_file(repo)
    known = _read_remembered(path)
    merged = known + [command for command in dict.fromkeys(keep) if command not in known]
    if merged != known:
        _write_remembered(path, merged, _read_local_sockets(path))
    return keep, skipped


def remember_local_sockets(repo: Path) -> None:
    """Record, for this repository, that the person let its commands open their own loopback sockets.

    It is the same host state as the remembered commands (`checks.local_sockets`), written whole and moved into place.
    """
    path = engine_home.remembered_checks_file(repo)
    _write_remembered(path, _read_remembered(path), True)


def _write_remembered(path: Path, commands: list[str], local_sockets: bool) -> None:
    engine_home.ensure_private(path.parent)
    temporary = path.with_name(f".{path.name}.{os.getpid()}")
    commands_text = "".join(f"  {json.dumps(command)},\n" for command in commands)
    sockets_text = "local_sockets = true\n" if local_sockets else ""
    temporary.write_text(f"[checks]\nremembered = [\n{commands_text}]\n{sockets_text}")
    temporary.replace(path)


def _rememberable(command: str) -> bool:
    return not SHELL_MARKS & set(command) and not any(controls.is_control(character) for character in command)


def _read_remembered(path: Path) -> list[str]:
    return _remembered_checks(path).get("remembered", [])


def _read_local_sockets(path: Path) -> bool:
    return _remembered_checks(path).get("local_sockets", False) is True


def _remembered_checks(path: Path) -> dict:
    if not path.is_file():
        return {}
    return tomllib.loads(path.read_text()).get("checks", {})


def checks_cage(store, workspace: Path, name: str, policy: cage.Policy, ports: tuple[int, ...] = (),
                build_dir: Path | None = None, local_sockets: bool = False) -> cage.RunCage:
    """The cage of the engine's own contract runs in `workspace`: the lane's limits, and loopback only on `ports`
    (the registered application port and the host's approved dependent ports). With `local_sockets` (the person's
    approval for the project, `checks.local_sockets`) the commands may also open their own sockets on the system's
    ephemeral port range. With `build_dir` (a row run on an engine checkout) the workspace and the Git folder are
    read-only and `build_dir` is the only writable folder. The Git folder the cage is given is the task's own
    repository's (`flow/task_repository.py`), never the person's; raises CageError for a task that has none."""
    roots = tuple(dict.fromkeys((engine_home.root(), engine_home.root_for_trust(), engine_home.account_root())))
    try:
        git_folder = task_repository.common_dir(store)
    except task_repository.TaskRepositoryError as error:
        raise cage.CageError(f"no command runs caged in {workspace}: {error}") from error
    site = cage.RunSite(store.root, name, "checks", workspace, git_folder, roots, store.repo,
                        loopback_ports=tuple(dict.fromkeys(ports)), build_dir=build_dir, local_sockets=local_sockets)
    return cage.prepare(site, policy, 0)


def loopback_ports(config: FlowConfig, application_port: int = 0) -> tuple[int, ...]:
    """The loopback ports a caged run may reach: the registered application port (0: none) and the host's approved ones."""
    return (*([application_port] if application_port else []), *config.values["checks"]["loopback_ports"])


def record_receipt(store, row: contract.Row, rows_digest: str, revision: str, exit_code: int, log: Path,
                   space: str = RECEIPTS_KEY) -> dict:
    """Write the engine's receipt of one run into the task state (`space`: check or evaluator receipts); returns it."""
    receipt = _receipt(row, rows_digest, revision, exit_code, log, store.root)
    store.update_state(lambda state: state.setdefault(space, {}).update({row_key(row): receipt}))
    return receipt


def write_row_receipt(store, row: contract.Row, rows_digest: str, revision: str, exit_code: int, log: Path) -> None:
    """Leave the receipt of an evaluator row run as a file in the task folder, for the engine process to take in.

    The run happens in a child of the agent's process tree, which may only change a stack's last-used time in the
    task state, so its receipt goes through a file that only the engine can write (the agent has no path into
    the engine home).
    """
    folder = store.root / ROW_RECEIPTS_DIR
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = folder / f"{row_key(row).replace('/', '-')}.json"
    temporary = target.with_name(f".{target.name}.{os.getpid()}")
    receipt = _receipt(row, rows_digest, revision, exit_code, log, store.root)
    temporary.write_text(json.dumps({"key": row_key(row), "receipt": receipt}))
    temporary.replace(target)


def ingest_row_receipts(store) -> None:
    """Move the receipts of evaluator row runs from their files into the evaluator receipts of the task state."""
    folder = store.root / ROW_RECEIPTS_DIR
    entries = []
    for path in sorted(folder.glob("*.json")) if folder.is_dir() else []:
        entries.append(json.loads(path.read_text()))
        path.unlink()
    if entries:
        store.update_state(lambda state: state.setdefault(EVALUATOR_RECEIPTS_KEY, {}).update(
            {entry["key"]: entry["receipt"] for entry in entries}))


def clear_row_receipts(store) -> None:
    """Forget receipt files nobody took in, before a new evaluator run."""
    shutil.rmtree(store.root / ROW_RECEIPTS_DIR, ignore_errors=True)


def _receipt(row: contract.Row, rows_digest: str, revision: str, exit_code: int, log: Path, root: Path) -> dict:
    return {"row": row.id, "viewport": row.viewport, "contract": rows_digest, "revision": revision,
            "exit": exit_code, "evidence": str(log.relative_to(root)),
            "evidence_sha256": hashlib.sha256(log.read_bytes()).hexdigest(),
            "at": datetime.now(timezone.utc).isoformat(timespec="seconds"), "at_ns": time.time_ns()}


def receipt_passes(state: dict, row: contract.Row, rows_digest: str, revision: str, since_ns: int = 0,
                   space: str = RECEIPTS_KEY, root: Path | None = None) -> bool:
    """True when the engine's receipt for `row` says exit 0 for this contract and this revision, written after
    `since_ns`; with the task folder `root`, also only while the log it names still has the digest it recorded
    (review 14, finding 3)."""
    receipt = state.get(space, {}).get(row_key(row))
    if not (receipt and receipt["exit"] == 0 and receipt["contract"] == rows_digest
            and receipt["revision"] == revision and receipt.get("at_ns", 0) >= since_ns):
        return False
    return root is None or _evidence_intact(root, receipt)


def _evidence_intact(root: Path, receipt: dict) -> bool:
    """Whether the log a receipt names, below the task folder `root`, still has the digest it recorded."""
    name = receipt.get("evidence")
    if not isinstance(name, str) or not name:
        return False
    try:
        return beneath.digest(root, name) == receipt.get("evidence_sha256")
    except (OSError, ValueError):
        return False


def wait_for_approval(task, waiting: "ChecksNeedApproval", rows: list[contract.Row], steps: PresetSteps,
                      flows: str | None = None) -> str:
    """Keep what waits for the person at the checks gate (`PENDING_KEY`: the commands, the contract they belong to and the
    flow bytes); returns the one-line summary the waiting line carries (the caller ledgers it under its own phase).

    An approval given for other flow bytes covers nothing now and is dropped."""
    contract_digest = hashlib.sha256(task.contract.read_bytes()).hexdigest() if rows else ""
    record = {"contract": contract_digest, "commands": waiting.commands, "sha256": waiting.digest, "flows": flows or ""}

    def keep(state: dict) -> None:
        state[PENDING_KEY] = record
        if granted(state) and not granted(state, flows):
            state["approvals"].pop(APPROVAL_GATE, None)

    task.update_state(keep)
    return pending_summary(waiting.commands, rows, steps)


def waiting_line(state: dict, task_id: str) -> str:
    """The person's item while contract commands wait for approval, or an empty string."""
    pending = state.get(PENDING_KEY)
    if not pending or not set(pending["commands"]) - set(granted(state)):
        return ""
    listed = "; ".join(shlex.quote(controls.escaped(command)) for command in pending["commands"])
    return (f"contract commands need your approval: {listed} (sakura flow approve {task_id} {APPROVAL_GATE}; "
            f"add --remember to run them without asking next time; add --local-sockets only if a command opens its own "
            f"local sockets: it opens the system's high loopback ports, where debuggers, dev servers and other tasks' "
            f"apps listen, to every command of this project, and is remembered for the project)")


def row_key(row: contract.Row) -> str:
    """How a receipt or a mark names its row: the id and the viewport (`api` without one)."""
    return f"{row.id}/{row.viewport or 'api'}"

"""Explicit Goose setup and bounded connection checks."""

from __future__ import annotations

import json
import os
import stat
import tempfile
from pathlib import Path

from sakura import engine_home, programs
from sakura.accounts import registry, write
from sakura.accounts.providers import GOOSE
from sakura.flow import acp, brakes
from sakura.setup import harnesses

CONNECT_PROMPT = "Reply with OK."
ACCOUNT_STATES = {"ready", "needs_setup", "unavailable"}


def configure(root: Path) -> None:
    """Replace the local operator command with the qualified Goose configuration flow."""
    _require_operator()
    installed = harnesses.qualify(GOOSE)
    environment = programs.environment("system", installed.argv[0])
    environment.update(installed.goose_environment(root))
    cwd = engine_home.root().resolve()
    installed.recheck(cwd)
    os.chdir(cwd)
    argv = (*installed.argv[:-1], "configure")
    os.execve(argv[0], argv, environment)


def check(account_id: str) -> dict:
    """Check only the selected account and store a result for the same backing configuration."""
    _require_operator()
    account = next((row for row in registry.load().accounts if (row.provider, row.id) == (GOOSE, account_id)), None)
    if account is None or not account.enabled:
        raise registry.AccountsConfigError("choose an enabled Goose account")
    identity = _config_stamp(account)
    result, artifact = _check(account, identity)
    if _config_stamp(account) != identity:
        return _status("needs_setup", "configuration_changed")
    receipt = _receipt(account)
    with write.locked(receipt):
        write.replace_atomically(receipt, json.dumps({"config": identity, "artifact": artifact, **result}))
    return result


def readiness(account: registry.Account) -> dict:
    """Return the last check for this configuration, without starting an adapter."""
    if not account.enabled:
        return _status("unavailable", "account_disabled")
    if (identity := _config_stamp(account)) is None:
        return _status("needs_setup", "setup_required")
    try:
        saved = json.loads(_receipt(account).read_text())
    except (OSError, ValueError, RecursionError, engine_home.EngineHomeError):
        return _status("unknown", "connection_required")
    if not isinstance(saved, dict) or saved.get("config") != identity:
        return _status("unknown", "configuration_changed")
    state, reason = saved.get("readiness"), saved.get("reason")
    if not isinstance(state, str) or state not in ACCOUNT_STATES or (reason is not None and not isinstance(reason, str)):
        return _status("unknown", "connection_required")
    if state == "ready" and not _artifact_current(saved.get("artifact")):
        return _status("unknown", "qualification_required")
    return _status(state, reason)


def _check(account: registry.Account, identity: list | None) -> tuple[dict, dict | None]:
    if identity is None:
        return _status("needs_setup", "setup_required"), None
    try:
        installed = harnesses.qualify(GOOSE)
        artifact = _artifact(installed)
        with tempfile.TemporaryDirectory(prefix="goose-check-", dir=engine_home.root()) as directory:
            cwd = Path(directory).resolve()
            with installed.connect(cwd, goose_root=account.config_dir, provider=account.goose_provider) as (client, caps):
                _probe(client, caps, cwd)
    except programs.ProgramError as error:
        return _status("unavailable", str(error)), None
    except acp.AcpError as error:
        if str(error) in ("wrong_protocol_version", "wrong_agent_identity", "unsupported_capability"):
            return _status("unavailable", "unsupported_capability"), None
        return _status("needs_setup", "connection_failed"), None
    return _status("ready", None), artifact


def _probe(client: acp.Client, caps: harnesses.Capabilities, cwd: Path) -> None:
    session = client.request("session/new", {"cwd": str(cwd), "mcpServers": []}, harnesses.probe_message).get("sessionId")
    if not isinstance(session, str) or not session or len(session) > 1024:
        raise acp.AcpError("unsupported_capability")
    received_reply = False

    def handle(message: dict) -> dict | None:
        nonlocal received_reply
        received_reply = received_reply or _reply_chunk(message, session)
        return harnesses.probe_message(message)

    try:
        response = client.request("session/prompt", {"sessionId": session,
                                  "prompt": [{"type": "text", "text": CONNECT_PROMPT}]}, handle)
        if response.get("stopReason") != "end_turn" or not received_reply:
            raise acp.AcpError("connection_failed")
    finally:
        if caps.close_session:
            client.request("session/close", {"sessionId": session}, handle)


def _reply_chunk(message: dict, session: str) -> bool:
    params = message.get("params")
    if message.get("method") != "session/update" or "id" in message or not isinstance(params, dict):
        return False
    update = params.get("update")
    if params.get("sessionId") != session or not isinstance(update, dict):
        return False
    content = update.get("content")
    return (update.get("sessionUpdate") == "agent_message_chunk" and isinstance(content, dict)
            and content.get("type") == "text" and isinstance(content.get("text"), str) and bool(content["text"].strip()))


def _artifact(installed: harnesses.Installation) -> dict:
    paths = [installed.adapter.path, str(engine_home.root() / programs.HOST_FILE)]
    if installed.runtime:
        paths.append(installed.runtime.path)
    return {path: _stamp(Path(path)) for path in paths}


def _artifact_current(artifact: object) -> bool:
    return (isinstance(artifact, dict) and bool(artifact)
            and all(expected is not None and expected == _stamp(Path(path)) for path, expected in artifact.items()))


def _config_stamp(account: registry.Account) -> list | None:
    root = account.config_dir
    try:
        if not account.goose_provider or not root.is_absolute() or root.resolve() != root:
            return None
    except (OSError, RuntimeError):
        return None
    markers = [(root, stat.S_ISDIR), (root / "config", stat.S_ISDIR),
               (root / "config" / "config.yaml", stat.S_ISREG)]
    stamps = [_stamp(path, has_type) for path, has_type in markers]
    if any(stamp is None for stamp in stamps):
        return None
    # Session startup may create data or state beside config; their creation never changes the backing identity.
    return [str(root), account.goose_provider, stamps[0][:2], stamps[1][:2], stamps[2]]


def _stamp(path: Path, has_type=None) -> list[int] | None:
    try:
        info = path.lstat()
    except OSError:
        return None
    if stat.S_ISLNK(info.st_mode) or (has_type and (
            not has_type(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & (stat.S_IWGRP | stat.S_IWOTH))):
        return None
    return [info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns]


def _receipt(account: registry.Account) -> Path:
    return engine_home.root() / "setup" / "goose-checks" / f"{account.id}.json"


def _status(readiness: str, reason: str | None) -> dict:
    return {"readiness": readiness, "reason": reason}


def _require_operator() -> None:
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        raise programs.ProgramError("local_setup_required")


def lane_account(account_id: str | None) -> registry.Account:
    """Select a ready Goose account with no cross-provider fallback."""
    try:
        candidates = [row for row in registry.load().accounts if row.provider == GOOSE]
    except registry.AccountsConfigError:
        raise programs.ProgramError("goose_account_registry_unavailable") from None
    selected = [row for row in candidates if row.enabled and (account_id is None or row.id == account_id)]
    if len(selected) != 1:
        raise programs.ProgramError("choose_an_enabled_goose_account")
    row = selected[0]
    if readiness(row)["readiness"] != "ready":
        raise programs.ProgramError("goose_account_connection_check_required")
    return row


def lane_identity(account: registry.Account) -> list | None:
    """Capture a backing configuration's metadata for a lane binding."""
    return _config_stamp(account)

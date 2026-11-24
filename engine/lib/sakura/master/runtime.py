"""Run one qualified Master open or turn under the issued launch identity."""

from __future__ import annotations

import json
import tomllib
from contextlib import contextmanager
from dataclasses import dataclass, field, replace
from pathlib import Path

from sakura import programs
from sakura.accounts.providers import GOOSE
from sakura.flow import acp, cage, goose_cage, presets
from sakura.master import decide, history, qualified, server, spec

TURN_SECONDS = 180
GUIDANCE_START = "Sakura Master instructions:\n"
GUIDANCE_END = "\n\nUser message:\n"
MAX_PROMPT = 256 * 1024


def operate(home: Path, slot: str, launch: str, prompt: str | None = None) -> dict:
    """Open or load a Master session and optionally send one user turn."""
    try:
        if prompt is not None and (not isinstance(prompt, str) or not prompt.strip() or len(prompt.encode()) > MAX_PROMPT):
            raise history.HistoryError("master_invalid_prompt")
        with history.turning(home, slot):
            request = Operation(home, slot, launch)
            return request.run(prompt)
    except (history.HistoryError, programs.ProgramError, acp.AcpError) as error:
        return {"outcome": "refused", "reason": str(error)}
    except (OSError, cage.CageError, ValueError):
        return {"outcome": "refused", "reason": "master_launch_unavailable"}


def permission(home: Path, slot: str, launch: str, message: dict) -> decide.Decision:
    """Evaluate a permission request against the current slot and session."""
    try:
        row = history.current(home, slot, launch)
        params = message.get("params", message)
        if not isinstance(params, dict) or not row["session_id"] or params.get("sessionId") != row["session_id"]:
            raise history.HistoryError("master_session_changed")
        return decide.scoped(message, row["parameters"]["tools"])
    except (history.HistoryError, KeyError) as error:
        return decide.Decision(decide.DENY, str(error))


@dataclass
class Replay:
    """Bound and coalesce the displayable text replayed by the adapter."""

    messages: list[dict] = field(default_factory=list)
    size: int = 0

    def add(self, role: str, text: str) -> None:
        if not text:
            return
        self.size += len(text.encode(errors="replace"))
        if self.size > acp.MAX_REPLY_BYTES or len(self.messages) >= 1000:
            raise acp.AcpError("adapter_output_limit")
        if self.messages and self.messages[-1]["role"] == role:
            self.messages[-1]["text"] += text
        else:
            self.messages.append({"role": role, "text": text})

    @property
    def has_guidance(self) -> bool:
        return any(row["role"] == "user" and row["text"].startswith(GUIDANCE_START)
                   and GUIDANCE_END in row["text"] for row in self.messages)


@dataclass(frozen=True)
class Operation:
    """Execute requests against one issued launch, with no provider fallback."""

    home: Path
    slot: str
    launch: str

    def active(self) -> dict:
        """Read the exact active launch for this slot."""
        return history.current(self.home, self.slot, self.launch)

    def checkpoint(self) -> None:
        self.active()

    def run(self, prompt: str | None) -> dict:
        """Try verified ACP replay before falling back to read-only history."""
        saved = self.active()
        if prompt is None and not saved["session_id"]:
            return {"outcome": "empty", "launch_id": self.launch, "reason": None, "messages": []}
        try:
            selected, files = self._revalidate(saved)
            replay = Replay()
            with self._connection(selected, files, saved) as (client, caps, cwd, mcp):
                turn = self._turn(selected, files, mcp, saved, replay)
                session = acp.Session(client, turn, lambda *_args, **_kwargs: None, self.checkpoint)
                return self._exchange(session, cwd, caps, prompt, saved, replay)
        except (acp.AcpError, programs.ProgramError, history.HistoryError) as error:
            replay_unavailable = isinstance(error, programs.ProgramError) or str(error) in {
                "session_load_refused", "session_load_unsupported", "wrong_agent_identity", "wrong_protocol_version",
                "adapter_mcp_transport_unsupported", "master_adapter_changed", "master_storage_changed",
            }
            if saved["session_id"] and replay_unavailable:
                self.checkpoint()
                return history.transcript(self.home, self.slot, saved["actor"])
            raise

    def _revalidate(self, saved: dict) -> tuple[qualified.Selection, qualified.Files]:
        ref, parameters = saved["binding"], saved["parameters"]
        selected = qualified.select(ref["harness"], ref["account"], parameters["model"], parameters["effort"])
        if selected.binding.storage_namespace != ref["storage_namespace"]:
            raise history.HistoryError("master_storage_changed")
        if history.digest(selected.artifact) != parameters["artifact"]:
            raise history.HistoryError("master_adapter_changed")
        preset = replace(presets.resolve(parameters["preset"]), cage=parameters["cage"], subagents=parameters["subagents"])
        files = qualified.prepare_files(self.home, preset, self.slot, self.launch, selected.harness)
        if files.scope != saved["policy"]:
            raise history.HistoryError("master_policy_changed")
        self.checkpoint()
        return selected, files

    @contextmanager
    def _connection(self, selected, files, saved):
        installed = selected.installed
        if selected.harness != GOOSE:
            mcp = qualified.read_launch_file(self.home, self.launch, "mcp.json")["mcpServers"]
            environment = installed.environment(selected.account.config_dir)
            environment.update(files.worker.env)
            if selected.harness == "codex":
                environment.update(_codex_env(files))
            cwd = files.site.workspace
            with installed.connect(cwd, launch=(installed.argv, environment), timeout=TURN_SECONDS,
                                   checkpoint=self.checkpoint) as (client, caps):
                yield client, caps, cwd, spec._acp_servers(mcp)
            return
        with self._goose_connection(selected, files, saved) as connected:
            yield connected

    @contextmanager
    def _goose_connection(self, selected, files, saved):
        installed = selected.installed
        provider, _ = goose_cage.qualify(installed, selected.account.goose_provider, selected.model)
        parent = self.home / "master" / "goose-sessions"
        identity = saved.get("model_storage")
        root, binding = goose_cage.model_storage(parent, saved["actor"], identity)
        if identity is None:
            history.record_storage(self.home, self.slot, self.launch, binding)
        policy = qualified.read_launch_file(self.home, self.launch, "policy.json")
        tools = server.scoped_tools(policy, server._controller(), saved["actor"], self.checkpoint, connectors=False)
        with goose_cage.open_launch(installed, provider, selected.account.goose_provider, selected.configured_model,
                                    root, files.site, files.cage_policy, files.worker, tools=tools) as adapter:
            self.checkpoint()
            with installed.connect(adapter.cwd, launch=(adapter.argv, adapter.env), timeout=TURN_SECONDS,
                                   checkpoint=self.checkpoint) as (client, caps):
                yield client, caps, adapter.cwd, [adapter.mcp]

    def _turn(self, selected, files, mcp, saved, replay) -> acp.Turn:
        def allow(message):
            return permission(self.home, self.slot, self.launch, message).decision == decide.ASK

        installed = selected.installed
        meta = self._metadata(selected, files)
        is_claude = selected.harness == "claude"
        instructions = "" if is_claude else qualified.instructions_text(self.home, self.launch)
        return acp.Turn(installed.agent_name, installed.adapter.version, "", session_id=saved["session_id"],
                        mcp_servers=tuple(mcp), permission=allow, model=selected.model, effort=selected.effort,
                        configured_model=selected.configured_model, setup_meta=meta, mode=qualified.mode_for(selected.harness),
                        legacy_mode=is_claude, on_replay=replay.add, system_prompt=instructions)

    def _metadata(self, selected, files) -> dict | None:
        if selected.harness != "claude":
            return None
        settings = qualified.read_launch_file(self.home, self.launch, "settings.json")
        instructions = qualified.instructions_text(self.home, self.launch)
        env = {**files.worker.env, **spec._claude_login(spec.MasterAccount(
            selected.account.id, selected.harness, str(selected.account.config_dir)))}
        options = {"settings": settings, "settingSources": [], "strictMcpConfig": True, "env": env,
                   "allowDangerouslySkipPermissions": False}
        if selected.model:
            options["model"] = selected.model
        return {"systemPrompt": {"append": instructions}, "claudeCode": {"options": options}}

    def _exchange(self, session, cwd, caps, prompt, saved, replay) -> dict:
        cancel = True
        try:
            if not saved["session_id"] and not caps.load_session:
                raise acp.AcpError("master_session_load_required")
            session_id = session.open(cwd, caps)
            row = history.opened(self.home, self.slot, self.launch, session_id)
            result = {**history.public(row), "launch_id": self.launch, "messages": replay.messages, "reason": None}
            if prompt is None:
                cancel = False
                return {**result, "outcome": "replayed"}
            text = self._prompt_text(prompt, row, replay, caps)
            reply = session.prompt(text)
            history.prompted(self.home, self.slot, self.launch)
            cancel = False
            return {**result, "outcome": "completed", "reply": reply}
        finally:
            session.shutdown(cancel=cancel)

    def _prompt_text(self, prompt: str, row: dict, replay: Replay, caps: acp.Capabilities) -> str:
        if caps.system_prompt or row["binding"]["harness"] == "claude" or not row["guidance_pending"] or replay.has_guidance:
            return prompt
        instructions = qualified.instructions_text(self.home, self.launch)
        return f"{GUIDANCE_START}{instructions}{GUIDANCE_END}{prompt}"


def _codex_env(files: qualified.Files) -> dict[str, str]:
    pairs = cage.codex_args(files.site, files.cage_policy, files.worker, cage.account_home())
    config = tomllib.loads("\n".join(pairs[1::2]))
    config.update(approval_policy="never", mcp_servers={}, apps={"_default": {"enabled": False}})
    config["features"].update(shell_tool=False, hooks=False)
    return {"CODEX_CONFIG": json.dumps(config), "INITIAL_AGENT_MODE": qualified.CODEX_MASTER_MODE,
            "MODEL_PROVIDER": "openai", "NO_BROWSER": "1"}

"""Bounded newline-delimited ACP transport for a single headless turn."""

from __future__ import annotations

import json
import os
import selectors
import subprocess
import time
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path

from sakura.flow import acp_choices

MAX_FRAME = 2 * 1024 * 1024
MAX_MESSAGES = 8192
MAX_DIAGNOSTIC = 4096
READ_SIZE = 8192
POLL_SECONDS = 0.1
CLOSE_SECONDS = 0.3
MAX_SESSION_BYTES = 1024
MAX_REPLY_BYTES = 1024 * 1024
SESSION_EVENT = "sakura_acp_session"
ACCEPTED_EVENT = "sakura_acp_prompt_accepted"
FINAL_EVENT = "sakura_acp_final"
MESSAGE_EVENT = "sakura_acp_message"
DELTA_EVENT = "sakura_acp_delta"
TOOL_EVENT = "sakura_acp_tool"
ERROR_EVENT = "sakura_acp_error"


class AcpError(RuntimeError):
    """A fixed transport failure code."""


class Client:
    """Drive ACP over two pipes with one deadline and bounded frames."""

    def __init__(self, child: subprocess.Popen, timeout: float, *,
                 checkpoint: Callable[[], None] | None = None):
        self.child = child
        self._deadline = time.monotonic() + timeout
        self._checkpoint = checkpoint
        self._received = bytearray()
        self._stderr = bytearray()
        self._sequence = 0
        self._messages = 0
        for pipe in (child.stdin, child.stdout, child.stderr):
            os.set_blocking(pipe.fileno(), False)

    @property
    def diagnostic(self) -> str:
        """Read the bounded provider stderr captured so far."""
        return self._stderr.decode(errors="replace")

    def request(self, method: str, params: dict, handle: Callable[[dict], dict | None]) -> dict:
        """Send a request and service agent messages until its response arrives."""
        self._sequence += 1
        self._send({"id": self._sequence, "method": method, "params": params})
        while True:
            message = self._receive()
            if "method" in message:
                answer = handle(message)
                if "id" in message:
                    self._send({"id": message["id"], **(answer or {
                        "error": {"code": -32601, "message": "Method unavailable"}})})
            elif type(message.get("id")) is int and message["id"] == self._sequence:
                result = message.get("result")
                error = message.get("error")
                if isinstance(error, dict) and error.get("code") == -32000:
                    raise AcpError("adapter_authentication_required")
                if "error" in message or not isinstance(result, dict):
                    raise AcpError("adapter_rejected_request")
                return result
            else:
                raise AcpError("invalid_adapter_response")

    def notify(self, method: str, params: dict) -> None:
        """Send a final notification without waiting for a response."""
        self._send({"method": method, "params": params})

    def close_grace(self) -> None:
        """Give protocol shutdown its own short deadline after the turn's deadline."""
        self._checkpoint = None
        self._deadline = time.monotonic() + CLOSE_SECONDS

    def _remaining(self) -> float:
        if self._checkpoint:
            self._checkpoint()
        remaining = self._deadline - time.monotonic()
        if remaining <= 0:
            raise AcpError("adapter_timeout")
        return remaining

    def _send(self, message: dict) -> None:
        try:
            payload = memoryview((json.dumps({"jsonrpc": "2.0", **message}) + "\n").encode())
            if len(payload) > MAX_FRAME:
                raise AcpError("adapter_frame_limit")
            with selectors.DefaultSelector() as selector:
                selector.register(self.child.stdin, selectors.EVENT_WRITE)
                while payload:
                    self._ready(selector)
                    payload = payload[os.write(self.child.stdin.fileno(), payload):]
        except OSError as error:
            raise AcpError("adapter_disconnected") from error

    def _receive(self) -> dict:
        self._remaining()
        self._messages += 1
        if self._messages > MAX_MESSAGES:
            raise AcpError("adapter_frame_limit")
        while (end := self._received.find(b"\n")) == -1:
            if len(self._received) >= MAX_FRAME:
                raise AcpError("adapter_frame_limit")
            self._read()
        if end >= MAX_FRAME:
            raise AcpError("adapter_frame_limit")
        raw = self._received[:end]
        del self._received[:end + 1]
        try:
            message = json.loads(raw)
        except (ValueError, RecursionError) as error:
            raise AcpError("invalid_adapter_response") from error
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            raise AcpError("invalid_adapter_response")
        return message

    def _ready(self, selector: selectors.BaseSelector) -> list:
        while True:
            remaining = self._remaining()
            ready = selector.select(min(remaining, POLL_SECONDS) if self._checkpoint else remaining)
            if ready:
                return ready

    def _read(self) -> None:
        with selectors.DefaultSelector() as selector:
            selector.register(self.child.stdout, selectors.EVENT_READ)
            if not self.child.stderr.closed:
                selector.register(self.child.stderr, selectors.EVENT_READ)
            ready = self._ready(selector)
        for key, _ in ready:
            data = os.read(key.fd, READ_SIZE)
            if key.fileobj is self.child.stderr:
                if not data:
                    self.child.stderr.close()
                self._stderr.extend(data[:MAX_DIAGNOSTIC - len(self._stderr)])
            elif data:
                self._received.extend(data)
            else:
                raise AcpError("adapter_disconnected")


@dataclass(frozen=True)
class Turn:
    """A qualified peer, approved tools and one prompt or continuation."""

    agent_name: str
    agent_version: str
    prompt: str
    session_id: str = ""
    mcp_servers: tuple[dict, ...] = ()
    permission: Callable[[dict], bool] | None = None
    log_prompt: bool = False
    on_accept: Callable[[], None] | None = None
    model: str = ""
    effort: str = ""
    configured_model: str = ""
    setup_meta: dict | None = None
    mode: str = ""
    legacy_mode: bool = False
    on_replay: Callable[[str, str], None] | None = None
    system_prompt: str = ""


@dataclass(frozen=True)
class Capabilities:
    """Optional server operations negotiated for this connection."""

    load_session: bool
    mcp_http: bool
    mcp_sse: bool
    close_session: bool
    system_prompt: bool = False


def initialize(client: Client, name: str, version: str) -> Capabilities:
    """Require ACP v1 and the exact installed peer identity."""
    result = client.request("initialize", {"protocolVersion": 1, "clientCapabilities": {}}, probe_message)
    selected = result.get("protocolVersion")
    if type(selected) is not int or selected != 1:
        raise AcpError("wrong_protocol_version")
    info = _object(result.get("agentInfo"))
    if (info.get("name"), info.get("version")) != (name, version):
        raise AcpError("wrong_agent_identity")
    caps = _object(result.get("agentCapabilities"))
    mcp = _object(caps.get("mcpCapabilities"))
    session = _object(caps.get("sessionCapabilities"))
    return Capabilities(caps.get("loadSession") is True, mcp.get("http") is True, mcp.get("sse") is True,
                        isinstance(session.get("close"), dict), isinstance(session.get("systemPrompt"), dict))


def probe_message(message: dict) -> dict | None:
    """Permit no tool on an unattended connection with no policy."""
    if message.get("method") == "session/request_permission":
        return {"result": permission_outcome(message, allowed=False, select_rejection=False)}
    return None


def valid_session_id(value: object) -> bool:
    """ACP session IDs stay opaque and bounded and never enter an argv or path."""
    return isinstance(value, str) and bool(value.strip()) and value.isprintable() and len(value.encode()) <= MAX_SESSION_BYTES


def _object(value: object) -> dict:
    return value if isinstance(value, dict) else {}


def permission_outcome(message: dict, *, allowed: bool, select_rejection: bool = True) -> dict:
    """Select the one-time choice from the agent's options or cancel the request."""
    options = _object(message.get("params")).get("options")
    kinds = ("allow_once",) if allowed else (("reject_once",) if select_rejection else ())
    for option in options if isinstance(options, list) else ():
        if not isinstance(option, dict) or option.get("kind") not in kinds:
            continue
        if valid_session_id(option.get("optionId")):
            return {"outcome": {"outcome": "selected", "optionId": option["optionId"]}}
    return {"outcome": {"outcome": "cancelled"}}


class Session:
    """Keep replay, permission decisions and final assistant output within one turn."""

    def __init__(self, client: Client, turn: Turn, write: Callable[..., None], activity: Callable[[], None]):
        self.client = client
        self.turn = turn
        self._write = write
        self._activity = activity
        self._parts: list[str] = []
        self._open_tools: set[str] = set()
        self._bytes = 0
        self._session = ""
        self._caps: Capabilities | None = None
        self._choices = acp_choices.Choices()
        self._model = ""
        self._configured_model = ""

    def run(self, cwd: Path, caps: Capabilities | None = None) -> str:
        """Perform exactly one prompt and return only the completed assistant reply."""
        self.open(cwd, caps)
        return self.prompt(self.turn.prompt)

    def open(self, cwd: Path, caps: Capabilities | None = None) -> str:
        """Open and validate one session before allowing prompts."""
        self._caps = caps or initialize(self.client, self.turn.agent_name, self.turn.agent_version)
        self._activity()
        self._session = self._open(cwd, self._caps)
        self._select_start_choices()
        model = self._choices.option("model")
        self._model = self.turn.model or (model.current if model else "")
        self._configured_model = self.turn.configured_model if model is None else ""
        self._verify_active_choices()
        self._write(SESSION_EVENT, session_id=self._session)
        return self._session

    def prompt(self, text: str) -> str:
        """Return only completed assistant text from the opened session."""
        params = {"sessionId": self._session, "prompt": [{"type": "text", "text": text}]}
        result = self.client.request("session/prompt", params, self._handle)
        if result.get("stopReason") != "end_turn" or self._open_tools:
            raise AcpError("turn_incomplete")
        self._write(ACCEPTED_EVENT)
        if self.turn.on_accept:
            self.turn.on_accept()
        if self.turn.log_prompt:
            self._write("sakura_message", text=text)
        reply = "".join(self._parts)
        if not reply.strip():
            raise AcpError("empty_assistant_reply")
        return reply

    def _select_start_choices(self) -> None:
        def request(method, params):
            return self.client.request(method, params, self._setup_message)

        change = not self.turn.session_id
        self._choices.select(request, "model", self.turn.model, default=self.turn.configured_model, allow_change=change)
        self._choices.select(request, "thought_level", self.turn.effort, allow_change=change)
        if self.turn.legacy_mode and self.turn.mode and self._choices.option("mode") is None:
            request("session/set_mode", {"sessionId": self._session, "modeId": self.turn.mode})
        else:
            self._choices.select(request, "mode", self.turn.mode)

    def _setup_message(self, message: dict) -> dict | None:
        self._choices.handle(message)
        if self.turn.on_replay and self.turn.session_id and not self._session:
            self._replay(message)
        return probe_message(message)

    def _replay(self, message: dict) -> None:
        params = _object(message.get("params"))
        if (message.get("method") != "session/update" or "id" in message
                or params.get("sessionId") != self.turn.session_id):
            return
        update = _object(params.get("update"))
        content = _object(update.get("content"))
        kind = update.get("sessionUpdate")
        text = content.get("text")
        if content.get("type") == "text" and isinstance(text, str) and kind in ("user_message_chunk", "agent_message_chunk"):
            self.turn.on_replay("user" if kind == "user_message_chunk" else "assistant", text)

    def shutdown(self, *, cancel: bool) -> None:
        """Cancel a pending prompt and close only when the peer advertised close."""
        if not self._session or not self._caps:
            return
        params = {"sessionId": self._session}
        self.client.close_grace()
        with suppress(AcpError, OSError):
            if cancel:
                self.client.notify("session/cancel", params)
            if self._caps.close_session:
                self.client.request("session/close", params, probe_message)

    def _open(self, cwd: Path, caps: Capabilities) -> str:
        for server in self.turn.mcp_servers:
            if (server.get("type") == "http" and not caps.mcp_http) or (server.get("type") == "sse" and not caps.mcp_sse):
                raise AcpError("adapter_mcp_transport_unsupported")
        params = {"cwd": str(cwd), "mcpServers": list(self.turn.mcp_servers)}
        if caps.system_prompt and self.turn.system_prompt:
            params["systemPrompt"] = {"mode": "append", "content": [{"type": "text", "text": self.turn.system_prompt}]}
        if self.turn.setup_meta:
            params["_meta"] = self.turn.setup_meta
        requested = self.turn.session_id
        if requested:
            if not valid_session_id(requested) or not caps.load_session:
                raise AcpError("session_load_unsupported")
            try:
                response = self.client.request("session/load", {**params, "sessionId": requested}, self._setup_message)
            except AcpError as error:
                if str(error) == "adapter_rejected_request":
                    raise AcpError("session_load_refused") from None
                raise
            if response.get("sessionId", requested) != requested:
                raise AcpError("session_load_refused")
            self._choices.opened(requested, response)
            return requested
        response = self.client.request("session/new", params, self._setup_message)
        received = response.get("sessionId")
        if not valid_session_id(received):
            raise AcpError("invalid_adapter_session")
        self._choices.opened(received, response)
        return received

    def _handle(self, message: dict) -> dict | None:
        params = _object(message.get("params"))
        matches = params.get("sessionId") == self._session
        method = message.get("method")
        if method == "session/request_permission" and "id" in message:
            allowed = bool(matches and self.turn.permission and self.turn.permission(message))
            return {"result": permission_outcome(message, allowed=allowed, select_rejection=matches)}
        if method == "session/update" and "id" not in message and matches:
            if self._choices.handle(message):
                self._verify_active_choices()
            self._update(_object(params.get("update")))
        return None

    def _verify_active_choices(self) -> None:
        model = self._choices.option("model")
        if self._model and self._model != (model.current if model else self._configured_model):
            raise AcpError("model_unavailable")
        effort = self._choices.option("thought_level")
        if self.turn.effort and (effort is None or effort.current != self.turn.effort):
            raise AcpError("effort_unavailable")


    def _update(self, update: dict) -> None:
        kind = update.get("sessionUpdate")
        if kind == "agent_message_chunk":
            content = _object(update.get("content"))
            text = content.get("text")
            if content.get("type") == "text" and isinstance(text, str) and text:
                self._bytes += len(text.encode(errors="replace"))
                if self._bytes > MAX_REPLY_BYTES:
                    raise AcpError("adapter_output_limit")
                self._parts.append(text)
                self._write(DELTA_EVENT, text=text)
                self._activity()
        elif kind in ("tool_call", "tool_call_update") and valid_session_id(update.get("toolCallId")):
            if kind == "tool_call" and self._parts:
                self._write(MESSAGE_EVENT, text="".join(self._parts))
                self._parts.clear()
            title = update.get("title")
            tool_id = update["toolCallId"]
            closed = update.get("status") in ("completed", "failed")
            if closed:
                self._open_tools.discard(tool_id)
            elif kind == "tool_call":
                self._open_tools.add(tool_id)
            self._write(TOOL_EVENT, tool_id=tool_id, title=title[:200] if isinstance(title, str) else "",
                        closed=closed, starts=kind == "tool_call")
            self._activity()

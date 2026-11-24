"""Separate the credential-bearing Goose adapter from its workspace tools."""

from __future__ import annotations

import contextlib
import hmac
import http.client
import json
import os
import secrets
import stat
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, replace
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from socket import SHUT_RDWR
from typing import TYPE_CHECKING

from sakura import mcp_stdio, programs
from sakura.flow import cage, cage_server, denied_commands, process
from sakura.flow.config import REVIEW_ROLES
from sakura.flow.store import TaskStore

if TYPE_CHECKING:
    from sakura.setup.harnesses import Installation

LOCAL = "127.0.0.1"
IO_TIMEOUT = 5
WORKER_TIMEOUT = 30
MAX_PROVIDER_INPUT = 8 * 1024 * 1024
MAX_PROVIDER_OUTPUT = 32 * 1024 * 1024
MAX_PROVIDER_SECONDS = 120
TOOL_REVOKE_SECONDS = 12
KEYCHAIN_SERVICES = ("com.apple.SecurityServer", "com.apple.securityd")
WRITE_ROLES = frozenset({"planner", "builder"})
MODEL_FILES = ("data", "state", "tmp", "cwd")
SHELL = "/bin/sh"


class ChannelError(RuntimeError):
    """A channel refusal whose message is safe to show."""


@dataclass(frozen=True)
class Provider:
    """A supported upstream with exact request paths and a fixed connection destination."""

    host: str
    port: int
    host_variable: str
    paths: frozenset[str]
    keychain: tuple[str, ...] = ()

    def permits(self, method: str, path: str) -> bool:
        """Only forward model inference on paths the engine owns."""
        return method == "POST" and path in self.paths

    def open(self, method: str, path: str, body: bytes, authorization: str):
        """Open one fixed upstream request without proxy environment or redirect handling."""
        if not self.permits(method, path):
            raise ChannelError("provider_path_refused")
        connect = http.client.HTTPSConnection if self.keychain else http.client.HTTPConnection
        connection = connect(self.host, self.port, timeout=IO_TIMEOUT)
        headers = {"Content-Type": "application/json"}
        if self.keychain and authorization:
            headers["Authorization"] = authorization
        try:
            connection.request(method, path, body, headers)
            response = connection.getresponse()
            if 300 <= response.status < 400:
                raise ChannelError("provider_redirect_refused")
            return connection, response
        except BaseException:
            connection.close()
            raise


PROVIDERS = {
    "ollama": Provider(LOCAL, 11434, "OLLAMA_HOST", frozenset({"/api/chat", "/api/generate"})),
    "openai": Provider("api.openai.com", 443, "OPENAI_HOST", frozenset({"/v1/chat/completions"}),
                       KEYCHAIN_SERVICES),
}


def qualify(installed: Installation, upstream: str, model: str) -> tuple[Provider, str]:
    """Accept only a provider covered by the host's pinned lane proof."""
    provider = PROVIDERS.get(upstream)
    default = dict(installed.lane_models).get(upstream)
    if provider is None:
        raise cage.CageError("unsupported_goose_provider")
    if sys.platform != "darwin" or not installed.setup_safe or not default or installed.runtime:
        raise cage.CageError("goose_confinement_proof_required")
    if not Path(cage.SEATBELT).is_file():
        raise cage.CageError("goose_seatbelt_unavailable")
    selected = model or default
    if len(selected) > 256 or not selected.isprintable():
        raise cage.CageError("invalid_goose_model")
    return provider, selected


def model_storage(parent: Path, run: str, expected: list[int] | None = None) -> tuple[Path, list[int]]:
    """Create a private per-conversation root or recheck its recorded identity."""
    if Path(run).name != run or run in {"", ".", ".."} or parent.resolve() != parent:
        raise cage.CageError("session_storage_changed")
    folder = parent / run
    try:
        parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        _private_directory(parent)
        if expected is None:
            folder.mkdir(mode=0o700)
            for name in ("config", *MODEL_FILES):
                (folder / name).mkdir(mode=0o700)
        identity = _private_directory(folder)
        for name in ("config", *MODEL_FILES):
            _private_directory(folder / name)
    except OSError:
        raise cage.CageError("session_storage_changed") from None
    if expected is not None and identity != expected:
        raise cage.CageError("session_storage_changed")
    return folder, identity


def _private_directory(path: Path) -> list[int]:
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise cage.CageError("session_storage_changed")
    return [info.st_dev, info.st_ino]


def model_profile(adapter: Path, home: Path, root: Path, ports: tuple[int, ...], provider: Provider) -> str:
    """Build a model profile with no tool execution or direct provider socket."""
    readable = [str(root), str(adapter), "/", "/usr/lib", "/System/Library", "/private/etc/ssl", "/private/etc/localtime"]
    writes = [str(root / name) for name in ("data", "state", "tmp")]
    metadata = cage._ancestors(readable, [Path("/")])
    keychains = cage.with_aliases([str(home / "Library/Keychains"), "/Library/Keychains"])
    fallback = [str(root / "**" / name) for name in ("secrets.yaml", "secrets.yml", "secrets.json")]
    services = [f'(allow mach-lookup (global-name "{service}"))' for service in provider.keychain]
    return "\n".join([
        "(version 1)", "(allow default)", "(deny file-read*)",
        f"(allow file-read* {cage._filters(readable)} {cage._device_filters()})",
        f"(allow file-read-metadata {cage._literals(metadata)})",
        "(deny file-write*)", f"(allow file-write* {cage._filters(writes)} {cage._device_filters()})",
        f"(deny file-write* {cage._literals(writes)})",
        f"(deny file-read* file-write* {cage._filters([*keychains, *fallback])})",
        "(deny network*)", *(_port_rule("allow", port) for port in ports),
        "(deny mach-lookup)", *services, "(deny process-fork)", "(deny process-exec)",
        f"(allow process-exec {cage._literals([str(adapter)])})",
        "(deny appleevent-send)", "(deny signal)", *cage.PROCESS_INFO_RULES, "",
    ])


def worker_profile(site: cage.RunSite, policy: cage.Policy, run_cage: cage.RunCage, home: Path,
                   model_root: Path, ports: tuple[int, ...]) -> str:
    """Reuse the worker baseline and close access to the model's files and channels."""
    site = replace(site, denied_reads=(*site.denied_reads, model_root))
    base = cage.seatbelt_profile(site, policy, run_cage, home, read_only=site.role not in WRITE_ROLES)
    blocked_ports = (*ports, PROVIDERS["ollama"].port)
    return base + "\n".join(["(deny mach-lookup)", *(_port_rule("deny", port) for port in blocked_ports), ""])


def _port_rule(action: str, port: int) -> str:
    if type(port) is not int or not 0 < port <= 65535:
        raise cage.CageError("invalid_goose_channel")
    # Seatbelt accepts only "*" or "localhost" as the host of a network address.
    return f'({action} network-outbound (remote ip "localhost:{port}"))'


class ToolChannel:
    """Authenticate one run's MCP requests and revoke its tool grant at teardown."""

    def __init__(self, tools: list[mcp_stdio.Tool]):
        self.authorization = f"Bearer {secrets.token_urlsafe(32)}"
        self.revoked = threading.Event()
        self._tools = {tool.spec["name"]: tool for tool in tools}

    def answer(self, authorization: str, body: bytes) -> dict | None:
        """Dispatch a bounded JSON-RPC request after checking the bearer."""
        if self.revoked.is_set() or not hmac.compare_digest(self.authorization.encode(), authorization.encode()):
            raise ChannelError("tool_channel_forbidden")
        if len(body) > mcp_stdio.MAX_LINE:
            raise ChannelError("tool_request_too_large")
        return mcp_stdio._answer("sakura", self._tools, body.decode(errors="replace"))

    def revoke(self) -> None:
        """Deny further dispatch and signal any active worker to stop."""
        self.revoked.set()


class _Server(HTTPServer):
    allow_reuse_address = False

    def __init__(self, handler, dispatch):
        self.dispatch = dispatch
        self.revoked = threading.Event()
        self._connections = set()
        self._lock = threading.Lock()
        super().__init__((LOCAL, 0), handler)

    def get_request(self):
        connected, peer = super().get_request()
        with self._lock:
            if self.revoked.is_set():
                connected.close()
                raise OSError("listener revoked")
            self._connections.add(connected)
        connected.settimeout(IO_TIMEOUT)
        return connected, peer

    def close_request(self, request):
        with self._lock:
            self._connections.discard(request)
        super().close_request(request)

    def revoke(self):
        self.revoked.set()
        with self._lock:
            for connection in self._connections:
                with contextlib.suppress(OSError):
                    connection.shutdown(SHUT_RDWR)

    @property
    def url(self) -> str:
        return f"http://{LOCAL}:{self.server_port}"


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        return None

    def do_POST(self):
        try:
            if self.headers.get("Host") != f"{LOCAL}:{self.server.server_port}":
                raise ChannelError("invalid_channel_host")
            self.server.dispatch(self)
        except ChannelError as error:
            self.send_error(403 if str(error) == "tool_channel_forbidden" else 400)
        except (OSError, ValueError, http.client.HTTPException):
            self.close_connection = True

    def body(self, limit: int) -> bytes:
        if self.headers.get("Transfer-Encoding") or len(self.headers.get_all("Content-Length", [])) != 1:
            raise ChannelError("invalid_content_length")
        size = int(self.headers["Content-Length"])
        if not 0 <= size <= limit:
            raise ChannelError("request_too_large")
        data = bytearray()
        deadline = time.monotonic() + IO_TIMEOUT
        while len(data) < size:
            self.connection.settimeout(max(0.001, deadline - time.monotonic()))
            chunk = self.rfile.read1(min(65536, size - len(data)))
            if not chunk or time.monotonic() >= deadline:
                raise ChannelError("incomplete_request")
            data.extend(chunk)
        return bytes(data)

    def reply(self, status: int, content_type: str = "application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Connection", "close")
        self.end_headers()


def _serve_tools(channel: ToolChannel, request: _Handler) -> None:
    if request.path != "/mcp":
        raise ChannelError("tool_path_refused")
    answer = channel.answer(request.headers.get("Authorization", ""), request.body(mcp_stdio.MAX_LINE))
    request.reply(202 if answer is None else 200)
    if answer is not None:
        request.wfile.write(json.dumps(answer).encode())


def _serve_provider(provider: Provider, request: _Handler) -> None:
    if not provider.permits(request.command, request.path):
        raise ChannelError("provider_path_refused")
    upstream, response = provider.open(request.command, request.path, request.body(MAX_PROVIDER_INPUT),
                                       request.headers.get("Authorization", ""))
    with contextlib.closing(upstream):
        upstream_type = response.getheader("Content-Type", "").partition(";")[0]
        known_types = ("application/json", "application/x-ndjson", "text/event-stream")
        request.reply(response.status, upstream_type if upstream_type in known_types else "application/json")
        _copy_response(response, request)


def _copy_response(response, request: _Handler) -> None:
    remaining = MAX_PROVIDER_OUTPUT
    deadline = time.monotonic() + MAX_PROVIDER_SECONDS
    while remaining and not request.server.revoked.is_set() and time.monotonic() < deadline:
        data = response.read1(min(65536, remaining))
        if not data:
            return
        remaining -= len(data)
        request.wfile.write(data)


@dataclass(frozen=True)
class Launch:
    """One caged ACP model and the MCP specification accompanying it."""

    argv: tuple[str, ...]
    cwd: Path
    env: dict[str, str]
    mcp: dict
    tools: frozenset[str]


@contextlib.contextmanager
def open_launch(installed: Installation, provider: Provider, upstream: str, model: str, model_root: Path,
                site: cage.RunSite, policy: cage.Policy, worker: cage.RunCage, *, tools: list[mcp_stdio.Tool] | None = None):
    """Hold both run-local listeners and the worker grant for the lifetime of the adapter."""
    home = cage.account_home().resolve()
    worker_profile_file = worker.folder / "goose-worker.sb"
    revoked = threading.Event()
    if tools is None:
        tools = _tools(site, worker, worker_profile_file, revoked)
    channel = ToolChannel(tools)
    with contextlib.ExitStack() as stack:
        server = stack.enter_context(_Server(_Handler, lambda request: _serve_tools(channel, request)))
        proxy = stack.enter_context(_Server(_Handler, lambda request: _serve_provider(provider, request)))
        ports = (server.server_port, proxy.server_port)
        _write_profiles(worker, site, policy, home, model_root, installed, ports, provider)
        _write_config(model_root, upstream, model)
        listeners = (server, proxy)
        threads = []
        stack.callback(_revoke, channel, revoked, listeners, threads)
        for listener in listeners:
            threads.append(_start_listener(listener, revoked))
        env = programs.environment("system", installed.argv[0], {})
        env.update(HOME=str(home), GOOSE_PATH_ROOT=str(model_root), TMPDIR=str(model_root / "tmp"),
                   GOOSE_MODE="approve", GOOSE_TELEMETRY_ENABLED="false", RUST_LOG="off")
        env[provider.host_variable] = proxy.url
        mcp = {"type": "http", "name": "sakura", "url": server.url + "/mcp",
               "headers": [{"name": "Authorization", "value": channel.authorization}]}
        yield Launch((cage.SEATBELT, "-f", str(worker.folder / "goose-model.sb"), *installed.argv),
                     model_root / "cwd", env, mcp, frozenset(tool.spec["name"] for tool in tools))


def _revoke(channel: ToolChannel, revoked: threading.Event, listeners: tuple[_Server, ...],
            threads: list[threading.Thread]) -> None:
    channel.revoke()
    revoked.set()
    for listener in listeners:
        listener.revoke()
    if threads:
        threads[0].join(TOOL_REVOKE_SECONDS)
    if len(threads) > 1:
        threads[1].join(IO_TIMEOUT)


def _start_listener(listener: _Server, revoked: threading.Event) -> threading.Thread:
    def serve() -> None:
        listener.timeout = 0.2
        while not revoked.is_set():
            try:
                listener.handle_request()
            except (OSError, ValueError):
                return

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return thread


def _write_profiles(worker, site, policy, home, root, installed, ports, provider):
    if cage._verified_roots(site) != worker.tmp:
        raise cage.CageError("goose_worker_roots_changed")
    token = cage._CAPTURED.set(cage._captured_roots(site, worker.tmp))
    try:
        worker_text = worker_profile(site, policy, worker, home, root, ports)
    finally:
        cage._CAPTURED.reset(token)
    if cage._verified_roots(site) != worker.tmp:
        raise cage.CageError("goose_worker_roots_changed")
    cage._write_text(worker.folder / "goose-worker.sb", worker_text)
    cage._write_text(worker.folder / "goose-model.sb", model_profile(Path(installed.adapter.path), home, root, ports, provider))


def _write_config(root: Path, upstream: str, model: str) -> None:
    # JSON is a YAML subset. The adapter gets no files from the account configuration.
    config = {"GOOSE_PROVIDER": upstream, "GOOSE_MODEL": model, "GOOSE_MODE": "approve",
              "GOOSE_TELEMETRY_ENABLED": False, "extensions": {}, "experiments": {}}
    descriptor = os.open(root / "config", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    temporary = f".config-{secrets.token_hex(8)}"
    try:
        output = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=descriptor)
        with os.fdopen(output, "w", encoding="utf-8") as stream:
            json.dump(config, stream)
            stream.write("\n")
        os.replace(temporary, "config.yaml", src_dir_fd=descriptor, dst_dir_fd=descriptor)
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(temporary, dir_fd=descriptor)
        os.close(descriptor)


def _tools(site, worker, profile, revoked) -> list[mcp_stdio.Tool]:
    policy = json.loads(worker.policy.read_text())
    if site.role == "master":
        from sakura.master import server
        return server.build(policy, server._controller(), site.run, connectors=False)
    tools = cage_server.tools_for(policy, connectors=False)
    if site.role in WRITE_ROLES or site.role in REVIEW_ROLES or site.role == "evaluator":
        tools.append(_developer_tool(site, worker, profile, revoked))
    return tools


def _developer_tool(site, worker, profile, revoked) -> mcp_stdio.Tool:
    spec = {"name": "workspace_command", "description": "Run a command under the role's workspace and network grants.",
            "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"],
                            "additionalProperties": False}}
    return mcp_stdio.Tool(spec, lambda args: _run_command(site, worker, profile, revoked, args))


def _run_command(site, worker, profile, revoked, args) -> tuple[str, bool]:
    command = args.get("command")
    if set(args) != {"command"} or not isinstance(command, str) or not 0 < len(command) <= 8192 or "\0" in command:
        return "invalid workspace command", True
    if reason := denied_commands.reason(command):
        cage_server._record({"run": site.run, "role": site.role, "requests": worker.requests}, "workspace_command", reason)
        return reason, True
    env = {**programs.environment("system", SHELL, {}), **worker.env, **site.worker_env, "HOME": str(worker.tmp)}
    env["PATH"] = f"{cage.KIT_BIN.parent}{os.pathsep}{env['PATH']}"

    def require_active() -> None:
        if revoked.is_set():
            raise ChannelError("tool_channel_revoked")
        if cage._verified_roots(site) != worker.tmp:
            raise ChannelError("goose_worker_roots_changed")

    require_active()
    with tempfile.TemporaryDirectory(dir=worker.folder, prefix="tool-") as folder:
        log = Path(folder) / "output"
        task = TaskStore(site.repo, site.task_root.name)
        with log.open("ab") as output:
            outcome = process.external_tracked_run(
                task, [cage.SEATBELT, "-f", str(profile), SHELL, "-c", command], cwd=site.workspace,
                env=env, log_path=log, budget_seconds=WORKER_TIMEOUT, idle_seconds=WORKER_TIMEOUT,
                stop_file=task.stop_file, on_poll=require_active, log_file=output)
        return process._read_back(log)[-12000:], outcome.exit_code != 0

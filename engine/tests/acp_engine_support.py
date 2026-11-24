"""Install and pin executable ACP fixtures without consulting host installs."""

import hashlib
import json
import sys
from pathlib import Path

from sakura import engine_home, programs

SCRIPT = Path(__file__).parent / "fixtures" / "acp_engine_agent.py"


def install(root, monkeypatch, provider, *, native=None, **config):
    folder = root / f"install-{provider}"
    folder.mkdir()
    logfile = root / f"{provider}-wire.jsonl"
    version = "1.60.2" if provider == "goose" else "1.0.0"
    variable = {"claude": "CLAUDE_CONFIG_DIR", "codex": "CODEX_HOME", "goose": "GOOSE_PROVIDER"}[provider]
    config = {"account_var": variable, "harness": provider, **config, "log": str(logfile), "version": version}
    adapter_name = {"claude": "claude-agent-acp", "codex": "codex-acp", "goose": "goose"}[provider]
    adapter = folder / adapter_name
    adapter.write_text(f"#!{sys.executable}\n{SCRIPT.read_text()}\nmain({config!r})\n")
    adapter.chmod(0o700)
    paths = {"adapter": adapter}
    if provider != "goose":
        native = Path(native or folder / provider)
        if not native.exists():
            native.write_text("#!/bin/sh\nexit 99\n")
            native.chmod(0o700)
        paths["native"] = native
    entries = "".join(f"{key} = {json.dumps(str(path.resolve()))}\n"
                      for key, path in {adapter_name: adapter, provider: paths.get("native", adapter)}.items())
    pins = "".join(f'{key} = {{path = {json.dumps(str(path.resolve()))}, version = "{version}", '
                   f'sha256 = "{hashlib.sha256(path.read_bytes()).hexdigest()}"}}\n' for key, path in paths.items())
    extra = 'setup_safe = true\nlane_models = {ollama = "fixture:3b"}\n' if provider == "goose" else ""
    (engine_home.root() / programs.HOST_FILE).write_text(
        f'{entries}\n[acp.{provider}]\nagent_name = "fixture"\nmaster_safe = true\n{extra}{pins}')
    monkeypatch.setenv("PATH", str(folder))
    monkeypatch.setattr(programs, "install_folders", list)
    return lambda: [json.loads(line) for line in logfile.read_text().splitlines()]

"""`sakura master decide --json`: the engine's answer to the master's ACP permission requests."""

import io
import json
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.flow import brakes, exits, presets
from sakura.master import cli, decide, spec

ACCOUNT_HOME = Path("/home/someone")


@pytest.fixture(autouse=True)
def written_settings():
    """The files a master start leaves behind, which `decide` reads."""
    return spec.prepare(engine_home.root(), presets.BUILTINS["standard"], account_home=ACCOUNT_HOME, environ={})


def _request(tool, tool_input=None, *, envelope=True) -> bytes:
    call = {"toolCallId": "t1", "title": tool, "rawInput": tool_input or {}, "_meta": {"claudeCode": {"toolName": tool}}}
    params = {"sessionId": "s1", "toolCall": call, "options": []}
    message = {"jsonrpc": "2.0", "id": 7, "method": decide.METHOD, "params": params} if envelope else params
    return json.dumps(message).encode()


def _ask(raw: bytes, capsys) -> dict:
    assert cli.main(["decide", "--json"], stdin=io.BytesIO(raw)) == exits.OK
    return json.loads(capsys.readouterr().out)


def test_MasterDecide_DeniedPath_Deny(capsys, tmp_path):
    inside_engine = str(engine_home.root() / "state.json")
    assert _ask(_request("Read", {"file_path": inside_engine}), capsys)["decision"] == "deny"
    assert _ask(_request("Edit", {"file_path": str(tmp_path / "elsewhere.txt")}), capsys)["decision"] == "deny"
    assert _ask(_request("Write", {"file_path": str(spec.master_folder(engine_home.root()) / "instructions.md")}),
                capsys)["decision"] == "deny"
    answer = _ask(_request("Bash", {"command": "cd /x && git push origin main"}), capsys)
    assert answer["decision"] == "deny" and "git push" in answer["reason"]


def test_MasterDecide_AllowedTool_Ask(capsys):
    """Claude Code applies the allow rules before it asks: what reaches the engine was not granted, so never `allow`."""
    answer = _ask(_request("mcp__sakura__lane_open", {"project": "p", "text": "t"}), capsys)
    assert answer["decision"] == "ask"


def test_MasterDecide_Unlisted_Ask(capsys):
    assert _ask(_request("Bash", {"command": "ls -la"}), capsys)["decision"] == "ask"
    assert _ask(_request("WebFetch", {"url": "https://unlisted.example/x"}), capsys)["decision"] == "ask"
    inside = str(spec.master_folder(engine_home.root()) / "notes.txt")
    assert _ask(_request("Edit", {"file_path": inside}), capsys)["decision"] == "ask"
    assert _ask(_request("mcp__other__thing", envelope=False), capsys)["decision"] == "ask"


@pytest.mark.parametrize("raw", [b"not json", b"\xff\xfe", b"[]", b'{"method": "session/prompt"}', b'{"params": {}}',
                                 b'{"params": {"toolCall": {"rawInput": {}}}}',
                                 b'{"params": {"toolCall": {"name": "Bash", "rawInput": []}}}',
                                 b'{"params": {"toolCall": {"name": "Edit", "rawInput": {}}}}',
                                 b'{"params": {"toolCall": {"name": "Read", "rawInput": {"file_path": 3}}}}'])
def test_MasterDecide_Malformed_DenyWithReason(raw, capsys):
    answer = _ask(raw, capsys)
    assert answer["decision"] == "deny" and answer["reason"]


def test_MasterDecide_Oversized_Deny(capsys):
    padding = "x" * (decide.MAX_REQUEST_BYTES + 1)
    answer = _ask(_request("Bash", {"command": "ls", "note": padding}), capsys)
    assert answer["decision"] == "deny" and "larger" in answer["reason"]


def test_MasterDecide_CaseVariantOfDeniedPath_Deny(capsys, monkeypatch):
    monkeypatch.setattr(decide, "_case_insensitive", lambda path: True)
    variant = str(engine_home.root() / "State.JSON").upper()
    assert _ask(_request("Read", {"file_path": variant}), capsys)["decision"] == "deny"
    assert _ask(_request("Edit", {"file_path": str(spec.master_folder(engine_home.root()) / "INSTRUCTIONS.MD")}),
                capsys)["decision"] == "deny"


def test_MasterDecide_CaseSensitiveVolume_CaseIsKept(capsys, monkeypatch):
    monkeypatch.setattr(decide, "_case_insensitive", lambda path: False)
    variant = str(engine_home.root() / "state.json").upper()
    assert _ask(_request("Read", {"file_path": variant}), capsys)["decision"] == "ask"


@pytest.mark.parametrize("tool, key, pattern", [("Glob", "pattern", "/etc/*"), ("Glob", "pattern", "~/.ssh/*"),
                                                ("Glob", "pattern", "../../*"), ("Glob", "pattern", "a/../../b"),
                                                ("Grep", "glob", "/etc/**"), ("Grep", "glob", "~/*"),
                                                ("Grep", "glob", "x\\..\\y")])
def test_MasterDecide_GlobPatternEscapes_Deny(tool, key, pattern, capsys):
    answer = _ask(_request(tool, {key: pattern, "path": str(spec.master_folder(engine_home.root()))}), capsys)
    assert answer["decision"] == "deny" and "pattern" in answer["reason"]


def test_MasterDecide_GlobPatternInsideTheFolder_Ask(capsys):
    assert _ask(_request("Glob", {"pattern": "**/*.md"}), capsys)["decision"] == "ask"


@pytest.mark.parametrize("url", ["https://good.example@evil.example/", "https://good.example\\@evil.example/",
                                 "https://good.example\\evil.example/", "https://good.example/a b",
                                 "https://good.example/\x00", "https://good.example/\n", "ftp://good.example/",
                                 "file:///etc/passwd", "javascript:alert(1)", "//good.example/", ""])
def test_MasterDecide_UrlWithUserinfoOrBackslash_Deny(url, capsys):
    assert _ask(_request("WebFetch", {"url": url}), capsys)["decision"] == "deny"


def test_MasterDecide_NeverAnswersAllow(capsys):
    folder = spec.master_folder(engine_home.root())
    requests = [_request("mcp__sakura__lanes"), _request("mcp__sakura__waits"), _request("WebFetch", {"url": "https://x.example/"}),
                _request("Read", {"file_path": str(folder / "a.md")}), _request("Bash", {"command": "ls"}),
                _request("Bash", {"command": "git status"}), _request("Edit", {"file_path": str(folder / "a.md")}),
                _request("Glob", {"pattern": "*.md"}), _request("Agent")]
    decisions = {_ask(raw, capsys)["decision"] for raw in requests}
    assert decisions <= {"deny", "ask"} and "ask" in decisions
    assert not hasattr(decide, "ALLOW")


def test_MasterDecide_InsideRoleSession_Refused(monkeypatch, capsys):
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    assert cli.main(["decide", "--json"], stdin=io.BytesIO(_request("Read", {"file_path": "/x"}))) == exits.USAGE
    assert brakes.ENGINE_ROLE_REFUSAL in capsys.readouterr().out


def test_MasterDecide_NoSettingsWritten_DenyWithReason(capsys):
    spec.settings_path(engine_home.root()).unlink()
    answer = _ask(_request("Read", {"file_path": "/x"}), capsys)
    assert answer["decision"] == "deny" and "settings" in answer["reason"]


def test_master_bash_approval_requires_a_canonical_working_directory(capsys, tmp_path):
    folder = spec.master_folder(engine_home.root())
    alias = folder / "alias"
    alias.symlink_to(tmp_path, target_is_directory=True)
    answer = _ask(_request("Bash", {"command": "pwd", "cwd": str(alias)}), capsys)
    assert answer["decision"] == "deny"


def test_master_bash_approval_refuses_unrecognized_action_metadata(capsys):
    answer = _ask(_request("Bash", {"command": "pwd", "commandActions": [{}]}), capsys)
    assert answer["decision"] == "deny"

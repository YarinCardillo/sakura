"""Qualify installed executables and verify the negotiated ACP capabilities."""

import hashlib
import json
import sys

import pytest
from sakura import engine_home, programs
from sakura.flow import acp
from sakura.setup import harnesses

FAKE = """#!{python}
import json
import sys

assert sys.argv[1:] == ["acp"]
result = {result}
for line in sys.stdin:
    request = json.loads(line)
    assert request["method"] == "initialize"
    print(json.dumps({{"jsonrpc": "2.0", "id": request["id"], "result": result}}), flush=True)
"""


@pytest.fixture
def adapter(tmp_path, monkeypatch):
    folder = tmp_path / "install"
    folder.mkdir()
    monkeypatch.setenv("PATH", str(folder))
    monkeypatch.setattr(programs, "install_folders", list)
    adapter = folder / "fixture-adapter"
    (folder / "goose").symlink_to(adapter)
    return adapter


def install(adapter, *, version="1.60.2", protocol=1, agent="fixture", capabilities=None):
    result = {"protocolVersion": protocol, "agentInfo": {"name": agent, "version": version}}
    if capabilities is not None:
        result["agentCapabilities"] = capabilities
    adapter.write_text(FAKE.format(python=sys.executable, result=repr(result)))
    adapter.chmod(0o700)
    digest = hashlib.sha256(adapter.read_bytes()).hexdigest()
    path = engine_home.root() / programs.HOST_FILE
    path.write_text('[acp.goose]\nagent_name = "fixture"\n'
                    f'adapter = {{path = {json.dumps(str(adapter.resolve()))}, version = "1.60.2", sha256 = "{digest}"}}\n')
    return adapter


def test_missing_adapter_is_unavailable(adapter):
    with pytest.raises(programs.ProgramError, match="install_program"):
        harnesses.qualify("goose")


def test_same_name_without_install_pin_is_unverified(adapter):
    install(adapter)
    (engine_home.root() / programs.HOST_FILE).unlink()
    with pytest.raises(programs.ProgramError, match="qualification_required"):
        harnesses.qualify("goose")


def test_spoofed_same_path_refuses_before_spawn(adapter):
    install(adapter)
    with adapter.open("a") as script:
        script.write("\n")
    with pytest.raises(programs.ProgramError, match="incompatible_install"):
        harnesses.qualify("goose")


def test_executable_replacement_invalidates_qualification(adapter, tmp_path):
    install(adapter)
    chosen = harnesses.qualify("goose")
    replacement = adapter.with_name("replacement")
    replacement.write_bytes(adapter.read_bytes())
    replacement.chmod(0o700)
    replacement.replace(adapter)
    with pytest.raises(programs.ProgramError, match="program_replaced"), chosen.connect(tmp_path):
        pytest.fail("replacement was started")


def test_workspace_path_decoy_cannot_replace_host_binary(adapter, monkeypatch, repo):
    install(adapter)
    decoy = repo / "goose"
    decoy.write_text("decoy")
    decoy.chmod(0o700)
    monkeypatch.setenv("PATH", f"{repo}:{adapter.parent}")
    assert harnesses.qualify("goose", repo).argv == (str(adapter.resolve()), "acp")


@pytest.mark.parametrize(("protocol", "version", "agent", "reason"), [
    (2, "1.60.2", "fixture", "wrong_protocol_version"),
    (True, "1.60.2", "fixture", "wrong_protocol_version"),
    (1, "1.60.1", "fixture", "wrong_agent_identity"),
    (1, "1.60.2", "lookalike", "wrong_agent_identity"),
])
def test_initialize_refuses_wrong_protocol_or_identity(adapter, tmp_path, protocol, version, agent, reason):
    install(adapter, protocol=protocol, version=version, agent=agent)
    with pytest.raises(acp.AcpError, match=reason), harnesses.qualify("goose").connect(tmp_path):
        pytest.fail("wrong peer accepted")


@pytest.mark.parametrize(("advertised", "expected"), [
    (None, (False, False, False, False)),
    ({"loadSession": True, "mcpCapabilities": {"http": True},
      "sessionCapabilities": {"close": {}}}, (True, True, False, True)),
])
def test_optional_methods_are_enabled_only_when_advertised(adapter, tmp_path, advertised, expected):
    install(adapter, capabilities=advertised)
    with harnesses.qualify("goose").connect(tmp_path) as (_, caps):
        assert (caps.load_session, caps.mcp_http, caps.mcp_sse, caps.close_session) == expected

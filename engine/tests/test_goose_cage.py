"""Goose model and worker boundary tests."""

import json
from pathlib import Path

import pytest
from sakura import mcp_stdio
from sakura.flow import cage, goose_cage


def test_model_profile_carries_only_its_provider_authority(tmp_path):
    root = tmp_path.resolve()
    adapter = root / "adapter-fixture"
    home = root / "fixture-home"
    cloud = goose_cage.PROVIDERS["openai"]
    local = goose_cage.PROVIDERS["ollama"]
    model = root / "model"
    profile = goose_cage.model_profile(adapter, home, model, (31231, 31232), cloud)
    assert '(deny network*)' in profile
    assert '(remote ip "localhost:31231")' in profile
    assert '(remote ip "localhost:31232")' in profile
    assert "11434" not in profile
    assert '(deny mach-lookup)' in profile
    assert 'com.apple.securityd' in profile
    assert 'com.apple.securityd' not in goose_cage.model_profile(adapter, home, model, (31231, 31232), local)
    assert str(home / "Library/Keychains") in profile
    assert "(deny process-fork)" in profile
    assert "(deny process-exec)" in profile
    assert r"secrets\.yaml" in profile
    assert str(root / "other-run") not in profile


@pytest.mark.parametrize("role,can_write", [("builder", True), ("code_reviewer", False), ("master", False)])
def test_worker_profile_keeps_git_protection_and_credential_denials(tmp_path, monkeypatch, role, can_write):
    root = tmp_path.resolve()
    workspace, common, temporary = (root / name for name in ("workspace", "common", "tmp"))
    for path in (workspace, common, temporary):
        path.mkdir()
    site = cage.RunSite(root / "task", "build-1", role, workspace, common, (root / "engine",), None,
                        tmp_dir=temporary, loopback_ports=(31231,))
    monkeypatch.setattr(cage, "_account_folders", lambda home: [])
    caged = cage.RunCage(root, root / "settings", root / "mcp", root / "policy", root / "requests",
                         temporary, (), root / "sandbox.sb")
    profile = goose_cage.worker_profile(site, cage.Policy("default"), caged, root / "fixture-home",
                                        root / "model", (31231, 31232))
    writable = next(line for line in profile.splitlines() if "(deny file-write* (require-not" in line)
    assert (str(workspace) in writable) == can_write
    assert str(common / "config") in profile
    assert 'com.apple.securityd' in profile
    assert str(root / "model") in profile
    assert '(deny network-outbound (remote ip "localhost:31231"))' in profile


def test_provider_route_refuses_unapproved_destinations_and_redirects(monkeypatch):
    class Upstream:
        def __init__(self, host, port, timeout):
            assert (host, port) == ("api.openai.com", 443)

        def request(self, method, path, body, headers):
            assert (method, path, body) == ("POST", "/v1/chat/completions", b"{}")
            assert headers == {"Content-Type": "application/json", "Authorization": "Bearer fixture"}

        def getresponse(self):
            return self

        status = 307

        def close(self):
            pass

    monkeypatch.setattr(goose_cage.http.client, "HTTPSConnection", Upstream)
    provider = goose_cage.PROVIDERS["openai"]
    assert not provider.permits("POST", "http://127.0.0.1:9999/v1/chat/completions")
    assert not provider.permits("POST", "/v1/chat/completions?url=http://localhost")
    assert not provider.permits("GET", "/v1/chat/completions")
    with pytest.raises(goose_cage.ChannelError, match="provider_redirect_refused"):
        provider.open("POST", "/v1/chat/completions", b"{}", "Bearer fixture")


def test_tool_channel_requires_its_bearer_and_revokes_it():
    tool = mcp_stdio.Tool({"name": "probe"}, lambda args: ("granted", False))
    channel = goose_cage.ToolChannel([tool])
    message = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "probe"}}).encode()
    with pytest.raises(goose_cage.ChannelError, match="tool_channel_forbidden"):
        channel.answer("Bearer wrong", message)
    answer = channel.answer(channel.authorization, message)
    assert answer["result"]["content"][0]["text"] == "granted"
    channel.revoke()
    with pytest.raises(goose_cage.ChannelError, match="tool_channel_forbidden"):
        channel.answer(channel.authorization, message)


def test_existing_model_storage_requires_its_captured_identity(tmp_path):
    parent = tmp_path.resolve()
    folder, identity = goose_cage.model_storage(parent, "build-1")
    assert goose_cage.model_storage(parent, "build-1", identity) == (folder, identity)
    with pytest.raises(cage.CageError, match="session_storage_changed"):
        goose_cage.model_storage(parent, "build-1")
    folder.rename(parent / "old")
    folder.mkdir(mode=0o700)
    with pytest.raises(cage.CageError, match="session_storage_changed"):
        goose_cage.model_storage(parent, "build-1", identity)
    folder.rmdir()
    folder.symlink_to(parent / "old", target_is_directory=True)
    with pytest.raises(cage.CageError, match="session_storage_changed"):
        goose_cage.model_storage(parent, "build-1", identity)


def test_loopback_mcp_listener_requires_run_bearer():
    import http.client
    import threading

    tool = mcp_stdio.Tool({"name": "fixture_tool"}, lambda args: ("ok", False))
    channel = goose_cage.ToolChannel([tool])
    dispatch = lambda request: goose_cage._serve_tools(channel, request)
    with goose_cage._Server(goose_cage._Handler, dispatch) as server:
        revoked = threading.Event()
        goose_cage._start_listener(server, revoked)
        try:
            body = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
            for bearer, status in (("wrong", 403), (channel.authorization, 200)):
                connection = http.client.HTTPConnection(goose_cage.LOCAL, server.server_port, timeout=2)
                try:
                    connection.request("POST", "/mcp", body, {"Authorization": bearer})
                    assert connection.getresponse().status == status
                finally:
                    connection.close()
        finally:
            channel.revoke()
            revoked.set()
            server.revoke()


def test_model_seatbelt_enforces_files_and_process_creation(tmp_path):
    import subprocess

    root = tmp_path.resolve()
    model, _ = goose_cage.model_storage(root, "build-1")
    outside = root / "other-run"
    outside.write_text("fixture")
    (model / "config" / "sentinel").write_text("fixture")
    system = Path("/usr/bin")
    versioned = sorted(path for path in system.glob("perl5.*") if path.name.rpartition(".")[2].isdigit())
    adapter = versioned[-1] if versioned else system / "perl"
    probe = """
        if (open(my $outside, '<', $ARGV[0])) { exit 1; }
        open(my $inside, '<', $ARGV[1]) or exit 2;
        my $child = fork();
        exit 0 unless defined $child;
        exit 3 if $child == 0;
        waitpid($child, 0);
        exit 3;
    """
    profile = root / "model.sb"
    profile.write_text(goose_cage.model_profile(adapter, root / "fixture-home", model, (31231,),
                                                goose_cage.PROVIDERS["ollama"]))
    subprocess.run([cage.SEATBELT, "-f", str(profile), str(adapter), "-e", probe,
                    str(outside), str(model / "config" / "sentinel")],
                   check=True, timeout=10, env={})


def test_generated_config_has_no_project_extensions_or_plaintext_credentials(tmp_path):
    model, _ = goose_cage.model_storage(tmp_path.resolve(), "build-1")
    goose_cage._write_config(model, "ollama", "qwen2.5:3b")
    config = model / "config" / "config.yaml"
    assert json.loads(config.read_text()) == {
        "GOOSE_PROVIDER": "ollama", "GOOSE_MODEL": "qwen2.5:3b", "GOOSE_MODE": "approve",
        "GOOSE_TELEMETRY_ENABLED": False, "extensions": {}, "experiments": {},
    }
    assert config.stat().st_mode & 0o777 == 0o600


def test_missing_confinement_proof_refuses_the_provider():
    from types import SimpleNamespace

    pinned = SimpleNamespace(lane_models=(), setup_safe=True, runtime=None)
    with pytest.raises(cage.CageError, match="goose_confinement_proof_required"):
        goose_cage.qualify(pinned, "ollama", "fixture")
    with pytest.raises(cage.CageError, match="unsupported_goose_provider"):
        goose_cage.qualify(pinned, "unqualified", "fixture")

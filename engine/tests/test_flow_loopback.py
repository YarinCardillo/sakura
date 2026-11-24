"""Caged commands reach loopback only on the registered application port and the host's approved dependent ports."""

import json
import os
import socket
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace

import pytest

from conftest import git
from flow_support import make_context
from sakura.flow import cage, checks, config, ports, runner, workspaces

CANARY_PORTS = range(40100, 40400)
NATIVE_LOOPBACK = os.environ.get("SAKURA_TEST_SEATBELT") == "1"
APP_PORT = 40100
CANARY_PORT = 40110
DEPENDENCY_PORT = 40120
CONNECT = ("import socket, sys\n"
           "try:\n"
           "    socket.create_connection(('127.0.0.1', int(sys.argv[1])), timeout=2).close()\n"
           "except OSError as error:\n"
           "    print('refused', type(error).__name__)\n"
           "    sys.exit(3)\n"
           "print('connected')\n")


@contextmanager
def _listener(port=APP_PORT, family=socket.AF_INET):
    if not NATIVE_LOOPBACK:
        yield port
        return
    with socket.socket(family) as listener:
        listener.bind(("::1" if family == socket.AF_INET6 else "127.0.0.1", 0))
        listener.listen(8)
        yield listener.getsockname()[1]


@contextmanager
def _canary():
    """A server on a fixed port below the ephemeral range, where a database or a dev server would listen."""
    if not NATIVE_LOOPBACK:
        yield CANARY_PORT
        return
    for port in CANARY_PORTS:
        server = socket.socket()
        try:
            server.bind(("127.0.0.1", port))
        except OSError:
            server.close()
            continue
        server.listen(8)
        try:
            yield port
        finally:
            server.close()
        return
    raise RuntimeError("no free canary port")


def _allows_connect(profile, port):
    filters = (f'(remote ip "localhost:{port}")', '(remote ip "localhost:*")')
    return any(any(address in rule for address in filters)
               for rule in profile.splitlines() if rule.startswith("(allow network-outbound "))


def _socket_probe(run_cage, cwd, script, target=None, *, own_sockets=False):
    if NATIVE_LOOPBACK:
        argv = [sys.executable, "-c", script, *([str(target)] if target is not None else [])]
        done = subprocess.run(run_cage.wrap(argv), cwd=cwd, capture_output=True, text=True, timeout=60, check=False)
        return done.returncode, done.stdout
    from sakura.flow import loopback

    profile = run_cage.seatbelt.read_text()
    assert "(deny network*)" in profile
    output = ""
    if own_sockets:
        first, _ = loopback.ephemeral_range()
        if '(allow network-bind (local ip "localhost:*"))' not in profile or not _allows_connect(profile, first):
            return 1, "bind refused\n"
        output = "own sockets work\n"
    if target is None:
        return 0, output
    if _allows_connect(profile, target):
        return 0, output + "connected\n"
    return 3, output + "refused PermissionError\n"


def _connect_from_the_cage(repo, tmp_path, ports, target, task="demo"):
    ctx = make_context(repo, own_repository=True, task=task)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "loopback", cage.load_policy("default"), ports)
    return _socket_probe(run_cage, tmp_path, CONNECT, target)


def test_Check_RegisteredApplicationPort_IsReachable(repo, tmp_path):
    with _listener() as app:
        assert _connect_from_the_cage(repo, tmp_path, (app,), app) == (0, "connected\n")


def test_Check_CanaryServerOnAnotherLocalPort_IsNotReachable(repo, tmp_path):
    with _listener() as app, _canary() as canary:
        code, output = _connect_from_the_cage(repo, tmp_path, (app,), canary)
    assert code == 3 and output.startswith("refused")


def test_Check_ApprovedDependentPort_IsReachableAndOthersStayClosed(repo, tmp_path):
    with _listener() as app, _listener(DEPENDENCY_PORT) as dependency, _canary() as canary:
        assert _connect_from_the_cage(repo, tmp_path, (app, dependency), dependency, "one")[0] == 0
        assert _connect_from_the_cage(repo, tmp_path, (app, dependency), canary, "two")[0] == 3


def test_Check_WithNoRegisteredPort_ReachesNoLoopbackPort(repo, tmp_path):
    with _canary() as canary:
        assert _connect_from_the_cage(repo, tmp_path, (), canary)[0] == 3


def test_SeatbeltProfile_NamesEachAllowedPortAndNeverAllLoopbackPorts(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "profile", cage.load_policy("default"), (31000, 5432))
    profile = run_cage.seatbelt.read_text()
    assert 'network-outbound (remote ip "localhost:31000")' in profile
    assert 'network-outbound (remote ip "localhost:5432")' in profile
    assert 'remote ip "localhost:*"' not in profile


def test_ChecksLoopbackPorts_AreAHostSetting(repo):
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text("[checks]\nloopback_ports = [5432]\n")
    with pytest.raises(config.FlowConfigError, match="checks"):
        config.load(repo)
    (repo / ".sakura" / "flow.toml").unlink()
    assert config.load(repo, overrides={"checks": {"loopback_ports": [5432]}}).values["checks"]["loopback_ports"] == [5432]


@pytest.mark.parametrize("ports", [[0], [70000], ["5432"], [True], 5432])
def test_ChecksLoopbackPorts_MustBePortNumbers(repo, ports):
    with pytest.raises(config.FlowConfigError, match="loopback_ports"):
        config.load(repo, overrides={"checks": {"loopback_ports": ports}})


def test_RoleCage_GetsTheApplicationPortAndTheApprovedOnes(repo):
    ctx = make_context(repo, own_repository=True, overrides={"checks": {"loopback_ports": [5432]}})
    agents = runner.AgentRunner(ctx.store, ctx.config)
    request = runner.RunRequest("evaluator", "evaluate", "x", {}, repo, {"SAKURA_FLOW_BASE_URL": "http://127.0.0.1:31000"})
    assert agents._loopback_ports(ctx.config, request) == (31000, 5432)
    assert agents._loopback_ports(ctx.config, runner.RunRequest("builder", "build", "x", {}, repo)) == (5432,)


# -- review 4: the application port is the engine's, and every cage names exact loopback ports -------------------


def _occupy_port(monkeypatch, family, port):
    if NATIVE_LOOPBACK:
        return

    @contextmanager
    def probe(address_family, _kind):
        def bind(address):
            if address_family == family and address[1] == port:
                raise OSError("address in use")
        yield SimpleNamespace(bind=bind)

    monkeypatch.setattr(ports, "socket", SimpleNamespace(socket=probe, SOCK_STREAM=socket.SOCK_STREAM))


def test_AppOrigin_ARepositoryLayerNamingIPv6_IsRefused(repo):
    with _listener() as port:
        (repo / ".sakura").mkdir()
        (repo / ".sakura" / "flow.toml").write_text(f'[app]\norigin = "http://[::1]:{port}"\n')
        with pytest.raises(config.FlowConfigError, match="app.origin may only be set by the host"):
            config.load(repo)


def test_AppOrigin_AHostIPv6Origin_CannotTakeAPortAnIPv4ServiceHolds(repo, tmp_path, monkeypatch):
    with _listener() as port:
        _occupy_port(monkeypatch, socket.AF_INET, port)
        ctx = make_context(repo, own_repository=True, overrides={"builders": {"max": 1}, "app": {"origin": f"http://[::1]:{port}"},
                                            "workspace": {"root": str(tmp_path / "trees")}})
        with pytest.raises(ports.PortsExhausted):
            workspaces.create(ctx, "0", "feat/flow-demo", git(repo, "rev-parse", "HEAD"))
        assert ports.owners() == {}


def test_PortAllocation_AnIPv6ServiceOnTheBlock_IsSkipped(monkeypatch):
    with _listener(family=socket.AF_INET6) as port:
        _occupy_port(monkeypatch, socket.AF_INET6, port)
        with pytest.raises(ports.PortsExhausted):
            ports.allocate("repo/t/0", preferred_start=port)


def test_ClaudeSettings_ContainTheExactLoopbackPortRules(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    site = cage.RunSite(ctx.store.root, "evaluate-1", "evaluator", repo, repo / ".git", (tmp_path / "engine",), repo,
                        loopback_ports=(31000, 5432))
    network = json.loads(cage.prepare(site, cage.Policy("default"), 0, Path("/home/someone")).settings.read_text()
                         )["sandbox"]["network"]
    for port in (31000, 5432):
        assert {f"127.0.0.1:{port}", f"localhost:{port}", f"[::1]:{port}"} <= set(network["allowedDomains"])
    loopback = [entry for entry in network["allowedDomains"] if entry.startswith(("127.", "localhost", "[::1]"))]
    assert all(entry.rsplit(":", 1)[1] in ("31000", "5432") for entry in loopback), loopback
    assert network["allowLocalBinding"] is False, "it would open every loopback port to Claude's sandbox"
    plain = json.loads(cage.prepare(cage.RunSite(ctx.store.root, "build-1", "builder", repo, repo / ".git",
                                                 (tmp_path / "engine",), repo), cage.Policy("default"), 0,
                                    Path("/home/someone")).settings.read_text())["sandbox"]["network"]
    assert not [entry for entry in plain["allowedDomains"] if entry.startswith(("127.", "localhost", "[::1]"))]


# -- loopback for lanes: a suite that opens its own sockets, the rest of loopback closed ------------------------------

OWN_SOCKETS = ("import socket\n"
               "server = socket.socket(); server.bind(('127.0.0.1', 0)); server.listen(1)\n"
               "socket.create_connection(server.getsockname(), timeout=2).close()\n"
               "print('own sockets work')\n")


def test_Check_ASuiteOpeningItsOwnEphemeralSockets_WorksWithTheLocalSocketsApproval(repo, tmp_path, real_ephemeral_range):
    ctx = make_context(repo, own_repository=True)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "sockets", cage.load_policy("default"), (), local_sockets=True)

    assert _socket_probe(run_cage, tmp_path, OWN_SOCKETS, own_sockets=True) == (0, "own sockets work\n")


def test_Check_OwnEphemeralSocketsWork_WhileACanaryOnAFixedPortStaysClosed(repo, tmp_path, real_ephemeral_range):
    ctx = make_context(repo, own_repository=True)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "both", cage.load_policy("default"), (), local_sockets=True)
    script = OWN_SOCKETS + CONNECT

    with _canary() as canary:
        code, output = _socket_probe(run_cage, tmp_path, script, canary, own_sockets=True)
    assert code == 3 and output.startswith("own sockets work\nrefused"), output


def test_PortRules_NameEachAllowedPortAndNothingElse():
    from sakura.flow import loopback

    rules = "\n".join(loopback.port_rules((31000, 31000, 5432)))

    assert rules.count('(remote ip "localhost:31000")') == 1 and '(remote ip "localhost:5432")' in rules
    assert '(allow network-bind (local ip "localhost:5432"))' in rules
    assert '(allow network-inbound (local ip "localhost:31000"))' in rules
    assert "localhost:*" not in rules and loopback.port_rules(()) == []


def test_SocketRules_OpenBindingAndEveryEphemeralPortAndNothingElse(real_ephemeral_range):
    from sakura.flow import loopback

    rules = "\n".join(loopback.socket_rules())
    first, last = loopback.ephemeral_range()

    assert '(allow network-bind (local ip "localhost:*"))' in rules
    assert f'"localhost:{first}"' in rules and f'"localhost:{last}"' in rules
    assert f'"localhost:{first - 1}"' not in rules and 'remote ip "localhost:*"' not in rules
    assert rules.count("(remote ip") == last - first + 1


def test_EphemeralRange_ComesFromTheSystemAndFallsBackToTheIanaRange(monkeypatch, real_ephemeral_range):
    from sakura.flow import loopback

    loopback._system_range.cache_clear()
    first, last = loopback.ephemeral_range()
    assert 1023 < first <= last <= 65535
    monkeypatch.setattr(loopback, "SYSCTL", "/usr/bin/false")
    loopback._system_range.cache_clear()
    assert loopback.ephemeral_range() == loopback.DEFAULT_EPHEMERAL_RANGE
    loopback._system_range.cache_clear()


@pytest.mark.parametrize("value,expected", [("50000-50010", (50000, 50010)), ("50010-50000", None), ("a-b", None),
                                            ("0-5", None), ("40000-70000", None), ("", None)])
def test_EphemeralRange_AnEnvironmentOverrideNarrowsItAndMalformedOnesAreIgnored(monkeypatch, value, expected):
    from sakura.flow import loopback

    monkeypatch.setenv("SAKURA_EPHEMERAL_PORT_RANGE", value)

    assert loopback.ephemeral_range() == (expected or loopback._system_range())


def test_RoleCage_AlsoGetsThePortBlockTheEngineAllocatedToTheWorkspace(repo):
    ctx = make_context(repo, own_repository=True, overrides={"checks": {"loopback_ports": [5432]}})
    agents = runner.AgentRunner(ctx.store, ctx.config)
    request = runner.RunRequest("builder", "build", "x", {}, repo, {"SAKURA_FLOW_PORT": "41010", "SAKURA_FLOW_PORTS": "41010-41012"})

    assert agents._loopback_ports(ctx.config, request) == (5432, 41010, 41011, 41012)


@pytest.mark.parametrize("text", ["", "41010", "41012-41010", "0-5", "a-b", "41010-70000"])
def test_RoleCage_IgnoresAMalformedPortBlock(repo, text):
    ctx = make_context(repo, own_repository=True)
    agents = runner.AgentRunner(ctx.store, ctx.config)
    request = runner.RunRequest("builder", "build", "x", {}, repo, {"SAKURA_FLOW_PORTS": text})

    assert agents._loopback_ports(ctx.config, request) == ()


# -- review 6: the ephemeral range is the person's decision, per project ----------------------------------------------


@contextmanager
def _ephemeral_listener():
    """A service on a port the system picked, where a debugger, a dev server or a database client could listen."""
    from sakura.flow import loopback

    with _listener(loopback.ephemeral_range()[0]) as port:
        yield port


def _sockets_probe(repo, tmp_path, local_sockets, target):
    ctx = make_context(repo, own_repository=True)
    run_cage = checks.checks_cage(ctx.store, tmp_path, "sockets", cage.load_policy("default"), (), local_sockets=local_sockets)
    return _socket_probe(run_cage, tmp_path, OWN_SOCKETS + CONNECT, target, own_sockets=True)


def test_Check_WithoutTheLocalSocketsApproval_AnEphemeralCanaryAndTheSuitesOwnSocketsAreRefused(repo, tmp_path, real_ephemeral_range):
    with _ephemeral_listener() as canary:
        code, output = _sockets_probe(repo, tmp_path, False, canary)
    assert code != 0 and "own sockets work" not in output, output
    with _ephemeral_listener() as canary:
        assert _connect_from_the_cage(repo, tmp_path, (), canary, "two")[0] == 3


def test_Check_WithTheLocalSocketsApproval_TheSuiteRunsAndTheEphemeralCanaryIsReachable(repo, tmp_path, real_ephemeral_range):
    with _ephemeral_listener() as canary:
        code, output = _sockets_probe(repo, tmp_path, True, canary)
    assert code == 0 and output == "own sockets work\nconnected\n", output


def test_SeatbeltProfile_WithoutTheApproval_OpensNoEphemeralPortAndNoWildcardBind(repo, tmp_path, real_ephemeral_range):
    from sakura.flow import loopback

    ctx = make_context(repo, own_repository=True)
    plain = checks.checks_cage(ctx.store, tmp_path, "plain", cage.load_policy("default"), (31000,)).seatbelt.read_text()
    broad = checks.checks_cage(ctx.store, tmp_path, "broad", cage.load_policy("default"), (31000,),
                               local_sockets=True).seatbelt.read_text()
    first, _ = loopback.ephemeral_range()
    assert f'"localhost:{first}"' not in plain and 'local ip "localhost:*"' not in plain
    assert '(allow network-bind (local ip "localhost:31000"))' in plain
    assert f'"localhost:{first}"' in broad and '(allow network-bind (local ip "localhost:*"))' in broad


def test_ApplicationPort_StillBindsAndServesInsideTheCageWithoutTheApproval(repo, tmp_path):
    ctx = make_context(repo, own_repository=True)
    port = APP_PORT
    run_cage = checks.checks_cage(ctx.store, tmp_path, "app", cage.load_policy("default"), (port,))
    script = (f"import socket\nserver = socket.socket(); server.bind(('127.0.0.1', {port})); server.listen(1)\n"
              f"socket.create_connection(('127.0.0.1', {port}), timeout=2).close()\nprint('served')\n")
    if NATIVE_LOOPBACK:
        assert _socket_probe(run_cage, tmp_path, script) == (0, "served\n")
    else:
        profile = run_cage.seatbelt.read_text()
        assert f'(allow network-bind (local ip "localhost:{port}"))' in profile
        assert f'(allow network-inbound (local ip "localhost:{port}"))' in profile
        assert _allows_connect(profile, port)


def test_ChecksLocalSockets_AreAHostSettingAndDefaultOff(repo):
    assert config.load(repo).values["checks"]["local_sockets"] is False
    (repo / ".sakura").mkdir()
    (repo / ".sakura" / "flow.toml").write_text("[checks]\nlocal_sockets = true\n")
    with pytest.raises(config.FlowConfigError, match="checks"):
        config.load(repo)
    (repo / ".sakura" / "flow.toml").unlink()
    assert config.load(repo, overrides={"checks": {"local_sockets": True}}).values["checks"]["local_sockets"] is True
    with pytest.raises(config.FlowConfigError, match="local_sockets"):
        config.load(repo, overrides={"checks": {"local_sockets": "yes"}})

import json
import os
import re
import subprocess
import threading
from pathlib import Path

import pytest
import test_cockpit_server as http
from cockpit_support import (
    OWN_ADDRESS,
    OWN_DNS_NAME,
    OWN_NODE_ID,
    settings_text,
    write_tailscale,
)
from conftest import trust_root_child_environment
from sakura import engine_home
from sakura.cockpit import actions, api, cli, gate, host_actions, proofs, snapshot
from sakura.cockpit.controller import Controller
from sakura.flow import brakes, exits, inbox, registry, resume, store, waits
from test_cockpit_actions import Spawns, flow_item, lane_item, log_rows
from test_cockpit_api import Engine
from test_cockpit_snapshot import NoTmux, create_task
from tmux_support import needs_tmux, private_tmux, wait_until

OWNER = gate.Identity("owner@example.com", "nMAC", "mac")


@pytest.fixture
def cockpit_toml(tmp_path):
    """A trusted cockpit.toml under the test's HOME, with a fake tailscale that never touches the network."""
    helper_dir = tmp_path / "bin"
    helper_dir.mkdir()
    helper = write_tailscale(helper_dir)
    path = tmp_path / "isolated-home" / ".config" / "sakura" / "cockpit.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(settings_text(helper))
    return path


def personal_flow_config(tmp_path, text):
    path = engine_home.config_file()
    path.write_text(text)


# -- --once -------------------------------------------------------------------------------------

def test_once_prints_the_registered_repositories_as_text(repo, capsys):
    create_task(repo, "demo", status="awaiting_approval", branch="feat/x\x1b]2;evil\x07")
    registry.remember(repo)

    assert cli.main(["--once"], client=NoTmux()) == exits.OK

    out = capsys.readouterr().out
    assert "demo" in out and "[gate]" in out and "\x1b" not in out and "\x07" not in out


def test_once_without_registered_repositories_still_prints_the_header(capsys):
    assert cli.main(["--once"], client=NoTmux()) == exits.OK
    assert "sakura cockpit" in capsys.readouterr().out


def test_once_ignores_the_shelved_lane_config_even_when_broken(repo, tmp_path, capsys):
    lanes = tmp_path / "isolated-home" / ".config" / "sakura" / "lanes.toml"
    lanes.parent.mkdir(parents=True, exist_ok=True)
    lanes.write_text("[broken\n")
    assert cli.main(["--once"], client=NoTmux()) == exits.OK
    assert "lane config" not in capsys.readouterr().out


def test_exactly_one_of_serve_once_and_open_is_required():
    for argv in ([], ["serve", "--once"], ["--once", "--open"]):
        with pytest.raises(SystemExit) as stop:
            cli.main(argv, client=NoTmux())
        assert stop.value.code == exits.USAGE


def _phone_target(page):
    return re.search(r'name="target" value="(t1\.[a-f0-9]+)"', page).group(1)


# -- serve --------------------------------------------------------------------------------------

def test_serve_feeds_registered_repositories_to_the_settings_and_wires_the_pages(repo, cockpit_toml):
    create_task(repo, "demo", status="awaiting_approval")
    registry.remember(repo)
    seen = []

    code = cli.main(["serve"], client=NoTmux(), serve=lambda *args, **_api: seen.append(args))

    assert code == exits.OK
    ((settings, index_page, action_runner),) = seen
    assert settings.allow_logins == {"owner@example.com"}
    page = index_page(OWNER, "csrf-token")
    assert "demo" in page and "csrf-token" in page
    assert "refused" in action_runner({"key": "x", "target": _phone_target(page)}, OWNER, "csrf-token")


def test_serve_refuses_a_tailscale_helper_inside_a_registered_repository(repo, tmp_path, capsys):
    helper = write_tailscale(repo)
    path = tmp_path / "isolated-home" / ".config" / "sakura" / "cockpit.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(settings_text(helper))
    registry.remember(repo)
    seen = []

    assert cli.main(["serve"], client=NoTmux(), serve=lambda *args, **_api: seen.append(args)) == exits.USAGE

    assert seen == [] and "registered repository" in capsys.readouterr().err


def test_serve_without_a_cockpit_file_is_a_usage_error(capsys):
    assert cli.main(["serve"], client=NoTmux(), serve=lambda *args: pytest.fail("served")) == exits.USAGE
    assert "cockpit.toml" in capsys.readouterr().err


def test_serve_with_a_damaged_proof_secret_prints_a_failed_line_not_a_traceback(repo, cockpit_toml, monkeypatch, capsys):
    def damaged():
        raise proofs.ProofStoreError("proof secret /home/x/proof-secret is damaged; delete it to create a new one")

    monkeypatch.setattr(proofs, "load_secret", damaged)
    code = cli.main(["serve"], client=NoTmux(), serve=lambda *args, **_api: pytest.fail("served"))
    assert code == exits.FAILED and "proof secret" in capsys.readouterr().err


def test_serve_wires_the_json_api_over_the_same_controller(repo, cockpit_toml):
    create_task(repo, "demo", status="running")
    registry.remember(repo)
    wired = {}

    assert cli.main(["serve"], client=NoTmux(), serve=lambda *args, **kwargs: wired.update(kwargs)) == exits.OK

    document = wired["api"].snapshot()
    assert document["api"] == 2 and len(document["items"]) == 1
    assert document["items"][0]["id"].startswith("t1.") and "key" not in document["items"][0]


def test_serve_signs_confirmations_with_the_persisted_secret_and_stores_used_nonces_on_disk(repo, cockpit_toml, monkeypatch):
    for name, value in trust_root_child_environment().items():
        monkeypatch.setenv(name, value)
    create_task(repo, "demo", status="running")
    registry.remember(repo)
    routes = []
    for _ in range(2):
        cli.main(["serve"], client=NoTmux(), serve=lambda *args, **kwargs: routes.append((args[2], kwargs["api"])))
    first, second = (route.snapshot()["items"][0]["id"] for _, route in routes)
    assert first == second
    asked = routes[0][0]({"key": "s", "target": first}, OWNER, "t")
    proof = re.search(r'name="proof" value="([^"]+)"', asked).group(1)

    assert "exit code" in routes[1][0]({"key": "s", "target": second, "proof": proof}, OWNER, "t")
    assert proofs.consumed_count() == 1


def test_serve_pages_use_one_snapshot_for_a_few_seconds_but_actions_a_fresh_one(repo, cockpit_toml, monkeypatch):
    registry.remember(repo)
    reads = []
    real = snapshot.collect
    monkeypatch.setattr(snapshot, "collect", lambda *args, **kwargs: reads.append(1) or real(*args, **kwargs))
    seen = []
    cli.main(["serve"], client=NoTmux(), serve=lambda *args, **_api: seen.append(args))
    ((_, index_page, action_runner),) = seen

    target = _phone_target(index_page(OWNER, "t"))
    index_page(OWNER, "t")
    assert len(reads) == 1
    action_runner({"key": "x", "target": target}, OWNER, "t")
    assert len(reads) == 2


# -- the snapshot cache -------------------------------------------------------------------------

class Ticker:
    def __init__(self):
        self.now = 100.0

    def __call__(self):
        return self.now


def test_cached_snapshot_reads_once_per_period_and_never_caches_a_failure():
    clock, calls = Ticker(), []

    def source():
        calls.append(clock.now)
        if len(calls) == 2:
            raise RuntimeError("gh is down")
        return object()

    cached = snapshot.CachedSnapshot(source, 5, clock)
    first = cached()
    assert cached() is first and len(calls) == 1
    clock.now += 5
    with pytest.raises(RuntimeError):
        cached()
    assert cached() is not first and len(calls) == 3


def test_cached_snapshot_lets_one_reader_refresh_while_the_others_wait():
    reading = threading.Event()
    release = threading.Event()
    contended = threading.Semaphore(0)
    calls = []
    lock = threading.Lock()

    class ObservedLock:
        def __enter__(self):
            if not lock.acquire(blocking=False):
                contended.release()
                lock.acquire()

        def __exit__(self, *exc):
            lock.release()

    def source():
        calls.append(1)
        reading.set()
        assert release.wait(15)
        return object()

    cached = snapshot.CachedSnapshot(source, 5, Ticker())
    cached._lock = ObservedLock()
    from concurrent.futures import ThreadPoolExecutor

    with ThreadPoolExecutor(max_workers=6) as workers:
        futures = [workers.submit(cached)]
        try:
            assert reading.wait(10)
            futures.extend(workers.submit(cached) for _ in range(5))
            for _ in range(5):
                assert contended.acquire(timeout=10)
            assert calls == [1]
        finally:
            release.set()
        results = [future.result(timeout=10) for future in futures]
    assert len(calls) == 1 and len({id(value) for value in results}) == 1 and len(results) == 6


# -- --open -------------------------------------------------------------------------------------

@pytest.fixture
def private(cockpit_toml):
    with private_tmux() as client:
        yield client


def open_cockpit(client, capsys, argv=("sleep", "60")):
    code = cli.main(["--open"], client=client, window_argv=list(argv))
    return code, capsys.readouterr()


@needs_tmux
def test_open_runs_serve_in_window_cockpit_of_the_cockpit_session_and_prints_the_url(private, capsys):
    code, output = open_cockpit(private, capsys)

    assert code == exits.OK
    assert output.out.strip() == f"http://{OWN_DNS_NAME}:8787/"
    assert [window.name for window in private.windows("flow")] == ["cockpit"]


@needs_tmux
def test_open_twice_keeps_one_live_window(private, capsys):
    open_cockpit(private, capsys)
    code, output = open_cockpit(private, capsys)

    assert code == exits.OK and output.out.strip().startswith("http://")
    assert [window.name for window in private.windows("flow")] == ["cockpit"]


@needs_tmux
def test_open_replaces_a_window_whose_server_has_ended(private, capsys, monkeypatch):
    monkeypatch.setattr(gate, "read_self", lambda _: gate.Self(OWN_ADDRESS, OWN_DNS_NAME, OWN_NODE_ID))
    home = Path.home()
    private.ensure_session("flow", home, "keep", ["/bin/cat"], {}, cli.SESSION_MARKER)
    ended = private.new_window("flow", "cockpit", ["/bin/sh", "-c", "read -r line"], home, {}, remain=True)
    private.set_option(ended, cli.WINDOW_MARKER_KEY, "1")
    pane, = next(window.panes for window in private.windows("flow") if window.id == ended)
    private.send_enter(pane.id)
    assert wait_until(lambda: any(w.id == ended and w.dead for w in private.windows("flow")))

    code, output = open_cockpit(private, capsys, argv=("/bin/cat",))

    assert code == exits.OK, output.err
    cockpits = [window for window in private.windows("flow") if window.name == "cockpit"]
    assert len(cockpits) == 1 and not cockpits[0].dead and cockpits[0].id != ended


@needs_tmux
def test_open_leaves_a_session_sakura_did_not_create_alone(private, capsys):
    private.ensure_session("flow", Path.home(), "mine", ["sleep", "60"], {}, ("@sakura-other", "1"))

    code, output = open_cockpit(private, capsys)

    assert code == exits.BLOCKED and "not created by the cockpit" in output.err
    assert [window.name for window in private.windows("flow")] == ["mine"]


def test_open_without_tmux_is_a_usage_error_and_prints_no_url(cockpit_toml, tmp_path, capsys):
    personal_flow_config(tmp_path, "[surface]\ntmux = false\n")
    code = cli.main(["--open"], client=NoTmux(), window_argv=["sleep", "60"])
    out = capsys.readouterr()
    assert code == exits.USAGE and "needs tmux (surface.tmux = true and tmux on PATH)" in out.err and "http" not in out.out


def test_open_without_tmux_never_waits_on_tailscale(tmp_path, capsys):
    calls = tmp_path / "tailscale-calls"
    helper_dir = tmp_path / "bin"
    helper_dir.mkdir()
    helper = write_tailscale(helper_dir, marker=calls)
    path = tmp_path / "isolated-home" / ".config" / "sakura" / "cockpit.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(settings_text(helper))
    personal_flow_config(tmp_path, "[surface]\ntmux = false\n")

    code = cli.main(["--open"], client=NoTmux(), window_argv=["sleep", "60"])

    assert code == exits.USAGE and "needs tmux" in capsys.readouterr().err
    assert not calls.exists()


def test_serve_and_once_are_exempt_from_the_tmux_requirement(cockpit_toml, tmp_path):
    personal_flow_config(tmp_path, "[surface]\ntmux = false\n")
    seen = []
    assert cli.main(["serve"], client=NoTmux(), serve=lambda *args, **_api: seen.append(args)) == exits.OK
    assert cli.main(["--once"], client=NoTmux()) == exits.OK
    assert len(seen) == 1


def test_open_with_untrusted_settings_starts_nothing(tmp_path, capsys):
    code = cli.main(["--open"], client=NoTmux(), window_argv=["sleep", "60"])
    out = capsys.readouterr()
    assert code == exits.USAGE and "cockpit.toml" in out.err and out.out == ""


def test_serve_and_open_are_operator_commands_refused_inside_an_engine_role(cockpit_toml, monkeypatch, capsys):
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    for argv in (["serve"], ["--open"]):
        code = cli.main(argv, client=NoTmux(), serve=lambda *args: pytest.fail("served"), window_argv=["sleep", "60"])
        assert code == exits.USAGE
    assert "operator commands cannot run inside an engine role" in capsys.readouterr().err
    assert cli.main(["--once"], client=NoTmux()) == exits.OK  # reading is allowed


def test_the_top_level_command_dispatches_to_the_cockpit(capsys):
    from sakura import cli as top

    assert top.main(["cockpit", "--once"]) == exits.OK
    assert "sakura cockpit" in capsys.readouterr().out


# -- cockpit snapshot and cockpit act: the dashboard's local face -------------------------------

DOC = Path(__file__).resolve().parents[1] / "docs" / "cockpit-api.md"
BIN = Path(__file__).resolve().parents[1] / "bin" / "sakura"
REPO_PATH = Path("/work/repo")


def target_of(repo, task="demo"):
    return api.target_id(flow_item(repo, task))


def act(capsys, argv, **kwargs):
    code = cli.main(["act", *argv, "--json"], client=NoTmux(), **kwargs)
    out = capsys.readouterr().out
    return code, json.loads(out)


@pytest.fixture
def registered(repo):
    create_task(repo, "demo", status="running")
    registry.remember(repo)
    return repo


def test_cli_snapshot_preserves_local_action_keys(tmp_path, monkeypatch, capsys):
    engine = Engine(tmp_path)
    monkeypatch.setattr(cli, "_collect", lambda: engine.snapshot)
    try:
        http_body = http._get(engine, "/api/snapshot")[2].decode()
    finally:
        engine.server.server_close()

    assert cli.main(["snapshot", "--json"], client=NoTmux()) == exits.OK
    local, remote = json.loads(capsys.readouterr().out), json.loads(http_body)
    assert local["api"] == 1 and local["items"][0]["id"] == target_of(REPO_PATH, "t1")
    assert remote["api"] == 2 and remote["items"][0]["id"].startswith("t1.")


def test_cli_snapshot_needs_json_and_is_read_only_in_role_sessions(monkeypatch, capsys):
    with pytest.raises(SystemExit) as stop:
        cli.main(["snapshot"], client=NoTmux())
    assert stop.value.code == exits.USAGE
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    assert cli.main(["snapshot", "--json"], client=NoTmux()) == exits.OK
    assert json.loads(capsys.readouterr().out)["api"] == 1


def test_cli_act_confirm_then_token_runs_once(registered, capsys):
    spawns = Spawns(stdout="stopped\n")
    argv = ["s", target_of(registered), "--actor", "owner@example.com"]

    code, asked = act(capsys, argv, run=spawns)
    assert code == exits.OK and asked["result"] == "confirm" and spawns.calls == []
    assert asked["expires"].endswith("Z") and asked["summary"].startswith("Stop: ") and "flow stop" in asked["command"]

    code, done = act(capsys, [*argv, "--confirm", asked["token"]], run=spawns)
    assert code == exits.OK and done == {"result": "done", "exit_code": 0, "output": "stopped\n"}
    (command, _), = spawns.calls
    assert "--actor=cli:owner@example.com" in command

    code, replay = act(capsys, [*argv, "--confirm", asked["token"]], run=spawns)
    assert code == exits.USAGE and replay["result"] == "refused" and len(spawns.calls) == 1


@pytest.mark.parametrize("key, status", [("R", "blocked"), ("s", "blocked"), ("C", "failed")])
def test_cli_act_blocked_wait_runs_its_lane_command_with_the_current_revision(repo, capsys, key, status):
    task = create_task(repo, "demo", status=status)
    registry.remember(repo)
    (wait,) = cli._collect().waits
    assert wait.kind == waits.BLOCKED
    spawns = Spawns()

    for target in (target_of(repo), wait.id):
        argv = [key, target, "--source", "app", "--actor", "owner@example.com"]
        code, asked = act(capsys, argv, run=spawns)
        assert code == exits.OK and asked["result"] == "confirm", asked
        code, done = act(capsys, [*argv, "--confirm", asked["token"]], run=spawns)
        assert code == exits.OK and done["result"] == "done" and done["exit_code"] == 0, done

    (lane_args, lane_env), (wait_args, wait_env) = spawns.calls
    if key in ("s", "C"):
        revision = f"--revision={resume.revision(task.read_state())}"
        assert revision in wait_args and revision not in lane_args
        wait_args = [arg for arg in wait_args if arg != revision]
    assert wait_args == lane_args
    assert wait_env == lane_env and wait_env["cwd"] == repo


def test_cli_act_blocked_wait_resume_rejects_a_changed_lane_revision(repo, capsys):
    task = create_task(repo, "demo", status="blocked")
    registry.remember(repo)
    (wait,) = cli._collect().waits
    spawns = Spawns()
    argv = ["R", wait.id, "--source", "app", "--actor", "owner@example.com"]
    code, asked = act(capsys, argv, run=spawns)
    assert code == exits.OK and asked["result"] == "confirm", asked
    revision = resume.revision(task.read_state())
    assert f"--stopped-revision={revision}" in asked["command"]

    task.update_state(lambda state: state.update(resume_generation=1))
    assert resume.revision(task.read_state()) != revision
    assert [current.id for current in cli._collect().waits] == [wait.id]
    code, answer = act(capsys, [*argv, "--confirm", asked["token"]], run=spawns)

    assert code == exits.USAGE and answer["result"] == "refused" and "confirmation proof rejected" in answer["reason"]
    assert spawns.calls == []


@pytest.mark.parametrize("key, option, value, words", [
    ("q", "--level", "slow", ("flow", "pace")),
    ("S", "--preset", "lean", ("flow", "preset")),
])
def test_cli_act_takes_the_lane_pace_level_and_preset_the_app_sends(registered, capsys, key, option, value, words):
    spawns = Spawns(stdout="ok\n")

    code, done = act(capsys, [key, target_of(registered), option, value, "--actor", "owner@example.com"], run=spawns)

    assert code == exits.OK and done["result"] == "done", done
    (command, _), = spawns.calls
    assert all(word in command for word in words) and command[-1] == value


@pytest.mark.parametrize("key, options, tail", [
    ("K", ["--rule", "builders", "--value", "2"], ["builders", "2"]),
    ("U", ["--preset", "lean"], ["lean"]),
    ("Q", ["--level", "fast"], ["fast"]),
])
def test_cli_act_takes_the_inputs_of_the_engine_controls(registered, capsys, key, options, tail):
    spawns = Spawns(stdout="ok\n")

    code, done = act(capsys, [key, api.target_id(host_actions.ENGINE), *options, "--actor", "owner@example.com"],
                     run=spawns)

    assert code == exits.OK and done["result"] == "done", done
    (command, _), = spawns.calls
    assert command[-len(tail):] == tail


@pytest.mark.parametrize("change", ["actor", "action", "target", "input"])
def test_cli_act_token_is_bound_to_actor_action_target_and_inputs(registered, capsys, change):
    create_task(registered, "other", status="running")
    spawns = Spawns()
    argv = ["s", target_of(registered), "--actor", "owner@example.com"]
    token = act(capsys, argv, run=spawns)[1]["token"]
    tried = {"actor": ["s", argv[1], "--actor", "friend@example.com"],
             "action": ["a", argv[1], "--actor", "owner@example.com"],
             "target": ["s", target_of(registered, "other"), "--actor", "owner@example.com"],
             "input": [*argv, "--text", "extra"]}[change]

    code, answer = act(capsys, [*tried, "--confirm", token], run=spawns)

    assert code == exits.USAGE and answer["result"] == "refused" and spawns.calls == []


def run_process(*argv):
    environment = {key: value for key, value in os.environ.items() if key not in ("TMUX", "TMUX_PANE")}
    done = subprocess.run([str(BIN), "cockpit", *argv], env=environment, capture_output=True, text=True, timeout=60)
    return done.returncode, json.loads(done.stdout)


def test_cli_act_token_is_one_time_across_processes(registered):
    argv = ["act", "s", target_of(registered), "--actor", "owner@example.com", "--json"]

    code, asked = run_process(*argv)
    assert code == exits.OK and asked["result"] == "confirm"
    code, done = run_process(*argv, "--confirm", asked["token"])
    assert code == exits.OK and done["result"] == "done" and done["exit_code"] == 0, done
    assert store.TaskStore(registered, "demo").stop_file.exists()
    code, replay = run_process(*argv, "--confirm", asked["token"])
    assert code == exits.USAGE and replay["result"] == "refused"


REFUSED_ARGS = [
    (["x", "{target}"], "unknown action"),
    (["enter", "{target}"], "unknown action"),
    (["d", "{target}"], "does not apply"),
    (["D", "{target}", "--step", "../x"], "invalid step"),
    (["D", "{target}"], "missing step"),
    (["D", "{target}", "--step", "power", "--text", "x"], "unexpected text"),
    (["D", '["flow","/etc","demo"]', "--step", "power"], "no such target"),
    (["D", "not json", "--step", "power"], "not JSON"),
    (["D", "{target}", "--step", "power", "--actor", "bad actor"], "invalid actor"),
    (["s", "{target}", "--confirm", "garbage"], "confirmation"),
]


@pytest.mark.parametrize(("argv", "reason"), REFUSED_ARGS)
def test_cli_act_refusals_print_json_and_spawn_nothing(registered, capsys, argv, reason):
    spawns = Spawns()
    argv = [part.replace("{target}", target_of(registered)) for part in argv]

    code, answer = act(capsys, argv, run=spawns)

    assert code == exits.USAGE and answer["result"] == "refused" and reason in answer["reason"]
    assert spawns.calls == []


def test_cli_act_writes_actor_to_action_log_and_ledger(registered, capsys):
    code, answer = act(capsys, ["D", target_of(registered), "--step", "power", "--actor", "owner@example.com"])

    assert code == exits.OK and answer["result"] == "done" and answer["exit_code"] == 0, answer
    (command,) = [c for _, c in inbox.pending(store.TaskStore(registered, "demo"))]
    assert (command["kind"], command["value"], command["actor"]) == ("dial", "power", "cli:owner@example.com")
    (row,) = log_rows(engine_home.state_dir() / "cockpit" / "actions.jsonl")
    assert row["actor"] == {"source": "cli", "login": "owner@example.com", "node_id": "", "device": ""}
    assert row["outcome"] == "done"


def test_cli_act_is_done_with_an_audit_note_when_the_log_cannot_be_written_after_the_run(registered, capsys):
    log = engine_home.state_dir() / "cockpit" / "actions.jsonl"
    log.mkdir(parents=True)
    spawns = Spawns()

    code, answer = act(capsys, ["D", target_of(registered), "--step", "power"], run=spawns)

    assert code == exits.OK and len(spawns.calls) == 1
    assert answer["result"] == "done" and answer["exit_code"] == 0 and answer["audit"].startswith("not written: ")


OVERLAY_ACTOR = "overlay:owner@example.com:nMAC"


def test_cli_act_refuses_a_overlay_token_even_when_the_environment_claims_a_overlay_actor(registered, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR", OVERLAY_ACTOR)
    http_token = proofs.issue(proofs.load_secret(), gate.Identity("owner@example.com", "nMAC", ""), "s",
                              ("flow", str(registered), "demo"), {}, source="overlay")
    spawns = Spawns()

    code, answer = act(capsys, ["s", target_of(registered), "--confirm", http_token], run=spawns)

    assert code == exits.USAGE and answer["result"] == "refused" and "confirmation" in answer["reason"]
    assert spawns.calls == []


def test_cli_act_actor_from_the_environment_is_always_a_cli_actor(registered, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR", OVERLAY_ACTOR)
    spawns = Spawns()

    code, answer = act(capsys, ["D", target_of(registered), "--step", "power"], run=spawns)

    assert code == exits.OK and answer["result"] == "done"
    assert "--actor=cli:owner@example.com:nMAC" in spawns.calls[0][0]
    (row,) = log_rows(engine_home.state_dir() / "cockpit" / "actions.jsonl")
    assert (row["actor"]["source"], row["actor"]["login"], row["actor"]["node_id"]) == ("cli", "owner@example.com", "nMAC")


def test_cli_act_without_actor_uses_the_local_user(registered, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR", "cli:operator")
    spawns = Spawns()
    code, answer = act(capsys, ["D", target_of(registered), "--step", "power"], run=spawns)
    assert code == exits.OK and answer["result"] == "done" and "--actor=cli:operator" in spawns.calls[0][0]


def test_cli_act_from_the_app_is_recorded_as_an_app_actor(registered, capsys, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR", "cli:operator")
    spawns = Spawns()

    code, answer = act(capsys, ["D", target_of(registered), "--step", "power", "--source", "app"], run=spawns)

    assert code == exits.OK and answer["result"] == "done"
    assert "--actor=app:operator" in spawns.calls[0][0]
    (row,) = log_rows(engine_home.state_dir() / "cockpit" / "actions.jsonl")
    assert (row["actor"]["source"], row["actor"]["login"]) == ("app", "operator")


def test_cli_act_with_an_unknown_source_is_refused(registered, capsys):
    with pytest.raises(SystemExit) as stop:
        act(capsys, ["D", target_of(registered), "--step", "power", "--source", "overlay"], run=Spawns())
    assert stop.value.code == exits.USAGE


def test_cli_and_http_share_one_controller(registered, tmp_path, monkeypatch, capsys):
    performed = []
    real = Controller.perform
    monkeypatch.setattr(Controller, "perform", lambda self, *a, **k: performed.append(1) or real(self, *a, **k))
    engine = Engine(tmp_path)
    try:
        http._post(engine, {"key": "D", "target": engine.target(engine.flow), "input.step": "power"}, path="/api/action")
        http._post(engine, {"key": "D", "target": engine.target(engine.flow), "input.step": "power"}, path="/action")
    finally:
        engine.server.server_close()
    assert len(performed) == 2

    act(capsys, ["D", target_of(registered), "--step", "power"], run=Spawns())
    assert len(performed) == 3


def test_cockpit_act_refused_in_role_session_and_braked(registered, capsys, monkeypatch):
    spawns = Spawns()
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    code, answer = act(capsys, ["D", target_of(registered), "--step", "power"], run=spawns)
    assert code == exits.USAGE and answer == {"result": "refused", "reason": brakes.ENGINE_ROLE_REFUSAL}
    assert spawns.calls == []
    monkeypatch.delenv("SAKURA_FLOW_ROLE")

    env = {"SAKURA_FLOW_TASK": "demo", "SAKURA_FLOW_ROLE": "builder", "SAKURA_FLOW_PHASE": "build"}
    payload = lambda command: {"session_id": "s1", "tool_name": "Bash", "tool_input": {"command": command}}  # noqa: E731
    denied = brakes.decision(registered, "PreToolUse", payload("sakura cockpit act s x --confirm y"), env)
    assert denied and denied[0] == "deny" and "cockpit act" in denied[1]
    for command in ("sakura cockpit snapshot --json", "sakura cockpit --once", "echo sakura cockpit act"):
        assert brakes.decision(registered, "PreToolUse", payload(command), env) is None


def test_doc_lists_every_cli_option():
    doc = DOC.read_text()
    for parser in (cli.act_parser(), cli.snapshot_parser(), cli.serve_parser()):
        for action in parser._actions:
            for option in action.option_strings:
                if option not in ("-h", "--help"):
                    assert option in doc, option
    for word in ("cockpit snapshot", "cockpit act", "cockpit serve"):
        assert word in doc, word


def _declared_inputs():
    """Every input any action of the table declares, needed or optional, for each of the targets it could take."""
    names = set()
    for action in actions.ACTIONS.values():
        for target in (flow_item(REPO_PATH), lane_item(REPO_PATH)):
            names |= set(action.needs(target)) | set(action.optional)
    return names


def test_cli_act_EveryInputAnActionDeclares_ParsesAndReachesTheControllerFields():
    parser = cli.act_parser()
    fields = {}
    for name in sorted(_declared_inputs()):
        args = parser.parse_args(["K", "target", f"--{name}=value", "--json"])
        fields[name] = cli._act_fields(args)

    assert all(fields[name].get(cli.INPUT_PREFIX + name) == "value" for name in fields)


@pytest.mark.parametrize(("argv", "unneeded"), [
    ([], ("sakura.flow.config", "sakura.flow.process", "sakura.flow.store")),
    (["cockpit", "act", "--help"], ("sakura.cockpit.api", "sakura.cockpit.server", "sakura.flow.phases")),
    (["cockpit", "snapshot", "--json"],
     ("sakura.cockpit.server", "sakura.cockpit.phone", "sakura.cockpit.text", "sakura.flow.phases")),
])
def test_cli_does_not_import_unused_command_modules(argv, unneeded):
    import sys

    script = """
import sys
sys.path.insert(0, sys.argv[1])
from sakura.cli import main
argv, unneeded = sys.argv[2].split(","), sys.argv[3].split(",")
if argv != [""]:
    try:
        assert main(argv) == 0
    except SystemExit as error:
        assert error.code == 0
assert not (loaded := set(sys.modules).intersection(unneeded)), sorted(loaded)
"""
    run = subprocess.run([sys.executable, "-B", "-c", script, str(BIN.parent.parent / "lib"),
                          ",".join(argv), ",".join(unneeded)], capture_output=True, text=True, check=False)
    assert run.returncode == 0, run.stderr


def test_connector_cli_revisions_parse_and_role_mutations_are_refused(capsys, monkeypatch):
    args = cli.act_parser().parse_args(["connectorreplace", '["host","connectors"]', "--op", "replace-1",
                                       "--name", "legacy", "--revision", "1", "--candidate", "cn-123",
                                       "--candidate_revision", "2", "--json"])
    assert cli._act_fields(args)["input.candidate_revision"] == "2"
    monkeypatch.setenv("SAKURA_FLOW_ROLE", "builder")
    code, result = act(capsys, ["connectordisconnect", '["host","connectors"]', "--op", "remove-1",
                                "--name", "legacy", "--revision", "1"])
    assert code == exits.USAGE and result["result"] == "refused"

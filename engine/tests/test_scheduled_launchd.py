import hashlib
import plistlib
from pathlib import Path

import pytest
from sakura import engine_home
from sakura.digest import launchd as digest_launchd
from sakura.digest.launchd import Agent
from sakura.scheduled import cli, launchd, store
from test_scheduled_store import DEFINITION, NOW


def test_override_keeps_plist_and_environment_in_engine_home(tmp_path, monkeypatch):
    root = engine_home.root()
    alias = tmp_path / "alias"
    alias.symlink_to(root.parent, target_is_directory=True)
    monkeypatch.setenv(engine_home.ENV_VARIABLE, str(alias / root.name))
    relative_folder = Path("injected-agents")
    monkeypatch.setattr(digest_launchd, "AGENTS_RELATIVE", relative_folder)
    account_home = tmp_path / "account-home"
    calls = []
    agent = Agent(lambda command: calls.append(command) or 0, home=account_home)

    tasks = store.Store()
    tasks.create(DEFINITION, "launch", NOW)
    launchd.reconcile(tasks, agent=agent)

    (path,) = (root / relative_folder).glob("*.plist")
    content = plistlib.loads(path.read_bytes())
    assert not account_home.exists()
    assert content["EnvironmentVariables"] == {
        "HOME": str(root.resolve()), engine_home.ENV_VARIABLE: str(root.resolve()),
        "PATH": ":".join(digest_launchd.AGENT_PATH).format(home=root.resolve()),
    }
    assert content["StandardOutPath"] == content["StandardErrorPath"] == str(root.resolve() / "scheduled/launchd.log")
    assert Path(calls[-1][-1]).resolve() == path.resolve()


def test_each_engine_home_uses_its_own_label_for_all_launchctl_calls(tmp_path, monkeypatch):
    folder = tmp_path / "agents"
    calls = []

    def control(command):
        calls.append(command)
        return 0

    agent = Agent(control, folder, tmp_path)
    monkeypatch.setattr(digest_launchd, "default_launchctl", control)
    targets = []
    for name in ("first", "second"):
        root = tmp_path / name
        root.mkdir(mode=0o700)
        monkeypatch.setenv(engine_home.ENV_VARIABLE, str(root))
        tasks = store.Store()
        tasks.create(DEFINITION, name, NOW)
        launchd.reconcile(tasks, agent=agent)
        launchd.reconcile(tasks, agent=agent, force_enabled=True)
        launchd.kick()
        digest = hashlib.sha256(str(root.resolve()).encode()).hexdigest()
        targets.append(f"local.sakura.scheduled.{digest[:16]}")

    assert {path.stem for path in folder.glob("*.plist")} == set(targets)
    for label in targets:
        path = folder / f"{label}.plist"
        assert plistlib.loads(path.read_bytes())["Label"] == label
        target = digest_launchd._target(label)
        assert ["bootout", target] in calls
        assert ["bootstrap", digest_launchd._domain(), str(path)] in calls
        assert ["print", target] in calls
        assert ["kickstart", target] in calls

    (task,) = tasks.tasks()
    tasks.change(task.id, task.revision, "scheduledpause", {}, NOW)
    launchd.reconcile(tasks, agent=agent)
    assert calls[-1] == ["bootout", digest_launchd._target(targets[1])]
    assert {path.stem for path in folder.glob("*.plist")} == {targets[0]}


@pytest.mark.parametrize("has_other_owner", [False, True])
def test_default_home_retires_only_its_legacy_plist(tmp_path, monkeypatch, has_other_owner):
    monkeypatch.delenv(engine_home.ENV_VARIABLE)
    account_home = tmp_path / "account-home"
    folder = tmp_path / "agents"
    folder.mkdir()
    legacy = folder / "local.sakura.scheduled.plist"
    environment = {"HOME": str(account_home)}
    if has_other_owner:
        environment[engine_home.ENV_VARIABLE] = str(tmp_path / "other-home")
    legacy.write_bytes(plistlib.dumps({"Label": legacy.stem, "EnvironmentVariables": environment}))
    calls = []
    agent = Agent(lambda command: calls.append(command) or 0, folder, account_home)
    tasks = store.Store()
    tasks.create(DEFINITION, "launch", NOW)
    launchd.reconcile(tasks, agent=agent)

    assert legacy.exists() is has_other_owner
    (path,) = folder.glob("local.sakura.scheduled.*.plist")
    content = plistlib.loads(path.read_bytes())
    assert content["EnvironmentVariables"]["HOME"] == str(account_home)
    assert engine_home.ENV_VARIABLE not in content["EnvironmentVariables"]
    assert (["bootout", digest_launchd._target(legacy.stem)] in calls) is not has_other_owner


def test_cli_reconcile_reloads_a_dispatcher_missing_from_the_login_session(tmp_path, monkeypatch):
    calls = []

    def control(command):
        calls.append(command)
        return int(command[0] == "print")

    agent = Agent(control, tmp_path / "agents", tmp_path)
    monkeypatch.setattr(digest_launchd, "Agent", lambda: agent)
    tasks = store.Store()
    tasks.create(DEFINITION, "launch", NOW)
    launchd.reconcile(tasks, agent=agent)
    calls.clear()

    assert cli.main(["reconcile"]) == 0
    assert [command[0] for command in calls] == ["print", "bootout", "bootstrap"]


def test_one_agent_for_enabled_work_and_none_after_pause(tmp_path):
    calls = []

    def run(command):
        calls.append(command)
        return 0

    agent = Agent(run, tmp_path, tmp_path)
    tasks = store.Store()
    launchd.reconcile(tasks, agent=agent)
    assert not calls
    task = tasks.create(DEFINITION, "launch", NOW)
    launchd.reconcile(tasks, agent=agent)
    launchd.reconcile(tasks, agent=agent)
    assert [command[0] for command in calls] == ["bootout", "bootstrap"]
    (path,) = tmp_path.glob("*.plist")
    content = plistlib.loads(path.read_bytes())
    assert content["RunAtLoad"] and len(content["StartCalendarInterval"]) == 60
    assert "KeepAlive" not in content
    tasks.change(task.id, task.revision, "scheduledpause", {}, NOW)
    launchd.reconcile(tasks, agent=agent)
    assert calls[-1][0] == "bootout"
    assert not list(tmp_path.glob("*.plist"))


def test_cutover_reinstalls_plist_left_before_bootstrap(tmp_path):
    calls = []

    def control(command):
        calls.append(command)
        return int(command[0] == "print")

    tasks = store.Store()
    tasks.create({**DEFINITION, "enabled": False}, "staged", NOW)
    (tmp_path / f"{launchd.label_of(engine_home.root())}.plist").write_bytes(b"staged before bootstrap")
    launchd.reconcile(tasks, agent=Agent(control, tmp_path, tmp_path), force_enabled=True)
    assert [command[0] for command in calls] == ["print", "bootout", "bootstrap"]


def test_pause_last_task_leaves_dispatcher_until_running_turn_finishes(tmp_path):
    from sakura.scheduled import dispatch, runner

    calls = []

    def control(command):
        calls.append(command)
        return int(command[0] == "print")

    agent = Agent(control, tmp_path, tmp_path)
    tasks = store.Store()
    task = tasks.create(DEFINITION, "active-turn", NOW)
    launchd.reconcile(tasks, agent=agent)
    dispatch.run_now(tasks, task.id, task.revision, "turn", NOW)
    claim = runner._begin(tasks, NOW)
    tasks.change(task.id, task.revision, "scheduledpause", {}, NOW)
    launchd.reconcile(tasks, agent=agent)
    assert [command[0] for command in calls] == ["bootout", "bootstrap"]
    runner._finish(tasks, claim.id, NOW, None)
    launchd.reconcile(tasks, agent=agent)
    assert not list(tmp_path.glob("*.plist"))

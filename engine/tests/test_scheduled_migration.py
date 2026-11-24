import shutil
from datetime import timedelta
from pathlib import Path

import pytest
from sakura import engine_home
from sakura.automations import registry as automations
from sakura.digest import config, launchd, run, state
from sakura.scheduled import dispatch, migrate, runner, store
from sakura.scheduled.launchd import label_of
from test_scheduled_store import NOW


@pytest.fixture
def legacy(tmp_path):
    executable = Path.home() / "reader script"
    executable.write_text("#!/bin/sh\nexit 1\n")
    executable.chmod(0o700)
    settings = config.DigestConfig("07:30", "sonnet", "it", 27, (
        config.Source("custom-a", (str(executable), "--one", "two words"), label="Updates", reads="Release notes", timeout_seconds=17),
        config.Source("custom-b", (str(executable), "other"), enabled=False, label="Updates"),
        config.Source("news", feed="https://example.invalid/feed", label="News"),
    ))
    config.save(settings)
    original = state.State({"custom-a": state.SourceState("opaque:ç/\U0001f6b2==", "2026-11-21T05:00:00.000000+00:00"),
                            "news": state.SourceState("", "2026-11-20T05:15:11+00:00")},
                           "2026-11-22", "2026-11-22T07:35:00+00:00")
    state.save(original)
    archive = config.digest_folder() / "2026-11-21.json"
    archive.write_text('{"archive":"keep"}')
    folder_copy = tmp_path / "legacy-copy"
    shutil.copytree(config.digest_folder(), folder_copy)
    shutil.rmtree(config.digest_folder())
    shutil.copytree(folder_copy, config.digest_folder())
    installed = automations.Automation("late-alias", "Later run", "daily 08:45", "digest run --scheduled", True, True)
    automations._write(automations.folder() / "late-alias.toml", installed)
    return settings, original, archive


def test_copy_records_exact_sources_and_state_without_reading_and_retries(legacy, tmp_path):
    settings, original, archive = legacy
    calls = []
    can_install = False

    def launchctl(command):
        calls.append(command)
        if command[0] == "bootstrap" and not can_install:
            return 1
        return 1 if command[0] == "print" else 0

    agent = launchd.Agent(launchctl, tmp_path / "agents", tmp_path)
    tasks = store.Store()
    with pytest.raises(store.Refused, match="launchd_reconciliation_failed"):
        migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    assert not migrate.owns_digest(tasks)
    assert not dispatch.claim_clock(tasks, NOW + timedelta(hours=1))
    assert state.load() == original
    can_install = True
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    assert migrate.owns_digest(tasks)
    (task,) = tasks.tasks()
    manifest, memory = migrate.digest_state(tasks)
    assert memory == original
    assert [source["id"] for source in manifest["sources"]] == [source.id for source in settings.sources]
    assert manifest["sources"][0]["run"] == list(settings.sources[0].run)
    assert manifest["sources"][0]["timeout_seconds"] == 17
    assert manifest["sources"][1]["enabled"] is False
    assert manifest["bindings"]["custom-a"] != manifest["bindings"]["custom-b"]
    assert manifest["aliases"][0]["schedule"] == "daily 08:45"
    assert archive.read_text() == '{"archive":"keep"}'
    assert not dispatch.claim_clock(tasks, NOW + timedelta(hours=1))
    assert dispatch.claim_clock(tasks, NOW + timedelta(days=1, hours=1))
    assert dispatch.runs(tasks, task.id)[0]["trigger"]["at"].startswith("2026-11-23T08:45")
    with pytest.raises(run.RunRefused, match="scheduled_migration_complete"):
        run.run(settings, NOW, scheduled=False)


def test_enabled_legacy_route_unavailable_preserves_writer(legacy, tmp_path):
    _, original, _ = legacy
    calls = []

    def unavailable(_):
        raise store.Refused("scheduled_provider_unavailable")

    tasks = store.Store()
    migrate.upgrade(tasks, NOW, agent=launchd.Agent(calls.append, tmp_path, tmp_path),
                    verify_route=unavailable, timezone="UTC")
    assert not calls and not migrate.owns_digest(tasks)
    assert migrate.status(tasks) == "scheduled_provider_unavailable"
    assert state.load() == original
    assert not tasks.tasks() or not tasks.tasks()[0].definition["enabled"]


def test_migrated_run_advances_only_successful_checkpoint(legacy, tmp_path, monkeypatch):
    from sakura.digest import installation, intake, supervise
    from test_scheduled_runner import _spec

    settings, original, _ = legacy
    tasks = store.Store()
    agent = launchd.Agent(lambda argv: int(argv[0] == "print"), tmp_path, tmp_path)
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    (task,) = tasks.tasks()
    calls = []
    read = intake.collect

    def program(argv, environment, timeout):
        calls.append((argv, environment[intake.CURSOR_VARIABLE], environment[intake.SINCE_VARIABLE], timeout))
        return supervise.Execution(1, b"", "fake failure")

    def fetch(url, timeout):
        assert url == "https://example.invalid/feed"
        return b"""<rss><channel>
        <item><guid>already-read</guid><title>Old</title><pubDate>Thu, 19 Nov 2026 00:00:00 GMT</pubDate></item>
        <item><guid>unseen</guid><title>New</title><pubDate>Fri, 20 Nov 2026 06:00:00 GMT</pubDate></item>
        </channel></rss>"""

    def collect(settings, memory, now, **kwargs):
        results = read(settings, memory, now, run=program, fetch=fetch, **kwargs)
        assert [item.id for result in results for item in result.items] == ["unseen"]
        return results

    monkeypatch.setattr(intake, "collect", collect)
    now = NOW + timedelta(days=1, hours=1)
    dispatch.claim_clock(tasks, now)
    runner.drain(tasks, lambda: now, prepare=lambda _run: _spec(tmp_path))
    argv = installation.inspect(settings.sources[0].run).argv
    assert calls == [(argv, original.of("custom-a").cursor, original.of("custom-a").at, 17)]
    _, memory = migrate.digest_state(tasks)
    assert memory.sources["custom-a"] == original.sources["custom-a"]
    assert memory.sources["news"].cursor == "2026-11-20T06:00:00+00:00"
    assert dispatch.runs(tasks, task.id)[0]["state"] == "finished"
    assert state.load() == original


def test_retry_after_install_reloads_legacy_checkpoints_before_activation(legacy, tmp_path, monkeypatch):
    _, original, _ = legacy
    tasks = store.Store()
    calls = []
    dispatcher = label_of(engine_home.root())

    def control(command):
        calls.append(command)
        if command[0] == "print":
            is_dispatcher = command[-1].endswith("/" + dispatcher)
            return int(not is_dispatcher or not any(call[0] == "bootstrap" for call in calls))
        return 0

    agent = launchd.Agent(control, tmp_path / "agents", tmp_path)
    activate = migrate._activate

    def interrupted(*args):
        raise OSError("interrupted before ownership")

    monkeypatch.setattr(migrate, "_activate", interrupted)
    with pytest.raises(OSError, match="interrupted"):
        migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    assert not migrate.owns_digest(tasks)
    advanced = state.State({**original.sources, "custom-a": state.SourceState("legacy advanced", original.sources["custom-a"].at)},
                           original.done_date, original.last_run_at)
    state.save(advanced)
    monkeypatch.setattr(migrate, "_activate", activate)
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    assert migrate.digest_state(tasks)[1] == advanced
    assert sum(call[0] == "bootstrap" for call in calls) == 1


def test_migration_keeps_maintenance_and_refreshes_pending_digest_approval(legacy, tmp_path):
    from sakura.flow import waits

    automations.propose("another-summary", "Later summary", "daily 11:00", "digest run --scheduled", "master:old", NOW)
    automations.propose("waiting-for-budget", "Wake lanes", "every 15 minutes", "flow wake", "master:old", NOW)
    maintenance = automations.Automation("maintenance", "Wake lanes", "every 15 minutes", "flow wake", True, True)
    automations._write(automations.folder() / "maintenance.toml", maintenance)
    tasks = store.Store()
    agent = launchd.Agent(lambda argv: int(argv[0] == "print"), tmp_path, tmp_path)
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    enabled = {row["name"] for row in automations.rows() if row["enabled"]}
    assert "maintenance" in enabled and "late-alias" not in enabled
    (pending,) = tasks.approvals()
    assert pending.definition["legacy_alias"] == "daily 11:00"
    (shown,) = [wait for wait in waits.collect([]) if wait.source == "scheduled"]
    assert shown.detail == pending.preview
    tasks.decide(pending.id, pending.revision, True, NOW)
    (task,) = tasks.tasks()
    assert "daily 11:00" in task.definition["legacy_dispatch"]["aliases"]
    with tasks.connect() as db:
        assert db.execute("SELECT reason FROM retired_automations").fetchone()[0] == "maintenance_only"


def test_corrupt_state_does_not_activate_or_load_agents(legacy, tmp_path):
    path = config.digest_folder() / config.STATE_FILE
    path.write_text('{"version":1,"sources":{"custom-a":{"cursor":{"unexpected":1}}}}')
    calls = []
    tasks = store.Store()
    migrate.upgrade(tasks, NOW, agent=launchd.Agent(calls.append, tmp_path, tmp_path),
                    verify_route=lambda _: None, timezone="UTC")
    assert not migrate.owns_digest(tasks)
    assert migrate.status(tasks) == "legacy_digest_state_unavailable"
    assert not calls


def test_failed_removal_keeps_single_writer_and_reconciles(legacy, tmp_path):
    keep_legacy = True
    calls = []

    def control(command):
        calls.append(command)
        return int(command[0] == "print" and not keep_legacy)

    tasks = store.Store()
    agent = launchd.Agent(control, tmp_path / "agents", tmp_path)
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: None, timezone="UTC")
    assert migrate.owns_digest(tasks)
    assert migrate.status(tasks) == "legacy_agent_removal_pending"
    keep_legacy = False
    migrate.upgrade(tasks, NOW, agent=agent, verify_route=lambda _: pytest.fail("unexpected reprobe"), timezone="UTC")
    assert migrate.status(tasks) is None
    assert sum(call[0] == "bootstrap" for call in calls) == 1


def test_bare_codex_model_cannot_be_dispatched_through_claude(legacy):
    from sakura.flow import presets
    from sakura.master import spec

    with pytest.raises(spec.MasterRefused, match="scheduled_provider_unavailable"):
        spec.scheduled_route({"legacy_model": "codex", "account": None}, presets.resolve("standard"))

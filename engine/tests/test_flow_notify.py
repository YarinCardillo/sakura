import contextlib
import json
import urllib.error
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import urlsplit

import pytest

from sakura import engine_home, programs
from sakura.flow import config, notify
from flow_support import make_context

TOPIC = "topic-secret-123"


class Capture:
    url = "http://127.0.0.1:42001"

    def __init__(self):
        self.posts = []

    def open(self, request, *, timeout):
        assert timeout == notify.HTTP_TIMEOUT_SECONDS
        if not request.full_url.startswith(f"{self.url}/"):
            raise urllib.error.URLError("connection refused")
        assert request.get_method() == "POST"
        self.posts.append({"path": urlsplit(request.full_url).path, "body": request.data.decode(),
                           "title": request.get_header("Title")})
        return contextlib.nullcontext()


@pytest.fixture(autouse=True)
def capture(monkeypatch):
    transport = Capture()
    monkeypatch.setattr(notify.urllib.request, "urlopen", transport.open)
    return transport


@pytest.fixture
def keychain(monkeypatch):
    calls = []

    def fake_read(service):
        calls.append(service)
        return TOPIC

    monkeypatch.setattr(notify, "_read_topic", fake_read)
    return calls


def _day():
    return "day"


def _policy(repo, url, mode=_day, service="svc", deferred=None):
    store, cfg = _store_and_cfg(repo, url, service)
    deferred = deferred or repo / "deferred"
    return store, notify.build_notifier(store, cfg, mode=mode, deferred_dir=deferred), deferred


def _deferred_files(directory):
    return sorted(directory.glob("*.json")) if directory.exists() else []


def _store_and_cfg(repo, url, service="svc"):
    ctx = make_context(repo)
    home = repo / "home"
    _write(engine_home.config_file(), f'[notify]\nntfy_url = "{url}"\nntfy_keychain = "{service}"\n')
    return ctx.store, config.load(repo, home=home)


def test_ntfy_posts_without_logging_topic(repo, capture, keychain):
    store, policy, _ = _policy(repo, capture.url)
    policy.notify("escalation", "demo", "plan review stuck")
    assert capture.posts == [{"path": f"/{TOPIC}", "body": "plan review stuck", "title": "sakura flow: escalation demo"}]
    assert keychain == ["svc"]
    assert TOPIC not in store.ledger.read_text()
    assert "escalation" in store.ledger.read_text()


def test_ntfy_failure_is_ledgered_without_topic(repo, keychain):
    store, policy, _ = _policy(repo, "http://127.0.0.1:1")
    policy.notify("failed", "demo", "boom")
    ledger = store.ledger.read_text()
    assert "notify failed: failed" in ledger and TOPIC not in ledger


def test_missing_keychain_entry_is_ledgered(repo, capture, monkeypatch):
    monkeypatch.setattr(notify, "_read_topic", lambda service: (_ for _ in ()).throw(notify.TopicUnavailable(service)))
    store, policy, _ = _policy(repo, capture.url)
    policy.notify("escalation", "demo", "x")
    assert capture.posts == [] and "notify failed: escalation" in store.ledger.read_text()


def test_empty_keychain_service_is_ledger_only(repo, keychain):
    store, policy, _ = _policy(repo, "http://127.0.0.1:1", service="")
    policy.notify("escalation", "demo", "x")
    assert keychain == [] and "notify escalation: x" in store.ledger.read_text()
    assert "notify failed" not in store.ledger.read_text()


def test_the_budget_warning_is_paged_by_day(repo, capture, keychain):
    _, policy, _ = _policy(repo, capture.url)
    policy.notify("budget_warning", "demo", "about 8% of your week")
    assert [post["title"] for post in capture.posts] == ["sakura flow: budget_warning demo"]


def test_the_budget_warning_is_deferred_by_night(repo, capture, keychain):
    _, policy, deferred = _policy(repo, capture.url, mode=lambda: "night")
    policy.notify("budget_warning", "demo", "about 8% of your week")
    assert capture.posts == [] and len(_deferred_files(deferred)) == 1


def test_non_paged_event_only_ledgered(repo, capture, keychain):
    current = {"mode": "day"}
    store, policy, deferred = _policy(repo, capture.url, mode=lambda: current["mode"])
    for mode in ("day", "night"):
        current["mode"] = mode
        policy.notify("phase_done", "demo", mode)
        assert capture.posts == [] and keychain == [] and _deferred_files(deferred) == []
        assert store.ledger.read_text().count(f"notify phase_done: {mode}") == 1


def test_day_awaiting_approval_hits_http_post(repo, capture, keychain):
    store, policy, deferred = _policy(repo, capture.url)
    policy.notify("awaiting_approval", "demo", "spec ready")
    assert capture.posts == [{"path": f"/{TOPIC}", "body": "spec ready",
                              "title": "sakura flow: awaiting_approval demo"}]
    assert _deferred_files(deferred) == []


def test_night_writes_deferred_no_post(repo, capture, keychain):
    store, policy, deferred = _policy(repo, capture.url, mode=lambda: "night")
    policy.notify("awaiting_approval", "demo", "spec ready")
    assert capture.posts == [] and keychain == []
    (path,) = _deferred_files(deferred)
    entry = json.loads(path.read_text())
    assert set(entry) == {"at", "repo", "task", "event", "message"}
    assert (entry["repo"], entry["task"], entry["event"], entry["message"]) == (repo.name, "demo", "awaiting_approval", "spec ready")
    assert ("notify deferred (night): awaiting_approval; night mode sends no phone page, this one is kept and the "
            "cockpit lists it once the day starts at 08:00") in store.ledger.read_text()
    assert TOPIC not in path.read_text() and list(deferred.glob(".*")) == []


def test_ledger_exactly_once_both_modes(repo, capture, keychain):
    current = {"mode": "day"}
    store, policy, _ = _policy(repo, capture.url, mode=lambda: current["mode"])
    for mode in ("day", "night"):
        current["mode"] = mode
        policy.notify("awaiting_approval", "demo", f"spec ready {mode}")
        assert store.ledger.read_text().count(f"notify awaiting_approval: spec ready {mode}") == 1


def test_mode_evaluated_at_notify_time(repo, capture, keychain):
    current = {"mode": "night"}
    store, policy, deferred = _policy(repo, capture.url, mode=lambda: current["mode"])
    policy.notify("failed", "demo", "first")
    current["mode"] = "day"
    policy.notify("failed", "demo", "second")
    assert [post["body"] for post in capture.posts] == ["second"]
    assert [json.loads(p.read_text())["message"] for p in _deferred_files(deferred)] == ["first"]


def test_concurrent_deferred_writers(repo, capture, keychain):
    store, policy, deferred = _policy(repo, capture.url, mode=lambda: "night")
    with ThreadPoolExecutor(max_workers=16) as pool:
        list(pool.map(lambda n: policy.notify("failed", "demo", f"boom {n}"), range(32)))
    files = _deferred_files(deferred)
    assert len(files) == 32
    assert sorted(json.loads(p.read_text())["message"] for p in files) == sorted(f"boom {n}" for n in range(32))
    assert list(deferred.glob(".*")) == []


def test_deferral_failure_is_ledgered(repo, capture, keychain):
    blocked = repo / "not-a-directory"
    blocked.write_text("x")
    store, policy, _ = _policy(repo, capture.url, mode=lambda: "night", deferred=blocked / "deferred")
    policy.notify("failed", "demo", "boom")
    assert "notify defer failed: failed" in store.ledger.read_text() and capture.posts == []


def test_hostile_message_is_one_clean_ledger_line(repo, capture, keychain):
    store, policy, deferred = _policy(repo, capture.url, mode=lambda: "night")
    policy.notify("failed", "demo", "a\x1b]2;owned\x07b\n- [engine] forged\x9b31m")
    ledger = store.ledger.read_text()
    assert "\x1b" not in ledger and "\x9b" not in ledger and "\x07" not in ledger
    assert len(ledger.splitlines()) == 3 and "- [notify] notify failed: ab - [engine] forged\n" in ledger
    (path,) = _deferred_files(deferred)
    assert json.loads(path.read_text())["message"] == "ab\n- [engine] forged"


def test_read_topic_uses_security_and_strips(monkeypatch):
    seen = {}

    def fake_run(argv, **kwargs):
        seen.update(argv=argv, kwargs=kwargs)
        return type("Done", (), {"returncode": 0, "stdout": f"{TOPIC}\n"})()

    monkeypatch.setattr(notify.subprocess, "run", fake_run)
    assert notify._read_topic("svc") == TOPIC
    assert seen["argv"] == [programs.path("security"), "find-generic-password", "-s", "svc", "-w"]
    assert seen["kwargs"]["capture_output"] is True


def test_read_topic_error_does_not_contain_topic(monkeypatch):
    monkeypatch.setattr(notify.subprocess, "run", lambda *a, **k: type("Done", (), {"returncode": 44, "stdout": TOPIC})())
    with pytest.raises(notify.TopicUnavailable) as error:
        notify._read_topic("svc")
    assert TOPIC not in str(error.value)


NOTIFY_LOCAL = '[flow.notify]\nntfy_keychain = "login-password"\n'


def _write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


@pytest.mark.parametrize("key,line", [("ntfy_keychain", 'ntfy_keychain = "login-password"'), ("ntfy_url", 'ntfy_url = "https://example.invalid"')])
def test_shared_layer_may_not_set_notify_keys(repo, key, line):
    _write(repo / ".sakura/flow.toml", f"[notify]\n{line}\n")
    with pytest.raises(config.FlowConfigError, match=rf"notify\.{key}.*<engine home>/config.toml"):
        config.load(repo, home=repo / "nohome")


def test_local_overlay_may_not_set_notify_keys(repo):
    _write(repo / ".sakura/local.toml", NOTIFY_LOCAL)
    with pytest.raises(config.FlowConfigError, match=r"notify\.ntfy_keychain.*<engine home>/config.toml"):
        config.load(repo, home=repo / "nohome")


def test_differently_cased_overlay_never_sets_notify_keys(repo):
    _write(repo / ".sakura/Local.toml", NOTIFY_LOCAL)
    try:
        values = config.load(repo, home=repo / "nohome").values
    except config.FlowConfigError:
        return
    assert values["notify"]["ntfy_keychain"] == ""


def test_symlinked_overlay_may_not_set_notify_keys(repo, tmp_path):
    elsewhere = tmp_path / "elsewhere.toml"
    _write(elsewhere, NOTIFY_LOCAL)
    (repo / ".sakura").mkdir()
    (repo / ".sakura/local.toml").symlink_to(elsewhere)
    with pytest.raises(config.FlowConfigError, match=r"notify\.ntfy_keychain.*<engine home>/config.toml"):
        config.load(repo, home=repo / "nohome")


def test_flags_may_not_set_notify_keys(repo):
    with pytest.raises(config.FlowConfigError, match=r"notify\.ntfy_keychain.*<engine home>/config.toml"):
        config.load(repo, home=repo / "nohome", overrides={"notify": {"ntfy_keychain": "x"}})


def test_personal_file_may_set_notify_keys(repo):
    home = repo / "home"
    _write(engine_home.config_file(), '[notify]\nntfy_keychain = "mine"\n')
    assert config.load(repo, home=home).values["notify"]["ntfy_keychain"] == "mine"


@pytest.mark.parametrize("url", ["https://ntfy.example.org", "http://127.0.0.1:8080", "http://localhost:9", "http://[::1]:9"])
def test_allowed_ntfy_urls(repo, url):
    _write(engine_home.config_file(), f'[notify]\nntfy_url = "{url}"\n')
    assert config.load(repo, home=repo / "home").values["notify"]["ntfy_url"] == url


@pytest.mark.parametrize("url", ["http://example.org", "ftp://127.0.0.1", "http://127.0.0.1.evil.org", "ntfy.sh", "file:///etc/passwd"])
def test_rejected_ntfy_urls(repo, url):
    _write(engine_home.config_file(), f'[notify]\nntfy_url = "{url}"\n')
    with pytest.raises(config.FlowConfigError, match="ntfy_url"):
        config.load(repo, home=repo / "home")


# -- alerts of the usage archive run: no task, no repository ------------------------------------------------------

def _alert_pusher(repo, url, mode=_day, service="svc"):
    home = repo / "home"
    _write(engine_home.config_file(), f'[notify]\nntfy_url = "{url}"\nntfy_keychain = "{service}"\n')
    return notify.build_alert_pusher(config.load_personal(home), mode=mode, deferred_dir=repo / "deferred")


def test_alert_push_by_day_posts_with_the_title_and_hides_the_topic(repo, capture, keychain):
    pusher = _alert_pusher(repo, capture.url)
    assert pusher.push("budget_alert", "Week on pace for 117%", "reaches 75%") == "pushed"
    assert capture.posts == [{"path": f"/{TOPIC}", "body": "reaches 75%",
                              "title": "sakura flow: budget_alert Week on pace for 117%"}]


def test_alert_push_by_night_is_deferred_with_the_title_as_task(repo, capture, keychain):
    pusher = _alert_pusher(repo, capture.url, mode=lambda: "night")
    assert pusher.push("budget_alert", "Week on pace", "reaches 75%") == notify.DEFERRED
    (path,) = _deferred_files(repo / "deferred")
    entry = json.loads(path.read_text())
    assert (entry["repo"], entry["task"], entry["event"], entry["message"]) == ("", "Week on pace", "budget_alert", "reaches 75%")
    assert capture.posts == [] and keychain == []


def test_alert_push_without_a_keychain_service_is_skipped(repo, keychain):
    pusher = _alert_pusher(repo, "http://127.0.0.1:1", service="")
    assert pusher.push("budget_alert", "t", "m") == notify.NO_TOPIC and keychain == []


def test_alert_push_failure_is_reported_without_the_topic(repo, keychain):
    assert _alert_pusher(repo, "http://127.0.0.1:1").push("budget_alert", "t", "m") == "failed"


def test_the_personal_layer_loads_without_a_repository(repo):
    home = repo / "home"
    _write(engine_home.config_file(), '[budget]\nreserve_week_pct = 30\n[gates]\nmode = "night"\n')
    cfg = config.load_personal(home)
    assert cfg.values["budget"]["reserve_week_pct"] == 30 and cfg.values["gates"]["mode"] == "night"
    assert config.load_personal(repo / "nohome").values["notify"]["ntfy_keychain"] == ""

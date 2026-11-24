#!/usr/bin/env python3
"""Captures what the real engine prints, for the `engine-real-*` fixtures of the app's tests.

    ENGINE_KIT=<engine checkout> make-engine-real.py <output folder>

The engine runs in a throwaway home against fixture accounts and Git repositories.
Injected connector ports supply provider checks without contacting a provider.

A second snapshot (`engine-real-snapshot-outside.json`) is taken after an outside lane and a day's morning digest are
added, so the fixtures hold the engine's `digest` kind, its titled links and its one-step rows; the first snapshot is
unchanged by them. The feed's build run ends with a shell call, which the engine now prints with its `command`.
"""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import os
import pwd
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

WRAPPER = r'''#!/usr/bin/env python3
import json, sys
book, rows = %(book)r, %(readings)r
args = sys.argv[1:]
if args[:1] == ["version"]:
    print(json.dumps({"version": "0.0.0", "store_schema": 1, "status_schema": 5,
                      "advice_schema": 2, "accounts_schema": 1}))
    raise SystemExit(0)
if args[:2] == ["accounts", "list"]:
    print(json.dumps({"schema_version": 1, "accounts": book}))
    raise SystemExit(0)
if args[:1] != ["status"]:
    raise SystemExit(2)
empty = {"pct": None, "resets_at": None}
no_forecast = {"average_pct_at_reset": None, "confidence": "none", "exhausts_at": None, "next_round_pct": None,
               "range_high": None, "range_low": None, "recent_pct_at_reset": None, "seven_day_pct_at_reset": None}
accounts = []
for entry in book:
    if entry["hidden"]:
        continue
    reading = rows.get(entry["label"], {})
    accounts.append({"aliases": [], "as_of": reading.get("as_of"), "config_dir": "", "error": None,
                     "five_hour": reading.get("five_hour", empty), "forecast": reading.get("forecast", no_forecast),
                     "hours_early": None, "id": entry["id"], "label": entry["label"], "plan": reading.get("plan"),
                     "provider": entry["provider"], "registered": True, "seven_day": reading.get("seven_day", empty),
                     "severity": None, "source": reading.get("source"), "stale": "as_of" not in reading,
                     "status": "on_target", "target_high": 100, "target_low": 75})
print(json.dumps({"accounts": accounts, "generated_at": "2026-11-18T08:00:00Z", "history": {}, "poller": "none",
                  "schema_version": 5}))
'''

ENGINE_RUNNER = """
import os, pwd, sys
from types import SimpleNamespace
sys.path.insert(0, sys.argv.pop(1))
real_lookup = pwd.getpwuid
pwd.getpwuid = lambda uid: SimpleNamespace(pw_dir=os.environ["HOME"], pw_name="fixture") \
    if uid == os.getuid() else real_lookup(uid)
from sakura.flow import model_catalog
model_catalog.ensure = lambda *args, **kwargs: False
from sakura.cli import main
raise SystemExit(main(sys.argv[1:]))
"""


READINGS = {
    "Studio": {"as_of": "2026-11-18T07:55:00Z", "plan": "max", "source": "probe",
               "five_hour": {"pct": 33, "resets_at": "2026-11-18T12:00:00Z"},
               "seven_day": {"pct": 61, "resets_at": "2026-11-21T17:00:00Z"},
               "forecast": {"average_pct_at_reset": 86, "confidence": "high", "exhausts_at": "2026-11-21T11:43:03Z",
                            "next_round_pct": 49, "range_high": 103.8, "range_low": 86,
                            "recent_pct_at_reset": 103.8, "seven_day_pct_at_reset": 103.8}},
}
NOW = datetime(2026, 11, 18, 8, 0, tzinfo=timezone.utc)


def main(argv: list[str]) -> int:
    kit = Path(os.environ["ENGINE_KIT"]).resolve()
    out = Path(argv[0]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    # The engine only runs helper programs from outside system temporary directories.
    with tempfile.TemporaryDirectory(prefix=".engine-real-tools-", dir=Path(__file__).resolve().parent) as tools:
        with tempfile.TemporaryDirectory(prefix="engine-real-") as home:
            return _capture(kit, out, Path(home).resolve(), Path(tools))


def _capture(kit: Path, out: Path, root: Path, tools: Path) -> int:
    paths = {name: root / name for name in ("home", "engine", "repo", "leeway-home")}
    paths["tools"] = tools
    for path in paths.values():
        path.mkdir(mode=0o700, exist_ok=True)
    environment = _environment(paths)
    os.chdir(root)
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(kit / "lib"))
    os.environ.update(environment)
    from types import SimpleNamespace
    real_lookup = pwd.getpwuid
    pwd.getpwuid = lambda uid: (
        SimpleNamespace(pw_dir=str(paths["home"]), pw_name="fixture")
        if uid == os.getuid() else real_lookup(uid)
    )
    def run(*words, cwd=None, override=None):
        return subprocess.run(
            [sys.executable, "-I", "-B", "-c", ENGINE_RUNNER, str(kit / "lib"), *words],
            env={**environment, **(override or {})}, cwd=cwd, text=True, capture_output=True,
            stdin=subprocess.DEVNULL, check=False,
        )

    has_tracks = importlib.util.find_spec("sakura.tracks") is not None
    if importlib.util.find_spec("sakura.accounts.goose"):
        _capture_goose(run, out, root)
        if not has_tracks:
            print("Goose fixtures captured. Use a Track-enabled engine for the shared and Track fixtures.")
            return 0

    task_id, run_id = _seed(paths)
    run("preset", "use", "standard")  # the listing marks the chosen preset only once one was chosen
    snapshot = run("cockpit", "snapshot", "--json")
    if snapshot.returncode != 0:
        sys.exit(f"snapshot failed ({snapshot.returncode}): {snapshot.stderr}{snapshot.stdout}")
    (out / "engine-real-snapshot.json").write_text(_anonymous(_pretty(snapshot.stdout), root))
    (out / "engine-real-rules.json").write_text(_anonymous(_pretty(run("engine", "rules", "--json").stdout), root))
    (out / "engine-real-preset-list.json").write_text(_pretty(run("preset", "list", "--json").stdout))
    (out / "engine-real-act-help.txt").write_text(run("cockpit", "act", "--help").stdout)
    (out / "engine-real-action-table.json").write_text(_pretty(json.dumps(_action_table())))
    (out / "engine-real-lane-steps.json").write_text(_pretty(json.dumps(_lane_steps(paths["repo"]))))
    for name, key, limit in (("history", task_id, "2"), ("conversation", run_id, "1")):
        result = run("scheduled", name, "--json", "--limit", limit, "--", key)
        result.check_returncode()
        (out / f"engine-real-scheduled-{name}.json").write_text(_pretty(result.stdout))
    paths["orchard"] = root / "orchard"
    _custom_connector(out, tools)
    _outside_lane_and_digest_day(paths["orchard"])
    outside = run("cockpit", "snapshot", "--json")
    (out / "engine-real-snapshot-outside.json").write_text(_anonymous(_pretty(outside.stdout), root))
    _capture_feeds(run, paths, out, root)
    if has_tracks:
        _capture_tracks(run, out, root, paths["orchard"])
    _capture_scheduled_edges(run, out, root, task_id, run_id)
    print(f"snapshot exit {snapshot.returncode}")
    return 0


def _capture_feeds(run, paths: dict, out: Path, root: Path) -> None:
    """The feed of a lane that only built (text and a tool call) and of a lane with every kind of entry. Both lanes are
    made after the snapshot was captured, so the snapshot keeps its two lanes."""
    _feed_lane(paths, "build-only", "Build only")
    _feed_runs(paths, "build-only", only_build=True)
    _write_feed(run, paths, "build-only", out / "engine-real-feed.json", root)
    _feed_lane(paths, "orchard-fix", "Fix the orchard")
    _feed_runs(paths, "orchard-fix", only_build=False)
    _feed_notices(paths, "orchard-fix")
    _feed_board(paths)
    _write_feed(run, paths, "orchard-fix", out / "engine-real-feed-kinds.json", root)


def _write_feed(run, paths: dict, task: str, target: Path, root: Path) -> None:
    done = run("flow", "feed", "--json", "--", task, cwd=paths["orchard"])
    if done.returncode != 0:
        sys.exit(f"feed of {task} failed ({done.returncode}): {done.stderr}{done.stdout}")
    target.write_text(_anonymous(_pretty(done.stdout), root))


FEED_DAY = datetime(2026, 11, 16, 9, 0, tzinfo=timezone.utc)
NOTICE_DAY = datetime(2026, 11, 18, 6, 26, 26, tzinfo=timezone.utc)
NOTICES = (("notify", "notify awaiting_approval: awaiting approval: sakura flow approve orchard-fix spec (read the spec first)"),
           ("operator", "start requested actor=app:me"), ("intake", "pace changed to fast (cmd-1) actor=app:me"),
           ("build", "preset changed to lean (cmd-2) actor=app:me"), ("pace", "plan moves to the account Studio"),
           ("review", "final review round 1: REVISE"), ("plan", "skip: the plan step is off, one task builds the goal"))


def _feed_lane(paths: dict, task: str, title: str) -> None:
    """A lane whose input starts with `title`, which is how the engine names the lane a board post came from."""
    from sakura.flow import store

    lane = store.TaskStore(paths["orchard"], task)
    lane.create({"task": task, "ref": f"specs/{task}.md", "history": [], "approvals": {}, "completed": {},
                 "active_run": None, "overrides": {}, "base_branch": "main", "phase": "build", "status": "running",
                 "reason": "", "title": title})
    lane.input.write_text(f"# {title}\nmore\n")


def _feed_runs(paths: dict, task: str, only_build: bool) -> None:
    """The runs of a lane with the logs of a Claude role, each event stamped as the stream stamps it."""
    from sakura.flow import run_records, store

    lane = store.TaskStore(paths["orchard"], task)
    workspace = paths["orchard"].parent / "workspace"
    workspace.mkdir(exist_ok=True)
    runs = [("build-1", "builder", [_say("Adding the --help flag.", 10), _touch("Edit", str(workspace / "cli.py"), 20),
                                    _shell("pytest -q\necho done", 30)])]
    if not only_build:
        result = 'SAKURA_RESULT {"verdict": "REVISE", "findings": ["missing test", "bad name"]}'
        runs.append(("review-1", "code_reviewer", [_say(f"I reviewed it.\n{result}", 40)]))
    for number, (name, role, events) in enumerate(runs, 1):
        record = {"name": name, "role": role, "provider": "claude", "cwd": str(workspace), "state": "done",
                  "started_at": (FEED_DAY + timedelta(seconds=number)).isoformat()}
        lane.update_state(lambda state, record=record: state.setdefault("runs", {}).update({record["name"]: record}))
        path = run_records.log_path(lane, name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(events))


def _stamp(seconds: int) -> str:
    return (FEED_DAY + timedelta(seconds=seconds)).isoformat()


def _say(text: str, seconds: int) -> str:
    block = {"type": "text", "text": text}
    return json.dumps({"type": "assistant", "timestamp": _stamp(seconds), "message": {"content": [block]}}) + "\n"


def _touch(tool: str, path: str, seconds: int) -> str:
    block = {"type": "tool_use", "name": tool, "input": {"file_path": path}}
    return json.dumps({"type": "assistant", "timestamp": _stamp(seconds), "message": {"content": [block]}}) + "\n"


def _shell(command: str, seconds: int) -> str:
    block = {"type": "tool_use", "name": "Bash", "input": {"command": command}}
    return json.dumps({"type": "assistant", "timestamp": _stamp(seconds), "message": {"content": [block]}}) + "\n"


def _feed_notices(paths: dict, task: str) -> None:
    """The ledger lines of the kinds a lane writes, each with the time the engine records beside the ledger."""
    from sakura.flow import store

    ledger = store.TaskStore(paths["orchard"], task).ledger
    ledger.parent.mkdir(parents=True, exist_ok=True)
    for number, (tag, text) in enumerate(NOTICES):
        offset = ledger.stat().st_size if ledger.exists() else 0
        with ledger.open("a") as stream:
            stream.write(f"- [{tag}] {text}\n")
        with ledger.with_name("ledger.times").open("a") as stream:
            stream.write(f"{offset}\t{(NOTICE_DAY + timedelta(seconds=number)).isoformat()}\n")


def _feed_board(paths: dict) -> None:
    """A post of the master and a post of another lane's builder, on the project's board."""
    from sakura import engine_home
    from sakura.flow import board

    _feed_lane(paths, "second", "Fix the signup")
    folder = engine_home.project_dir(paths["orchard"])
    board.post(folder, board.Author("master", "", "", "s1"), "heads up from the master",
               datetime(2026, 11, 16, 10, 30, tzinfo=timezone.utc))
    board.post(folder, board.Author("role", "second", "builder", "build-1"), "I renamed login()",
               datetime(2026, 11, 16, 11, 0, tzinfo=timezone.utc))


def _action_table() -> dict:
    """The engine's own table of actions: what each takes and which target kinds it applies to, the actions each kind of
    wait and each fixed host target offers, the shapes it accepts for an input and the pace levels. The snapshot lists
    only the actions of the waits and lanes that exist; this covers every kind, so the app's answers to a kind the
    throwaway home has no example of are held to the engine too."""
    from sakura.cockpit import actions, background_actions, decided, host_actions, wait_actions
    from sakura.connectors import kinds
    from sakura.digest import config as digest_config
    from sakura.flow import pace

    def needs(action):
        try:
            return sorted(action.needs(None)) if action.key != "m" else None  # `m` needs depend on the lane's kind
        except (AttributeError, TypeError):
            return None

    return {"actions": {key: {"label": action.label, "kinds": sorted(action.kinds), "needs": needs(action),
                              "optional": sorted(action.optional), "confirm": action.confirm}
                        for key, action in actions.ACTIONS.items()},
            "wait_actions": {kind: list(keys) for kind, keys in wait_actions.ACTION_KEYS.items()},
            "host_targets": {name: sorted(keys) for name, keys in host_actions._ACTIONS_OF.items()},
            "input_patterns": {f"{name}/{kind}": found.pattern for (name, kind), found in actions._PATTERNS.items()},
            "pace_levels": list(pace.LEVELS), "decided_answers": list(decided.ANSWERS), "connector_kinds": list(kinds.KINDS),
            "service_names": list(background_actions.SERVICE_NAMES),
            "digest_inputs": {"at": digest_config.TIME_OF_DAY.pattern, "language": digest_config.LANGUAGE.pattern,
                              "model": digest_config.MODEL_NAME.pattern, "account": digest_config.ACCOUNT_NAME.pattern,
                              "max_lookback_hours": digest_config.MAX_LOOKBACK_HOURS}}


def _lane_steps(repo: Path) -> dict:
    """The engine's own list of a lane's steps (`lane_steps.of_lane`), for a lane in its build with the default flow."""
    from sakura.flow import config, lane_steps
    return {"pipeline": lane_steps.of_lane(config.load(repo).values, {"phase": "build", "history": []})}


def _environment(paths: dict) -> dict:
    wrapper = paths["tools"] / "fixture-usage"
    book = []
    for name, provider, hidden in (("Studio", "claude", False), ("Archive", "codex", False),
                                   ("Spare", "claude", True)):
        account = paths["leeway-home"] / name.lower()
        account.mkdir(mode=0o700)
        book.append({"id": name.lower(), "label": name, "provider": provider,
                     "config_dir": str(account), "hidden": hidden})
    wrapper.write_text(WRAPPER % {"book": book, "readings": READINGS})
    wrapper.chmod(stat.S_IRWXU)
    return {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin", "SAKURA_PYTHON": sys.executable, "HOME": str(paths["home"]),
            "SAKURA_ENGINE_HOME": str(paths["engine"]), "SAKURA_LEEWAY": str(wrapper), "SAKURA_LEEWAY_INSTALLED": "",
            "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1", "LANG": "en_US.UTF-8", "LOGNAME": "fixture"}


def _seed(paths: dict) -> tuple[str, str]:
    repo = _repository(paths["repo"])
    from sakura import keeper
    from sakura.cockpit import decided
    from sakura.flow import registry, store, waits

    registry.remember(repo)
    for task, state in _lanes():
        store.TaskStore(repo, task).create({"task": task, "ref": f"specs/{task}.md", "history": [], "approvals": {},
                                            "completed": {}, "active_run": None, "overrides": {},
                                            "base_branch": "main", **state})
    ask = waits.Wait(json.dumps(["wait", str(repo), "ship-docs", "approval", "spec@1a2b"], separators=(",", ":")),
                     "approval", str(repo), "orchard", "ship-docs", "Approve the spec", "", None)
    decided.record(decided.default_state_dir(), wait=decided.wait_fields(ask), answer="approve", choice=None,
                   text=None, actor="cli:owner@example.com", at=NOW - timedelta(hours=2))
    _keeper(keeper, repo)
    feed_id = _connectors_and_digest()
    _automations(paths)
    return _scheduled(feed_id)


def _repository(path: Path) -> Path:
    named = path.parent / "orchard"
    path.rename(named)
    git = ["git", "-C", str(named), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.com"]
    subprocess.run([*git, "init", "-q", "-b", "main"], check=True)
    (named / "README").write_text("fixture\n")
    subprocess.run([*git, "add", "README"], check=True)
    subprocess.run([*git, "commit", "-q", "-m", "first"], check=True)
    return named


def _lanes() -> list[tuple[str, dict]]:
    return [("add-search", {"phase": "build", "status": "running", "reason": "", "title": "Add search to the orchard"}),
            ("fix-login", {"phase": "spec", "status": "awaiting_approval", "reason": "awaiting approval: spec",
                           "title": "Fix the login form"})]


def _keeper(keeper, repo: Path) -> None:
    """A keeper that looked after a restart: the real look, over a lane that no process drives."""
    ports = keeper.Ports(repos=lambda: [repo], is_alive=lambda run: False, resume=lambda repo, task: None,
                         lanes=lambda: 4)
    keeper._begin(datetime.now(timezone.utc))
    keeper.run_once(datetime.now(timezone.utc), ports)


def _connectors_and_digest() -> str:
    from types import SimpleNamespace

    from sakura.automations import registry
    from sakura.connectors import kinds, registry as connectors
    from sakura.digest import config

    def inspect(_kind, selectors, _timeout):
        resource = selectors["team"]
        return kinds.Access("fixture", resource, resource, ("read",))

    ports = kinds.Ports(inspect=inspect, which=lambda _: "fixture", run=lambda *_: SimpleNamespace(exit_code=0))
    github = connectors.connect("github", "GitHub", "", ports, NOW)
    connectors.mark_used(github.id, NOW - timedelta(hours=1), revision=github.revision, is_ok=True)
    linear = connectors.mutate("add", "linear", ports, NOW, fields={
        "kind": "linear", "label": "Linear", "team": "ORCHARD"})
    connectors.mutate("revoke", "disconnect-linear", ports, NOW, connector_id=linear["id"], revision=linear["revision"])
    feed = connectors.mutate("add", "news", ports, NOW, fields={
        "kind": "feed", "label": "News", "address": "https://example.com/feed.xml"})
    settings = config.DigestConfig("07:00", "sonnet", "en", 24, (
        config.Source("github", connector=github.id), config.Source("news", feed="https://example.com/feed.xml"),
        config.Source("local-notes", run=("/usr/bin/true",), enabled=False)))
    config.save(settings)
    registry.note_morning_digest(settings, is_installed=True)
    return feed["id"]


def _custom_connector(out: Path, tools: Path) -> None:
    from sakura.connectors import kinds, registry

    program = tools / "read-notes"
    program.write_text("#!/bin/sh\nexit 0\n")
    program.chmod(stat.S_IRWXU)
    receipt = registry.mutate("add", "custom", kinds.Ports(), NOW, fields={
        "kind": "program", "label": "Local program", "program": str(program), "reads": "Reads local notes"})
    (out / "engine-real-connector-receipt.json").write_text(_pretty(json.dumps(receipt)))


def _scheduled(feed_id: str) -> tuple[str, str]:
    from sakura.connectors import events, registry
    from sakura.scheduled import dispatch, store

    tasks = store.Store()
    definition = {"title": "Orchard news", "prompt": "Summarize new entries.", "connector_ids": [feed_id]}
    task = tasks.create({**definition, "when": {"kind": "connector_event", "connector_id": feed_id,
                                                "event_kind": events.ENTRY_ADDED}}, "on-news", NOW)
    tasks.create({**definition, "title": "Hourly news", "when": {"kind": "interval", "minutes": 60}}, "hourly", NOW)
    revision = events.revision(registry.find(feed_id))
    event = events.ConnectorEvent(feed_id, revision, events.ENTRY_ADDED, "orchard-news", "entry-1",
                                  store.stamp(NOW), {})
    dispatch.admit_events(tasks, events.EventBatch(feed_id, revision, events.ENTRY_ADDED, None, "baseline", ()), NOW)
    run_id, = dispatch.admit_events(tasks, events.EventBatch(feed_id, revision, events.ENTRY_ADDED, "baseline", "next",
                                                           (event,)), NOW)
    dispatch.run_now(tasks, task.id, task.revision, "read-news", NOW)
    # Seed a completed transcript without launching an agent or a scheduler service.
    with tasks.connect(write=True) as db:
        db.execute("UPDATE runs SET state='finished', started_at=?, ended_at=? WHERE id=?",
                   (store.stamp(NOW), store.stamp(NOW), run_id))
        db.execute("INSERT INTO conversation (run_id,kind,text) VALUES (?,?,?)", (run_id, "prompt", definition["prompt"]))
    return task.id, run_id


def _capture_goose(run, out: Path, root: Path) -> None:
    home = root / "goose-home"
    engine = root / "goose-engine"
    home.mkdir(mode=0o700)
    engine.mkdir(mode=0o700)
    overrides = {"HOME": str(home), "SAKURA_ENGINE_HOME": str(engine)}
    with patch.dict(os.environ, overrides):
        _capture_goose_outputs(lambda *words: run(*words, override=overrides), out, root, home)


def _capture_goose_outputs(run, out: Path, root: Path, home: Path) -> None:
    from sakura.accounts import registry
    from sakura.flow import model_catalog

    def capture(name, *words):
        result = run(*words)
        if result.returncode:
            sys.exit(f"{name} failed ({result.returncode}): {result.stderr}{result.stdout}")
        (out / f"engine-real-goose-{name}.json").write_text(_anonymous(_pretty(result.stdout), root))
        return json.loads(result.stdout)

    folder = home / ".goose-local"
    (folder / "config").mkdir(mode=0o700, parents=True)
    (folder / "config" / "config.yaml").write_text("GOOSE_PROVIDER: ollama\n")
    capture("setup", "setup", "detect", "--json")
    choices = {"accounts": [{"harness": "goose", "folder": str(folder), "name": "Local models",
                            "provider_config": {"provider": "ollama"}}],
               "master_account": {"harness": "goose", "folder": str(folder)}}
    applied = capture("apply", "cockpit", "act", "setupapply", '["host","engine"]',
                      "--op", "goose-fixture", "--choices", json.dumps(choices), "--json")
    reference = json.loads(applied["output"])["master_account"]
    account = next(row for row in registry.load().accounts if row.id == reference["id"])
    _seed_goose_catalog(model_catalog, account)
    capture("snapshot", "cockpit", "snapshot", "--json")
    capture("rules", "engine", "rules", "--json")
    capture("catalog", "engine", "models", "--json")
    capture("master-spec", "master", "spec", "--json", "--harness", "goose",
            "--account", reference["id"], "--slot", "fixture-tab")
    for target in out.glob("engine-real-goose-*.json"):
        target.write_text(target.read_text().replace(reference["id"], "goose-fixture"))


def _seed_goose_catalog(model_catalog, account) -> None:
    model = model_catalog.ModelOption("qwen2.5:3b", "Qwen 2.5 (Ollama)", is_default=True)
    catalog = model_catalog.Catalog("goose", account.id, NOW.isoformat(), "ACP session", (model,), "ollama")
    target = model_catalog.path(account)
    target.parent.mkdir(exist_ok=True)
    backing = model_catalog._backing(account)
    model_catalog._keep(catalog, target, backing)
    model_catalog._mark(account, target, model_catalog.FAILED, "adapter_timeout", backing=backing)


def _capture_tracks(run, out: Path, root: Path, repo: Path) -> None:
    from sakura import engine_home
    from sakura.actors import Actor
    from sakura.flow import registry
    from sakura.tracks import TrackService
    from sakura.tracks.domain import ConversationKey
    from sakura.tracks.starts import LaunchInputs, TrackStarts

    person = Actor("app", "fixture")
    service = TrackService(clock=lambda: NOW)
    active = service.create("Release", "Ship the beta", actor=person, op_id="new-release")
    paused = service.create("Research", "Compare options", actor=person, op_id="new-research")
    closed = service.create("Previous release", "Ship version one", actor=person, op_id="new-previous")
    service.update(paused.id, status="paused", actor=person, op_id="pause")
    service.update(closed.id, status="closed", actor=person, op_id="close")
    service.add_decision(active.id, "Enable the new onboarding?", actor=person, op_id="question")
    settled = service.add_decision(active.id, "Use the beta channel?", actor=person, op_id="beta-question")
    service.settle_decision(active.id, settled.number, "Yes, on the beta channel.",
                            actor=Actor("overlay", "fixture", "phone"), op_id="answer")
    service.link_conversation(active.id, ConversationKey("claude", "studio", "fixture-session"),
                              "Beta planning", actor=person, op_id="conversation")
    service.link_lane(active.id, repo, "add-search", actor=person, op_id="working-link")
    gone = _track_lane(repo, "retired", "Retired lane")
    service.link_lane(active.id, repo, "retired", actor=person, op_id="gone-link")
    gone.root.rename(gone.root.parent / ".retired")
    project = engine_home.project_id(repo)
    reserved = service.reserve_start(active.id, project, LaunchInputs("Verify beta builds"), person, "start")
    TrackStarts(service).execute(project, reserved.task, person, lambda _: (1, ""))

    missing_path = root / "missing"
    missing_path.mkdir()
    subprocess.run(["git", "-C", str(missing_path), "init", "-q", "-b", "main"], check=True)
    registry.remember(missing_path)
    _track_lane(missing_path, "saved", "Saved lane")
    service.link_lane(paused.id, missing_path, "saved", actor=person, op_id="missing-link")
    missing_path.rename(root / "moved")
    _write_track_snapshot(run, out, root, "engine-real-tracks.json")
    (engine_home.root() / "tracks" / "tracks.db").write_bytes(b"unreadable fixture")
    _write_track_snapshot(run, out, root, "engine-real-tracks-unavailable.json")


def _track_lane(repo: Path, task: str, title: str):
    from sakura.flow import store

    lane = store.TaskStore(repo, task)
    lane.create({"task": task, "title": title, "phase": "build", "status": "running",
                 "history": [], "approvals": {}, "completed": {}, "active_run": None,
                 "overrides": {}, "base_branch": "main"})
    return lane


def _write_track_snapshot(run, out: Path, root: Path, name: str) -> None:
    snapshot = run("cockpit", "snapshot", "--json")
    snapshot.check_returncode()
    fields = ("api", "tracks", "track_problem", "track_actions", "waits", "decided", "projects", "items")
    document = {key: value for key, value in json.loads(snapshot.stdout).items() if key in fields}
    (out / name).write_text(_anonymous(_pretty(json.dumps(document)), root))


def _capture_scheduled_edges(run, out: Path, root: Path, task_id: str, run_id: str) -> None:
    from sakura.connectors import registry
    from sakura.digest import schema
    from sakura.scheduled import findings, store

    tasks = store.Store()
    with tasks.connect(write=True) as db:
        db.execute("UPDATE runs SET state='running' WHERE id=?", (run_id,))
        item = schema.parse_item({"id": "entry-1", "title": "Orchard release", "url": "https://example.com/release"})
        evidence = findings.remember(db, run_id, "orchard", "News", item)
    findings.stage(tasks, run_id, "Review the release", "A new release was announced.", source_ref=evidence)
    with tasks.connect(write=True) as db:
        findings.publish(db, run_id)
        db.execute("UPDATE runs SET state='finished' WHERE id=?", (run_id,))
    snapshot = run("cockpit", "snapshot", "--json")
    snapshot.check_returncode()
    finding, = [row for row in json.loads(snapshot.stdout)["waits"] if row["source"] == "scheduled"]
    (out / "engine-real-scheduled-finding.json").write_text(_anonymous(_pretty(json.dumps(finding)), root))

    registry.path().write_text("invalid")
    broken = run("cockpit", "snapshot", "--json")
    broken.check_returncode()
    snapshot = json.loads(broken.stdout)
    expired = run("scheduled", "conversation", "--json", "--", "expired-run")
    tasks.path.write_bytes(b"invalid database")
    history = run("scheduled", "history", "--json", "--", task_id)
    conversation = run("scheduled", "conversation", "--json", "--", run_id)
    errors = {"snapshot": {key: snapshot[key] for key in ("api", "scheduled", "connectors_problem")},
              "history": json.loads(history.stdout), "conversation": json.loads(conversation.stdout),
              "expired": json.loads(expired.stdout)}
    (out / "engine-real-scheduled-problems.json").write_text(_pretty(json.dumps(errors)))


def _outside_lane_and_digest_day(repo: Path) -> None:
    """An outside lane with a titled link, a row of one step and a question, and a day's morning digest with two
    news, a check with a titled link and a decision, so the second snapshot holds a `digest` item with its `reading`,
    waits with `links` and a one-step row as the sentence the engine sends. Made after the first snapshot was
    captured, so that one keeps its two lanes."""
    from sakura import questions
    from sakura.digest import publish, schema
    from sakura.outside import records, store

    root, agent, lane = store.path(), records.Agent("claude", repo), "measure-help"
    store.open_lane(root, agent, lane, title="Add --help", now=NOW,
                    links=(("PR 12", "https://github.com/example/orchard/pull/12"),))
    store.put_row(root, agent, lane, "help-prints", title="Help prints", steps=("Run measure.py --help",),
                  expected="usage: measure", now=NOW)
    store.ask(root, agent, lane, questions.make("flag-short", "Also accept -h?", ("yes", "no"), NOW), NOW)
    pr = schema.Link("PR 12", "https://github.com/example/orchard/pull/12")
    output = schema.ModelOutput(
        (schema.News("github", "Release", "Version 2 shipped.", (pr,)),
         schema.News("news", "Outage", "A short outage on Monday.", ())),
        (schema.Check("check-pr", "Check PR 12", "CI is red on the merge.", (pr,)),),
        (schema.Decision("ship-today", "Ship today?", ("yes", "no"), (pr,)),))
    day = NOW.strftime("%Y-%m-%d")
    window = schema.Window((NOW - timedelta(hours=24)).isoformat(), NOW.isoformat())
    digest = schema.Digest(day, NOW.isoformat(), window, "sonnet", f"digest-{day}",
                           (schema.SourceReport("github", True, 3, 0),), output)

    class Pusher:
        def push(self, event, title, message):
            return "sent"

    publish.publish(digest, "en", NOW, pusher=Pusher())


def _automations(paths: dict) -> None:
    from sakura.automations import registry

    found = registry.validate("weekly-wake", "Wake the lanes every morning", "daily 06:30", "flow wake")
    registry._write(registry.folder() / "weekly-wake.toml", dataclasses.replace(found, enabled=True, installed=True))
    registry._write(registry.folder() / "hourly-wake.toml", registry.validate(
        "hourly-wake", "Look at the lanes", "every 60 minutes", "flow wake"))


def _pretty(text: str) -> str:
    return json.dumps(json.loads(text), indent=2, sort_keys=True) + "\n"


def _anonymous(text: str, root: Path) -> str:
    """The throwaway folder's name is replaced by a plain one, so no path of this machine is kept."""
    return text.replace(str(root), "/work").replace(str(root).replace("/private", ""), "/work")


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

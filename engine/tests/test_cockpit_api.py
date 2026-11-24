import json
import re
import threading
import time
from dataclasses import replace
from datetime import UTC, datetime, timedelta, timezone
from pathlib import Path

import pytest
from cockpit_support import OWN_DNS_NAME
from sakura.cockpit import api, gate, pages, proofs, remote, server
from sakura.cockpit.controller import Controller
from sakura.cockpit.items import GroupRow
from sakura.cockpit.settings import CockpitSettings
from sakura.flow import progress
from sakura.usage import aggregate
from sakura.usage.model import Usage
from test_cockpit_actions import Spawns, flow_item, lane_item, log_rows, wait_target
from test_cockpit_server import (
    LOOPBACK,
    SECRET,
    SECURITY_HEADERS,
    MemoryServer,
    _exchange,
    _get,
    _post,
)
from test_cockpit_snapshot import make_snapshot

OWNER = gate.Identity("owner@example.com", "nMAC", "mac")
REPO = Path("/work/repo")
ESC = "\x1b"
DOC = Path(__file__).resolve().parents[1] / "docs" / "cockpit-api.md"
ACTOR_OPTION = "--actor=overlay:owner@example.com:nMAC"


class Engine:
    """A running cockpit server wired to a Controller over a fixed snapshot and a recording spawner."""

    def __init__(self, tmp_path, *, own_node_id="", allow_nodes=(OWNER.node_id,), spawns=None, clock=time.time):
        self.flow, self.lane = flow_item(REPO, "t1"), lane_item(REPO, "proc", "auth")
        self.group = GroupRow("proc", REPO, "proc", 1, "", "")
        self.snapshot = make_snapshot(items=[self.flow, self.lane], groups=[self.group])
        self.spawns = spawns or Spawns(stdout="engine said ok\n")
        self.fresh_reads = self.perform_calls = 0
        self.who = OWNER
        self.log = tmp_path / "actions.jsonl"
        self.controller = Controller(b"s" * 32, lambda: self.snapshot, self._fresh, run=self.spawns, log_path=self.log, clock=clock)
        settings = CockpitSettings(0, frozenset({OWNER.login}), frozenset(allow_nodes), tmp_path / "tailscale")
        self.server = MemoryServer(gate.Self(LOOPBACK, OWN_DNS_NAME, own_node_id), settings, self.controller.index,
                                     self.controller.phone_act, identify=lambda _ip: self.who,
                                     access_log=tmp_path / "access.log", csrf_secret=SECRET,
                                     api=api.Api(self.controller), action_log=self.log, clock=clock)
        self.port = self.server.server_address[1]
        self.host = f"{LOOPBACK}:{self.port}"

    def _fresh(self):
        self.fresh_reads += 1
        return self.snapshot

    def target(self, item):
        return self.controller.remote_ids.register(item.key)


@pytest.fixture
def engine(tmp_path):
    running = Engine(tmp_path)
    yield running
    running.server.server_close()


PHONE = gate.Identity(OWNER.login, "nPHONE", "phone")


@pytest.fixture
def hosted(tmp_path):
    """A cockpit whose own overlay node is the person's Mac (nMAC); the phone is allowed too."""
    running = Engine(tmp_path, own_node_id=OWNER.node_id, allow_nodes=(OWNER.node_id, PHONE.node_id))
    yield running
    running.server.server_close()


def get_json(engine, path):
    status, headers, body = _get(engine, path)
    return status, headers, json.loads(body)


def post_action(engine, fields, **kwargs):
    status, headers, body = _post(engine, fields, path="/api/action", **kwargs)
    return status, headers, json.loads(body)


def action_fields(engine, item, key, confirm=None, **inputs):
    fields = {"key": key, "target": engine.target(item)}
    fields.update({f"input.{name}": value for name, value in inputs.items()})
    if confirm:
        fields["confirm"] = confirm
    return fields


# -- the gate -----------------------------------------------------------------------------------

ROUTES = [("GET", "/api/session"), ("GET", "/api/snapshot"), ("POST", "/api/action")]


def request(engine, method, path, *, host=None, origin=None, csrf=True):
    if method == "GET":
        return _get(engine, path, host=host)
    return _post(engine, {"key": "D", "target": engine.target(engine.flow), "input.step": "power"}, path=path,
                 host=host, origin=origin, csrf=csrf)


READ_ATTACKS = ["no identity", "other node", "wrong host"]
WRITE_ATTACKS = [*READ_ATTACKS, "foreign origin", "null origin", "missing csrf", "csrf of another node"]
GATE_CASES = [(method, path, attack) for method, path in ROUTES
              for attack in (WRITE_ATTACKS if method == "POST" else READ_ATTACKS)]


@pytest.mark.parametrize(("method", "path", "attack"), GATE_CASES)
def test_api_requires_the_gate(engine, monkeypatch, method, path, attack):
    calls = []
    real = Controller.perform
    monkeypatch.setattr(Controller, "perform", lambda self, *a, **k: calls.append(1) or real(self, *a, **k))
    host = origin = None
    csrf = True
    if attack == "no identity":
        engine.who = None
    elif attack == "other node":
        engine.who = gate.Identity(OWNER.login, "nPHONE", "phone")
    elif attack == "wrong host":
        host = "evil.example"
    elif attack == "foreign origin":
        origin = "http://evil.example"
    elif attack == "null origin":
        origin = "null"
    elif attack == "missing csrf":
        csrf = False
    else:
        csrf = gate.csrf_token(SECRET, gate.Identity(OWNER.login, "nPHONE", "phone"), time.time())

    status, _headers, body = request(engine, method, path, host=host, origin=origin, csrf=csrf)

    assert status == 403 and body == b"forbidden\n"
    assert calls == [] and engine.spawns.calls == []


# -- session and snapshot -----------------------------------------------------------------------

def test_session_returns_identity_and_csrf(engine):
    status, _headers, document = get_json(engine, "/api/session")

    assert status == 200 and document["api"] == 2
    assert document["identity"] == {"login": "owner@example.com", "node_id": "nMAC", "device": "mac"}
    assert gate.csrf_valid(SECRET, document["csrf"], OWNER, time.time())
    status, _headers, answered = post_action(engine, action_fields(engine, engine.flow, "D", step="power"),
                                             csrf=document["csrf"])
    assert status == 200 and answered["result"] == "done"


def hostile_snapshot():
    item = flow_item(REPO, "t1")
    item = replace(item, branch=f"feat/{ESC}]2;pwned\x07x", last_commit=f"{ESC}[2Jabc",
                   models={f"{ESC}[1mbuilder": f"claude:{ESC}[1mopus"}, pending=[f"steer{ESC}[H (3 chars)"],
                   usage=Usage(1, 2, 3, 4, None), next_command="cmd\x00 x")
    now = datetime(2026, 10, 26, 9, 0, tzinfo=timezone(timedelta(hours=2)))
    deferred = [{"at": "2026-10-26T01:00:00+02:00", "task": "demo", "event": "awaiting_approval",
                 "message": f"gate{ESC}]2;x\x07 open"}]
    events = [{"source": "flow", "scope": "repo/t1", "at": "", "text": f"{ESC}[31mred{ESC}[0m line"},
              {"source": "lane", "scope": "proc/auth", "at": "2026-10-26T08:00:00Z", "text": "open done"}]
    return make_snapshot(items=[item, lane_item(REPO, "proc", "auth")], groups=[GroupRow("proc", REPO, "proc", 1, "", "")],
                         usage=aggregate.WeekUsage(now, ()), deferred=deferred, events=events)


def test_snapshot_json_shape_and_strips_hostile_values(engine):
    engine.snapshot = hostile_snapshot()
    status, _headers, raw = _get(engine, "/api/snapshot")
    document = json.loads(raw)

    assert status == 200 and document["api"] == 2
    assert {"mode", "deferred", "load", "usage", "groups", "items", "events"} <= set(document)
    assert ESC.encode() not in raw and b"\\u001b" not in raw and b"\\u0007" not in raw and b"\\u0000" not in raw
    flow = next(item for item in document["items"] if item["kind"] == "flow")
    assert flow["branch"] == "feat/x" and flow["last_commit"] == "abc" and flow["pending"] == []
    assert flow["models"] == {"builder": "claude:opus"} and flow["usage"]["total"] == 10
    assert "key" not in flow and flow["repo"] == REPO.name
    assert document["deferred"][0]["at"] == "2026-10-25T23:00:00Z" and document["deferred"][0]["message"] == "gate open"
    assert document["usage"]["now"] == "2026-10-26T07:00:00Z"
    assert document["events"] == []


def test_snapshot_lists_the_decided_asks_as_time_stamped_clean_rows(engine):
    entry = {"id": '["wait","/work/repo","t1","approval","spec@r1"]', "key": ["wait", "/work/repo", "t1", "approval", "spec@r1"],
             "kind": "approval", "project": "repo", "task": "t1", "title": f"Approve{ESC}[31m the spec", "answer": "approve",
             "choice": None, "text": f"go{ESC}]0;x\x07 ahead", "actor": "overlay:owner@example.com:nMAC",
             "at": "2026-11-18T12:00:00+00:00"}
    engine.snapshot = make_snapshot(items=[engine.flow], decided=[entry])
    status, _headers, raw = _get(engine, "/api/snapshot")
    [shown] = json.loads(raw)["decided"]

    assert status == 200 and ESC.encode() not in raw and b"\\u0007" not in raw
    assert shown["at"] == "2026-11-18T12:00:00Z" and shown["title"] == "Approve the spec" and shown["text"] == "go ahead"
    assert {"id", "kind", "project", "task", "answer", "choice", "actor"} <= set(shown) and "key" not in shown


def test_snapshot_without_decided_asks_lists_none(engine):
    assert get_json(engine, "/api/snapshot")[2]["decided"] == []


def test_api_doc_describes_the_decided_asks():
    doc = DOC.read_text()
    for word in ("decided", "mark_ko", "decided.jsonl"):
        assert word in doc, word


def test_every_item_and_group_has_a_stable_id_both_faces_accept_as_the_target(engine):
    first = get_json(engine, "/api/snapshot")[2]
    second = get_json(engine, "/api/snapshot")[2]
    rows = [*first["items"], *first["groups"]]

    assert all(isinstance(row["id"], str) for row in rows) and len({row["id"] for row in rows}) == len(rows)
    assert [row["id"] for row in rows] == [row["id"] for row in [*second["items"], *second["groups"]]]
    for row in rows:
        assert row["id"].startswith("t1.") and "key" not in row
    assert first["items"][0]["id"] == engine.target(engine.flow)


def test_snapshot_lists_only_applicable_actions(engine):
    document = get_json(engine, "/api/snapshot")[2]
    flow, lane = document["items"]
    (group,) = document["groups"]

    def keys(row):
        return {action["key"] for action in row["actions"]}

    assert keys(flow) == {"m", "D", "R", "s", "w", "q"}
    assert keys(lane) == {"m", "w"}
    assert keys(group) == set()
    by_key = {action["key"]: action for action in flow["actions"]}
    assert by_key["m"] == {"key": "m", "label": "Change model", "needs": ["role", "model"], "optional": [],
                           "confirm": False}
    assert by_key["s"]["confirm"] is True and by_key["R"] == {
        "key": "R", "label": "Resume", "needs": [], "optional": ["note"], "confirm": True} and by_key["D"]["needs"] == ["step"]
    assert {a["key"]: a["needs"] for a in lane["actions"]}["m"] == ["model"]
    assert group["actions"] == []


def test_snapshot_lists_waits_with_their_actions_and_every_flow_item_with_its_progress(engine):
    approval, owner_row = wait_target("approval", "checks").wait, wait_target("owner_row", "R1").wait
    engine.snapshot = make_snapshot(items=[replace(engine.flow, progress=progress.Progress(3, 7, "build", 3, 5, 0, 2))],
                                    waits=[approval, owner_row])

    document = get_json(engine, "/api/snapshot")[2]

    assert document["items"][0]["progress"] == {
        "step": "build", "phase": "Build", "done": 3, "total": 5, "unit": "tasks", "text": "Build, 3 of 5 tasks",
        "engine_phase": "build", "tasks_done": 3, "tasks_total": 5, "checks_done": 0, "checks_total": 2,
        "overall_done": 3, "overall_total": 7}
    assert [wait["kind"] for wait in document["waits"]] == ["approval", "approval"]
    assert [wait["subkind"] for wait in document["waits"]] == ["approval", "owner_row"]
    first, second = document["waits"]
    assert first["id"].startswith("t1.") and "key" not in first
    assert {"project", "task", "title", "detail", "since", "repo"} <= set(first)
    for row in (first, second):
        assert [action["key"] for action in row["actions"]] == ["read", "unread", "snooze"]



# -- actions ------------------------------------------------------------------------------------

HOST_WRITES = [("/api/action", False), ("/api/action", True), ("/action", False), ("/action", True)]


@pytest.mark.parametrize(("path", "confirmed"), HOST_WRITES)
def test_a_write_from_the_hosts_own_node_never_reaches_the_controller(hosted, monkeypatch, path, confirmed):
    performed = []
    monkeypatch.setattr(Controller, "perform", lambda self, *a, **k: performed.append(1))
    fields = action_fields(hosted, hosted.flow, "s")
    if confirmed:
        fields["confirm" if path == "/api/action" else "proof"] = proofs.issue(
            b"s" * 32, OWNER, "s", hosted.flow.key, {}, source="overlay")

    status, _headers, body = _post(hosted, fields, path=path)

    assert status == 200 and performed == [] and hosted.spawns.calls == [] and hosted.fresh_reads == 0
    reason = "writes from the cockpit's own machine go through sakura cockpit act"
    if path == "/api/action":
        assert json.loads(body) == {"result": "refused", "reason": reason}
    else:
        assert pages.text(reason).encode() in body
    row = log_rows(hosted.log)[-1]
    assert (row["outcome"], row["key"], row["actor"]["node_id"]) == ("refused", "s", "nMAC")
    assert "argv" not in row


class BlockingSpawns(Spawns):
    """A runner that holds every command until `release` is set, so a test can fill the action budget."""

    def __init__(self):
        super().__init__(stdout="engine said ok\n")
        self.release = threading.Event()
        self.started = threading.Semaphore(0)

    def __call__(self, argv, **kwargs):
        self.started.release()
        assert self.release.wait(20)
        return super().__call__(argv, **kwargs)


def test_a_third_action_is_refused_at_once_while_two_run_and_reads_still_answer(tmp_path):
    spawns = BlockingSpawns()
    busy = Engine(tmp_path, spawns=spawns)
    fields = action_fields(busy, busy.flow, "D", step="strong")
    answers = []
    running = [threading.Thread(target=lambda: answers.append(post_action(busy, fields)), daemon=True)
               for _ in range(server.MAX_RUNNING_ACTIONS)]
    try:
        for thread in running:
            thread.start()
        for _ in running:
            assert spawns.started.acquire(timeout=10)

        status, _headers, third = post_action(busy, fields)
        html_status, _headers, html_body = _post(busy, fields, path="/action")
        snapshot_status = get_json(busy, "/api/snapshot")[0]

        assert (status, third) == (200, {"result": "refused", "reason": "another action is running"})
        assert html_status == 503 and b"another action is running" in html_body
        assert snapshot_status == 200 and len(spawns.calls) == 0 and not spawns.started.acquire(blocking=False)
        assert log_rows(busy.log)[-1]["outcome"] == "refused"
    finally:
        spawns.release.set()
        for thread in running:
            thread.join(10)
        busy.server.server_close()
    assert [answer[2]["result"] for answer in answers] == ["done", "done"] and len(spawns.calls) == 2


def test_the_action_budget_is_released_when_an_action_ends(engine):
    for _ in range(server.MAX_RUNNING_ACTIONS + 2):
        assert post_action(engine, action_fields(engine, engine.flow, "D", step="strong"))[2]["result"] == "done"


def test_reads_from_the_hosts_own_node_stay_allowed(hosted):
    assert [_get(hosted, path)[0] for path in ("/", "/api/session", "/api/snapshot")] == [200, 200, 200]


def test_another_allowed_node_can_still_write(hosted):
    hosted.who = PHONE
    token = gate.csrf_token(SECRET, PHONE, time.time())
    status, _headers, body = _post(hosted, action_fields(hosted, hosted.flow, "D", step="strong"),
                                   path="/api/action", csrf=token)
    assert status == 200 and json.loads(body)["result"] == "done" and len(hosted.spawns.calls) == 1

def test_api_action_confirm_then_proof_runs_once(engine):
    instant = datetime(2026, 11, 22, 12, tzinfo=UTC)
    engine.server.clock = engine.controller._clock = instant.timestamp
    fields = action_fields(engine, engine.flow, "s")
    status, _headers, asked = post_action(engine, fields)

    assert status == 200 and asked["result"] == "confirm" and engine.spawns.calls == []
    expires = datetime.strptime(asked["expires"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=UTC)
    assert expires == instant + timedelta(seconds=120)
    assert asked["summary"].startswith("Stop: ") and "command" not in asked

    status, _headers, done = post_action(engine, {**fields, "confirm": asked["token"]})
    assert status == 200 and done == {"result": "done", "exit_code": 0, "output": ""}
    (argv, kwargs), = engine.spawns.calls
    assert ACTOR_OPTION in argv and kwargs["cwd"] == REPO

    status, _headers, replay = post_action(engine, {**fields, "confirm": asked["token"]})
    assert status == 400 and replay["result"] == "refused" and replay["reason"] == remote.REFUSED
    assert len(engine.spawns.calls) == 1


def test_api_confirm_of_a_lane_action_names_the_lane_by_its_title_never_its_slug_folder_or_command(engine):
    engine.flow = replace(engine.flow, title="Count the lines")
    engine.snapshot = make_snapshot(items=[engine.flow, engine.lane], groups=[engine.group])

    status, _headers, asked = post_action(engine, action_fields(engine, engine.flow, "s"))

    assert status == 200 and asked["summary"] == "Stop: Count the lines"
    assert "t1" not in asked["summary"] and str(REPO) not in asked["summary"] and "command" not in asked


def stop_flow(engine):
    engine.flow = replace(engine.flow, agent_state="waiting", next_command="sakura flow resume t1")
    engine.snapshot = make_snapshot(items=[engine.flow, engine.lane], groups=[engine.group])


def test_api_resume_asks_first_runs_once_and_refuses_a_replay(engine):
    stop_flow(engine)
    fields = action_fields(engine, engine.flow, "R")
    status, _headers, asked = post_action(engine, fields)
    assert status == 200 and asked["result"] == "confirm" and engine.spawns.calls == []
    assert asked["summary"].startswith("Resume: ") and "command" not in asked

    status, _headers, done = post_action(engine, {**fields, "confirm": asked["token"]})
    assert status == 200 and done == {"result": "done", "exit_code": 0, "output": ""}
    (argv, _), = engine.spawns.calls
    assert argv[1:4] == ["flow", "resume", "--detach"]

    status, _headers, replay = post_action(engine, {**fields, "confirm": asked["token"]})
    assert status == 400 and replay["result"] == "refused" and len(engine.spawns.calls) == 1


def test_api_resume_of_a_working_flow_is_refused_before_a_token_is_issued(engine):
    stop_flow(engine)
    engine.flow = replace(engine.flow, agent_state="working", next_command="sakura flow attach t1")
    engine.snapshot = make_snapshot(items=[engine.flow, engine.lane], groups=[engine.group])

    status, _headers, answer = post_action(engine, action_fields(engine, engine.flow, "R"))

    assert status == 200 and answer["result"] == "refused" and answer["reason"] == remote.REFUSED
    assert engine.spawns.calls == [] and log_rows(engine.log)[-1]["outcome"] == "refused"


def test_api_resume_that_finds_the_engine_busy_is_a_refusal_not_a_done(tmp_path):
    busy = Spawns(stdout="engine already running (pid 4242)\n", returncode=6)
    running = Engine(tmp_path, spawns=busy)
    try:
        stop_flow(running)
        fields = action_fields(running, running.flow, "R")
        token = post_action(running, fields)[2]["token"]
        status, _headers, answer = post_action(running, {**fields, "confirm": token})
        assert status == 200 and answer == {"result": "refused", "reason": remote.REFUSED}
    finally:
        running.server.server_close()


def test_api_action_refuses_a_command_line_token(engine):
    token = proofs.issue(b"s" * 32, OWNER, "s", engine.flow.key, {}, source="cli")
    status, _headers, answer = post_action(engine, action_fields(engine, engine.flow, "s", confirm=token))

    assert status == 400 and answer["result"] == "refused" and answer["reason"] == remote.REFUSED
    assert engine.spawns.calls == []


def test_api_action_runs_a_plain_action_in_one_step(engine):
    status, _headers, done = post_action(engine, action_fields(engine, engine.flow, "D", step="strong"))
    assert status == 200 and done["result"] == "done" and done["exit_code"] == 0
    assert engine.spawns.calls[0][0][-2:] == ["t1", "strong"]
    assert log_rows(engine.log)[-1]["actor"]["node_id"] == "nMAC"


def test_api_action_is_done_with_an_audit_note_when_the_log_cannot_be_written_after_the_run(engine, capsys):
    engine.log.mkdir()
    status, _headers, done = post_action(engine, action_fields(engine, engine.flow, "w", text="steer it"))

    assert status == 200 and len(engine.spawns.calls) == 1
    assert done["result"] == "done" and done["exit_code"] == 0 and done["output"] == ""
    assert done["audit"] == remote.AUDIT_FAILED
    assert "cannot write" in capsys.readouterr().err


def test_html_action_shows_done_when_the_log_cannot_be_written_after_the_run(engine):
    engine.log.mkdir()
    status, _headers, body = _post(engine, action_fields(engine, engine.flow, "w", text="steer it"), path="/action")
    assert status == 200 and b"done: exit code 0" in body and remote.AUDIT_FAILED.encode() in body
    assert len(engine.spawns.calls) == 1


REFUSALS = [
    (lambda e: {"key": "x", "target": e.target(e.flow)}, 200, remote.REFUSED),
    (lambda e: action_fields(e, e.lane, "R"), 200, remote.REFUSED),
    (lambda e: action_fields(e, e.flow, "D", step="../strong"), 200, remote.REFUSED),
    (lambda e: action_fields(e, e.flow, "D"), 200, remote.REFUSED),
    (lambda e: action_fields(e, e.flow, "D", step="power", extra="x"), 200, remote.REFUSED),
    (lambda e: {"key": "D", "target": json.dumps(["flow", "/etc", "t1"]), "input.step": "power"}, 400, remote.REFUSED),
    (lambda e: {"key": "D", "input.step": "power"}, 400, remote.REFUSED),
    (lambda e: {"key": "D", "target": "not json", "input.step": "power"}, 400, remote.REFUSED),
    (lambda e: {"key": "D", "target": "[" * 4000, "input.step": "power"}, 400, remote.REFUSED),
    (lambda e: {**action_fields(e, e.flow, "D", step="power"), "surprise": "1"}, 400, remote.REFUSED),
    (lambda e: {**action_fields(e, e.flow, "s"), "proof": "1.2.3"}, 400, remote.REFUSED),
    (lambda e: {**action_fields(e, e.flow, "s"), "confirm": "garbage"}, 400, remote.REFUSED),
]


@pytest.mark.parametrize(("build", "status", "reason"), REFUSALS)
def test_api_action_refusals_are_json_and_spawn_nothing(engine, build, status, reason):
    answered, headers, document = post_action(engine, build(engine))

    assert answered == status and headers["content-type"] == "application/json; charset=utf-8"
    assert document["result"] == "refused" and reason in document["reason"] and set(document) == {"result", "reason"}
    assert engine.spawns.calls == []
    assert log_rows(engine.log)[-1]["outcome"] == "refused"


def test_api_action_target_resolved_server_side(engine):
    forged = {"key": "D", "target": json.dumps(["flow", "/etc", "t1"]), "input.step": "power"}
    assert post_action(engine, forged)[2]["reason"] == remote.REFUSED
    assert engine.spawns.calls == [] and engine.fresh_reads == 0

    post_action(engine, action_fields(engine, engine.flow, "D", step="power"))
    (_, kwargs), = engine.spawns.calls
    assert kwargs["cwd"] == engine.flow.repo and engine.fresh_reads == 1


def test_api_action_confirm_token_is_bound_to_the_identity(engine):
    fields = action_fields(engine, engine.flow, "s")
    token = post_action(engine, fields)[2]["token"]
    engine.who = gate.Identity(OWNER.login, "nPHONE", "phone")
    engine.server.settings = CockpitSettings(0, frozenset({OWNER.login}), frozenset({"nMAC", "nPHONE"}), Path("/x"))
    csrf = gate.csrf_token(SECRET, engine.who, time.time())

    status, _headers, document = post_action(engine, {**fields, "confirm": token}, csrf=csrf)

    assert status == 400 and document["result"] == "refused" and engine.spawns.calls == []


# -- transport ----------------------------------------------------------------------------------

def test_api_responses_carry_json_type_and_security_headers(engine):
    responses = [
        _get(engine, "/api/session"),
        _get(engine, "/api/snapshot"),
        _post(engine, action_fields(engine, engine.flow, "D", step="power"), path="/api/action"),
        _post(engine, {"key": "x"}, path="/api/action"),
    ]
    for status, headers, body in responses:
        assert headers["content-type"] == "application/json; charset=utf-8", body
        assert headers["x-content-type-options"] == "nosniff" and headers["cache-control"] == "no-store"
        for name, value in SECURITY_HEADERS.items():
            assert headers[name] == value, name
        assert isinstance(json.loads(body), dict)
    assert [response[0] for response in responses] == [200, 200, 200, 400]


def test_api_sends_no_cors_headers_and_refuses_preflight_and_other_methods(engine):
    answers = [_get(engine, "/api/snapshot"),
               _get(engine, "/api/snapshot", extra="Origin: http://evil.example\r\n"),
               _exchange(engine, f"OPTIONS /api/action HTTP/1.1\r\nHost: {engine.host}\r\nOrigin: http://evil.example\r\n"
                                     "Access-Control-Request-Method: POST\r\n\r\n".encode()),
               _exchange(engine, f"DELETE /api/snapshot HTTP/1.1\r\nHost: {engine.host}\r\n\r\n".encode()),
               _get(engine, "/api/action"),
               _get(engine, "/api/nowhere")]
    assert [answer[0] for answer in answers] == [200, 200, 405, 405, 405, 404]
    for _status, headers, _body in answers:
        assert not [name for name in headers if name.startswith("access-control-")]
        for name, value in SECURITY_HEADERS.items():
            assert headers[name] == value


def test_api_is_absent_when_the_server_is_built_without_it(tmp_path):
    settings = CockpitSettings(0, frozenset({OWNER.login}), frozenset({OWNER.node_id}), tmp_path / "tailscale")
    bare = MemoryServer(gate.Self(LOOPBACK, OWN_DNS_NAME), settings, lambda *_: "", lambda *_: "",
                          identify=lambda _ip: OWNER, access_log=tmp_path / "access.log", csrf_secret=SECRET)
    try:
        bare.port, bare.host = bare.server_address[1], f"{LOOPBACK}:{bare.server_address[1]}"
        assert _get(bare, "/api/snapshot")[0] == 404
    finally:
        bare.server_close()


def test_serve_hands_the_api_to_the_server(tmp_path, monkeypatch):
    captured = {}

    class Stub:
        def __init__(self, own, settings, index_page, action_runner, **kwargs):
            captured.update(kwargs)

        def serve_forever(self):
            pass

        def server_close(self):
            pass

    monkeypatch.setattr(server, "CockpitServer", Stub)
    monkeypatch.setattr(gate, "read_self", lambda settings: gate.Self("100.64.0.1", OWN_DNS_NAME))
    settings = CockpitSettings(8787, frozenset({"a"}), frozenset({"b"}), tmp_path / "tailscale")

    server.serve(settings, lambda *_: "", lambda *_: "", api="the routes")

    assert captured["api"] == "the routes"


# -- the document -------------------------------------------------------------------------------

def test_api_doc_lists_every_route():
    doc = DOC.read_text()
    for method, path in server.API_ROUTES:
        assert f"{method} {path}" in doc, (method, path)
    served = set(re.findall(r'"(/api/[a-z/]+)"', Path(server.__file__).read_text()))
    assert served == {path for _, path in server.API_ROUTES}


def test_api_doc_names_the_gate_rules_and_the_version():
    doc = DOC.read_text()
    for word in ("api: 1", "api: 2", "Host", "Origin", "CSRF", "allow_logins", "allow_nodes", "no CORS", "confirm", "400", "403",
                 "404", "405", "413", "503", "waits", "progress", "remember", "owner_row"):
        assert word in doc, word


def test_time_without_a_zone_is_read_as_utc_not_host_local(monkeypatch):
    monkeypatch.setenv("TZ", "Asia/Tokyo")
    time.tzset()
    try:
        assert api._utc("2026-10-26T10:00:00") == "2026-10-26T10:00:00Z"
        assert api._utc("2026-10-26T12:00:00+02:00") == "2026-10-26T10:00:00Z"
    finally:
        monkeypatch.undo()
        time.tzset()


def test_server_serializes_json_with_the_shared_dumps(monkeypatch):
    monkeypatch.setattr(api, "dumps", lambda document: '{"marker": 1}')
    sent = []
    handler = server._Handler.__new__(server._Handler)
    monkeypatch.setattr(handler, "_finish", lambda status, body, kind: sent.append(body), raising=False)
    handler._finish_json(200, {"api": 1})
    assert sent == [b'{"marker": 1}']


def test_SnapshotWait_CarriesTheRawDetailApartFromTheSentence():
    from sakura.cockpit import api
    from sakura.flow import waits

    wait = waits.Wait("[]", "blocked", "/r", "p", "t", "This lane is stuck", "A sentence.", None, technical="raw: /x/y.log")

    document = api._wait(wait)

    assert document["detail"] == "A sentence." and document["technical"] == "raw: /x/y.log"


def test_api_a_wait_carries_its_findings_when_it_has_some_and_none_otherwise():
    from sakura.flow.waits import Wait

    identity = '["wait","/r","t","approval","final-review@abc"]'
    plain = Wait(identity, "approval", "/r", "p", "t", "Title", "Detail", None)
    with_findings = Wait(identity, "approval", "/r", "p", "t", "Title", "Detail", None, findings=("a finding",))

    assert api._wait(plain)["findings"] == [] and api._wait(with_findings)["findings"] == ["a finding"]


@pytest.mark.parametrize("key", ["a", "k", "d", "C", "l", "c", "n", "e", "p"])
def test_phone_policy_reserves_owner_decisions_and_account_changes(key):
    from sakura.cockpit import actions

    assert actions.is_local_only(key, ())


def test_connector_controls_discoverable_for_empty_and_unreadable_sections():
    from sakura.connectors import registry

    empty = api.snapshot_document(make_snapshot())
    controls = empty["connector_controls"]
    assert empty["connectors"] == [] and "connectors_problem" not in empty
    assert controls["version"] == 1 and controls["target"] == '["host","connectors"]'
    assert {row["key"]: row["needs"] for row in controls["actions"]} == {
        "connectoradd": ["op", "choices"],
        "connectoredit": ["op", "name", "revision", "choices"],
        "connectordisconnect": ["op", "name", "revision"],
        "connectorreplace": ["op", "name", "revision", "candidate", "candidate_revision"],
    }
    assert all(row["confirm"] for row in controls["actions"])
    assert [row["key"] for row in empty["digest"]["connect"]["actions"]] == ["O", "o"]
    registry.path().write_text("invalid")
    unreadable = api.snapshot_document(make_snapshot())
    assert unreadable["connector_controls"] == controls
    assert unreadable["connectors"] == [] and unreadable["connectors_problem"] == "the connectors cannot be read"


def test_connector_preview_and_log_omit_private_program_inputs(tmp_path):
    program = str(tmp_path / "approved-reader")
    fields = {"key": "connectoradd", "target": '["host","connectors"]', "input.op": "new-reader",
              "input.choices": json.dumps({"kind": "program", "label": "Reader", "reads": "Reads stories",
                                           "program": program, "args": ["private argument"]})}
    snapshot = make_snapshot()
    spawn = Spawns(stdout='{"id":"cn-123","revision":1}')
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawn, log_path=tmp_path / "actions")
    from sakura.actors import Actor
    from sakura.cockpit.controller import PROOF_SOURCE_CLI

    def act(request):
        return api.action_document(controller.perform(request, Actor("cli", "owner"), proof_field="confirm",
                                                      proof_source=PROOF_SOURCE_CLI))

    preview = act(fields)
    assert preview["result"] == "confirm"
    assert program not in json.dumps(preview) and "private argument" not in json.dumps(preview)
    done = act({**fields, "confirm": preview["token"]})
    assert done["connector"] == {"id": "cn-123", "revision": 1}
    assert program not in (tmp_path / "actions").read_text()


def test_connector_replacement_preview_refuses_unknown_coverage_and_never_spawns(tmp_path):
    from sakura.actors import Actor
    from sakura.cockpit.controller import PROOF_SOURCE_CLI
    from sakura.connectors import contract, registry

    def unproven(*args, **kwargs):
        raise contract.Refusal("replacement_unproven")

    snapshot = make_snapshot()
    spawn = Spawns()
    controller = Controller(SECRET, lambda: snapshot, lambda: snapshot, run=spawn, log_path=tmp_path / "actions")
    fields = {"key": "connectorreplace", "target": '["host","connectors"]', "input.op": "replace",
              "input.name": "cn-custom", "input.revision": "1", "input.candidate": "cn-built-in",
              "input.candidate_revision": "2"}
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(registry, "preview_replacement", unproven)
        result = controller.perform(fields, Actor("cli", "owner"), proof_field="confirm", proof_source=PROOF_SOURCE_CLI)
    assert api.action_document(result) == {"result": "refused", "reason": "replacement_unproven",
                                           "reason_code": "replacement_unproven", "reason_args": {}}
    assert not spawn.calls


def test_snapshot_does_not_probe_legacy_digest_program(tmp_path, monkeypatch):
    from sakura.connectors import kinds
    from sakura.digest import config

    config.config_path().write_text(f'[[source]]\nid = "reader"\nrun = ["{tmp_path / "reader"}"]\n')
    config.config_path().chmod(0o600)

    def unexpected(*args):
        pytest.fail("snapshot tried to inspect a program")

    monkeypatch.setattr(kinds, "program_problem", unexpected)
    assert api._connectors_document()["connectors"][0]["id"] == "reader"


def test_phone_connector_action_is_refused_before_snapshot_or_command(tmp_path):
    from sakura.actors import Actor
    from sakura.cockpit.controller import PROOF_SOURCE_OVERLAY

    def unexpected():
        pytest.fail("phone mutation reached snapshot resolution")

    spawn = Spawns()
    controller = Controller(SECRET, unexpected, unexpected, run=spawn, log_path=tmp_path / "actions")
    fields = {"key": "connectordisconnect", "target": '["host","connectors"]', "input.op": "remove-1",
              "input.name": "cn-news", "input.revision": "1"}
    result = controller.perform(fields, Actor("overlay", "owner", "phone"), proof_field="confirm",
                                proof_source=PROOF_SOURCE_OVERLAY)
    assert api.action_document(result)["reason_code"] == "local_only"
    assert not spawn.calls


def test_snapshot_exposes_empty_scheduled_without_creating_store():
    from sakura import engine_home

    document = api.snapshot_document(make_snapshot())
    scheduled = document["scheduled"]
    assert scheduled["tasks"] == []
    assert scheduled["target"] == '["host","scheduled"]'
    assert scheduled["actions"][0]["key"] == "scheduledcreate"
    assert not (engine_home.root() / "scheduled").exists()


def test_phone_scheduled_read_hides_prompts_and_refuses_operator_actions(engine):
    from sakura.cockpit import snapshot
    from sakura.flow import waits
    from sakura.scheduled import store
    from test_scheduled_store import DEFINITION, NOW

    tasks = store.Store()
    secret = "private scheduled instructions"
    task = tasks.create({**DEFINITION, "prompt": secret}, "private-task", NOW)
    tasks.propose({**DEFINITION, "prompt": secret}, "master:fixture", NOW)
    engine.snapshot = snapshot.collect([], NOW)
    status, _, raw = _get(engine, "/api/snapshot")
    document = json.loads(raw)
    [row] = document["scheduled"]["tasks"]
    [approval] = [wait for wait in document["waits"] if wait["subkind"] == "scheduled_approval"]
    assert status == 200 and secret.encode() not in raw
    assert row["target"].startswith("t1.") and row["actions"] == []
    assert approval["detail"] == "" and {"y", "N"}.isdisjoint(action["key"] for action in approval["actions"])
    for key, target in (("scheduledpause", row["target"]), ("y", approval["id"])):
        _, _, refusal = post_action(engine, {"key": key, "target": target})
        assert refusal["result"] == "refused"
    assert tasks.get(task.id).revision == task.revision and len(tasks.approvals()) == 1
    assert any(wait.source == "scheduled" for wait in waits.collect([]))


def test_master_default_snapshot_selects_only_goose_for_acp(monkeypatch):
    from sakura.setup import apply

    for provider, route in (("claude", "legacy"), ("codex", "legacy"), ("goose", "acp")):
        reference = {"provider": provider, "id": "work"}
        monkeypatch.setattr(apply, "master_account", lambda reference=reference: reference)
        document = api.snapshot_document(make_snapshot())
        assert document["engine"]["master_launch_route"] == route
        assert document["engine"]["master_account"] == reference

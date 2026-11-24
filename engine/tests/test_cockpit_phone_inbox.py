import json
from dataclasses import replace
from datetime import UTC
from pathlib import Path

import pytest
from sakura.cockpit import api, pages
from sakura.cockpit.gate import Identity
from test_cockpit_digest_reading import _published, collect
from test_cockpit_pages import CSRF, Audit, make_wait, waiting_page
from test_cockpit_proposal_start import DESKTOP, _act, _lane, world  # noqa: F401
from test_cockpit_snapshot import make_item, make_snapshot

PHONE = Identity("owner@example.com", "nPHONE", "Phone")
PROJECT = Path("/work/repo")


def _proposal(repo):
    _published()
    return next(wait for wait in collect([repo]).waits if wait.kind == "question")


def test_phone_inbox_orders_needs_you_then_triage_and_groups_by_lane_or_source(repo):
    question = replace(make_wait("question", "q", title="What next?"), lane_title="Lane title")
    proposal = _proposal(repo)
    page = waiting_page(proposal, question)
    assert page.index("Needs you") < page.index("Lane title") < page.index("Triage") < page.index("Morning digest")
    assert "Question" in page and "Proposal" in page and page.count("unread-dot") == 2
    row = api._wait(question)
    assert row["section"] == "needs_you" and row["resolution"] == "open" and row["group"]["label"] == "Lane title"


def test_phone_inbox_lists_snoozed_waits_with_unsnooze_and_lists_done():
    hidden = replace(make_wait("question", "q", title="Snoozed wait"), is_snoozed=True)
    page = waiting_page(hidden, decided=[{"title": "Resolved item", "actor": "overlay:owner:phone", "place": "phone",
                                         "answer": "reply"}])
    assert page.index("<h3>Snoozed</h3>") < page.index("Snoozed wait") < page.index("<h3>Done</h3>")
    assert "unsnooze" in Audit(page).hidden_values("key")
    assert "<h3>Done</h3>" in page and "Resolved item" in page and "overlay:owner:phone" in page and "Phone" in page


@pytest.mark.parametrize(("projects", "label"), [([], "Add a project"), ([PROJECT], "Start a lane")])
def test_phone_inbox_empty_explains_what_appears(projects, label):
    page = waiting_page(projects=projects)
    assert "Questions" in page and "proposals" in page and label in page


@pytest.mark.parametrize(("kind", "keys"), [
    ("question", {"r"}),
    ("approval", {"a"}),
    ("review", {"markreviewed"}),
    ("blocked", {"s"}),
    ("proposal", {"start", "dismiss"}),
])
def test_phone_detail_shows_actions_and_the_same_kind_and_state(repo, kind, keys):
    wait = make_wait("question" if kind == "proposal" else kind, kind, title="Wait title")
    if kind == "proposal":
        wait = _proposal(repo)
    if kind == "review":
        wait = replace(wait, links=(("Open pull request", "https://example.test/pull/1"),))
    lane = make_item(key=("flow", str(PROJECT), "demo"), status="blocked", agent_state="blocked")
    wait = replace(wait, is_read=True)
    snapshot = make_snapshot(waits=[wait], items=[lane], projects=[PROJECT])
    page = pages.index(snapshot, PHONE, CSRF)
    detail = pages.wait_view(snapshot, wait, CSRF)
    assert kind.capitalize() in page and kind.capitalize() in detail
    assert 'data-resolution="open"' in page and 'data-resolution="open"' in detail
    assert "Properties" in detail and "unread-dot" not in page
    assert set(Audit(detail).hidden_values("key")) >= keys | {"snooze", "unread"}
    if kind == "review":
        assert 'href="https://example.test/pull/1"' in detail


def test_phone_and_desktop_start_with_the_same_operation(repo, request):
    proposal_world = request.getfixturevalue("world")
    wait, controller, fresh = proposal_world
    page = controller.act({"key": "read", "target": wait.id}, PHONE, CSRF)
    assert "Check PR 12" in page and "Properties" in page
    [op] = Audit(page).hidden_values("input.op")
    current = fresh()
    visible = next(row for row in api.snapshot_document(current)["waits"] if row["id"] == wait.id)
    assert visible["is_read"] is True and visible["proposal"]["op"] == op
    started = controller.act({"key": "start", "target": wait.id, "input.op": op, "input.project": str(repo)}, PHONE, CSRF)
    assert "exit code 0" in started
    lane = _lane(_act(proposal_world, "start", DESKTOP, op=op))
    assert fresh().decided[0]["lane"] == lane and fresh().decided[0]["place"] == "phone"


def test_phone_answer_leaves_desktop_inbox_and_enters_done(repo, monkeypatch):
    import contextlib
    import io
    import subprocess
    from datetime import datetime

    from sakura.cockpit import snapshot
    from sakura.cockpit.controller import Controller
    from sakura.flow import cli
    from test_flow_lane_questions import _asked

    _, wait = _asked(repo)
    monkeypatch.chdir(repo)
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")

    def fresh():
        return snapshot.collect([repo], datetime.now(UTC))

    def run(argv, **kwargs):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = cli.main(argv[2:])
        return subprocess.CompletedProcess(argv, code, output.getvalue(), "")

    assert wait.id in {row["id"] for row in api.snapshot_document(fresh())["waits"]}
    controller = Controller(b"test", fresh, fresh, run=run)
    reply = controller.act({"key": "r", "target": wait.id, "input.choice": "yes"}, PHONE, CSRF)
    assert "exit code 0" in reply
    document = api.snapshot_document(fresh())
    assert wait.id not in {row["id"] for row in document["waits"]}
    [entry] = document["decided"]
    assert (entry["id"], entry["actor"], entry["place"]) == (wait.id, "overlay:owner@example.com:nPHONE", "phone")



def test_phone_resume_accepts_multiline_notes():
    wait = make_wait("blocked", "blocked", title="Blocked")
    lane = make_item(key=("flow", str(PROJECT), "demo"), status="blocked", agent_state="blocked")
    snapshot = make_snapshot(waits=[wait], items=[lane])
    detail = pages.wait_view(snapshot, wait, CSRF)
    assert '<textarea name="input.note"' in detail and 'maxlength="2000"' in detail


@pytest.mark.parametrize("record", ["state", "config"])
def test_phone_inbox_keeps_read_errors_in_host_logs(repo, monkeypatch, capsys, record):
    from flow_support import make_context
    from sakura.flow import config, waits

    task = make_context(repo).store
    if record == "state":
        task.state_file.write_text("{")
        path = str(task.state_file)
    else:
        path = "/private/example/project/config.toml"

        def unreadable(*args, **kwargs):
            raise config.FlowConfigError(f"cannot read {path}")

        monkeypatch.setattr(config, "load", unreadable)
    [wait] = waits.collect_stores({repo: [task]})
    detail = pages.wait_view(make_snapshot(waits=[wait]), wait, CSRF)
    assert path not in detail and path not in api._wait(wait)["detail"]
    assert path in capsys.readouterr().err


def test_track_phone_form_replays_the_same_answer_after_the_wait_leaves(repo):
    from datetime import datetime

    from sakura.cockpit import snapshot
    from sakura.cockpit.controller import Controller
    from sakura.tracks import TrackService

    service = TrackService()
    track = service.create("Release", "Ship", actor=DESKTOP, op_id="new")
    service.add_decision(track.id, "Ready to ship?", actor=DESKTOP, op_id="question")

    def fresh():
        return snapshot.collect([repo], datetime.now(UTC))

    controller = Controller(b"test", fresh, fresh)
    [wait] = fresh().waits
    reference = controller.remote_ids.register(tuple(json.loads(wait.id)))
    page = controller.phone_act({"key": "read", "target": reference}, PHONE, CSRF)
    [op] = Audit(page).hidden_values("input.op")
    assert "tracksettle" in Audit(page).hidden_values("key")
    assert '<textarea name="input.answer"' in page and 'maxlength="2000"' in page
    fields = {"key": "tracksettle", "target": reference, "input.op": op, "input.answer": "Yes\r\nShip."}
    assert "exit code 0" in controller.phone_act(fields, PHONE, CSRF)
    assert "exit code 0" in controller.phone_act(fields, PHONE, CSRF)
    assert "already settled" in controller.phone_act({**fields, "input.op": "different"}, PHONE, CSRF)
    assert fresh().waits == []
    [done] = fresh().decided
    assert (done["id"], done["text"], done["place"]) == (wait.id, "Yes\nShip.", "phone")


def test_track_phone_http_accepts_a_full_length_unicode_answer_and_requires_csrf(tmp_path):
    from datetime import datetime

    from sakura.cockpit import snapshot
    from sakura.tracks import TrackService
    from sakura.tracks.domain import ANSWER_LIMIT
    from test_cockpit_api import Engine
    from test_cockpit_server import _post

    service = TrackService()
    track = service.create("Release", "Ship", actor=DESKTOP, op_id="new")
    service.add_decision(track.id, "Ready to ship?", actor=DESKTOP, op_id="question")
    engine = Engine(tmp_path)
    try:
        engine.snapshot = snapshot.collect([], datetime.now(UTC))
        [wait] = engine.snapshot.waits
        reference = engine.controller.remote_ids.register(tuple(json.loads(wait.id)))
        fields = {"key": "tracksettle", "target": reference, "input.op": "answer", "input.answer": "字" * ANSWER_LIMIT}
        assert _post(engine, fields)[0] == 413
        path = "/action?track-decision"
        assert _post(engine, fields, path=path, csrf="wrong")[0] == 403
        status, _, body = _post(engine, fields, path=path)
        assert status == 200 and b"exit code 0" in body
        [done] = snapshot.collect([], datetime.now(UTC)).decided
        assert done["text"] == fields["input.answer"]
    finally:
        engine.server.server_close()

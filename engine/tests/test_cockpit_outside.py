"""Outside lanes in the cockpit: snapshot, waits, actions and the JSON document (S8a.16)."""

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import questions
from sakura.actors import Actor
from sakura.cockpit import actions, api, pages, wait_actions
from sakura.cockpit.controller import PROOF_SOURCE_CLI, Confirm, Controller, Done, Refused
from sakura.outside import inbox, records, store
from test_cockpit_actions import Spawns, log_rows
from test_cockpit_snapshot import collect, create_task

NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
WHERE = Path("/work/work")
CLAUDE = records.Agent("claude", WHERE)
SCRIPT = records.Agent("script", Path("/work/other"))
PHONE = Actor("overlay", "owner@example.com", "nPHONE1", "Owner's phone")
LOCAL = Actor("cli", "owner")
LANE = "measure-help"


@pytest.fixture
def root():
    return store.path()


def open_lane(root, agent=CLAUDE, lane_id=LANE, now=NOW, **fields):
    return store.open_lane(root, agent, lane_id, **{"title": "Add --help", **fields}, now=now).lane


def add_row(root, row_id="help-prints", agent=CLAUDE, lane_id=LANE, now=NOW, **fields):
    given = {"title": "Help prints", "steps": ("Run measure.py --help", "Read the usage line"),
             "expected": "usage: measure", **fields}
    return store.put_row(root, agent, lane_id, row_id, **given, now=now).lane.rows[-1]


def ask(root, question_id="flag-short", choices=("yes", "no"), agent=CLAUDE, lane_id=LANE, now=NOW):
    asked = questions.make(question_id, "Also accept -h?\nSecond line", choices, now)
    store.ask(root, agent, lane_id, asked, now)
    return asked


def outside_waits(repo):
    return collect([repo]).waits


def target_of(repo, kind):
    return wait_actions.target_of(next(wait for wait in outside_waits(repo) if wait.kind == kind))


def test_Snapshot_OutsideLane_ItemWithTitleAuthorLinksProgressUnread(repo, root):
    open_lane(root, links=(("PR 12", "https://github.com/x/y/pull/12"),), phase="ready")
    row = add_row(root)
    add_row(root, "second")
    inbox.mark(root, PHONE, LANE, row.id, row.digest, "ok", "", NOW)
    inbox.steer(root, PHONE, LANE, "use the tag", NOW)
    item = next(found for found in collect([repo]).items if found.kind == "outside")
    assert item.key == ("outside", LANE) and item.repo == WHERE
    assert (item.title, item.author, item.links) == ("Add --help", "outside:claude",
                                                     (("PR 12", "https://github.com/x/y/pull/12"),))
    assert (item.progress.done, item.progress.total, item.progress.phase) == (1, 2, "ready")
    assert item.unread == 2 and item.agent_state == "waiting" and item.since == "2026-11-16T10:00:00+00:00"
    assert item.last_activity_at and item.worktree is None and item.usage is None


def test_Snapshot_OutsideLaneWithNothingOpen_StateUnknownNoWaits(repo, root):
    open_lane(root)
    found = collect([repo])
    assert [item.agent_state for item in found.items if item.kind == "outside"] == ["unknown"]
    assert found.waits == []


def test_Snapshot_OutsideRowAndQuestion_WaitsWithSourceAuthorChoices(repo, root):
    open_lane(root)
    row = add_row(root, note="on a clean clone")
    asked = ask(root)
    row_wait, question_wait = outside_waits(repo)
    assert json.loads(row_wait.id) == ["wait", "outside", LANE, "owner_row", f"help-prints@{row.digest}"]
    assert json.loads(question_wait.id) == ["wait", "outside", LANE, "question", f"flag-short@{asked.digest}"]
    assert row_wait.detail == ("1. Run measure.py --help\n2. Read the usage line\nExpected: usage: measure\n"
                               "Note: on a clean clone")
    assert (row_wait.title, row_wait.source, row_wait.author, row_wait.choices) == (
        "Help prints", "outside", "outside:claude", ())
    assert (question_wait.title, question_wait.detail, question_wait.choices) == (
        "Also accept -h? Second line", "Also accept -h?\nSecond line", ("yes", "no"))
    assert (row_wait.repo, row_wait.project, row_wait.task) == (str(WHERE), "work", LANE)


def test_Snapshot_RowWithNoExpectedResult_HasNoExpectedLine(repo, root):
    open_lane(root)
    add_row(root, expected="")
    (wait,) = outside_waits(repo)
    assert wait.detail == "1. Run measure.py --help\n2. Read the usage line"


def test_Snapshot_RowWithOneStep_HasItsTextNotANumberedList(repo, root):
    open_lane(root)
    add_row(root, steps=("Accept the invitation",), expected="")
    (wait,) = outside_waits(repo)
    assert wait.detail == "Accept the invitation"


def test_Snapshot_RowNoteWithLinks_ShowsTheNoteWithoutThemAndGivesTheLinksTitled(repo, root):
    open_lane(root)
    add_row(root, steps=("Look at it",), expected="",
            note="Seen on staging. Issue #1783: https://github.com/x/y/issues/1783 PR #5: https://github.com/x/y/pull/5")
    (wait,) = outside_waits(repo)
    assert wait.detail == "Look at it\nNote: Seen on staging."
    assert wait.links == (("Issue #1783", "https://github.com/x/y/issues/1783"), ("PR #5", "https://github.com/x/y/pull/5"))


def test_Snapshot_RowNoteThatIsOnlyLinks_HasNoNoteLine(repo, root):
    open_lane(root)
    add_row(root, steps=("Look at it",), expected="", note="Issue #7: https://github.com/x/y/issues/7")
    (wait,) = outside_waits(repo)
    assert wait.detail == "Look at it" and wait.links == (("Issue #7", "https://github.com/x/y/issues/7"),)


def test_Snapshot_QuestionWithLinks_TitleAndDetailLeaveThemOutAndLinksAreGiven(repo, root):
    open_lane(root)
    asked = questions.make("accept", "Accept the invitation?\nInvitation: https://github.com/x/y/invitations", ("yes", "no"), NOW)
    store.ask(root, CLAUDE, LANE, asked, NOW)
    (wait,) = outside_waits(repo)
    assert (wait.title, wait.detail) == ("Accept the invitation?", "Accept the invitation?")
    assert wait.links == (("Invitation", "https://github.com/x/y/invitations"),)


def test_Api_WaitCarriesItsLinksAsLabelAndUrl(repo, root):
    open_lane(root)
    add_row(root, steps=("Look",), expected="", note="PR #5: https://github.com/x/y/pull/5")
    document = api.snapshot_document(collect([repo]))
    (wait,) = document["waits"]
    assert wait["links"] == [{"label": "PR #5", "url": "https://github.com/x/y/pull/5"}]


def test_Snapshot_AnsweredRowAndQuestion_NoLongerWaits(repo, root):
    open_lane(root)
    row, asked = add_row(root), ask(root)
    inbox.mark(root, PHONE, LANE, row.id, row.digest, "ko", "", NOW)
    inbox.reply(root, PHONE, LANE, asked.id, asked.digest, "yes", "", NOW)
    assert outside_waits(repo) == []


def test_Waits_OutsideAndEngine_OneListOrderedBySince(repo, root):
    create_task(repo, "demo", status="awaiting_approval", history=[{"at": "2026-11-16T09:00:00+00:00"}])
    open_lane(root, now=NOW - timedelta(hours=2))
    add_row(root, now=NOW - timedelta(hours=2))
    ask(root, now=NOW + timedelta(hours=1))
    found = outside_waits(repo)
    assert [(wait.source, wait.kind) for wait in found] == [("outside", "owner_row"), ("flow", "approval"),
                                                            ("outside", "question")]
    assert [wait.since for wait in found] == sorted(wait.since for wait in found)


def test_Snapshot_UnreadableOutsideLane_SkippedWithProblemLine(repo, root):
    open_lane(root)
    open_lane(root, lane_id="second", title="Second")
    (root / LANE / "lane.json").write_text("{broken")
    found = collect([repo])
    assert [item.key for item in found.items if item.kind == "outside"] == [("outside", "second")]
    assert len([line for line in found.problems if LANE in line]) == 1


def test_Snapshot_NoOutsideStore_NothingCreated(repo, root):
    found = collect([repo])
    assert [item for item in found.items if item.kind == "outside"] == [] and not root.exists()


def test_Snapshot_ClosedLane_LeavesItemsAndWaits(repo, root):
    open_lane(root)
    add_row(root)
    store.close(root, PHONE, LANE, outcome="dropped", now=NOW)
    found = collect([repo])
    assert [item for item in found.items if item.kind == "outside"] == [] and found.waits == []


def test_Snapshot_OutsideLogLines_AreEventsWithScope(repo, root):
    open_lane(root)
    add_row(root)
    inbox.steer(root, PHONE, LANE, "hello", NOW)
    events = [event for event in collect([repo]).events if event["source"] == "outside"]
    assert {event["scope"] for event in events} == {f"outside/{LANE}"}
    assert [event["text"].split()[0] for event in events] == ["opened", "row", "steer"]


def test_Snapshot_EventsOfOtherLanes_SortedByTheirTime(repo, root):
    open_lane(root, lane_id="late", now=NOW + timedelta(minutes=5))
    open_lane(root, lane_id="early", now=NOW)
    scopes = [event["scope"] for event in collect([repo]).events if event["source"] == "outside"]
    assert scopes == ["outside/early", "outside/late"]


# -- actions ---------------------------------------------------------------------------------------


def act(key, target, inputs=None, actor=PHONE, log=None):
    spawns = Spawns()
    result = actions.run(key, target, inputs or {}, actor, run=spawns, log_path=log)
    return result, spawns


def test_Act_MarkOutsideRow_RecordsMarkForTheActorInTheStoreFolderSpawningNothing(repo, root, tmp_path):
    open_lane(root)
    row = add_row(root)
    result, spawns = act("k", target_of(repo, "owner_row"), {"verdict": "ok", "note": "looks right"},
                         log=tmp_path / "log.jsonl")
    assert result.exit_code == 0 and spawns.calls == [] and result.invocation.cwd == root
    line = store.log(root, LANE)[-1]
    assert (line["kind"], line["verdict"], line["note"], line["row"], line["digest"]) == (
        "mark", "ok", "looks right", row.id, row.digest)
    assert line["by"] == "overlay:owner@example.com:nPHONE1"
    assert "--actor=overlay:owner@example.com:nPHONE1" in result.invocation.argv
    assert log_rows(tmp_path / "log.jsonl")[0]["outcome"] == "done"


def test_Act_MarkOutsideRow_ByCliActor_Recorded(repo, root):
    open_lane(root)
    add_row(root)
    act("k", target_of(repo, "owner_row"), {"verdict": "ko"}, actor=Actor("cli", "me"))
    assert store.log(root, LANE)[-1]["by"] == "cli:me"


def test_Act_MarkAfterAgentChangedRow_RefusedNoLongerWaiting(repo, root, tmp_path):
    open_lane(root)
    add_row(root)
    seen = [wait for wait in outside_waits(repo) if wait.kind == "owner_row"][0]
    add_row(root, note="changed after the person looked")
    controller = Controller(b"s" * 32, lambda: collect([repo]), lambda: collect([repo]), run=Spawns(),
                            log_path=tmp_path / "log.jsonl")
    outcome = controller.perform({"key": "k", "target": api.target_id(wait_actions.target_of(seen)),
                                  "input.verdict": "ok"}, LOCAL, proof_field="proof", proof_source=PROOF_SOURCE_CLI)
    assert isinstance(outcome, Refused) and outcome.reason.startswith("no longer waiting")
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "row", "row"]


def test_Act_MarkStaleBetweenLookupAndRun_RefusedByTheStoreNoLine(repo, root):
    open_lane(root)
    add_row(root)
    target = target_of(repo, "owner_row")
    add_row(root, note="changed")
    result, _ = act("k", target, {"verdict": "ok"})
    assert result.exit_code == 2 and "no longer waiting" in result.output
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "row", "row"]


def test_Act_ReplyChoice_RecordsReply(repo, root):
    open_lane(root)
    ask(root)
    result, spawns = act("r", target_of(repo, "question"), {"choice": "yes"})
    assert result.exit_code == 0 and spawns.calls == []
    line = store.log(root, LANE)[-1]
    assert (line["kind"], line["choice"], line["text"], line["question"]) == ("reply", "yes", "", "flag-short")


def test_Act_ReplyText_RecordsReply(repo, root):
    open_lane(root)
    ask(root, choices=())
    act("r", target_of(repo, "question"), {"text": "for scripts"})
    assert store.log(root, LANE)[-1]["text"] == "for scripts"


def test_Act_ReplyChoiceNotOffered_RefusedBeforeSpawn(repo, root, tmp_path):
    open_lane(root)
    ask(root)
    with pytest.raises(actions.ActionRefused, match="not one of the choices"):
        act("r", target_of(repo, "question"), {"choice": "maybe"}, log=tmp_path / "log.jsonl")
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "asked"]
    assert log_rows(tmp_path / "log.jsonl")[0]["outcome"] == "refused"


@pytest.mark.parametrize("inputs", [{}, {"choice": "yes", "text": "words"}])
def test_Act_ReplyWithNeitherChoiceNorText_Refused(repo, root, inputs):
    open_lane(root)
    ask(root)
    with pytest.raises(actions.ActionRefused, match="one of them"):
        act("r", target_of(repo, "question"), inputs)


def test_Act_ReplyOnAnOwnerRow_Refused(repo, root):
    open_lane(root)
    add_row(root)
    with pytest.raises(actions.ActionRefused, match="only a question"):
        act("r", target_of(repo, "owner_row"), {"text": "x"})


def test_Act_WriteToOutsideLane_AppendsSteer(repo, root):
    open_lane(root)
    item = next(found for found in collect([repo]).items if found.kind == "outside")
    result, spawns = act("w", item, {"text": "use the tag, not the date"})
    assert result.exit_code == 0 and spawns.calls == [] and result.invocation.cwd == root
    line = store.log(root, LANE)[-1]
    assert (line["kind"], line["text"]) == ("steer", "use the tag, not the date")
    assert inbox.unread(store.read(root, LANE), store.log(root, LANE)) == 1


def test_Act_CloseOutsideLane_AsksFirst(repo, root, tmp_path):
    open_lane(root)
    item = next(found for found in collect([repo]).items if found.kind == "outside")
    controller = Controller(b"s" * 32, lambda: collect([repo]), lambda: collect([repo]), run=Spawns(),
                            log_path=tmp_path / "log.jsonl")
    fields = {"key": "d", "target": api.target_id(item)}
    asked = controller.perform(fields, LOCAL, proof_field="proof", proof_source=PROOF_SOURCE_CLI)
    assert isinstance(asked, Confirm) and store.read(root, LANE).closed_at is None
    done = controller.perform({**fields, "proof": asked.token}, LOCAL, proof_field="proof", proof_source=PROOF_SOURCE_CLI)
    assert isinstance(done, Done) and done.result.exit_code == 0
    closed = store.read(root, LANE)
    assert closed.closed_at and closed.outcome == "dropped"
    assert store.log(root, LANE)[-1]["by"] == "cli:owner"


def test_Act_OutsideActorCannotAnswer_RefusedByTheStore(repo, root):
    open_lane(root)
    add_row(root)
    result, _ = act("k", target_of(repo, "owner_row"), {"verdict": "ok"}, actor=Actor("outside", "claude"))
    assert result.exit_code == 2 and "only the person" in result.output
    assert [line["kind"] for line in store.log(root, LANE)] == ["opened", "row"]


def test_Act_ActionsOfEngineOnlyKinds_DoNotApplyToOutsideLanes(repo, root):
    open_lane(root)
    item = next(found for found in collect([repo]).items if found.kind == "outside")
    assert sorted(action.key for action in actions.applicable(item)) == ["d", "w"]
    add_row(root)
    ask(root)
    assert [action.key for action in actions.applicable(target_of(repo, "owner_row"))] == ["k", "read", "unread", "snooze"]
    assert [action.key for action in actions.applicable(target_of(repo, "question"))] == ["r", "read", "unread", "snooze"]


# -- the JSON document -------------------------------------------------------------------------------


def test_Api_OutsideItemAndWaits_CarryTheNewFieldsAndEngineOnesTheirDefaults(repo, root):
    create_task(repo, "demo", status="awaiting_approval")
    open_lane(root, links=(("PR", "https://github.com/x/y/pull/1"),))
    add_row(root)
    ask(root)
    document = api.snapshot_document(collect([repo]))
    outside = next(item for item in document["items"] if item["kind"] == "outside")
    assert (outside["title"], outside["author"], outside["links"], outside["unread"]) == (
        "Add --help", "outside:claude", [{"label": "PR", "url": "https://github.com/x/y/pull/1"}], 0)
    assert outside["progress"] == {"done": 0, "total": 1, "phase": "draft"}
    assert [action["key"] for action in outside["actions"]] == ["d", "w"]
    flow = next(item for item in document["items"] if item["kind"] == "flow")
    assert (flow["title"], flow["author"], flow["links"], flow["unread"]) == ("", "", [], 0)
    by_kind = {wait["subkind"]: wait for wait in document["waits"]}
    assert (by_kind["question"]["source"], by_kind["question"]["choices"], by_kind["question"]["author"]) == (
        "outside", ["yes", "no"], "outside:claude")
    assert [a["key"] for a in by_kind["question"]["actions"]] == ["r", "read", "unread", "snooze"]
    assert (by_kind["approval"]["source"], by_kind["approval"]["choices"], by_kind["approval"]["author"]) == (
        "flow", [], "")
    assert by_kind["owner_row"]["id"] == api.target_id(wait_actions.target_of(next(
        w for w in collect([repo]).waits if w.kind == "owner_row")))


def test_Api_HostileAgentText_StripsControls(repo, root):
    open_lane(root, title="Add help")
    add_row(root, title="Help", expected="line\u202e rtl")
    document = api.snapshot_document(collect([repo]))
    detail = next(wait["detail"] for wait in document["waits"] if wait["subkind"] == "owner_row")
    assert "\u202e" not in detail and "line rtl" in detail


def test_Text_OutsideLane_RendersWithoutBreaking(repo, root):
    from sakura.cockpit import text
    open_lane(root)
    add_row(root)
    lines = text.render(collect([repo]), 100, None)
    assert any("outside/measure-help" in line for line in lines)


def test_Pages_ConfirmOfOutsideClose_NamesTheLane(repo, root, tmp_path):
    open_lane(root)
    item = next(found for found in collect([repo]).items if found.kind == "outside")
    controller = Controller(b"s" * 32, lambda: collect([repo]), lambda: collect([repo]), run=Spawns(),
                            log_path=tmp_path / "log.jsonl")
    asked = controller.perform({"key": "d", "target": api.target_id(item)}, LOCAL,
                               proof_field="proof", proof_source=PROOF_SOURCE_CLI)
    assert isinstance(asked, Confirm)
    page = pages.confirm(asked.action, asked.target, asked.invocation, asked.inputs, "csrf", asked.token)
    assert "outside/measure-help" in page and "Confirm" in page


def test_Text_OutsideWait_CarriesItsFromLine(repo, root):
    from sakura.cockpit import text
    open_lane(root)
    add_row(root)
    lines = text.render(collect([repo]), 100, None)
    assert any("owner_row" in line and "From claude in work" in line for line in lines)

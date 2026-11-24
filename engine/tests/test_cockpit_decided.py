"""Decided asks: every answer the person gives is kept, with the words typed, so Inbox > Done never misses one."""

import json
from dataclasses import replace
from datetime import datetime, timedelta, timezone

import pytest

from flow_support import make_context
from sakura import proposal_cli
from sakura.actors import Actor
from sakura.cockpit import actions, decided, snapshot, wait_actions
from sakura.flow import cli, exits
from test_cockpit_actions import Spawns, flow_item, wait_target
from test_flow_lane_questions import _asked
from test_flow_owner_continue import owner_task
from test_proposals import _proposed, configured, folders, notes  # noqa: F401  (fixtures)

NOW = datetime(2026, 11, 18, 12, 0, tzinfo=timezone.utc)
PERSON = Actor("overlay", "owner@example.com", "nPHONE1", "Owner's phone")
PERSON_TEXT = "overlay:owner@example.com:nPHONE1"
ENTRY_FIELDS = {"id", "key", "kind", "project", "task", "lane_title", "title", "answer", "choice", "text", "actor", "place", "at", "lane"}


def _fields(kind="approval", key="spec@rev1", task="t1"):
    return decided.wait_fields(wait_target(kind, key, task).wait)


def _record(state_dir, answer="approve", *, at=NOW, text=None, choice=None, **wait):
    decided.record(state_dir, wait=_fields(**wait), answer=answer, choice=choice, text=text, actor=PERSON_TEXT, at=at)


@pytest.fixture
def log_path(tmp_path):
    return tmp_path / "cockpit" / "actions.jsonl"


def _kept(log_path, now=NOW):
    return decided.recent(log_path.parent, now=now)


# -- the store -----------------------------------------------------------------------------------------------------


def test_Record_OneAnswer_IsListedWithEveryField(tmp_path):
    _record(tmp_path, "reply", text="Keep the tests.", kind="question", key="q1@abc")
    [entry] = decided.recent(tmp_path, now=NOW)
    assert set(entry) == ENTRY_FIELDS
    assert (entry["answer"], entry["text"], entry["choice"], entry["actor"]) == ("reply", "Keep the tests.", None, PERSON_TEXT)
    assert (entry["kind"], entry["project"], entry["task"], entry["title"]) == ("question", "repo", "t1", "title")
    assert entry["key"] == ["wait", "/work/repo", "t1", "question", "q1@abc"] and entry["id"] == json.dumps(entry["key"], separators=(",", ":"))
    assert entry["at"] == "2026-11-18T12:00:00+00:00"


def test_Record_TextOverTheCap_IsCutAtFourThousandCharacters(tmp_path):
    _record(tmp_path, "reply", text="x" * 5000, kind="question", key="q1@abc")
    assert len(decided.recent(tmp_path, now=NOW)[0]["text"]) == 4000


def test_Record_TerminalControlsInTheTextAndTheTitle_AreRemoved(tmp_path):
    wait = {**_fields(), "title": "Approve\x1b[31m it"}
    decided.record(tmp_path, wait=wait, answer="reply", choice=None, text="ok\x1b]0;x\x07 done", actor=PERSON_TEXT, at=NOW)
    [entry] = decided.recent(tmp_path, now=NOW)
    assert "\x1b" not in entry["text"] and "\x07" not in entry["text"] and "\x1b" not in entry["title"]


def test_Record_AnAnswerOutsideTheList_IsRefused(tmp_path):
    with pytest.raises(ValueError, match="not an answer"):
        _record(tmp_path, "shrug")


def test_Recent_OlderThanSevenDays_IsLeftOut_NewestFirst(tmp_path):
    _record(tmp_path, "approve", at=NOW - timedelta(days=8), key="old@1")
    _record(tmp_path, "approve", at=NOW - timedelta(days=6), key="mid@1")
    _record(tmp_path, "reject", at=NOW - timedelta(hours=1), kind="proposal", key="p1@1")
    assert [entry["answer"] for entry in decided.recent(tmp_path, now=NOW)] == ["reject", "approve"]


def test_Recent_MoreThanTheLimit_KeepsTheNewest(tmp_path):
    for minute in range(5):
        _record(tmp_path, "approve", at=NOW - timedelta(minutes=minute), key=f"g{minute}@1")
    kept = decided.recent(tmp_path, now=NOW, limit=2)
    assert [entry["key"][4] for entry in kept] == ["g0@1", "g1@1"]


def test_Recent_NothingRecordedYet_IsEmptyAndCreatesNothing(tmp_path):
    assert decided.recent(tmp_path, now=NOW) == []
    assert not (tmp_path / "cockpit").exists()


def test_Recent_CorruptAndTornLines_AreSkippedNotFatal(tmp_path):
    _record(tmp_path, key="one@1")
    path = tmp_path / "cockpit" / "decided.jsonl"
    with path.open("a") as stream:
        stream.write("this is not json\n[1, 2]\n" + '{"seq": 9, "answer": "approve"}\n')
        stream.write('{"seq": 10, "id": "torn')
    assert [entry["key"][4] for entry in decided.recent(tmp_path, now=NOW)] == ["one@1"]
    _record(tmp_path, key="two@1", at=NOW + timedelta(minutes=1))
    assert [entry["key"][4] for entry in decided.recent(tmp_path, now=NOW + timedelta(minutes=2))] == ["two@1", "one@1"]


def test_Record_IsAppendOnly_EarlierLinesStayAsTheyWere(tmp_path):
    _record(tmp_path, key="one@1")
    path = tmp_path / "cockpit" / "decided.jsonl"
    before = path.read_bytes()
    _record(tmp_path, key="two@1", at=NOW + timedelta(minutes=1))
    assert path.read_bytes().startswith(before) and len(path.read_bytes()) > len(before)
    assert oct(path.stat().st_mode & 0o777) == "0o600"


def test_Record_SameAnswerAgainWithinTheWindow_IsKeptOnce(tmp_path):
    _record(tmp_path, text="a")
    _record(tmp_path, text="a", at=NOW + timedelta(seconds=decided.DUPLICATE_SECONDS - 1))
    assert len(decided.recent(tmp_path, now=NOW + timedelta(minutes=5))) == 1


@pytest.mark.parametrize("second", [
    dict(answer="mark_ko"),
    dict(key="spec@rev2"),
    dict(task="t2"),
    dict(kind="blocked", key="blocked"),
    dict(at=NOW + timedelta(seconds=decided.DUPLICATE_SECONDS + 1)),
])
def test_Record_ADifferentAnswerWaitOrLaterTime_IsKeptToo(tmp_path, second):
    _record(tmp_path)
    _record(tmp_path, **{"at": NOW + timedelta(seconds=1), **second})
    assert len(decided.recent(tmp_path, now=NOW + timedelta(minutes=5))) == 2


def test_Record_SameWaitWithoutARevisionAndWithOne_IsTheSameAnswer(tmp_path):
    _record(tmp_path, key="spec")
    _record(tmp_path, key="spec@rev1", at=NOW + timedelta(seconds=2))
    assert len(decided.recent(tmp_path, now=NOW + timedelta(minutes=5))) == 1


def test_Record_AFolderThatCannotBeWritten_ReportsOnStderrAndRaisesNothing(tmp_path, capsys):
    blocked = tmp_path / "file"
    blocked.write_text("x")
    _record(blocked)
    assert "cannot write" in capsys.readouterr().err


# -- the cockpit's actions -------------------------------------------------------------------------------------


def _act(key, target, inputs, log_path, spawns=None):
    return actions.run(key, target, inputs, PERSON, run=spawns or Spawns(), log_path=log_path)


def test_Act_ApproveOnAWait_IsKeptAsApprove(log_path):
    target = wait_target("approval", "spec@rev1")
    _act("a", target, {}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["key"], entry["actor"]) == ("approve", list(target.key), PERSON_TEXT)


def test_Act_ReplyWithText_KeepsTheText(log_path):
    _act("r", wait_target("question", "q1@abc"), {"text": "Keep the tests."}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["text"], entry["choice"]) == ("reply", "Keep the tests.", None)


def test_Act_ReplyWithAChoice_IsKeptAsChoice(log_path):
    target = wait_target("question", "q1@abc")
    target = replace(target, wait=replace(target.wait, choices=("yes", "no")))
    _act("r", target, {"choice": "yes"}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["choice"], entry["text"]) == ("choice", "yes", None)


@pytest.mark.parametrize("verdict, answer", [("ok", "mark_ok"), ("ko", "mark_ko")])
def test_Act_MarkWithANote_KeepsTheNote(log_path, verdict, answer):
    _act("k", wait_target("owner_row", "row1@h1"), {"verdict": verdict, "note": "Button is cut off."}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["text"]) == (answer, "Button is cut off.")


@pytest.mark.parametrize("key, answer", [("y", "accept"), ("N", "reject")])
def test_Act_ProposalDecision_IsKept(log_path, key, answer):
    _act(key, wait_target("proposal", "p12345678@dig"), {}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["kind"]) == (answer, "proposal")


def test_Act_ACommandThatFails_KeepsNothing(log_path):
    _act("a", wait_target("approval", "spec@rev1"), {}, log_path, Spawns(returncode=2))
    assert _kept(log_path, datetime.now(timezone.utc)) == []


def test_Act_ARefusedAction_KeepsNothing(log_path):
    with pytest.raises(actions.ActionRefused):
        _act("r", wait_target("question", "q1@abc"), {}, log_path)
    assert _kept(log_path, datetime.now(timezone.utc)) == []


def test_Act_ActionsThatAnswerNothing_KeepNothing(log_path):
    _act("H", wait_target("owner_row", "row1@h1"), {}, log_path)
    _act("w", flow_item("/work/repo"), {"text": "hello"}, log_path)
    assert _kept(log_path, datetime.now(timezone.utc)) == []


@pytest.mark.parametrize("key, status, answer", [("s", "blocked", "stop"), ("s", "failed", "stop")])
def test_Act_OnALaneBlockedOrFailed_IsKeptAgainstThatAsk(log_path, key, status, answer):
    item = replace(flow_item("/work/repo", "t9"), status=status, agent_state="blocked", next_command="sakura flow resume t9")
    _act(key, item, {}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["task"], entry["project"], entry["kind"]) == (answer, "t9", "repo", "blocked")


def test_Act_ApproveOnALaneAtAGate_IsKeptAgainstThatGate(repo, log_path):
    ctx = make_context(repo)
    ctx.store.update_state(lambda state: state.update(status="awaiting_approval", reason="waiting: sakura flow approve demo spec"))
    item = replace(flow_item(repo, "demo"), status="awaiting_approval", agent_state="gate")
    _act("a", item, {}, log_path)
    [entry] = _kept(log_path, datetime.now(timezone.utc))
    assert (entry["answer"], entry["kind"], entry["key"][3:]) == ("approve", "approval", ["approval", "spec"])


@pytest.mark.parametrize("status", ["running", "stopped", ""])
def test_Act_StopOnALaneWithNoAsk_KeepsNothing(log_path, status):
    _act("s", replace(flow_item("/work/repo", "t9"), status=status), {}, log_path)
    assert _kept(log_path, datetime.now(timezone.utc)) == []


# -- the answer paths of the command line ----------------------------------------------------------------------------


def _install(repo, monkeypatch):
    from sakura import install
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    monkeypatch.chdir(repo)
    install.install(repo)


def _now():
    return datetime.now(timezone.utc)


def test_FlowReply_AnsweredFromTheCommandLine_IsKeptOnceWithTheQuestionsOwnWords(repo, monkeypatch):
    _ctx, wait = _asked(repo)
    _install(repo, monkeypatch)
    question, digest = json.loads(wait.id)[4].split("@")
    assert cli.main(["reply", "--choice=yes", f"--actor={PERSON_TEXT}", "--", "demo", question, digest]) == exits.OK
    [entry] = decided.recent(decided.default_state_dir(), now=_now())
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["choice"], entry["kind"], entry["task"]) == ("choice", "yes", "question", "demo")
    assert entry["title"] == "Also accept -h?" and entry["key"] == json.loads(wait.id) and entry["actor"] == PERSON_TEXT


def test_FlowReply_ThenTheCockpitRecordsTheSameAnswer_StaysOne(repo, monkeypatch):
    _ctx, wait = _asked(repo)
    _install(repo, monkeypatch)
    question, digest = json.loads(wait.id)[4].split("@")
    cli.main(["reply", "--text=no", f"--actor={PERSON_TEXT}", "--", "demo", question, digest])
    actions.run("r", wait_actions.target_of(wait), {"text": "no"}, PERSON, run=Spawns(), log_path=None)
    assert len(decided.recent(decided.default_state_dir(), now=_now())) == 1


def test_FlowReply_ARefusedReply_KeepsNothing(repo, monkeypatch):
    _asked(repo)
    _install(repo, monkeypatch)
    assert cli.main(["reply", "--choice=yes", "--", "demo", "qdeadbeef", "0" * 12]) == exits.USAGE
    assert decided.recent(decided.default_state_dir(), now=_now()) == []


def test_ProposalReject_FromTheCommandLine_IsKeptOnce(repo, notes, configured, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    made = _proposed(repo, notes)
    assert proposal_cli.main(["reject", f"--actor={PERSON_TEXT}", "--", made.id, made.digest]) == exits.OK
    [entry] = decided.recent(decided.default_state_dir(), now=_now())
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["kind"], entry["task"], entry["actor"]) == ("reject", "proposal", "demo", PERSON_TEXT)
    assert "style.md" in entry["title"] and entry["key"][4] == f"{made.id}@{made.digest}"


def test_ProposalAccept_FromTheCommandLine_IsKept(repo, notes, configured, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    made = _proposed(repo, notes)
    assert proposal_cli.main(["accept", f"--actor={PERSON_TEXT}", "--", made.id, made.digest]) == exits.OK
    assert [entry["answer"] for entry in decided.recent(decided.default_state_dir(), now=_now())] == ["accept"]


def test_ProposalAccept_Refused_KeepsNothing(repo, notes, configured, monkeypatch):
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")
    made = _proposed(repo, notes)
    assert proposal_cli.main(["accept", "--", made.id, "0" * 12]) == exits.USAGE
    assert decided.recent(decided.default_state_dir(), now=_now()) == []


@pytest.fixture(autouse=False)
def trusted(monkeypatch):
    """The cockpit's own child may carry the viewer's identity."""
    monkeypatch.setenv("SAKURA_ACTOR_TRUSTED", "1")


def _lane(repo, status, **more):
    ctx = make_context(repo, state={"status": status, "phase": "plan", "reason": f"{status} for a test", **more})
    ctx.store.spec.write_text("# Add greeting\n\n## Journeys\n- one\n")
    return ctx


def _kept_now():
    return decided.recent(decided.default_state_dir(), now=_now())


def test_FlowApprove_FromTheCommandLine_IsKeptAgainstTheGateThatWaited(repo, trusted, monkeypatch):
    _lane(repo, "awaiting_approval", reason="awaiting approval: sakura flow approve demo spec (read x)")
    monkeypatch.chdir(repo)
    assert cli.main(["approve", f"--actor={PERSON_TEXT}", "demo", "spec"]) == exits.OK
    [entry] = _kept_now()
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["kind"], entry["task"], entry["actor"]) == ("approve", "approval", "demo", PERSON_TEXT)
    assert entry["title"] == "Read the spec and approve it" and entry["key"][4].startswith("spec@")


def test_FlowApprove_Refused_KeepsNothing(repo, monkeypatch):
    _lane(repo, "running")
    monkeypatch.chdir(repo)
    assert cli.main(["approve", "--actor=cli:owner", "demo", "spec"]) == exits.USAGE
    assert _kept_now() == []


@pytest.mark.parametrize("status", ["blocked", "failed"])
def test_FlowStop_OnABlockedLane_IsKeptAsStop(repo, trusted, monkeypatch, status):
    _lane(repo, status)
    monkeypatch.chdir(repo)
    assert cli.main(["stop", f"--actor={PERSON_TEXT}", "demo"]) == exits.OK
    [entry] = _kept_now()
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["kind"], entry["task"]) == ("stop", "blocked", "demo")


def test_FlowStop_OnALaneThatWaitsForNobody_KeepsNothing(repo, monkeypatch):
    _lane(repo, "running")
    monkeypatch.chdir(repo)
    assert cli.main(["stop", "--actor=cli:owner", "demo"]) == exits.OK
    assert _kept_now() == []


def test_FlowResume_OnABlockedLane_IsKeptAsResume(repo, trusted, monkeypatch):
    _lane(repo, "blocked")
    monkeypatch.chdir(repo)
    monkeypatch.setattr(cli, "_drive", lambda *args, **kwargs: exits.OK)
    assert cli.main(["resume", f"--actor={PERSON_TEXT}", "demo"]) == exits.OK
    [entry] = _kept_now()
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["kind"], entry["task"]) == ("resume", "blocked", "demo")


def test_FlowMark_KoWithANote_IsKeptWithTheNoteAndTheRowsOwnTitle(repo, trusted, monkeypatch):
    owner_task(repo, ("R1",))
    monkeypatch.chdir(repo)
    assert cli.main(["mark", "demo", "R1", "ko", "--note", "Button is cut off.", f"--actor={PERSON_TEXT}"]) == exits.OK
    [entry] = _kept_now()
    assert entry["place"] == "phone"
    assert (entry["answer"], entry["text"], entry["kind"], entry["title"]) == ("mark_ko", "Button is cut off.", "owner_row", "Try it yourself: look at R1")


def test_FlowMark_Ok_IsKeptOnceForTheRowItMarked(repo, trusted, monkeypatch):
    owner_task(repo, ("R1", "R2"))
    monkeypatch.chdir(repo)
    assert cli.main(["mark", "demo", "R1", "ok", f"--actor={PERSON_TEXT}"]) == exits.OK
    [entry] = _kept_now()
    assert (entry["answer"], entry["text"], entry["key"][4].split("@")[0]) == ("mark_ok", None, "R1")


def test_FlowMark_Refused_KeepsNothing(repo, monkeypatch):
    owner_task(repo, ("R1",))
    monkeypatch.chdir(repo)
    assert cli.main(["mark", "demo", "R9", "ok", "--actor=cli:owner"]) == exits.USAGE
    assert _kept_now() == []


# -- the snapshot -----------------------------------------------------------------------------------------------------


def test_Collect_ReadsTheDecidedAsksOfTheStateFolder(tmp_path):
    _record(decided.default_state_dir(), text="done", at=datetime.now(timezone.utc))
    found = snapshot.collect([], datetime.now(timezone.utc))
    assert [entry["answer"] for entry in found.decided] == ["approve"]


# -- an answer kept after its ask is gone is still said in words

def test_FallbackWait_AnApprovalAtAGate_HasTheTitleOfThatAskAndTheLanesTitle(repo):
    from sakura.flow import tasks

    ctx = make_context(repo, task="count-chars", state={tasks.TITLE_KEY: "Count characters"})

    fields = decided.fallback_wait(repo, ctx.store.task_id, "approval", "overlap")

    assert fields["title"] == "Delivery overlaps another lane" and fields["lane_title"] == "Count characters"
    assert "overlap gate" not in json.dumps(fields) and fields["task"] == "count-chars"


@pytest.mark.parametrize("gate,title", [("spec", "Read the spec and approve it"), ("final-review", "Deliver although the review still wants changes?"),
                                        ("opus:T2", "Use the strongest model for a task?"), ("some-new-gate", "Approve to continue")])
def test_FallbackWait_EveryGateHasItsOwnWords(repo, gate, title):
    ctx = make_context(repo, task="demo")

    assert decided.fallback_wait(repo, ctx.store.task_id, "approval", gate)["title"] == title


def test_FallbackWait_ABlockedLaneWithNoTitleHasEmptyLaneTitle(repo):
    ctx = make_context(repo, task="demo")

    fields = decided.fallback_wait(repo, ctx.store.task_id, "blocked", "blocked")

    assert fields["title"] == "This lane is stuck" and fields["lane_title"] == ""


def test_WaitFields_CarryTheLanesTitle(repo):
    wait = replace(wait_target("approval", "spec@rev1", "t1").wait, lane_title="Count lines")

    assert decided.wait_fields(wait)["lane_title"] == "Count lines"

import io
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from sakura import actors
from sakura import cli as sakura_cli
from sakura.flow import notify
from sakura.outside import access, cli, inbox, records, store, verbs

NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
PERSON = actors.Actor("cli", "me")


class FakePusher:
    """Records what would have paged the person; `answer` is what the real pusher would return."""

    def __init__(self, answer=notify.PUSHED):
        self.calls, self.answer = [], answer

    def push(self, event, title, message):
        self.calls.append((event, title, message))
        return self.answer


class Run:
    """Runs `sakura outside` in-process with a fake clock and pusher and returns code and answer."""

    def __init__(self, tmp_path):
        self.now, self.pusher, self.env, self.cwd = NOW, FakePusher(), {}, tmp_path

    def __call__(self, *argv, agent="claude"):
        out = io.StringIO()
        extra = ["--agent", agent] if agent else []
        code = cli.main([argv[0], *extra, *argv[1:]], now=lambda: self.now, pusher=self.pusher, env=self.env,
                        cwd=self.cwd, stdout=out)
        return code, json.loads(out.getvalue())


@pytest.fixture
def run(tmp_path, monkeypatch):
    caller = Run(tmp_path)
    monkeypatch.setattr(access, "resolve", lambda name, token: access.Grant(
        records.agent_here(name, caller.cwd), frozenset(verbs.VERBS)))
    return caller


def open_demo(run):
    return run("open", "demo", "--title", "Demo lane")


def test_OutsideCli_OpenRowAskTwice_SecondUnchangedNoDuplicate(run):
    calls = [("open", "demo", "--title", "Demo lane"),
             ("row", "demo", "version-prints", "--title", "Version", "--step", "Run it", "--step", "Read it",
              "--expected", "1.0"),
             ("ask", "demo", "version-source", "--text", "Tag or date?", "--choice", "tag", "--choice", "date")]
    first = [run(*call) for call in calls]
    second = [run(*call) for call in calls]
    assert [answer["result"] for _, answer in first] == ["opened"] * 3
    assert [answer["result"] for _, answer in second] == ["unchanged"] * 3
    assert all(code == 0 for code, _ in first + second)
    code, listed = run("list", agent=None)
    assert code == 0 and len(listed["lanes"]) == 1 and listed["lanes"][0]["open_rows"] == 1
    _, shown = run("show", "demo")
    assert [line["kind"] for line in shown["log"]] == ["opened", "row", "asked"]


def test_OutsideCli_NewQuestion_PushesOnceRetryDoesNot(run):
    open_demo(run)
    ask = ("ask", "demo", "flag-short", "--text", "Also accept -h?", "--choice", "yes", "--choice", "no")
    _, first = run(*ask)
    _, again = run(*ask)
    assert first["push"] == "sent" and again["push"] == "skipped"
    assert len(run.pusher.calls) == 1
    event, title, message = run.pusher.calls[0]
    assert "claude" in title and "Demo lane" in title and message == "Also accept -h?"


def test_OutsideCli_SecondQuestionWithinTenMinutes_NoPush(run):
    open_demo(run)
    run("ask", "demo", "one", "--text", "First?")
    run.now = NOW + timedelta(minutes=9)
    _, second = run("ask", "demo", "two", "--text", "Second?")
    assert second["push"] == "skipped" and len(run.pusher.calls) == 1
    run.now = NOW + timedelta(minutes=11)
    _, third = run("ask", "demo", "three", "--text", "Third?")
    assert third["push"] == "sent" and len(run.pusher.calls) == 2


@pytest.mark.parametrize("returned,name", [(notify.DEFERRED, "deferred"), (notify.NO_TOPIC, "skipped"),
                                           (notify.FAILED, "failed")])
def test_OutsideCli_PushOutcomes_NamedInTheAnswer(run, returned, name):
    run.pusher.answer = returned
    open_demo(run)
    code, answer = run("ask", "demo", "q", "--text", "Why?")
    assert code == 0 and answer["push"] == name and answer["result"] == "opened"


def test_OutsideCli_PusherRaises_AskStillSucceedsWithPushFailed(run):
    class Broken:
        def push(self, *arguments):
            raise OSError("no network")

    run.pusher = Broken()
    open_demo(run)
    code, answer = run("ask", "demo", "q", "--text", "Why?")
    assert code == 0 and answer["push"] == "failed" and answer["result"] == "opened"


def test_OutsideCli_InboxWithoutAfter_AcknowledgesNothing(run):
    open_demo(run)
    inbox.steer(store.path(), PERSON, "demo", "use the tag", NOW)
    for _ in range(2):
        code, page = run("inbox", "demo")
        assert code == 0 and [entry["text"] for entry in page["entries"]] == ["use the tag"]
    cursor = page["cursor"]
    _, after = run("inbox", "demo", "--after", str(cursor))
    assert after["entries"] == [] and after["cursor"] == cursor
    _, again = run("inbox", "demo")
    assert again["entries"] == []


def test_OutsideCli_InboxWait_ZeroReturnsAtOnce(run):
    open_demo(run)
    code, page = run("inbox", "demo", "--wait", "0")
    assert code == 0 and page["entries"] == [] and page["closed"] is False


def test_OutsideCli_WaitOverThreeHundred_Refused(run):
    open_demo(run)
    code, answer = run("inbox", "demo", "--wait", "301")
    assert code == 2 and "300" in answer["reason"]


@pytest.mark.parametrize("argv", [("open", "demo", "--title", "x"), ("row", "demo", "r", "--title", "x", "--expected", "e"),
                                  ("inbox", "demo")])
def test_OutsideCli_InsideEngineRole_Refused(run, argv, tmp_path):
    run.env = {"SAKURA_FLOW_ROLE": "builder"}
    code, answer = run(*argv)
    assert code == 2 and "engine role" in answer["reason"]
    assert not (tmp_path / "engine-home" / "outside").exists()


def test_OutsideCli_InsideEngineRole_ReadsAreRefused(run):
    run.env = {"SAKURA_FLOW_ROLE": "builder"}
    code, answer = run("list", agent=None)
    assert code == 2 and answer["result"] == "refused"


def test_OutsideCli_EngineHomeNotWritable_RefusedNamingTheMcpServer(run, monkeypatch):
    def denied(*arguments, **keywords):
        raise store.StoreError("cannot write") from PermissionError(1, "Operation not permitted")

    monkeypatch.setattr(store, "open_lane", denied)
    code, answer = open_demo(run)
    assert code == 2 and answer["result"] == "refused" and "sakura outside serve" in answer["reason"]


def test_OutsideCli_StoreFileUnreadable_ExitFour(run):
    open_demo(run)
    (store.path() / "demo" / "lane.json").write_text("{broken")
    code, answer = run("row", "demo", "r", "--title", "x", "--step", "s", "--expected", "e")
    assert code == 4 and answer["result"] == "error"


def test_OutsideCli_NoAgentName_Refused(run):
    code, answer = run("open", "demo", "--title", "x", agent=None)
    assert code == 2 and "--agent" in answer["reason"]
    run.env = {"SAKURA_OUTSIDE_AGENT": "script"}
    code, answer = run("open", "demo", "--title", "x", agent=None)
    assert code == 0 and store.read(store.path(), "demo").agent.name == "script"


def test_OutsideCli_AnotherAgentsLane_RefusedNamingIt(run):
    open_demo(run)
    code, answer = run("row", "demo", "x", "--title", "t", "--step", "s", "--expected", "e", agent="script")
    assert code == 2 and "claude" in answer["reason"]


def test_OutsideCli_ValueStartingWithDash_WrittenWithEquals(run):
    code, _ = run("open", "demo", "--title=-x")
    assert code == 0 and store.read(store.path(), "demo").title == "-x"


def test_OutsideCli_LinkWithoutEquals_RefusedAndHttpsChecked(run):
    assert run("open", "demo", "--title", "t", "--link", "nolabel")[0] == 2
    assert run("open", "demo", "--title", "t", "--link", "PR=javascript:x")[0] == 2
    assert run("open", "demo", "--title", "t", "--link", "PR=https://github.com/x/y/pull/1")[0] == 0


def test_OutsideCli_WhereIsTheMainWorktreeOfTheAgentsRepository(run, repo):
    run.cwd = repo
    open_demo(run)
    assert store.read(store.path(), "demo").agent.where == repo


@pytest.mark.parametrize("verb", ["mark", "reply", "steer"])
def test_OutsideCli_MarkVerb_NotOffered(run, verb, capsys):
    open_demo(run)
    code, answer = run(verb, "demo", "x")
    assert code == 2 and answer["result"] == "refused" and "invalid choice" in answer["reason"]


def test_OutsideCli_ActorOption_Refused(run):
    open_demo(run)
    for argv in (("close", "demo", "--actor=cli:me"), ("open", "other", "--title", "t", "--actor", "cli:me")):
        code, answer = run(*argv)
        assert code == 2 and "unrecognized" in answer["reason"]
    assert store.read(store.path(), "demo").closed_at is None


def test_OutsideCli_AgentCannotMarkItsOwnRow_NoLogLine(run):
    open_demo(run)
    _, row = run("row", "demo", "r", "--title", "t", "--step", "s", "--expected", "e")
    run("mark", "--verdict=ok", "--", "demo", "r", row["digest"])
    assert [line["kind"] for line in store.log(store.path(), "demo")] == ["opened", "row"]


def test_OutsideCli_CloseOtherAgentsLane_Refused(run):
    open_demo(run)
    code, answer = run("close", "demo", "--outcome", "done", agent="script")
    assert code == 2 and "claude" in answer["reason"]
    assert store.read(store.path(), "demo").closed_at is None


def test_OutsideCli_CloseOwnLane_ClosedThenUnchanged(run):
    open_demo(run)
    assert run("close", "demo", "--outcome", "done")[1]["result"] == "closed"
    assert run("close", "demo", "--outcome", "done")[1]["result"] == "unchanged"
    assert run("list", "--closed", agent=None)[1]["lanes"][0]["lane"] == "demo"


def test_OutsideCli_UnknownVerbOrMissingArgument_RefusedNotCrashed(run):
    out = io.StringIO()
    assert cli.main(["frobnicate"], stdout=out) == 2 and json.loads(out.getvalue())["result"] == "refused"
    assert run("open", "demo")[0] == 2


def test_Dispatch_Outside_ReachesOutsideCli(tmp_path, capsys):
    assert sakura_cli.main(["outside", "list"]) == 0
    assert json.loads(capsys.readouterr().out) == {"result": "lanes", "lanes": []}
    assert "outside" in sakura_cli.ENGINE_COMMANDS_HELP


def test_OutsideCli_RowWithNoExpected_IsAcceptedAndKeepsNone(run):
    open_demo(run)
    code, answer = run("row", "demo", "r", "--title", "t", "--step", "s")
    assert code == 0 and answer["result"] == "opened"

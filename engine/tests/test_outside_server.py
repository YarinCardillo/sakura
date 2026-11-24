import io
import json
from datetime import datetime, timezone

import pytest

from sakura import actors
from sakura.flow import notify
from sakura.mcp_stdio import MAX_LINE
from sakura.outside import access, cli, inbox, records, server, store

NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)
VERBS = ["open", "row", "ask", "close", "inbox", "list", "show"]
PERSON = actors.Actor("cli", "me")


class Pusher:
    def __init__(self):
        self.calls = []

    def push(self, event, title, message):
        self.calls.append(message)
        return notify.PUSHED


class Session:
    """One MCP session of the outside server, driven with JSON-RPC lines."""

    def __init__(self, tmp_path, name="claude", env=None):
        self.tmp_path, self.name, self.env, self.pusher, self.next_id = tmp_path, name, env or {}, Pusher(), 0

    def talk(self, *messages, raw=None):
        stdin = io.StringIO(raw if raw is not None else "".join(json.dumps(message) + "\n" for message in messages))
        stdout = io.StringIO()
        server.serve(records.agent_here(self.name, self.tmp_path), stdin, stdout, now=lambda: NOW,
                     pusher=self.pusher)
        return [json.loads(line) for line in stdout.getvalue().splitlines()]

    def call(self, tool, **arguments):
        return self.calls((tool, arguments))[0]

    def calls(self, *calls):
        messages = []
        for tool, arguments in calls:
            self.next_id += 1
            messages.append({"jsonrpc": "2.0", "id": self.next_id, "method": "tools/call",
                             "params": {"name": tool, "arguments": arguments}})
        answers = self.talk(*messages)
        return [(json.loads(answer["result"]["content"][0]["text"]), answer["result"]["isError"])
                for answer in answers]


@pytest.fixture
def session(tmp_path):
    return Session(tmp_path)


def test_OutsideServer_ToolsList_ExactlyTheSevenVerbs(session):
    answer = session.talk({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})[0]["result"]
    assert [tool["name"] for tool in answer["tools"]] == VERBS
    inbox_schema = next(tool for tool in answer["tools"] if tool["name"] == "inbox")["inputSchema"]
    assert sorted(inbox_schema["properties"]) == ["lane", "wait"]
    assert all(tool["description"] and tool["inputSchema"]["type"] == "object" for tool in answer["tools"])


def test_OutsideMcp_ToolsList_HasNoPersonVerbs(session):
    answer = session.talk({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})[0]["result"]
    names = {tool["name"] for tool in answer["tools"]}
    assert not names & {"mark", "reply", "steer", "approve", "act"}
    answers = session.talk({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "mark",
                                                                                          "arguments": {}}})
    assert answers[0]["error"]["code"] == -32602


def test_OutsideServer_Call_SameDocumentAsCli(session, tmp_path, monkeypatch):
    monkeypatch.setattr(access, "resolve", lambda name, token: access.Grant(records.Agent(name, tmp_path), frozenset(VERBS)))
    document, is_error = session.call("open", lane="demo", title="Demo lane")
    out = io.StringIO()
    cli.main(["open", "demo", "--agent", "claude", "--title", "Demo lane"], now=lambda: NOW, cwd=tmp_path, stdout=out)
    assert (document["result"], is_error) == ("opened", False)
    assert json.loads(out.getvalue()) == {**document, "result": "unchanged"}
    listed, _ = session.call("list")
    out = io.StringIO()
    cli.main(["list"], now=lambda: NOW, cwd=tmp_path, stdout=out)
    assert listed == json.loads(out.getvalue())


def test_OutsideServer_Refusal_IsErrorWithTheCliDocument(session):
    session.call("open", lane="demo", title="Demo")
    document, is_error = Session(session.tmp_path, "script").call("row", lane="demo", row="r", title="t",
                                                                  steps=["s"], expected="e")
    assert is_error and document["result"] == "refused" and "claude" in document["reason"]


def test_OutsideServer_AskWithChoices_PushesOnce(session):
    session.call("open", lane="demo", title="Demo")
    first, _ = session.call("ask", lane="demo", question="q", text="Which?", choices=["a", "b"])
    again, _ = session.call("ask", lane="demo", question="q", text="Which?", choices=["a", "b"])
    assert (first["push"], again["push"]) == ("sent", "skipped") and session.pusher.calls == ["Which?"]


def test_OutsideServer_SecondInboxCall_AcknowledgesFirst(session):
    session.call("open", lane="demo", title="Demo")
    inbox.steer(store.path(), PERSON, "demo", "one", NOW)
    first, second = session.calls(("inbox", {"lane": "demo"}), ("inbox", {"lane": "demo"}))
    assert [entry["text"] for entry in first[0]["entries"]] == ["one"]
    assert second[0]["entries"] == [] and store.read(store.path(), "demo").read_up_to == first[0]["cursor"]


def test_OutsideServer_LastRead_LaneShowsNothingUnreadYetNextSessionStillGetsIt(session):
    session.call("open", lane="demo", title="Demo")
    inbox.steer(store.path(), PERSON, "demo", "one", NOW)
    assert len(session.call("inbox", lane="demo")[0]["entries"]) == 1

    lane, log = store.read(store.path(), "demo"), store.log(store.path(), "demo")
    assert inbox.unread(lane, log) == 0 and lane.read_up_to == 0
    assert len(Session(session.tmp_path).call("inbox", lane="demo")[0]["entries"]) == 1


def test_OutsideServer_NewSession_GetsUnacknowledgedAgain(session):
    session.call("open", lane="demo", title="Demo")
    inbox.steer(store.path(), PERSON, "demo", "one", NOW)
    assert len(session.call("inbox", lane="demo")[0]["entries"]) == 1
    assert len(Session(session.tmp_path).call("inbox", lane="demo")[0]["entries"]) == 1
    both = Session(session.tmp_path).calls(("inbox", {"lane": "demo"}), ("inbox", {"lane": "demo"}))
    assert [len(document["entries"]) for document, _ in both] == [1, 0]


def test_OutsideServer_InboxIgnoresAnAfterTheModelSends(session):
    session.call("open", lane="demo", title="Demo")
    inbox.steer(store.path(), PERSON, "demo", "one", NOW)
    document, _ = session.call("inbox", lane="demo", after=99)
    assert len(document["entries"]) == 1


def test_OutsideServer_LineOver64KiB_ErrorNextLineServed(session):
    huge = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                       "params": {"name": "open", "arguments": {"lane": "demo", "title": "t", "about": "x" * MAX_LINE}}})
    ping = json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping"})
    answers = session.talk(raw=huge + "\n" + ping + "\n")
    assert answers[0]["error"]["code"] == -32600 and answers[1]["result"] == {}
    assert store.lanes(store.path()) == []


def test_OutsideServer_WaitOverFifty_Refused(session):
    session.call("open", lane="demo", title="Demo")
    document, is_error = session.call("inbox", lane="demo", wait=51)
    assert is_error and document["result"] == "refused" and "50" in document["reason"]


def test_OutsideServer_WaitZeroToFifty_Accepted(session):
    session.call("open", lane="demo", title="Demo")
    document, is_error = session.call("inbox", lane="demo", wait=0)
    assert not is_error and document["entries"] == []


def test_OutsideServer_InsideEngineRole_RefusesToStart(tmp_path, capsys):
    code = cli.main(["serve", "--agent", "claude"], env={"SAKURA_FLOW_ROLE": "builder"}, cwd=tmp_path)
    assert code == 2 and "engine role" in capsys.readouterr().err


def test_OutsideServer_NoAgentName_RefusesToStart(tmp_path, capsys):
    assert cli.main(["serve"], env={}, cwd=tmp_path) == 2
    assert "--agent" in capsys.readouterr().err


def test_OutsideServer_WhereIsTheFolderItStartedIn(repo):
    assert records.agent_here("claude", repo).where == repo


def test_outside_bridge_requires_a_host_grant_before_opening_stdio(tmp_path, monkeypatch):
    sessions = []
    monkeypatch.setattr(server, "serve", lambda *args, **kwargs: sessions.append(args))
    assert server.main(["--agent", "reader"], env={}, cwd=tmp_path) != 0
    assert sessions == []


def test_outside_listing_is_scoped_to_the_assigned_agent(session):
    session.call("open", lane="mine", title="Mine")
    Session(session.tmp_path, "second").call("open", lane="theirs", title="Theirs")

    document, is_error = session.call("list")

    assert not is_error and [lane["lane"] for lane in document["lanes"]] == ["mine"]


def test_host_grant_only_lists_its_issued_verbs(tmp_path, monkeypatch):
    principal = records.Agent("reader", tmp_path)
    monkeypatch.setattr(access, "resolve", lambda name, token: access.Grant(principal, frozenset({"list"})))
    incoming = io.StringIO('{"id":1,"method":"tools/list"}\n')
    outgoing = io.StringIO()
    monkeypatch.setattr(server.sys, "stdin", incoming)
    monkeypatch.setattr(server.sys, "stdout", outgoing)

    assert server.main(["--agent", "reader"], cwd=tmp_path, env={}) == 0
    [listed] = json.loads(outgoing.getvalue())["result"]["tools"]
    assert listed["name"] == "list"

import json
from datetime import UTC, datetime

from sakura.cli import main
from sakura.cockpit import snapshot
from sakura.flow import brakes, exits, registry


def invoke(capsys, *args, expected=exits.OK):
    assert main(["track", *args, "--json"]) == expected
    return json.loads(capsys.readouterr().out)


def test_track_cli_round_trip_and_attribution(repo, capsys):
    registry.remember(repo)
    assert invoke(capsys, "list") == []
    track = invoke(capsys, "new", "--name=Shipping", "--goal=Release", f"--project={repo}", "--op=new")
    identity = track["id"]
    assert invoke(capsys, "set", identity, "--status=paused", "--summary=Work\nin progress",
                  "--op=edit")["status"] == "paused"
    key = '{"provider":"p","account":"a","session":"s"}'
    assert invoke(capsys, "link", identity, f"--conversation={key}", "--title=Planning", "--op=link")["title"] == "Planning"
    decision = invoke(capsys, "decision", "add", identity, "--text=Ready?", "--op=question")
    reply = ("decision", "settle", identity, "--decision=1", "--answer=Yes\nShip", "--op=answer")
    assert decision["number"] == 1
    assert invoke(capsys, *reply) == invoke(capsys, *reply)
    record = invoke(capsys, "show", identity, "--audit")
    assert record["decisions"][0]["answer"] == "Yes\nShip"
    assert [entry["operation"] for entry in record["audit"]].count("settle_decision") == 1
    [done] = snapshot.collect([repo], datetime.now(UTC)).decided
    assert done["place"] == "cli" and done["text"] == "Yes\nShip"
    invoke(capsys, "unlink", identity, f"--conversation={key}", "--op=unlink")
    assert invoke(capsys, "show", identity)["conversations"] == []


def test_track_cli_refuses_unknown_project_and_role_mutation(monkeypatch, capsys):
    refused = invoke(capsys, "new", "--name=A", "--goal=B", "--project=/unknown", "--op=new", expected=exits.USAGE)
    assert refused["reason_code"] == "not_found"
    monkeypatch.setenv(brakes.ROLE_ENV, "builder")
    refused = invoke(capsys, "new", "--name=A", "--goal=B", "--op=new", expected=exits.USAGE)
    assert refused["reason_code"] == "forbidden"

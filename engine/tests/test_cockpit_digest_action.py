"""The "Digest now" action (`G`): the phone page's button, the engine command it runs, and who may press it."""

import io
import json

import pytest

from sakura import engine_home
from sakura.cockpit import actions, api, host_actions, pages, remote
from sakura.digest import cli, config, run
from sakura.flow import exits
from test_cockpit_actions import Spawns, invoke
from test_cockpit_api import PHONE, Engine, OWNER as OWNER_NODE, get_json, post_action
from test_cockpit_server import _post
from test_cockpit_snapshot import make_snapshot

HOST = host_actions.DIGEST


def _configured():
    path = engine_home.root() / config.CONFIG_FILE
    path.write_text('[[source]]\nid = "work"\nrun = ["/opt/work.sh"]\n')
    path.chmod(0o600)


def test_DigestNow_Action_RunsTheDetachedCommandWithNoInputs():
    result, spawns = invoke("G", HOST)
    argv = spawns.calls[0][0]
    assert result.exit_code == 0 and argv[1:4] == ["digest", "run", "--detach"] and argv[-1] == "--"
    assert any(word.startswith("--actor=") for word in argv)
    assert spawns.calls[0][1]["cwd"].as_posix() == "/"
    assert actions.ACTIONS["G"].label == "Digest now" and actions.applicable(HOST) == [actions.ACTIONS["G"]]
    with pytest.raises(actions.ActionRefused, match="takes no inputs"):
        invoke("G", HOST, {"text": "x"})


def test_DigestNow_NoOtherTargetTakesIt(tmp_path):
    from test_cockpit_actions import flow_item
    with pytest.raises(actions.ActionRefused, match="does not apply"):
        invoke("G", flow_item(tmp_path, "t1"))


def test_DigestNow_RunsDetached_OnceAtATime():
    _configured()
    spawned = []

    def go():
        out = io.StringIO()
        return cli.main(["run", "--detach", "--actor=overlay:owner@example.com:nPHONE", "--"], env={}, stdout=out,
                        spawn=spawned.append), out.getvalue()

    code, text = go()
    assert code == exits.OK and "started in the background" in text and len(spawned) == 1
    assert spawned[0][1:] == ["digest", "run"]  # the child runs in the foreground of its own session
    with run._lock() as is_free:  # a run is going: the next press is refused at once and nothing is started
        assert is_free
        code, text = go()
    assert (code, text.strip()) == (exits.BUSY, "another digest run is in progress") and len(spawned) == 1


def test_DigestNow_BusyAnswer_IsARefusalTheClientSees(tmp_path):
    engine = Engine(tmp_path, spawns=Spawns(stdout="another digest run is in progress\n", returncode=exits.BUSY))
    try:
        status, _headers, body = post_action(engine, {"key": "G", "target": _phone_target(engine)})
    finally:
        engine.server.server_close()
    assert status == 200 and body == {"result": "refused", "reason": remote.REFUSED}


def test_DigestNow_FromAnAllowedPhone_ReachesTheCommand(tmp_path):
    engine = Engine(tmp_path, own_node_id=OWNER_NODE.node_id, allow_nodes=(OWNER_NODE.node_id, PHONE.node_id))
    engine.who = PHONE
    try:
        status, _headers, body = _post(engine, {"key": "G", "target": _phone_target(engine)}, path="/api/action",
                                       csrf=_csrf(PHONE))
    finally:
        engine.server.server_close()
    assert status == 200 and json.loads(body)["result"] == "done" and len(engine.spawns.calls) == 1


def _phone_target(engine):
    return get_json(engine, "/api/snapshot")[2]["host"]["id"]


def _csrf(identity):
    import time

    from sakura.cockpit import gate
    from test_cockpit_server import SECRET

    return gate.csrf_token(SECRET, identity, time.time())


@pytest.mark.parametrize("path", ["/api/action", "/action"])
def test_DigestNow_RefusedFromTheMacsOwnNode(tmp_path, path):
    engine = Engine(tmp_path, own_node_id=OWNER_NODE.node_id, allow_nodes=(OWNER_NODE.node_id, PHONE.node_id))
    try:
        status, _headers, body = _post(engine, {"key": "G", "target": _phone_target(engine)}, path=path)
    finally:
        engine.server.server_close()
    assert status == 200 and engine.spawns.calls == [] and b"sakura cockpit act" in body


def test_DigestNow_Snapshot_OffersTheHostActionAndThePageTheButton():
    document = api.snapshot_document(make_snapshot())
    assert document["host"] == {"id": '["host","digest"]', "key": ["host", "digest"],
                                "actions": [{"key": "G", "label": "Digest now", "needs": [], "optional": [],
                                             "confirm": False}]}
    page = pages.index(make_snapshot(), OWNER_NODE, "csrf-token")
    assert 'name="key" value="G"' in page and "Digest now" in page and 'value="[&quot;host&quot;, &quot;digest&quot;]"' in page

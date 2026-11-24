import json
from datetime import UTC, datetime

from sakura import engine_home, grants
from sakura.connectors import kinds, registry, tools

NOW = datetime(2026, 11, 23, tzinfo=UTC)


def test_broker_limits_each_role_and_scheduled_selection_and_rechecks_live_grants():
    calls = []

    def page(connector, access, after, timeout):
        calls.append(connector.id)
        return kinds.Page([{"resource": "repo", "item": {"id": "pr", "title": "A pull request"}}])

    ports = kinds.Ports(inspect=lambda *args: kinds.Access("account", "repo", "Web", ("comment_pr",)), page=page)
    fields = {"kind": "github", "repo": "acme/web", "whole_repo": True, "label": "Web"}
    master = registry.mutate("add", "one", ports, NOW, fields=fields)["id"]
    builder = registry.mutate("add", "two", ports, NOW, fields=fields)["id"]
    grant_file = engine_home.root() / grants.GRANTS_FILE
    grant_file.write_text(f'[master.connectors]\n{master} = ["read", "comment_pr"]\n{builder} = ["read"]\n'
                          f'[builder.connectors]\n{builder} = ["read"]\n')

    def broker(role, selection=None):
        prepared = tools.prepare(role, grants.load(engine_home.root(), ()).for_class(role), selection)
        return tools.Broker({"role": role, "run": "test-run", "connectors": prepared}, ports=ports)

    scheduled = broker("master", (master,))
    lane = broker("builder")
    assert scheduled.ids("read") == [master] and scheduled.ids("comment_pr") == [master]
    assert lane.ids("read") == [builder] and lane.ids("comment_pr") == []
    assert broker("digest").specs() == []
    assert tools.Broker({"role": "operator", "run": "other", "connectors": {}}).specs() == []
    assert lane.call("comment_pr", {"connector": builder, "target": "1", "text": "Hello"})[1]
    assert scheduled.call("read", {"connector": builder})[1]
    assert not calls
    value, failed = scheduled.call("read", {"connector": master})
    assert not failed and json.loads(value)["items"][0]["id"] == "pr"
    grant_file.write_text("")
    assert scheduled.call("read", {"connector": master})[1]
    assert calls == [master]


def test_policy_cannot_import_connector_authority_from_a_role_file(tmp_path):
    policy = {"role": "builder", "run": "build-1", "connectors": {"cn-example": {"revision": 1, "operations": ["read"]}}}
    env = {"SAKURA_FLOW_ROLE": "builder", "SAKURA_FLOW_RUN": "build-1", "SAKURA_FLOW_TASK": "demo"}
    path = engine_home.root() / "project" / "flow" / "demo" / "cage" / "build-1" / "policy.json"
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps(policy))
    assert tools.trusted_policy(path, policy, env) == policy
    assert "connectors" not in tools.trusted_policy(path, policy, {**env, "SAKURA_FLOW_ROLE": "planner"})
    forged = tmp_path / "policy.json"
    forged.write_text(json.dumps(policy))
    assert "connectors" not in tools.trusted_policy(forged, policy, env)

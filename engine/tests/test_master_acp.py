"""Qualified Master launch fixtures keep the same tools and binding policy."""

from contextlib import contextmanager

import pytest
from acp_engine_support import install
from sakura import engine_home
from sakura.accounts.registry import Account
from sakura.flow import goose_cage, presets
from sakura.master import spec

MODELS = [{"value": "fixture", "efforts": ["low"]}]


def account(tmp_path, provider, name="work"):
    folder = tmp_path / f"{provider}-{name}"
    folder.mkdir()
    return Account(provider, name, name.title(), folder, True, None, (), (),
                   "ollama" if provider == "goose" else None)


def test_master_default_and_wrong_account(tmp_path, monkeypatch):
    from sakura.flow import agent_accounts

    codex = account(tmp_path, "codex")
    monkeypatch.setattr(agent_accounts, "usable", lambda: [codex])
    home = engine_home.root()
    assert spec.prepare(home, presets.resolve("standard")).document()["acp"]["readiness"] == "unavailable"
    prepared = spec.prepare_acp(home, presets.resolve("standard"), harness="claude", account_id="work", slot="tab")
    assert prepared["reason"] == "master_account_unavailable" and prepared["argv"] == []


def test_goose_master_refuses_without_prerequisites(tmp_path, monkeypatch):
    from sakura.accounts import goose, registry

    row = account(tmp_path, "goose")
    monkeypatch.setattr(registry, "load", lambda **_: registry.Registry((row,), False))
    monkeypatch.setattr(goose, "readiness", lambda _: {"readiness": "needs_setup"})
    prepared = spec.prepare_acp(engine_home.root(), presets.resolve("standard"),
                                harness="goose", account_id="work", slot="tab")
    assert prepared["reason"] == "goose_account_connection_check_required"
    assert prepared["argv"] == []


@pytest.mark.parametrize("provider", ["claude", "codex", "goose"])
def test_master_acp_turn_and_reload_keep_instructions_and_tools(tmp_path, monkeypatch, provider):
    from sakura.accounts import goose, registry
    from sakura.flow import agent_accounts
    from sakura.master import runtime

    selected = account(tmp_path, provider)
    trace = install(tmp_path, monkeypatch, provider, models=[] if provider == "goose" else MODELS,
                    master=True, system_prompt=provider == "codex")
    monkeypatch.setattr(agent_accounts, "usable", lambda: [selected])
    monkeypatch.setattr(registry, "load", lambda **_: registry.Registry((selected,), False))
    monkeypatch.setattr(goose, "readiness", lambda _: {"readiness": "ready"})
    monkeypatch.setattr(goose, "lane_identity", lambda _: ["config-identity"])
    if provider == "goose":
        monkeypatch.setattr(goose_cage, "qualify", lambda *_: (goose_cage.PROVIDERS["ollama"], "fixture:3b"))

        @contextmanager
        def socket_free_fixture(installed, provider, upstream, model, root, site, policy, worker, **kwargs):
            names = frozenset(tool.spec["name"] for tool in kwargs["tools"])
            yield goose_cage.Launch(installed.argv, root / "cwd", {}, {"type": "http", "name": "sakura"}, names)
        monkeypatch.setattr(goose_cage, "open_launch", socket_free_fixture)

    home = engine_home.root()
    preset = presets.resolve("standard")
    launch = spec.prepare_acp(home, preset, harness=provider, account_id="work", slot="tab",
                              model="" if provider == "goose" else "fixture")
    assert launch["readiness"] == "ready"
    assert {"projects", "board_post", "lane_open", "knowledge_propose", "scheduled_propose"} <= set(launch["mcp_tools"])
    assert result(runtime.operate(home, "tab", launch["launch_id"], "first")) == ("completed", "fixture reply")
    loaded = runtime.operate(home, "tab", launch["launch_id"], "second")
    assert result(loaded) == ("completed", "fixture reply")
    assert loaded["messages"] == [{"role": "user", "text": "history"}]
    calls = trace()
    opened = [params for method, params, _ in calls if method in ("session/new", "session/load")]
    assert len(opened) == 2 and all(params["mcpServers"][0]["name"] == "sakura" for params in opened)
    prompts = [params["prompt"][0]["text"] for method, params, _ in calls if method == "session/prompt"]
    if provider == "claude":
        assert all(params["_meta"]["systemPrompt"]["append"] for params in opened)
        assert all(params["_meta"]["claudeCode"]["options"]["strictMcpConfig"] for params in opened)
        assert prompts == ["first", "second"]
    elif provider == "codex":
        assert prompts == ["first", "second"]
        assert all(params["systemPrompt"]["mode"] == "append" for params in opened)
    else:
        assert prompts[0].endswith("first") and prompts[0] != "first" and prompts[1] == "second"


def result(value):
    return value["outcome"], value["reply"]


@pytest.mark.parametrize("load", [False, True])
def test_legacy_codex_replay_or_read_only_transcript_without_fresh_session(tmp_path, monkeypatch, load):
    from sakura.flow import agent_accounts
    from sakura.master import history, runtime

    selected = account(tmp_path, "codex")
    trace = install(tmp_path, monkeypatch, "codex", load=load)
    monkeypatch.setattr(agent_accounts, "usable", lambda: [selected])
    day = selected.config_dir / "sessions" / "2026" / "11" / "23"
    day.mkdir(parents=True)
    (day / "rollout-2026-11-23T12-00-00-thread.jsonl").write_text(
        '{"type":"session_meta","payload":{"id":"thread"}}\n'
        '{"type":"event_msg","payload":{"type":"user_message","message":"saved"}}\n')
    home = engine_home.root()
    binding = history.Binding.for_account("codex", "work", selected.config_dir)
    history.import_legacy(home, "tab", binding, "thread", "Saved Codex chat")
    launch = spec.prepare_acp(home, presets.resolve("standard"), harness="codex", account_id="work", slot="tab")
    opened = runtime.operate(home, "tab", launch["launch_id"])
    assert opened["outcome"] == ("replayed" if load else "transcript")
    assert opened["messages"] == [{"role": "user", "text": "history" if load else "saved"}]
    assert opened["title"] == "Saved Codex chat"
    assert "session/new" not in {row[0] for row in trace()}


def test_two_master_slots_keep_separate_policies_and_approvals(tmp_path, monkeypatch):
    from sakura.flow import agent_accounts
    from sakura.master import history, runtime

    selected = account(tmp_path, "codex")
    install(tmp_path, monkeypatch, "codex")
    monkeypatch.setattr(agent_accounts, "usable", lambda: [selected])
    home, preset = engine_home.root(), presets.resolve("standard")
    left, right = [spec.prepare_acp(home, preset, harness="codex", account_id="work", slot=slot)
                   for slot in ("left", "right")]
    assert left["launch_id"] != right["launch_id"]
    assert history.current(home, "left", left["launch_id"])["actor"] != history.current(home, "right", right["launch_id"])["actor"]
    history.opened(home, "left", left["launch_id"], "session")
    tool = {"params": {"sessionId": "session", "toolCall": {"name": "mcp__sakura__board_post"}}}
    assert runtime.permission(home, "left", left["launch_id"], tool).decision == "ask"
    tool["params"]["toolCall"]["name"] = "mcp__sakura__ungranted"
    assert runtime.permission(home, "left", left["launch_id"], tool).decision == "deny"
    assert runtime.permission(home, "right", left["launch_id"], tool).decision == "deny"


def test_cli_explicit_qualified_launch_open_and_app_outcome(tmp_path, monkeypatch, capsys):
    import io
    import json

    from sakura.flow import agent_accounts
    from sakura.master import cli

    selected = account(tmp_path, "codex")
    trace = install(tmp_path, monkeypatch, "codex")
    monkeypatch.setattr(agent_accounts, "usable", lambda: [selected])
    assert cli.main(["spec", "--json", "--harness", "codex", "--account", "work", "--acp", "--slot", "tab"]) == 0
    configured = json.loads(capsys.readouterr().out)
    assert configured["acp"]["readiness"] == "ready"
    assert configured["acp"]["argv"][1:3] == ["master", "acp"]
    assert cli.main(configured["acp"]["argv"][2:], stdin=io.BytesIO(b"{}")) == 0
    assert json.loads(capsys.readouterr().out)["outcome"] == "empty"
    assert cli.main(configured["acp"]["argv"][2:], stdin=io.BytesIO(b'{"prompt":"first"}')) == 0
    outcome = json.loads(capsys.readouterr().out)
    assert outcome["outcome"] == "completed"
    assert cli.main(["history", "record", "--json", "--slot", "tab", "--launch-id", outcome["launch_id"]],
                    stdin=io.BytesIO(json.dumps({"session_id": outcome["session_id"]}).encode())) == 0
    assert json.loads(capsys.readouterr().out)["session_id"] == outcome["session_id"]
    assert [method for method, _, _ in trace()].count("session/prompt") == 1

"""Model catalog parity and account-scoped refresh tests."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from pathlib import Path
from threading import Event

import pytest
from acp_engine_support import install
from sakura import programs
from sakura.flow import acp, model_catalog, rule_agents, rules_cli
from test_flow_model_catalog import (
    CLAUDE_PROGRAM,
    CODEX_PROGRAM,
    FIXTURES,
    _account,
    _program,
)


def test_acp_catalog_matches_native_fixture_catalog(tmp_path, monkeypatch):
    for provider, native, source in (
        ("claude", CLAUDE_PROGRAM, "claude-initialize.json"),
        ("codex", CODEX_PROGRAM, "codex-model-list.json"),
    ):
        account = _account(provider, tmp_path=tmp_path)
        account.config_dir.mkdir()
        program = _program(tmp_path, native, FIXTURES / source, provider)
        legacy = model_catalog.refresh(account, program=program)
        items = [{"value": model.id, "name": model.label, "description": model.description,
                  "resolvedModel": model.resolved, "efforts": list(model.efforts),
                  "default_effort": model.default_effort} for model in legacy.models]
        default = next((model.id for model in legacy.models if model.is_default), "")
        trace = install(tmp_path, monkeypatch, provider, models=items, default=default, native=program)
        fresh = model_catalog.refresh(account, via_acp=True)
        assert [(m.id, m.label, m.description, m.resolved, m.efforts) for m in fresh.models] == [
            (m.id, m.label, m.description, m.resolved, m.efforts) for m in legacy.models]
        if provider == "codex":
            assert fresh.models == legacy.models
        assert [m.id for m in fresh.models if m.is_default] == [default or items[0]["value"]]
        assert model_catalog.read(account) == legacy
        assert model_catalog.read(account, via_acp=True) == fresh
        assert all(row[2] == str(account.config_dir) for row in trace())
        before = trace()
        for _ in range(3):
            rules_cli._kept(account, via_acp=True)
        assert trace() == before


def test_goose_catalog_keeps_provider_scoped_last_good_list(tmp_path, monkeypatch):
    from sakura.flow import acp_catalog

    account = replace(_account("goose", tmp_path=tmp_path), goose_provider="ollama")
    other = replace(account, goose_provider="openai")
    models = (model_catalog.ModelOption("same", "Local model", is_default=True),)
    monkeypatch.setattr(acp_catalog, "fetch", lambda *_: models)
    kept = model_catalog.refresh(account)
    assert model_catalog.read(other) is None
    assert kept.upstream_provider == "ollama"

    def fail(*_):
        raise model_catalog.CatalogError("adapter_timeout")

    monkeypatch.setattr(acp_catalog, "fetch", fail)
    with pytest.raises(model_catalog.CatalogError, match="adapter_timeout"):
        model_catalog.refresh(account)
    assert model_catalog.read(account) == kept
    assert model_catalog.status(account).state == "stale"
    assert model_catalog.status(account).reason == "adapter_timeout"
    assert model_catalog.status(other).state == "missing"


def test_refresh_coalesces_and_keeps_account_switch_independent(tmp_path, monkeypatch):
    from sakura.flow import acp_catalog

    old = _account("codex", "old", tmp_path)
    new = _account("codex", "new", tmp_path)
    started, release = Event(), Event()

    def fetch(account, _):
        if account == old:
            started.set()
            assert release.wait(5)
        return (model_catalog.ModelOption(account.id, account.label, is_default=True),)

    monkeypatch.setattr(acp_catalog, "fetch", fetch)
    with ThreadPoolExecutor(max_workers=1) as pool:
        future = pool.submit(model_catalog.refresh, old, via_acp=True)
        try:
            assert started.wait(5)
            with pytest.raises(model_catalog.CatalogError, match="refresh_in_progress"):
                model_catalog.refresh(old, via_acp=True)
            updated = model_catalog.refresh(new, via_acp=True)
        finally:
            release.set()
        assert future.result().find("old")
    assert model_catalog.read(new, via_acp=True) == updated


def test_rule_effort_and_model_use_selected_goose_account(tmp_path, monkeypatch):
    from sakura.flow import acp_catalog, config

    local = replace(_account("goose", "local", tmp_path), goose_provider="ollama")
    cloud = replace(_account("goose", "cloud", tmp_path), goose_provider="openai")
    monkeypatch.setattr(acp_catalog, "fetch", lambda account, _: (
        model_catalog.ModelOption("shared:3b", account.label, efforts=("high",) if account == cloud else ()),
        model_catalog.ModelOption("other", "Other", efforts=("low",)),
    ))
    for account in (local, cloud):
        model_catalog.refresh(account)
    agents = rule_agents.load([cloud, local])
    cfg = config.load(None, overrides={"roles.builder": "goose:shared:3b",
                                       "step_accounts.build": "local", "efforts.builder": "high"})
    assert rule_agents.efforts_of("build", cfg, agents) == []
    assert rule_agents.check_model("build", "goose:shared:3b", cfg, agents) == "goose:shared:3b"
    assert rule_agents.effort_after_model("build", "builder", "goose:shared:3b", cfg, agents) == {"efforts.builder": ""}


def test_goose_probe_fallback_uses_pinned_model_and_keyless_endpoint(tmp_path, monkeypatch):
    from sakura.accounts import goose
    from sakura.flow import acp_catalog, cage
    from sakura.setup import harnesses

    home = cage.account_home()
    home.mkdir(exist_ok=True)
    folder = home / ".goose-fixture"
    folder.mkdir()
    account = replace(_account("goose", tmp_path=tmp_path), config_dir=folder, goose_provider="ollama")
    trace = install(tmp_path, monkeypatch, "goose", account_var="OLLAMA_HOST")
    monkeypatch.setattr(goose, "readiness", lambda _: {"readiness": "ready"})
    seatbelt = tmp_path / "fixture-sandbox"
    seatbelt.write_text('#!/bin/sh\n[ "$1" = -f ] && [ -f "$2" ] || exit 99\nshift 2\nexec "$@"\n')
    seatbelt.chmod(0o700)
    monkeypatch.setattr(cage, "SEATBELT", str(seatbelt))
    fetched = model_catalog.refresh(account)
    assert [(model.id, model.label, model.efforts) for model in fetched.models] == [("fixture:3b", "fixture:3b (Ollama)", ())]
    assert {row[2] for row in trace()} == {"http://127.0.0.1:11434"}
    assert [row[0] for row in trace()] == ["initialize", "session/new", "session/close"]
    monkeypatch.setattr(harnesses, "qualify", lambda *_: (_ for _ in ()).throw(programs.ProgramError("install_adapter")))
    with pytest.raises(model_catalog.CatalogError, match="install_adapter"):
        acp_catalog.fetch(account, Path(tmp_path))


def test_launch_validates_the_model_before_effort_and_never_prompts_on_invalid_choice(tmp_path, monkeypatch):
    from sakura.setup import harnesses

    models = [{"value": "first", "efforts": ["high"]}, {"value": "second", "efforts": ["low"]}]
    trace = install(tmp_path, monkeypatch, "codex", models=models)
    with harnesses.qualify("codex").connect(tmp_path) as (client, caps):
        turn = acp.Turn("fixture", "1.0.0", "must not run", model="second", effort="high")
        with pytest.raises(acp.AcpError, match="effort_unavailable"):
            acp.Session(client, turn, lambda *_args, **_kwargs: None, lambda: None).run(tmp_path, caps)
    assert [row[0] for row in trace()] == ["initialize", "session/new", "session/set_config_option"]
    assert trace()[-1][1]["value"] == "second"


def test_active_turn_rejects_an_adapter_model_switch(tmp_path, monkeypatch):
    from sakura.setup import harnesses

    models = [{"value": "first", "efforts": []}, {"value": "second", "efforts": []}]
    install(tmp_path, monkeypatch, "codex", models=models, switch_on_prompt="second")
    with harnesses.qualify("codex").connect(tmp_path) as (client, caps):
        turn = acp.Turn("fixture", "1.0.0", "do not deliver", model="first")
        session = acp.Session(client, turn, lambda *_args, **_kwargs: None, lambda: None)
        with pytest.raises(acp.AcpError, match="model_unavailable"):
            session.run(tmp_path, caps)

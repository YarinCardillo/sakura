"""Master bindings preserve history across account changes and process restarts."""

import json

import pytest
from sakura import engine_home


def test_open_outcome_is_atomic_when_tab_changes_account(tmp_path):
    from sakura.master import history

    home = engine_home.root()
    first = history.Binding.for_account("codex", "work", tmp_path)
    second = history.Binding.for_account("claude", "personal", tmp_path)
    old = history.issue(home, "tab", first, "policy")
    history.opened(home, "tab", old["launch_id"], "original")
    active = history.issue(home, "tab", second, "policy")
    with pytest.raises(history.HistoryError, match="master_launch_changed"):
        history.opened(home, "tab", old["launch_id"], "late")
    history.opened(home, "tab", active["launch_id"], "new")
    assert [row["session_id"] for row in history.listed(home, "tab")] == ["original", "new"]
    again = history.issue(home, "tab", first, "policy")
    assert again["session_id"] == "original"


def test_legacy_import_is_idempotent_and_fallback_verifies_transcript_identity(tmp_path):
    from sakura.master import history

    account = tmp_path / "fixture-account"
    day = account / "sessions" / "2026" / "11" / "23"
    day.mkdir(parents=True)
    session_id = "abcdef-123"
    path = day / f"rollout-2026-11-23T10-00-00-{session_id}.jsonl"
    rows = [{"type": "session_meta", "payload": {"id": session_id, "instructions": "private metadata"}},
            {"type": "response_item", "payload": {"type": "message", "role": "assistant",
                                                "content": [{"type": "output_text", "text": "from transcript"}]}},
            {"type": "response_item", "payload": {"type": "function_call", "arguments": "hidden"}}]
    path.write_text("\n".join(map(json.dumps, rows)))
    binding = history.Binding.for_account("codex", "work", account)
    home = engine_home.root()
    for _ in range(2):
        history.import_legacy(home, "tab", binding, session_id, "Prior chat")
    assert len(history.listed(home, "tab")) == 1
    recovered = history.transcript(home, "tab", binding.storage_namespace)
    assert recovered["outcome"] == "transcript" and recovered["title"] == "Prior chat"
    assert recovered["messages"] == [{"role": "assistant", "text": "from transcript"}]
    path.write_text(path.read_text().replace(session_id, "wrong"))
    assert history.transcript(home, "tab", binding.storage_namespace)["reason"] == "master_transcript_unavailable"
    path.unlink()
    decoy = tmp_path / "decoy"
    decoy.write_text("\n".join(map(json.dumps, rows)))
    path.symlink_to(decoy)
    assert history.transcript(home, "tab", binding.storage_namespace)["outcome"] == "unavailable"


def test_failed_replay_requires_explicit_start_fresh_and_keeps_prior_binding(tmp_path):
    from sakura.master import history

    home = engine_home.root()
    binding = history.Binding.for_account("codex", "work", tmp_path)
    history.import_legacy(home, "tab", binding, "thread", "Prior chat")
    existing = history.issue(home, "tab", binding, "policy")
    with pytest.raises(history.HistoryError, match="master_policy_changed"):
        history.issue(home, "tab", binding, "new-policy")
    fresh = history.issue(home, "tab", binding, "new-policy", fresh=True)
    assert existing["session_id"] == "thread" and fresh["session_id"] == ""
    history.opened(home, "tab", fresh["launch_id"], "replacement")
    saved = history.listed(home, "tab")
    assert {(row["session_id"], row["title"]) for row in saved} == {("thread", "Prior chat"), ("replacement", "")}


def test_legacy_fallback_refuses_changed_storage_root(tmp_path):
    from sakura.master import history

    account = tmp_path / "account"
    account.mkdir()
    binding = history.Binding.for_account("codex", "work", account)
    home = engine_home.root()
    history.import_legacy(home, "tab", binding, "thread", "Prior chat")
    account.rename(tmp_path / "old")
    account.mkdir()
    assert history.transcript(home, "tab", binding.storage_namespace)["reason"] == "master_storage_changed"

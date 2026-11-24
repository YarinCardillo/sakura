"""The first-launch action applies the person's choices and can be resumed."""

import json
import os
import sys

import pytest
from leeway_support import account, calls
from sakura import cli, engine_home
from sakura.cockpit import actions, host_actions
from sakura.flow import config, exits, presets, rules
from sakura.usage import leeway
from test_cockpit_actions import invoke

HELPER = """#!{python}
import json
import sys
from pathlib import Path

state = Path({state!r})
args = sys.argv[1:]
with open({log!r}, "a") as log:
    log.write(json.dumps(args) + "\\n")
if args[0] == "version":
    print(json.dumps({{"status_schema": 5, "advice_schema": 2, "accounts_schema": 1, "store_schema": 2, "version": "1.0.0"}}))
    raise SystemExit(0)
entries = json.loads(state.read_text())
if args[:1] == ["status"]:
    template = {status!r}
    rows = [{{**template, **{{key: row[key] for key in ("provider", "id", "label", "config_dir")}}}}
            for row in entries if not row["hidden"]]
    print(json.dumps({{"schema_version": 5, "generated_at": "2026-11-18T12:00:00Z", "accounts": rows}}))
    raise SystemExit(0)
if args[:2] == ["accounts", "add"]:
    folder, provider = args[2], args[4]
    if not any(row["provider"] == provider and Path(row["config_dir"]).resolve() == Path(folder).resolve()
               for row in entries):
        entries.append({{"provider": provider, "id": f"added-{{len(entries) + 1}}", "config_dir": folder,
                        "hidden": False, "label": Path(folder).name, "origin": "added",
                        "folder_exists": True, "order": len(entries)}})
        state.write_text(json.dumps(entries))
elif args[:2] == ["accounts", "rename"]:
    next(row for row in entries if row["id"] == args[2])["label"] = args[3]
    state.write_text(json.dumps(entries))
elif args[:2] != ["accounts", "list"]:
    raise SystemExit(1)
book = [{{key: value for key, value in row.items() if key != "config_dir"}} for row in entries]
print(json.dumps({{"schema_version": 1, "accounts": book}}))
"""


@pytest.fixture
def book(tmp_path, monkeypatch):
    folder = tmp_path / "commands"
    folder.mkdir(mode=0o700)
    helper, state = folder / "leeway", folder / "book.json"
    state.write_text("[]")
    helper.write_text(HELPER.format(python=sys.executable, state=str(state), log=str(helper) + ".log",
                                    status=account("template")[0]))
    helper.chmod(0o700)
    monkeypatch.setenv("PATH", str(folder) + os.pathsep + os.environ["PATH"])
    monkeypatch.setenv(leeway.ENV_VARIABLE, str(helper))
    return helper, state


def _choice(account_home, harness, name):
    folder = account_home / f".{harness}"
    folder.mkdir(mode=0o700)
    return {"harness": harness, "folder": str(folder), "name": name}


def _inputs(*accounts, op="first-launch-01"):
    return {"op": op, "choices": json.dumps({"accounts": accounts, "preset": "standard", "pace": "fast"},
                                           separators=(",", ":"))}


def _apply(inputs):
    return invoke("setupapply", host_actions.ENGINE, inputs)[0]


def _mutations(helper):
    return [call for call in calls(helper) if call[:2] in (["accounts", "add"], ["accounts", "rename"])]


def test_apply_adds_chosen_accounts_with_names(account_home, book, capsys):
    claude = _choice(account_home, "claude", "My Claude")
    claude["folder"] += "/"
    codex = _choice(account_home, "codex", "My Codex")
    choices = _inputs(claude, codex)
    argv = ["cockpit", "act", "setupapply", '["host","engine"]', "--source", "app", "--json",
            "--op", choices["op"], "--choices", choices["choices"]]
    assert cli.main(argv) == exits.OK
    outcome = json.loads(capsys.readouterr().out)
    assert outcome["result"] == "done" and outcome["exit_code"] == exits.OK
    assert json.loads(outcome["output"]) == {"accounts": [
        {"id": "added-1", "provider": "claude", "label": "My Claude", "folder": claude["folder"]},
        {"id": "added-2", "provider": "codex", "label": "My Codex", "folder": codex["folder"]},
    ]}
    assert presets.chosen() == "standard"
    assert config.load(None).values["knobs"]["pace"] == "fast"


def test_apply_twice_same_op_changes_nothing(account_home, book):
    helper, state = book
    inputs = _inputs(_choice(account_home, "claude", "My Claude"))
    first = _apply(inputs)
    assert first.exit_code == exits.OK
    writes = _mutations(helper)
    before = state.stat().st_mtime_ns
    pace_file = engine_home.root() / config.PACE_FILE
    pace_before = pace_file.stat().st_mtime_ns
    assert _apply(inputs).output == first.output
    assert _mutations(helper) == writes
    assert state.stat().st_mtime_ns == before
    assert pace_file.stat().st_mtime_ns == pace_before


@pytest.mark.parametrize("settings", [
    {},
    {"preset": "standard"},
    {"pace": "fast"},
    {"preset": "standard", "pace": "fast"},
], ids=["accounts-only", "with-preset", "with-pace", "with-both"])
def test_apply_updates_only_supplied_settings(account_home, book, settings):
    presets.choose("solo")
    rules.set_pace("slow", None)
    root = engine_home.root()
    files = {"preset": root / presets.CHOICE_FILE, "pace": root / config.PACE_FILE}
    before = {key: path.stat().st_mtime_ns for key, path in files.items()}
    choice = _choice(account_home, "claude", "My Claude")
    inputs = {"op": "look-again", "choices": json.dumps({"accounts": [choice], **settings})}

    result = _apply(inputs)

    assert result.exit_code == exits.OK
    assert json.loads(result.output) == {"accounts": [
        {"id": "added-1", "provider": "claude", "label": "My Claude", "folder": choice["folder"]},
    ]}
    assert presets.chosen() == settings.get("preset", "solo")
    assert config.load(None).values["knobs"]["pace"] == settings.get("pace", "slow")
    after = {key: path.stat().st_mtime_ns for key, path in files.items()}
    assert all(after[key] == before[key] for key in files.keys() - settings.keys())

    retry = _apply(inputs)
    assert retry.exit_code == exits.OK and retry.output == result.output
    assert {key: path.stat().st_mtime_ns for key, path in files.items()} == after
    assert [call[1] for call in _mutations(book[0])] == ["add", "rename"]


@pytest.mark.parametrize("choices", [
    {"preset": "standard", "pace": "fast"},
    {"accounts": [], "preset": None},
    {"accounts": [], "preset": "missing"},
    {"accounts": [], "pace": None},
    {"accounts": [], "pace": "missing"},
    {"accounts": [], "preset": "standard", "pace": "fast", "extra": True},
], ids=["missing-accounts", "null-preset", "unknown-preset", "null-pace", "unknown-pace", "extra-key"])
def test_apply_refuses_invalid_choices(choices):
    argv = ["cockpit", "act", "setupapply", '["host","engine"]', "--source", "app", "--json",
            "--op", "invalid-choices", "--choices", json.dumps(choices)]

    assert cli.main(argv) == exits.USAGE
    root = engine_home.root()
    assert not (root / "setup" / "invalid-choices.json").exists()
    assert not (root / presets.CHOICE_FILE).exists()
    assert not (root / config.PACE_FILE).exists()


@pytest.mark.parametrize("name", ["Claude", "A new name"])
def test_checked_claude_login_reuses_existing_book_entry(account_home, book, name):
    helper, state = book
    choice = _choice(account_home, "claude", name)
    _, entry = account("existing", label="Claude")
    initial = {**entry, "config_dir": choice["folder"]}
    state.write_text(json.dumps([initial]))
    result = _apply(_inputs(choice))
    assert result.exit_code == exits.OK
    assert json.loads(result.output) == {"accounts": [
        {"id": "existing", "provider": "claude", "label": "Claude", "folder": choice["folder"]},
    ]}
    assert json.loads(state.read_text()) == [initial]
    assert _mutations(helper) == []


def test_interrupted_apply_resumes(account_home, book, monkeypatch):
    helper, state = book
    inputs = _inputs(_choice(account_home, "claude", "My Claude"), _choice(account_home, "codex", "My Codex"))
    with monkeypatch.context() as patch:
        def interrupted(*args):
            raise leeway.LeewayUnavailable("the helper stopped")

        patch.setattr(leeway, "account_action", interrupted)
        assert _apply(inputs).exit_code == exits.FAILED
    assert [row["label"] for row in json.loads(state.read_text())] == [".claude"]
    leeway.forget()
    result = _apply(inputs)
    assert result.exit_code == exits.OK
    assert [row["label"] for row in json.loads(result.output)["accounts"]] == ["My Claude", "My Codex"]
    assert [call[2] for call in _mutations(helper) if call[1] == "add"] == [
        str(account_home / ".claude"), str(account_home / ".codex"),
    ]


def test_setup_apply_is_local_only():
    assert actions.is_local_only("setupapply", host_actions.ENGINE.key)


def test_retry_after_add_before_recording_preserves_the_book_name(account_home, book, monkeypatch):
    helper, _ = book
    choice = _choice(account_home, "claude", "My Claude")
    choice["folder"] += "/"
    inputs = _inputs(choice)
    add = leeway.account_add
    with monkeypatch.context() as patch:
        def interrupted(*args):
            add(*args)
            raise leeway.LeewayUnavailable("the helper stopped after saving the book")

        patch.setattr(leeway, "account_add", interrupted)
        assert _apply(inputs).exit_code == exits.FAILED
    leeway.forget()
    result = _apply(inputs)
    assert result.exit_code == exits.OK
    assert json.loads(result.output)["accounts"] == [
        {"id": "added-1", "provider": "claude", "label": ".claude", "folder": choice["folder"]},
    ]
    assert [call[1] for call in _mutations(helper)] == ["add"]


def test_same_op_refuses_different_choices(account_home, book):
    helper, _ = book
    choice = _choice(account_home, "claude", "My Claude")
    assert _apply(_inputs(choice)).exit_code == exits.OK
    writes = _mutations(helper)
    result = _apply(_inputs({**choice, "name": "Another name"}))
    assert result.exit_code == exits.USAGE
    assert _mutations(helper) == writes


def test_preset_error_is_logged_without_exposing_its_path(monkeypatch, capsys):
    from sakura.setup import apply

    path = "/private/example/presets/selected.toml"

    def unreadable(*args):
        raise presets.PresetError(f"cannot read {path}")

    monkeypatch.setattr(presets, "resolve", unreadable)
    code, message = apply.run("bad-preset", {"accounts": [], "preset": "selected"})
    assert code == exits.USAGE and "preset" in message and path not in message
    assert path in capsys.readouterr().err


def _goose_choices(folder, op="goose-setup"):
    return {"op": op, "choices": json.dumps({
        "accounts": [{"harness": "goose", "folder": str(folder), "name": "Local models",
                      "provider_config": {"provider": "ollama"}}],
        "preset": "standard", "pace": "normal",
        "master_account": {"harness": "goose", "folder": str(folder)},
    })}


def test_resume_after_registration_before_master_default_is_saved(account_home, monkeypatch):
    from sakura.accounts import registry
    from sakura.setup import apply

    folder = account_home / ".goose-work"
    folder.mkdir(mode=0o700)
    inputs = _goose_choices(folder)
    with monkeypatch.context() as patch:
        def interrupted(*_):
            raise OSError("interrupted")

        def save_default(path, contents):
            if path.name == apply.MASTER_ACCOUNT_FILE:
                interrupted()
            else:
                replace_atomically(path, contents)

        from sakura.accounts.write import replace_atomically
        patch.setattr(apply.write, "replace_atomically", save_default)
        assert _apply(inputs).exit_code == exits.FAILED
    [registered] = registry.load().accounts
    resumed = _apply(inputs)
    assert resumed.exit_code == exits.OK
    expected = {"provider": "goose", "id": registered.id}
    assert json.loads(resumed.output)["master_account"] == expected
    assert apply.master_account() == expected
    assert registry.load().accounts == (registered,)
    assert _apply(inputs).output == resumed.output


def test_simultaneous_apply_registers_each_checked_folder_once(account_home):
    from concurrent.futures import ThreadPoolExecutor

    from sakura.accounts import registry
    from sakura.setup import apply

    folder = account_home / ".goose-work"
    folder.mkdir(mode=0o700)
    inputs = _goose_choices(folder)
    choices = apply.parse_choices(inputs["op"], inputs["choices"])
    with ThreadPoolExecutor(max_workers=2) as workers:
        outcomes = list(workers.map(lambda _: apply.run(inputs["op"], choices), range(2)))
    assert outcomes[0] == outcomes[1] and outcomes[0][0] == exits.OK
    assert len(registry.load().accounts) == 1


def test_goose_setup_preserves_implicit_subscription_routing(account_home, monkeypatch):
    from pathlib import Path

    from sakura.accounts import registry
    from sakura.flow import agent_accounts

    home = Path.home()
    (home / ".claude" / "projects").mkdir(parents=True)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CODEX_HOME", raising=False)
    before = agent_accounts.usable()
    assert [(account.provider, account.id) for account in before] == [("claude", "default")]
    folder = account_home / ".goose-work"
    folder.mkdir(mode=0o700)
    assert _apply(_goose_choices(folder)).exit_code == exits.OK
    registered = registry.load()
    assert registered.implicit
    [goose] = [account for account in registered.accounts if account.provider == "goose"]
    assert (goose.label, goose.config_dir, goose.goose_provider) == ("Local models", folder, "ollama")
    assert agent_accounts.usable() == [*before, goose]

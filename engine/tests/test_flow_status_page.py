import json
import os
from datetime import datetime, timezone
from pathlib import Path

import pytest

from sakura import engine_home
from sakura.flow import config, engine, plan, status_page
from sakura.flow.store import TaskStore

NOW = datetime(2026, 10, 26, 15, 30, tzinfo=timezone.utc)


def _task(task_id, title="Pagina iniziale", owner_line="", files=("a.py",)):
    return plan.BuildTask(task_id, title, [], ["R1"], list(files), False, owner_line)


def _state(**overrides):
    state = {"task": "t1", "ref": "specs/t1.md", "phase": "build", "status": "running", "reason": "", "history": [],
             "build": {}, "completed": {}, "active_runs": [], "pr": {}}
    state.update(overrides)
    return state


def _render(state, tasks=(), rulings=(), link=None, now=NOW):
    return status_page.render(state, list(tasks), list(rulings), link, now, project="demo")


def _section(text, heading):
    lines = text.splitlines()
    start = lines.index(heading) + 1
    end = next((i for i in range(start, len(lines)) if lines[i].startswith("## ")), len(lines))
    return "\n".join(lines[start:end])


def _history(count):
    return [{"phase": "build", "status": "started", "at": f"2026-10-26T{hour:02d}:00:00+00:00", "detail": f"passo {hour}"}
            for hour in range(count)]


# -- configuration ----------------------------------------------------------


def test_status_dir_defaults_under_owner_dir(tmp_path, monkeypatch):
    home = tmp_path / "me"
    monkeypatch.setenv("HOME", str(home))
    values = config.load(tmp_path / "myrepo", home=home).values
    assert status_page.path(values, "myrepo", "t1") == engine_home.root() / "reports/myrepo/stato/t1.md"
    engine_home.config_file().write_text('[report.owner_dirs]\nmyrepo = "/srv/notes/report"\n')
    values = config.load(tmp_path / "myrepo", home=home).values
    assert status_page.path(values, "myrepo", "t1") == Path("/srv/notes/report/stato/t1.md")


def test_status_dir_set_in_the_personal_file_wins(tmp_path):
    home = tmp_path / "me"
    engine_home.config_file().write_text('[report]\nstatus_dir = "/srv/stato/{repo}"\n')
    values = config.load(tmp_path / "myrepo", home=home).values
    assert status_page.path(values, "myrepo", "t1") == Path("/srv/stato/myrepo/t1.md")


@pytest.mark.parametrize("status_dir", ["", "relative/stato", "   "])
def test_status_dir_relative_is_a_config_error(tmp_path, status_dir):
    with pytest.raises(config.FlowConfigError, match=r"report\.status_dir"):
        config.load(tmp_path, home=tmp_path / "nohome", overrides={"report": {"status_dir": status_dir}})


def test_status_dir_from_a_repository_layer_is_a_config_error(tmp_path):
    repo = tmp_path / "myrepo"
    (repo / ".sakura").mkdir(parents=True)
    (repo / ".sakura/flow.toml").write_text('[report]\nstatus_dir = "/home/me/.claude/rules"\n')
    with pytest.raises(config.FlowConfigError, match=r"report\.status_dir.*personal layer"):
        config.load(repo, home=tmp_path / "me")


# -- plan -------------------------------------------------------------------


def _write_plan(store, **extra):
    entry = {"id": "T1", "title": "Titolo", "depends_on": [], "rows": ["R1"], "files": ["a.py"], "seam": False, **extra}
    store.root.mkdir(parents=True, exist_ok=True)
    store.plan_json.write_text(json.dumps({"tasks": [entry]}))
    store.plan_md.write_text("# Plan\n")
    return plan.load(store, None)[0]


def test_plan_reads_owner_line_and_defaults_to_empty(repo):
    store = TaskStore(repo, "t1")
    assert _write_plan(store, owner_line="La home mostra un saluto").owner_line == "La home mostra un saluto"
    assert _write_plan(store).owner_line == ""


def test_plan_rejects_a_non_string_owner_line(repo):
    store = TaskStore(repo, "t1")
    _write_plan(store)
    with pytest.raises(plan.PlanError):
        _write_plan(store, owner_line=["no"])


def test_planner_prompt_asks_for_owner_line():
    prompt = (Path(plan.__file__).parent / "prompts/plan.md").read_text()
    assert '"owner_line"' in prompt


# -- render -----------------------------------------------------------------


def test_render_groups_build_tasks_by_state_with_counts():
    tasks = [_task("T1", "Uno"), _task("T2", "Due"), _task("T3", "Tre"), _task("T4", "Quattro")]
    build = {"T1": {"status": "merged"}, "T2": {"status": "building"}, "T3": {"status": "pending"}}
    text = _render(_state(build=build), tasks)
    board = _section(text, "## Bacheca")
    assert "### In corso (1)\n- **T2** Due" in board
    assert "### Fatto (1)\n- **T1** Uno" in board
    assert "### Da fare (2)" in board and "**T3** Tre" in board and "**T4** Quattro" in board
    assert "### Bloccato (0)" in board and "### In attesa dell'owner (0)" in board


@pytest.mark.parametrize("status,label", [("blocked", "Bloccato"), ("failed", "Bloccato"),
                                          ("awaiting_approval", "In attesa dell'owner")])
def test_render_moves_unfinished_started_work_by_the_flow_status(status, label):
    tasks = [_task("T1"), _task("T2")]
    state = _state(status=status, reason="why", build={"T1": {"status": "merged"}, "T2": {"status": "review"}})
    board = _section(_render(state, tasks), "## Bacheca")
    assert f"### {label} (1)\n- **T2**" in board
    assert "### Fatto (1)\n- **T1**" in board


def test_render_without_a_plan_says_so():
    assert "Nessun task nel piano" in _section(_render(_state(phase="intake")), "## Bacheca")


def test_render_history_is_newest_first_and_capped():
    state = _state(history=_history(12))
    updates = _section(_render(state), "## Ultimo aggiornamento")
    lines = [line for line in updates.splitlines() if line.startswith("- ")]
    assert len(lines) == status_page.STATUS_HISTORY_ITEMS == 8
    assert "passo 11" in lines[0] and "passo 4" in lines[-1] and "passo 3" not in updates
    assert "2026-10-26 11:00" in lines[0]
    assert "Costruzione" in lines[0] and "avviato" in lines[0]


def test_render_awaiting_approval_says_what_the_owner_must_do():
    state = _state(phase="intake", status="awaiting_approval", reason="awaiting approval: sakura flow approve t1 spec")
    phase = _section(_render(state), "## Fase")
    assert "in attesa del tuo ok" in phase
    assert "`sakura flow approve t1 spec`" in phase


def test_render_blocked_shows_the_reason():
    phase = _section(_render(_state(status="blocked", reason="tests failed: 3")), "## Fase")
    assert "bloccato" in phase and "tests failed: 3" in phase


def test_render_uses_owner_line_and_falls_back_to_title():
    tasks = [_task("T1", "Titolo uno", owner_line="La home saluta chi arriva"), _task("T2", "Titolo due")]
    section = _section(_render(_state(), tasks), "## Task")
    assert "**T1** Titolo uno" in section and "La home saluta chi arriva" in section
    assert "**T2** Titolo due" in section and section.count("Titolo due") == 2
    assert "1 file" in section


def test_render_hostile_owner_line_cannot_break_front_matter_or_markdown():
    hostile = 'x"\n---\nagent: root\n## Bacheca\x1b]2;pwn\x07 <script>alert(1)</script> [a](http://evil) `code`'
    text = _render(_state(), [_task("T1", owner_line=hostile)])
    front, _, body = text[4:].partition("\n---\n")
    assert [line.split(":")[0] for line in front.splitlines()] == [
        "title", "description", "project", "kind", "task", "agent", "updated", "links"]
    assert "\x1b" not in text and "\x07" not in text and "<script>" not in text
    assert text.count("\n## Bacheca") == 1 and "\n---\n" not in body
    assert not any(line.startswith(("agent:", "---")) for line in body.splitlines())


def test_render_front_matter_is_engine_written():
    state = _state(pr={"url": "https://example.test/pull/7"})
    text = _render(state, link="../t1.md")
    front = text.split("\n---\n", 1)[0].splitlines()
    assert front[0] == "---"
    assert front[1] == 'title: "Stato: t1"'
    assert front[2] == 'description: "Stato del flusso per specs/t1.md"'
    assert front[3:] == ["project: demo", "kind: status", "task: t1", "agent: engine",
                         "updated: 2026-10-26T15:30:00+00:00", 'links: ["https://example.test/pull/7"]']
    assert "[Rapporto per il proprietario](../t1.md)" in text


def test_render_lists_rulings():
    rulings = [{"decision": "grant", "kind": "timeout", "path": "tests/slow`.py", "why": "serve piu tempo",
                "cost_if_wrong": "un'ora persa", "spec_change": False}]
    section = _section(_render(_state(), rulings=rulings), "## Decisioni")
    assert "concesso" in section and "serve piu tempo" in section and "un'ora persa" in section
    assert section.count("`") == 2


# -- refresh ----------------------------------------------------------------


def _config(tmp_path, status_dir=None):
    overrides = {"report": {"owner_dir": str(tmp_path / "notes"), "status_dir": str(status_dir or tmp_path / "notes/stato")}}
    return config.load(tmp_path / "nohome-repo", home=tmp_path / "nohome", overrides=overrides)


def _store(repo, **overrides):
    store = TaskStore(repo, "t1")
    store.create(_state(**overrides))
    return store


def test_EngineSources_WriteNoMiddleDotSeparator():
    """The engine writes no title or label joined with a middle dot, which the person's pages do not use."""
    sakura = Path(__file__).resolve().parents[1] / "lib" / "sakura"
    found = [str(path.relative_to(sakura)) for path in sorted(sakura.rglob("*"))
             if path.is_file() and path.suffix in (".py", ".md", ".toml", ".json", ".html", ".js", ".css")
             and "\u00b7" in path.read_text(encoding="utf-8")]
    assert found == []


def test_refresh_writes_the_page_atomically_with_mode_0644(repo, tmp_path):
    store = _store(repo)
    cfg = _config(tmp_path)
    status_page.refresh(store, cfg, now=NOW)
    page = tmp_path / "notes/stato/t1.md"
    assert 'title: "Stato: t1"' in page.read_text() and "project: repo" in page.read_text()
    assert os.stat(page).st_mode & 0o777 == 0o644
    assert [entry.name for entry in page.parent.iterdir()] == ["t1.md"]


def test_refresh_links_the_owner_report_once_it_exists(repo, tmp_path):
    store, cfg = _store(repo), _config(tmp_path)
    status_page.refresh(store, cfg, now=NOW)
    assert "Rapporto per il proprietario" not in (tmp_path / "notes/stato/t1.md").read_text()
    (tmp_path / "notes").mkdir(exist_ok=True)
    (tmp_path / "notes/t1.md").write_text("report\n")
    status_page.refresh(store, cfg, now=NOW)
    assert "[Rapporto per il proprietario](../t1.md)" in (tmp_path / "notes/stato/t1.md").read_text()


def test_refresh_runs_after_every_state_write(repo, tmp_path):
    store, cfg = _store(repo), _config(tmp_path)
    store.set_state_observer(lambda written: status_page.refresh(written, cfg))
    page = tmp_path / "notes/stato/t1.md"
    assert not page.exists()
    store.update_state(lambda state: state.update(phase="plan", status="awaiting_approval", reason="r"))
    assert "in attesa del tuo ok" in page.read_text()
    store.write_state({**store.read_state(), "status": "blocked", "reason": "stop"})
    assert "bloccato" in page.read_text() and "in attesa del tuo ok" not in page.read_text()


def test_refresh_is_not_called_inside_the_state_lock(repo, tmp_path):
    store = _store(repo)
    seen = []

    def observer(written):
        seen.append(written.read_state()["phase"])
        written.append_ledger("x", "no deadlock")

    store.set_state_observer(observer)
    store.update_state(lambda state: state.update(phase="plan"))
    assert seen == ["plan"]


def test_refresh_skips_identical_body(repo, tmp_path):
    store, cfg = _store(repo), _config(tmp_path)
    page = tmp_path / "notes/stato/t1.md"
    status_page.refresh(store, cfg, now=NOW)
    first = (os.stat(page).st_ino, page.read_text())
    status_page.refresh(store, cfg, now=NOW.replace(hour=18))
    assert (os.stat(page).st_ino, page.read_text()) == first
    store.update_state(lambda state: state.update(phase="plan"))
    status_page.refresh(store, cfg, now=NOW.replace(hour=19))
    assert "T19:30:00" in page.read_text()


def test_refresh_failure_is_logged_and_never_blocks_the_flow(repo, tmp_path):
    blocker = tmp_path / "file"
    blocker.write_text("not a directory")
    store, cfg = _store(repo), _config(tmp_path, status_dir=blocker / "stato")
    store.set_state_observer(lambda written: status_page.refresh(written, cfg))
    store.update_state(lambda state: state.update(phase="plan"))
    store.update_state(lambda state: state.update(phase="build"))
    assert store.read_state()["phase"] == "build"
    ledger = store.ledger.read_text()
    assert ledger.count("status_page_failed") == 1


def test_refresh_survives_a_malformed_plan_file(repo, tmp_path):
    store, cfg = _store(repo), _config(tmp_path)
    store.plan_json.write_text("{not json")
    store.plan_md.write_text("plan")
    status_page.refresh(store, cfg, now=NOW)
    assert (tmp_path / "notes/stato/t1.md").is_file()
    assert "status_page_failed" not in store.ledger.read_text()


def test_open_context_refreshes_the_page_after_a_state_write(repo, tmp_path):
    store = _store(repo, overrides={"report": {"owner_dir": str(tmp_path / "notes")}})
    ctx = engine.open_context(repo, store)
    ctx.store.update_state(lambda state: state.update(status="blocked", reason="stop here"))
    page = tmp_path / "notes/stato/t1.md"
    assert "stop here" in page.read_text()

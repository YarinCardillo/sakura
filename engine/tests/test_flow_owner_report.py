import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from sakura import engine_home
from sakura.flow import config, contract, owner_report
from sakura.flow.phases import report
from conftest import git
from flow_support import make_context

SECTIONS = ["## Cosa fa quest'area\nMostra le note ai visitatori.", "## Cosa cambia\nLa pagina ha un titolo.",
            "## Prima com'era\nNon c'era nessun titolo.", "## Come provarlo a mano\n1. Apri la pagina.",
            "## Cosa resta aperto\nNiente."]


def _owner_text(title="Titolo della pagina", description="La pagina mostra un titolo", sections=None):
    return f"Titolo: {title}\nDescrizione: {description}\n\n" + "\n\n".join(sections or SECTIONS) + "\n"


def _english_body():
    return "\n\n".join(["## What changed and why\nSmall.", "## Decisions and rulings\nNone.",
                        "## Risks\nNone.", "## What to try\n- Run it."])


def test_owner_dir_default_is_under_the_engine_home_and_names_the_repo(tmp_path):
    cfg = config.load(tmp_path / "myrepo", home=tmp_path / "me")
    target = owner_report.path(cfg.values, "myrepo", "add-greeting")
    assert target == engine_home.root() / "reports/myrepo/add-greeting.md"


@pytest.mark.parametrize("owner_dir", ["", "relative/report", "   "])
def test_owner_dir_empty_or_relative_is_a_config_error(tmp_path, owner_dir):
    with pytest.raises(config.FlowConfigError, match="report.owner_dir"):
        config.load(tmp_path, home=tmp_path / "nohome", overrides={"report": {"owner_dir": owner_dir}})


def _layer(directory, name, text):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / name).write_text(text)


def _personal(home, text):
    _layer(engine_home.root(), "config.toml", text)


@pytest.mark.parametrize("where,name,text", [
    ("repo", ".sakura/flow.toml", '[report]\nowner_dir = "/home/me/.claude/rules"\n'),
    ("repo", ".sakura/local.toml", '[flow.report]\nowner_dir = "/home/me/.claude/rules"\n'),
    ("repo", ".sakura/flow.toml", '[report.owner_dirs]\nmyrepo = "/home/me/.claude/rules"\n'),
])
def test_owner_dir_from_a_repository_layer_is_a_config_error_naming_the_personal_layer(tmp_path, where, name, text):
    repo, home = tmp_path / "myrepo", tmp_path / "me"
    _layer(repo / Path(name).parent, Path(name).name, text)
    with pytest.raises(config.FlowConfigError, match=r"report\.owner_dir.*personal layer.*<engine home>/config.toml"):
        config.load(repo, home=home)


def test_personal_owner_dir_applies_with_the_repo_name(tmp_path):
    home = tmp_path / "me"
    _personal(home, '[report]\nowner_dir = "/srv/reports/{repo}"\n')
    cfg = config.load(tmp_path / "myrepo", home=home)
    assert owner_report.path(cfg.values, "myrepo", "t1") == Path("/srv/reports/myrepo/t1.md")


def test_personal_owner_dirs_map_picks_a_directory_per_repository(tmp_path):
    home = tmp_path / "me"
    _personal(home, '[report.owner_dirs]\nwork = "~/knowledge/work/report"\n')
    values = config.load(tmp_path / "work", home=home).values
    assert owner_report.path(values, "work", "t1") == Path.home() / "knowledge/work/report/t1.md"
    assert owner_report.path(values, "other", "t1") == engine_home.root() / "reports/other/t1.md"


def test_personal_owner_dirs_must_be_absolute(tmp_path):
    home = tmp_path / "me"
    _personal(home, '[report.owner_dirs]\nwork = "relative/dir"\n')
    with pytest.raises(config.FlowConfigError, match=r"report\.owner_dirs"):
        config.load(tmp_path / "work", home=home)


def test_project_defaults_to_repo_name(tmp_path):
    cfg = config.load(tmp_path, home=tmp_path / "nohome")
    assert owner_report.project(cfg.values, "myrepo") == "myrepo"
    cfg = config.load(tmp_path, home=tmp_path / "nohome", overrides={"report": {"project": "work"}})
    assert owner_report.project(cfg.values, "myrepo") == "work"


def test_validate_accepts_a_complete_report():
    assert owner_report.validate(_owner_text()) == []


def test_validate_requires_title_description_and_sections():
    assert "Titolo" in " ".join(owner_report.validate("Descrizione: x\n\n" + "\n\n".join(SECTIONS)))
    assert "Descrizione" in " ".join(owner_report.validate("Titolo: x\n\n" + "\n\n".join(SECTIONS)))
    assert "Titolo" in " ".join(owner_report.validate(_owner_text(title="  ")))
    errors = " ".join(owner_report.validate(_owner_text(sections=SECTIONS[:-1])))
    assert "## Cosa resta aperto" in errors and "## Cosa cambia" not in errors


def test_blank_lines_around_the_header_are_accepted_and_dropped_from_the_body():
    text = "\nTitolo: Saluto\n\nDescrizione: La pagina saluta\n\n" + "\n\n".join(SECTIONS) + "\n"
    assert owner_report.validate(text) == []
    document = owner_report.compose(text, "T-1", "p", __import__("datetime").date(2026, 1, 2), [])
    front, body = document.split("\n---\n", 1)
    assert 'title: "Saluto"' in front and 'description: "La pagina saluta"' in front
    assert body.lstrip("\n").startswith("## Cosa fa quest'area") and "Descrizione:" not in body


def test_validate_limits_manual_steps():
    many = [section.replace("1. Apri la pagina.", "1. Uno\n2. Due\n3. Tre\n4. Quattro") for section in SECTIONS]
    assert "at most 3" in " ".join(owner_report.validate(_owner_text(sections=many)))
    none = [section.replace("1. Apri la pagina.", "Nessun passo.") for section in SECTIONS]
    assert "at least 1" in " ".join(owner_report.validate(_owner_text(sections=none)))


@pytest.mark.parametrize("leak", [
    "-----BEGIN OPENSSH PRIVATE KEY-----", "AKIAABCDEFGHIJKLMNOP", "ghp_" + "a" * 36, "github_pat_" + "b" * 30,
    "sk-" + "c" * 30, "xoxb-1234567890-abcdef", "password: hunter2", "token = abc123", "secret: s3cr3t",
    "API_TOKEN=abc123", "client_secret: xyz", "DB_PASSWORD=hunter2", "secrets: abc"])
def test_validate_rejects_credentials(leak):
    sections = [SECTIONS[0] + f"\n{leak}", *SECTIONS[1:]]
    errors = owner_report.validate(_owner_text(sections=sections))
    assert any("credential" in error for error in errors)
    assert leak not in " ".join(errors)


def test_validate_is_linear_on_hostile_key_runs():
    hostile = "-token" * 20000
    assignment = hostile + ": value"
    for position in (1, 6, len(hostile) // 2, len(hostile) - 5):
        assert not any(pattern.match(assignment, position) for pattern in owner_report.CREDENTIAL_PATTERNS)
    assert any((match := pattern.search(assignment)) and match.start() == 0
               for pattern in owner_report.CREDENTIAL_PATTERNS)
    report = _owner_text(sections=[SECTIONS[0] + f"\n{hostile}", *SECTIONS[1:]])
    assert owner_report.validate(report) == [
        f"owner report is larger than {owner_report.MAX_OWNER_REPORT_BYTES} bytes",
    ]
    assert not any(pattern.search(hostile) for pattern in owner_report.CREDENTIAL_PATTERNS)


def test_validate_rejects_an_oversized_report():
    padding = "x" * owner_report.MAX_OWNER_REPORT_BYTES
    errors = owner_report.validate(_owner_text(sections=[SECTIONS[0] + f"\n{padding}", *SECTIONS[1:]]))
    assert errors == [f"owner report is larger than {owner_report.MAX_OWNER_REPORT_BYTES} bytes"]


def test_compose_front_matter_is_engine_written_names_the_reporter_and_hostile_title_is_contained():
    hostile = 'x"\\ agent: root ---\x1b]2;pwn\x07 ok'
    document = owner_report.compose(_owner_text(title=hostile, description='d" \\ e'), "T-1", "work",
                                    __import__("datetime").date(2026, 10, 26), ["https://example.test/pull/1"])
    front, body = document.split("\n---\n", 1)
    lines = front.splitlines()
    assert lines[0] == "---" and len(lines) == 9
    assert lines[1] == 'title: "x\\"\\\\ agent: root --- ok"'
    assert lines[2] == 'description: "d\\" \\\\ e"'
    assert lines[3:] == ["project: work", "kind: report", "task: T-1", "agent: reporter", "date: 2026-10-26",
                         'links: ["https://example.test/pull/1"]']
    assert "\x1b" not in document and "\x07" not in document
    assert body.startswith("\n## Cosa fa quest'area") and "Titolo:" not in body


def test_compose_without_pull_request_has_empty_links():
    document = owner_report.compose(_owner_text(), "T-1", "p", __import__("datetime").date(2026, 1, 2), [])
    assert "links: []\n" in document


def test_write_is_atomic_and_replaces_the_previous_report(tmp_path):
    target = tmp_path / "deep" / "dir" / "T-1.md"
    owner_report.write(target, "first\n")
    owner_report.write(target, "second\n")
    assert target.read_text() == "second\n"
    assert os.stat(target).st_mode & 0o777 == 0o644
    assert [entry.name for entry in target.parent.iterdir()] == ["T-1.md"]


def _context(repo, tmp_path, owner_dir, behaviour):
    overrides = {"report": {"owner_dir": str(owner_dir), "project": "demo"}}
    head = git(repo, "rev-parse", "HEAD")
    # Delivered and verified at the workspace's head: the report is written about that head only.
    state = {"base_commit": head, "workspaces": {"0": {"path": str(repo)}}, "full_green_head": head,
             "contract_sha256": "", "familiarity": "familiar", "preset": "verified", "title": "Demo",
             "pr": {"url": "https://example.test/pull/7", "head": head}}
    ctx = make_context(repo, overrides=overrides, state=state)
    contract.save(ctx.store, [contract.Row("R1", "journey", "step", "api", None, "true", False, True)])
    digest = __import__("hashlib").sha256(ctx.store.contract.read_bytes()).hexdigest()
    ctx.store.update_state(lambda current: current.update(contract_sha256=digest))
    runner = SimpleNamespace(requests=[])

    def run(request):
        runner.requests.append(request)
        behaviour(request, len(runner.requests))
        return SimpleNamespace(exit_code=0, stalled=False, stopped=False, final="", result=None, log_path=Path("run.log"))

    runner.run = run
    object.__setattr__(ctx, "runner", runner)
    return ctx


def _reporter(owner_texts):
    def behaviour(request, call):
        Path(request.values["output_path"]).write_text(_english_body())
        text = owner_texts[min(call, len(owner_texts)) - 1]
        if text is not None:
            Path(request.values["owner_output_path"]).write_text(text)
    return behaviour


def test_report_phase_writes_the_owner_report_before_report_ready(repo, tmp_path):
    notes = tmp_path / "notes"
    seen = []

    def behaviour(request, call):
        _reporter([_owner_text()])(request, call)

    ctx = _context(repo, tmp_path, notes, behaviour)
    ctx.notifier.notify = lambda kind, task, text: seen.append((kind, (notes / f"{task}.md").exists()))
    assert report.run(ctx).kind == "next"
    written = (notes / "demo.md").read_text()
    assert written.startswith('---\ntitle: "Titolo della pagina"\n')
    assert 'links: ["https://example.test/pull/7"]' in written and "project: demo" in written
    assert "## Cosa resta aperto" in written
    assert str(ctx.store.root / "owner-report.md") == ctx.runner.requests[0].values["owner_output_path"]
    assert ctx.store.read_state()["status"] == "report_ready" and ctx.store.report.is_file()
    assert seen == [("ready_for_review", True)]


def test_report_phase_retries_then_blocks_without_a_valid_owner_report(repo, tmp_path):
    notes = tmp_path / "notes"
    ctx = _context(repo, tmp_path, notes, _reporter([None, "no header\n"]))
    outcome = report.run(ctx)
    assert outcome.kind == "blocked" and "owner report invalid" in outcome.reason
    assert len(ctx.runner.requests) == report.MAX_REPORT_ATTEMPTS
    assert "owner-report.md was not written" in ctx.runner.requests[1].values["previous_error"]
    assert not notes.exists() and not ctx.store.report.exists()
    assert ctx.store.read_state()["status"] != "report_ready"


def test_report_phase_retry_recovers_with_a_valid_second_attempt(repo, tmp_path):
    ctx = _context(repo, tmp_path, tmp_path / "notes", _reporter(["no header\n", _owner_text()]))
    assert report.run(ctx).kind == "next"
    assert (tmp_path / "notes" / "demo.md").is_file()


def test_report_phase_blocks_when_owner_dir_is_unwritable(repo, tmp_path):
    blocker = tmp_path / "file"
    blocker.write_text("not a directory")
    ctx = _context(repo, tmp_path, blocker / "report", _reporter([_owner_text()]))
    outcome = report.run(ctx)
    assert outcome.kind == "blocked" and "owner report" in outcome.reason
    assert not ctx.store.report.exists() and ctx.store.read_state()["status"] != "report_ready"


# -- invisible characters cannot hide a credential or reach the the notes app file ---------------------


def test_credential_split_by_a_zero_width_character_is_still_rejected():
    token = "sk-" + "A" * 10 + "​" + "B" * 12
    sections = [SECTIONS[0] + f"\nLa chiave e {token}"] + SECTIONS[1:]
    assert any("credential" in error for error in owner_report.validate(_owner_text(sections=sections)))


def test_compose_strips_control_and_invisible_characters_from_the_body():
    from datetime import date

    sections = [SECTIONS[0] + "\nciao‮\x1b[31m rosso​ fine"] + SECTIONS[1:]
    document = owner_report.compose(_owner_text(sections=sections), "t1", "demo", date(2026, 10, 26), [])
    assert "ciao rosso fine" in document
    assert not {"‮", "\x1b", "​"} & set(document)


# -- the report language is a config value ---------------------------------------------------------------------

ENGLISH_SECTIONS = ["## What this area does\nShows notes to visitors.", "## What changes\nThe page has a title.",
                    "## How it was before\nThere was no title.", "## How to try it by hand\n1. Open the page.",
                    "## What is still open\nNothing."]


def _english_owner_text():
    return "Title: Page title\nDescription: The page shows a title\n\n" + "\n\n".join(ENGLISH_SECTIONS) + "\n"


def test_report_language_defaults_to_italian(tmp_path):
    assert config.load(tmp_path, home=tmp_path / "me").values["knobs"]["report_language"] == "it"


def test_report_language_must_be_a_known_template(tmp_path):
    with pytest.raises(config.FlowConfigError, match="knobs.report_language"):
        config.load(tmp_path, home=tmp_path / "me", overrides={"knobs": {"report_language": "fr"}})


def test_validate_uses_the_template_of_the_language():
    assert owner_report.validate(_english_owner_text(), "en") == []
    assert owner_report.validate(_owner_text(), "it") == []
    errors = " ".join(owner_report.validate(_owner_text(), "en"))
    assert "Title" in errors and "## What changes" in errors


def test_compose_reads_the_header_of_the_language():
    document = owner_report.compose(_english_owner_text(), "T-1", "demo", __import__("datetime").date(2026, 11, 12),
                                    [], "en")
    assert document.startswith('---\ntitle: "Page title"\ndescription: "The page shows a title"\n')
    assert "Title:" not in document and "## What is still open" in document


def test_report_phase_asks_for_and_accepts_an_english_owner_report(repo, tmp_path):
    notes = tmp_path / "notes"

    def behaviour(request, call):
        Path(request.values["output_path"]).write_text(_english_body())
        Path(request.values["owner_output_path"]).write_text(_english_owner_text())

    ctx = _context(repo, tmp_path, notes, behaviour)
    ctx.config.values["knobs"]["report_language"] = "en"
    assert report.run(ctx).kind == "next"
    assert "## How to try it by hand" in ctx.runner.requests[0].values["owner_format"]
    assert "plain English" in ctx.runner.requests[0].values["owner_format"]
    assert (notes / "demo.md").read_text().startswith('---\ntitle: "Page title"\n')


def test_report_phase_without_a_reporter_writes_the_engine_section_only_and_no_owner_report(repo, tmp_path):
    notes = tmp_path / "notes"
    ctx = _context(repo, tmp_path, notes, _reporter([_owner_text()]))
    ctx.config.values["knobs"]["reporter"] = False
    assert report.run(ctx).kind == "next"
    assert ctx.runner.requests == []
    written = ctx.store.report.read_text()
    assert "## Verification status" in written and report.ENGINE_LABEL in written
    assert report.REPORTER_LABEL not in written
    assert not notes.exists()
    assert ctx.store.read_state()["status"] == "report_ready"


def test_report_inputs_digest_follows_the_reporter_and_language_knobs(repo, tmp_path):
    ctx = _context(repo, tmp_path, tmp_path / "notes", _reporter([_owner_text()]))
    before = report.inputs_hash(ctx)
    ctx.config.values["knobs"]["reporter"] = False
    assert report.inputs_hash(ctx) != before
    ctx.config.values["knobs"]["reporter"] = True
    ctx.config.values["knobs"]["report_language"] = "en"
    assert report.inputs_hash(ctx) != before

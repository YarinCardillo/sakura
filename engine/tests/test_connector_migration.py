import json
from dataclasses import replace
from datetime import UTC, datetime

import pytest
from sakura.connectors import kinds, migration, registry, replacement
from sakura.digest import config, installation, intake, run, state, supervise

NOW = datetime(2026, 11, 23, 8, tzinfo=UTC)


def test_import_legacy_shapes_reuses_mappings_and_preserves_each_source_checkpoint(tmp_path):
    program = tmp_path / "stories"
    program.write_text("#!/bin/sh\n")
    program.chmod(0o700)
    registry.path().write_text(json.dumps({"connectors": [{"id": "github", "kind": "github", "label": "Code"}]}))
    config.config_path().write_text(f"""
[[source]]
id = "first"
connector = "github"
[[source]]
id = "second"
connector = "github"
enabled = false
[[source]]
id = "work-stories"
run = ["{program}", "--include", "mine"]
timeout_seconds = 40
[[source]]
id = "news"
feed = "https://example.test/rss"
label = "Daily news"
reads = "Reads the news feed"
""")
    config.config_path().chmod(0o600)
    before = state.State({"first": state.SourceState("cursor-a", "2026-11-22T01:00:00+00:00"),
                          "second": state.SourceState("cursor-b", "2026-11-22T02:00:00+00:00")})
    state.save(before)

    def interrupted(handoff):
        raise RuntimeError("staging interrupted")

    with pytest.raises(RuntimeError, match="interrupted"):
        migration.cutover(interrupted)
    assert migration.ready() is False and state.load() == before
    staged = migration.stage()
    assert staged == migration.stage()
    bindings = {binding.source_id: binding for binding in staged.bindings}
    assert bindings["first"].connector_id == bindings["second"].connector_id == "github"
    assert bindings["first"].checkpoint.cursor == "cursor-a"
    assert bindings["second"].checkpoint.at == "2026-11-22T02:00:00+00:00"
    assert not bindings["second"].enabled
    custom = registry.find(bindings["work-stories"].connector_id)
    assert custom.label == "Work stories" and custom.reads_text == "Reads items from the configured program."
    assert custom.arguments == ("--include", "mine") and bindings["work-stories"].timeout == 40
    assert [row["id"] for row in registry.rows()].count("work-stories") == 0
    assert bindings["news"].reads == "Reads the news feed"

    def fake_program(argv, env, timeout):
        assert argv == installation.inspect((str(program), "--include", "mine")).argv and timeout == 40
        return supervise.Execution(0, b'{"id":"story","title":"A story"}\n', "")

    result = intake.run_source(bindings["work-stories"].source(), "", "", run=fake_program)
    assert result.report.ok and result.items[0].id == "story"
    program.write_text("#!/bin/sh\n# replaced\n")
    refused = intake.run_source(bindings["work-stories"].source(), "", "", run=fake_program)
    assert not refused.report.ok and refused.report.error == installation.CHANGED
    adopted = []
    assert migration.cutover(adopted.append) == staged == migration.cutover(adopted.append)
    assert adopted == [staged] and migration.ready() and state.load() == before
    outcome = run.run(config.load(), NOW, scheduled=False)
    assert not outcome.ran and outcome.reason == "Morning summary owns this schedule"


def test_replacement_requires_equal_coverage_and_a_verified_cursor_handoff(tmp_path):
    program = tmp_path / "stories"
    program.write_text("#!/bin/sh\n")
    program.chmod(0o700)
    ports = kinds.Ports(inspect=lambda *args: kinds.Access("account", "db", "Database"),
                       page=lambda *args: kinds.Page([
                           {"item": {"id": "old", "title": "Old", "at": "2026-11-22T07:00:00+00:00"}, "resource": "db"},
                           {"item": {"id": "next", "title": "New", "at": "2026-11-23T07:00:00+00:00"}, "resource": "db"}]))
    original = registry.mutate("add", "original", ports, NOW, fields={
        "kind": "program", "program": str(program), "label": "Stories", "reads": "Reads database pages"})["id"]
    candidate = registry.mutate("add", "candidate", ports, NOW, fields={
        "kind": "notion", "label": "Notion", "database": "db"})["id"]
    coverage = replacement.Coverage("account", ("db",), ("page",), "all-pages", ("read-db",),
                                    ("read",), (), "by-time", "by-id")
    evidence = replacement.Evidence(coverage, coverage, {"stories": state.SourceState("2026-11-22T07:00:00+00:00", "")},
                                    {"stories": state.SourceState("2026-11-22T07:00:00+00:00", "")})
    args = {"connector_id": original, "revision": 1, "candidate": candidate, "candidate_revision": 1}
    with pytest.raises(ValueError, match="replacement_unproven"):
        registry.preview_replacement(ports, **args)
    for candidate_coverage in (replace(coverage, permissions=("read-db", "read-more")), replace(coverage, permissions=()),
                               replace(coverage, selection="recent-only")):
        mismatch = replace(evidence, candidate=candidate_coverage)
        fake = replace(ports, prove_replacement=lambda *unused, proof=mismatch: proof)
        with pytest.raises(ValueError, match="replacement_incompatible"):
            registry.mutate("replace", "swap", fake, NOW, **args)
    incompatible = replace(ports, prove_replacement=lambda *unused: replace(evidence, translated={}))
    with pytest.raises(ValueError, match="replacement_incompatible"):
        registry.mutate("replace", "swap", incompatible, NOW, **args)
    verified = replace(ports, prove_replacement=lambda *unused: evidence)
    assert registry.preview_replacement(verified, **args)["candidate_label"] == "Notion"
    with pytest.raises(ValueError, match="stale_revision"):
        registry.mutate("replace", "swap", verified, NOW, **{**args, "candidate_revision": 2})
    assert registry.find(original).kind == "program"
    result = registry.mutate("replace", "swap", verified, NOW, **args)
    found = registry.find(original)
    assert result == {"id": original, "revision": 2} and found.kind == "notion" and found.label == "Stories"
    cursor = replacement.checkpoint(found, "stories", evidence.current_checkpoints["stories"])
    assert [item.id for item in kinds.read(found, "", cursor.cursor, 10, ports).items] == ["next"]


def test_imported_feed_and_legacy_provider_still_use_existing_readers():
    registry.path().write_text(json.dumps({"connectors": [{"id": "github", "kind": "github", "label": "Code"}]}))
    config.config_path().write_text('[[source]]\nid = "code"\nconnector = "github"\n'
                                    '[[source]]\nid = "feed"\nfeed = "https://example.test/rss"\n')
    config.config_path().chmod(0o600)
    taken = migration.cutover(lambda handoff: None)
    code, news = taken.bindings
    assert code.connector_id == "github"

    def read_legacy(connector, since, cursor, timeout):
        assert connector.id == "github" and connector.selectors == {}
        return kinds.Fetched()

    assert intake.run_source(code.source(), "", "", connect=read_legacy).report.ok

    def fetch(url, timeout):
        assert url == "https://example.test/rss"
        return b'<rss version="2.0"><channel><item><title>News</title><guid>today</guid></item></channel></rss>'

    fetched = intake.run_source(news.source(), "", "", fetch=fetch)
    assert fetched.report.ok and [item.id for item in fetched.items] == ["today"]

"""The writers: the JSON files and the lane on the Waiting page."""

import json
from datetime import datetime, timezone

import pytest

from sakura import actors
from sakura.digest import config, publish, schema
from sakura.outside import inbox, records, store
from sakura.outside.records import Agent

NOW = datetime(2026, 11, 17, 6, 30, tzinfo=timezone.utc)
LINK = schema.Link("PR 12", "https://example.org/pr/12")


class Pusher:
    def __init__(self):
        self.sent = []

    def push(self, event, title, message):
        self.sent.append((event, title, message))
        return "sent"


def _digest(output: schema.ModelOutput, date: str = "2026-11-17", failed: bool = False) -> schema.Digest:
    reports = (schema.SourceReport("work", True, 3, 0), *((schema.SourceReport("feed", False, 0, 0, "timed out"),)
                                                          if failed else ()))
    return schema.Digest(date, "2026-11-17T06:30:00+00:00",
                         schema.Window("2026-11-16T06:30:00+00:00", "2026-11-17T06:30:00+00:00"), "sonnet",
                         f"digest-{date}", reports, output)


FULL = schema.ModelOutput(
    (schema.News("work", "Release", "Version 2 shipped.", (LINK,)),),
    (schema.Check("check-pr", "Check PR 12", "CI is red on the merge.", (LINK,)),),
    (schema.Decision("ship-today", "Ship today?", ("yes", "no"), (LINK,)),))


def _publish(digest, pusher=None, language="en"):
    return publish.publish(digest, language, NOW, pusher=pusher or Pusher())


def test_Publish_JsonFileAndLatest_Atomic():
    digest = _digest(FULL)
    _publish(digest)
    folder = config.digest_folder()
    dated, latest = folder / "2026-11-17.json", folder / "latest.json"
    assert dated.read_bytes() == latest.read_bytes()
    assert schema.digest_from_json(json.loads(dated.read_text())) == digest
    assert dated.stat().st_mode & 0o777 == 0o600 and folder.stat().st_mode & 0o777 == 0o700
    assert sorted(path.name for path in folder.iterdir()) == ["2026-11-17.json", "latest.json"]  # no part files left
    _publish(_digest(schema.ModelOutput(), "2026-11-18"))
    assert json.loads(latest.read_text())["date"] == "2026-11-18" and dated.exists()


def test_Publish_LaneNewsInAboutChecksAsRowsDecisionsAsQuestions():
    pusher = Pusher()
    done = _publish(_digest(FULL, failed=True), pusher)
    lane = store.read(store.path(), "digest-2026-11-17")
    assert (done.rows, done.questions, lane.agent.name, lane.phase) == (1, 1, "digest", "ready")
    assert lane.about == "Release: Version 2 shipped.\nCould not read: feed." and lane.title == "Digest 2026-11-17"
    assert [(link.label, link.url) for link in lane.links] == [("PR 12", "https://example.org/pr/12")]
    row, question = lane.rows[0], lane.questions[0]
    assert (row.id, row.title, row.steps) == ("check-pr", "Check PR 12", ("CI is red on the merge.",))
    assert "https://example.org/pr/12" in row.note
    assert row.expected == "", "a digest check carries no generic expected line"
    assert (question.id, question.choices) == ("ship-today", ("yes", "no"))
    assert question.text.startswith("Ship today?") and "https://example.org/pr/12" in question.text
    assert len(pusher.sent) == 1


def test_Publish_SameDayAgain_UpdatesTheLaneAndKeepsAnAnsweredQuestion():
    _publish(_digest(FULL))
    root = store.path()
    question = store.read(root, "digest-2026-11-17").questions[0]
    person = actors.Actor("overlay", "owner@example.com", "nPHONE1", "phone")
    inbox.reply(root, person, "digest-2026-11-17", question.id, question.digest, "yes", "", NOW)
    again = _publish(_digest(FULL))
    assert (again.lane, again.questions) == ("digest-2026-11-17", 1)  # the same question, still answered
    assert len(store.lanes(root)) == 1 and len(store.read(root, again.lane).rows) == 1
    reworded = schema.ModelOutput(FULL.news, FULL.checks, (schema.Decision("ship-today", "Ship tomorrow?", ("yes", "no")),))
    assert _publish(_digest(reworded)).questions == 0  # an answered id is never asked again with other words


def test_Publish_PreviousDigestLaneClosed():
    root = store.path()
    agent = Agent("digest", config.digest_folder())
    store.open_lane(root, agent, "digest-2026-11-16", title="Digest 2026-11-16", now=NOW)
    store.open_lane(root, Agent("someone", config.digest_folder()), "digest-2026-11-10", title="Not ours", now=NOW)
    done = _publish(_digest(FULL))
    assert done.closed == ("digest-2026-11-16",)
    assert store.read(root, "digest-2026-11-16").outcome == "done"
    assert store.read(root, "digest-2026-11-10").closed_at is None
    assert store.read(root, "digest-2026-11-17").closed_at is None


@pytest.mark.parametrize("language, line", [("en", "Nothing new since 2026-11-16."), ("it", "Niente di nuovo dal 2026-11-16."),
                                            ("fr", "Nothing new since 2026-11-16.")])
def test_Publish_NothingNew_OneLineNoRows(language, line):
    done = _publish(_digest(schema.ModelOutput()), language=language)
    lane = store.read(store.path(), "digest-2026-11-17")
    assert lane.about == line and lane.rows == () and lane.questions == () and (done.rows, done.questions) == (0, 0)


def test_Publish_StoreLimits_Respected():
    news = tuple(schema.News("work", f"Title {number}" + "t" * 100, "x" * schema.MAX_NEWS_TEXT, (LINK,))
                 for number in range(schema.MAX_NEWS))
    checks = tuple(schema.Check(f"check-{number}", "c" * 200, "w" * 300, (LINK,) * 4) for number in range(schema.MAX_CHECKS))
    decisions = tuple(schema.Decision(f"decision-{number}", "q" * schema.MAX_QUESTION, tuple(f"choice {n}" for n in range(6)),
                                      (LINK,) * 4) for number in range(schema.MAX_DECISIONS))
    _publish(_digest(schema.ModelOutput(news, checks, decisions)))
    lane = store.read(store.path(), "digest-2026-11-17")
    assert len(lane.about) <= records.MAX_ABOUT and "more in the digest file" in lane.about
    assert (len(lane.rows), len(lane.questions)) == (schema.MAX_CHECKS, schema.MAX_DECISIONS)
    assert all(len(row.note) <= records.MAX_NOTE for row in lane.rows)


def test_Publish_StoreFull_ReportedAsPublishError(monkeypatch):
    monkeypatch.setattr(store, "MAX_OPEN_LANES", 0)
    with pytest.raises(publish.PublishError, match="digest-2026-11-17"):
        _publish(_digest(FULL))
    assert (config.digest_folder() / "latest.json").exists()


def test_published_digest_links_omit_url_authentication_fields():
    incoming = "https://fixture:private@example.org/pr/12?session=fixture#private"
    news = schema.News("work", "Release", f"See {incoming}", (schema.Link("PR 12", incoming),))
    digest = _digest(schema.ModelOutput(news=(news,)))

    _publish(digest)

    public = schema.digest_from_json(json.loads((config.digest_folder() / "latest.json").read_text()))
    assert public.output.news == (schema.News("work", "Release", f"See {LINK.url}", (LINK,)),)
    lane = store.read(store.path(), digest.lane)
    assert incoming not in lane.about and lane.links[0].url == LINK.url

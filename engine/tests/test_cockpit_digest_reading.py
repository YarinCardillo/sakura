"""The digest's item in the snapshot carries what a reading view needs: its text, its news and its asks with their state."""

from datetime import datetime, timezone

from sakura import actors
from sakura.cockpit import api
from sakura.digest import config, publish, schema
from sakura.outside import inbox, store
from test_cockpit_snapshot import collect

NOW = datetime(2026, 11, 17, 6, 30, tzinfo=timezone.utc)
PR = schema.Link("PR 12", "https://example.org/pr/12")
ISSUE = schema.Link("Issue 3", "https://example.org/issues/3")
PERSON = actors.Actor("app", "owner")
OUTPUT = schema.ModelOutput(
    (schema.News("work-github", "Release", "Version 2 shipped.", (PR,)),
     schema.News("news", "Outage", "A short outage on Monday.", ())),
    (schema.Check("check-pr", "Check PR 12", "CI is red on the merge.", (PR,)),
     schema.Check("check-issue", "Look at issue 3", "A customer wrote in.", (ISSUE,))),
    (schema.Decision("ship-today", "Ship today?", ("yes", "no"), (PR,)),))


class Pusher:
    def push(self, event, title, message):
        return "sent"


def _published(output=OUTPUT):
    digest = schema.Digest("2026-11-17", "2026-11-17T06:30:00+00:00",
                           schema.Window("2026-11-16T06:30:00+00:00", "2026-11-17T06:30:00+00:00"), "sonnet",
                           "digest-2026-11-17", (schema.SourceReport("work-github", True, 3, 0),), output)
    publish.publish(digest, "en", NOW, pusher=Pusher())


def _item(repo):
    return next(item for item in collect([repo]).items if item.kind == "digest")


def test_Reading_DigestItem_CarriesTheTextTheNewsAndTheAsks(repo):
    _published()
    reading = _item(repo).facts["reading"]
    assert reading["date"] == "2026-11-17"
    assert reading["text"] == "Release: Version 2 shipped.\nOutage: A short outage on Monday."
    assert reading["news"] == [
        {"title": "Release", "text": "Version 2 shipped.", "source": "work-github",
         "links": [{"label": "PR 12", "url": "https://example.org/pr/12"}]},
        {"title": "Outage", "text": "A short outage on Monday.", "source": "news", "links": []}]


def test_Reading_Asks_AreOpenWithTheirWaitIdAndTitledLinks(repo):
    _published()
    asks = _item(repo).facts["reading"]["asks"]
    assert [(ask["kind"], ask["title"], ask["state"]) for ask in asks] == [
        ("owner_row", "Check PR 12", "open"), ("owner_row", "Look at issue 3", "open"), ("question", "Ship today?", "open")]
    assert asks[0]["text"] == "CI is red on the merge."
    assert asks[0]["links"] == [{"label": "PR 12", "url": "https://example.org/pr/12"}]
    assert asks[2]["choices"] == ["yes", "no"]
    assert all(ask["id"].startswith('["wait","outside","digest-2026-11-17"') for ask in asks)


def test_Reading_AnAnsweredAsk_IsDecidedWithTheAnswerAndNoWaitId(repo):
    _published()
    lane = store.read(store.path(), "digest-2026-11-17")
    row, question = lane.rows[0], lane.questions[0]
    inbox.mark(store.path(), PERSON, lane.id, row.id, row.digest, "ok", "", NOW)
    inbox.reply(store.path(), PERSON, lane.id, question.id, question.digest, "yes", "", NOW)
    asks = _item(repo).facts["reading"]["asks"]
    assert [(ask["title"], ask["state"], ask["answer"], ask["id"]) for ask in (asks[0], asks[2])] == [
        ("Check PR 12", "decided", "ok", None), ("Ship today?", "decided", "yes", None)]
    assert asks[1]["state"] == "open" and "answer" not in asks[1]


def test_Reading_ADigestFileThatIsGone_StillGivesTheTextAndTheAsksWithNoNews(repo):
    _published()
    (config.digest_folder() / "2026-11-17.json").unlink()
    reading = _item(repo).facts["reading"]
    assert reading["news"] == [] and reading["text"].startswith("Release:") and len(reading["asks"]) == 3


def test_Reading_AnOutsideLaneThatIsNotTheDigest_HasNoReadingAndStaysAnOutsideLane(repo):
    from test_cockpit_outside import open_lane, root  # noqa: F401

    open_lane(store.path())
    [item] = collect([repo]).items
    assert item.kind == "outside" and "reading" not in item.facts


def test_Api_TheDigestItem_HasTheReadingOnItsJson(repo):
    _published()
    item = next(found for found in api.snapshot_document(collect([repo]))["items"] if found["kind"] == "digest")
    assert item["reading"]["asks"][0]["title"] == "Check PR 12" and item["reading"]["news"][0]["source"] == "work-github"


def test_Api_AnOpenAsksIdIsTheIdOfItsWaitInTheSnapshot(repo):
    _published()
    document = api.snapshot_document(collect([repo]))
    item = next(found for found in document["items"] if found["kind"] == "digest")
    assert sorted(ask["id"] for ask in item["reading"]["asks"]) == sorted(wait["id"] for wait in document["waits"])


def test_Api_TheDigestItem_IsOfKindDigestWithATitleADateAndNoProject(repo):
    _published()
    document = api.snapshot_document(collect([repo]))
    [item] = [found for found in document["items"] if found["kind"] == "digest"]
    assert (item["title"], item["date"], item.get("repo")) == ("Morning digest", "2026-11-17", None)
    assert item["key"] == ["outside", "digest-2026-11-17"]
    assert not [found for found in document["items"] if found["kind"] == "outside"]
    assert [project["folder"] for project in document["projects"]] == [str(repo)]


def test_Api_TheDigestsAsks_BelongToNoProjectAndNameTheDigestByItsDate(repo):
    _published()
    waits = api.snapshot_document(collect([repo]))["waits"]
    assert {(wait["project"], wait["lane_title"], wait.get("digest")) for wait in waits} == {("", "Morning digest", "2026-11-17")}


def test_Api_AnOutsideLanesWait_KeepsItsProjectAndHasNoDigestDate(repo):
    from test_cockpit_outside import add_row, open_lane, root  # noqa: F401

    open_lane(store.path())
    add_row(store.path())
    waits = api.snapshot_document(collect([repo]))["waits"]
    assert waits and all(wait["project"] and wait.get("digest") is None for wait in waits)


def test_spoofed_digest_identity_does_not_grant_triage(repo):
    from sakura.outside.records import Agent

    root = store.path()
    agent = Agent("digest", config.digest_folder())
    store.open_lane(root, agent, "digest-2026-11-17", title="Pretend digest", now=NOW)
    store.put_row(root, agent, "digest-2026-11-17", "fake", title="Start this", steps=["Work"], now=NOW)

    [wait] = api.snapshot_document(collect([repo]))["waits"]
    assert wait["section"] == "needs_you"
    assert {"start", "dismiss"}.isdisjoint(action["key"] for action in wait["actions"])


def test_editing_a_published_digest_row_does_not_grant_triage_to_the_edit(repo):
    _published()
    root = store.path()
    lane = store.read(root, "digest-2026-11-17")
    store.put_row(root, lane.agent, lane.id, "check-pr", title="Edited", steps=["Different work"], now=NOW)

    row = next(wait for wait in api.snapshot_document(collect([repo]))["waits"] if wait["title"] == "Edited")
    assert row["section"] == "needs_you"

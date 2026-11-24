import json
from datetime import UTC, datetime

import pytest
from sakura.cockpit import decided
from test_cockpit_actions import wait_target


@pytest.mark.parametrize(("actor", "place"), [
    ("app:owner", "desktop"),
    ("overlay:owner:phone", "phone"),
    ("cli:owner", "cli"),
    ("github", "github"),
])
def test_done_keeps_actor_and_place(tmp_path, actor, place):
    now = datetime.now(UTC)
    wait = decided.wait_fields(wait_target("question", "q1@abc").wait)
    decided.record(tmp_path, wait=wait, answer="reply", choice=None, text="Yes", actor=actor, at=now)
    [entry] = decided.recent(tmp_path, now=now)
    assert (entry["actor"], entry["place"]) == (actor, place)
    assert json.loads(decided.path(tmp_path).read_text())["place"] == place


def test_duplicate_paths_still_one_line_with_first_place(tmp_path):
    now = datetime.now(UTC)
    wait = decided.wait_fields(wait_target("question", "q1@abc").wait)
    for actor in ("overlay:owner:phone", "app:owner"):
        decided.record(tmp_path, wait=wait, answer="reply", choice=None, text="Yes", actor=actor, at=now)
    [entry] = decided.recent(tmp_path, now=now)
    assert (entry["actor"], entry["place"]) == ("overlay:owner:phone", "phone")


def test_legacy_done_entry_derives_its_place(tmp_path):
    now = datetime.now(UTC)
    path = decided.path(tmp_path)
    path.parent.mkdir()
    path.write_text(json.dumps({"at": now.isoformat(), "answer": "reply", "actor": "app:owner"}) + "\n")
    [entry] = decided.recent(tmp_path, now=now)
    assert entry["place"] == "desktop"

"""Hostile Unicode (bidi overrides and isolates, zero-width marks, line separators) in the data every cockpit
face draws: none of them may come out of the HTML page, the text view or the JSON snapshot."""

import json
from pathlib import Path

import pytest

from sakura.cockpit import api, gate, pages, text
from sakura.cockpit.items import GroupRow
from test_cockpit_snapshot import make_item, make_snapshot

BIDI = "‮⁦⁧⁨⁩​  "
OWNER = gate.Identity("owner@example.com", "nMAC", "mac")


def hostile_snapshot():
    item = make_item(key=("flow", "/r", f"de{BIDI}mo"), branch=f"feat/{BIDI}x", last_commit=f"abc{BIDI}d subject",
                     pending=[f"steer{BIDI}"], models={f"build{BIDI}er": f"claude:{BIDI}opus"})
    group = GroupRow(f"pro{BIDI}c", Path("/r"), "proc", 1, f"prob{BIDI}lem", "")
    events = [{"source": "flow", "scope": "r/demo", "at": "", "text": f"ledger{BIDI} line"},
              {"source": "lane", "scope": "proc/auth", "at": "", "text": f"lane log{BIDI} text"}]
    return make_snapshot(items=[item], groups=[group], events=events)


def faces():
    found = hostile_snapshot()
    return {"html": pages.index(found, OWNER, "csrf"),
            "text": "\n".join(text.render(found, 120, None)),
            "json": json.dumps(json.loads(api.dumps(api.snapshot_document(found))), ensure_ascii=False)}


@pytest.mark.parametrize("face", ["html", "text", "json"])
def test_snapshot_faces_drop_bidi_zero_width_and_line_separators(face):
    page = faces()[face]
    assert not any(character in page for character in BIDI)
    assert "demo" in page and "ledger line" in page and "lane log text" in page


def test_a_surrogate_escaped_task_name_renders_in_the_html_and_text_faces():
    """A task directory whose name is not valid UTF-8 reaches the snapshot as surrogate-escaped text."""
    found = make_snapshot(items=[make_item(key=("flow", "/r", "de\udcffmo"), branch="feat/x")], groups=[])
    for face in (pages.index(found, OWNER, "csrf"), "\n".join(text.render(found, 120, None))):
        face.encode("utf-8")
        assert "demo" in face

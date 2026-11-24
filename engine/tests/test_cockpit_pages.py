import json
import re
import shlex
from dataclasses import replace
from html.parser import HTMLParser
from pathlib import Path

import pytest

from sakura.cockpit import actions, gate, pages, wait_actions
from sakura.cockpit.items import GroupRow
from sakura.flow import progress, waits
from sakura.usage import aggregate
from test_cockpit_snapshot import NIGHT, make_item, make_snapshot

HOSTILE = ('</pre></div><script>alert(1)</script>"\'&amp;<meta http-equiv="refresh" content="0;url=//evil.example">'
           '\x1b[31m\x1b]52;c;ZXZpbA==\x07\x1b]2;title\x1b\\\x9b2J\x00')
OWNER = gate.Identity("owner@example.com", "nMAC", HOSTILE)
CSRF = "1700000000.abc123"
ALLOWED_TAGS = {"html", "head", "meta", "title", "link", "body", "header", "main", "section", "article", "h1",
                "h2", "h3", "p", "ul", "li", "form", "label", "input", "textarea", "button", "pre", "code", "a",
                "span", "div", "dl", "dt", "dd", "footer", "nav", "strong", "small", "ol", "h4", "time", "br", "select", "option"}
FORBIDDEN_CONTROLS = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")


HOST_KEY = "G"  # "Digest now": the Inbox section's own button, on every page; its tests are in test_cockpit_digest_action


class Audit(HTMLParser):
    """Collects every tag with its attributes so a test can say what the page may contain.

    The tags, hidden fields and input names of the forms are those of the forms for targets of the snapshot: the
    host's form (`G`) is on every page whatever the snapshot holds, so it is left out of them."""

    def __init__(self, page):
        super().__init__(convert_charrefs=True)
        self.tags, self.hidden, self.names, self.metas, self.text_chunks = [], [], [], [], []
        self._form = None
        self.feed(page)
        self.close()

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "form":
            self._form = ([], [], [])
        (self._form[2] if self._form is not None else self.tags).append((tag, attributes))
        if tag == "meta":
            self.metas.append(attributes)
        hidden, names = self._form[:2] if self._form is not None else (self.hidden, self.names)
        if tag == "input" and attributes.get("type") == "hidden":
            hidden.append((attributes.get("name"), attributes.get("value")))
        if tag in ("input", "textarea") and attributes.get("type") != "hidden":
            names.append((attributes.get("name"), "required" in attributes))

    def handle_endtag(self, tag):
        if tag == "form" and self._form is not None:
            hidden, names, tags = self._form
            self._form = None
            if ("key", HOST_KEY) not in hidden:
                self.hidden += hidden
                self.names += names
                self.tags += tags

    def handle_data(self, data):
        self.text_chunks.append(data)

    def hidden_values(self, name):
        return [value for field, value in self.hidden if field == name]


def hostile_item(**over):
    fields = dict(kind="flow", key=("flow", f"/r{HOSTILE}", f"demo{HOSTILE}"), repo=Path(f"/r{HOSTILE}"),
                  worktree=Path(f"/w{HOSTILE}"), branch=HOSTILE, last_commit=HOSTILE, stack=HOSTILE,
                  agent_state=HOSTILE, pr=HOSTILE, models={HOSTILE: HOSTILE}, pending=[HOSTILE], usage=None,
                  next_command=HOSTILE)
    fields.update(over)
    return make_item(**fields)


def hostile_lane():
    return hostile_item(kind="lane", key=("lane", HOSTILE, HOSTILE))


def make_wait(kind, name, *, task="demo", project="repo", title="a title", detail="a detail", since=None):
    identity = json.dumps(["wait", "/work/repo", task, kind, name], separators=(",", ":"))
    return waits.Wait(identity, kind, "/work/repo", project, task, title, detail, since)


def hostile_waits():
    return [make_wait("approval", "checks", task=HOSTILE, project=HOSTILE, title=HOSTILE, detail=HOSTILE),
            make_wait("owner_row", HOSTILE, task=HOSTILE, project=HOSTILE, title=HOSTILE, detail=HOSTILE),
            make_wait("blocked", "blocked", task=HOSTILE, project=HOSTILE, title=HOSTILE, detail=HOSTILE)]


def hostile_snapshot():
    group = GroupRow(HOSTILE, Path(f"/g{HOSTILE}"), HOSTILE, 3, HOSTILE, HOSTILE)
    event = {"source": HOSTILE, "scope": HOSTILE, "at": HOSTILE, "text": HOSTILE}
    deferred = [{"at": HOSTILE, "repo": HOSTILE, "task": HOSTILE, "event": HOSTILE, "message": HOSTILE}]
    return make_snapshot(mode=HOSTILE, deferred=deferred, groups=[group], items=[hostile_item(), hostile_lane()],
                         events=[event], problems=[HOSTILE], waits=hostile_waits())


def hostile_invocation():
    return actions.Invocation(["/bin/sakura", "flow", "steer", "--", HOSTILE, HOSTILE], Path(f"/r{HOSTILE}"))


def every_page():
    action = actions.ACTIONS["s"]
    target = hostile_item()
    confirm = pages.confirm(action, target, hostile_invocation(), {"text": HOSTILE}, CSRF, f"1.{HOSTILE}.2")
    done = actions.ActionResult(hostile_invocation(), 3, HOSTILE)
    return {"index": pages.index(hostile_snapshot(), OWNER, CSRF), "confirm": confirm,
            "result": pages.result(done), "refused": pages.refused(HOSTILE)}


def test_text_strips_controls_then_escapes_everything_active():
    assert pages.text(HOSTILE).count("<") == 0 and pages.text(HOSTILE).count('"') == 0
    assert "&lt;script&gt;" in pages.text(HOSTILE) and "&#x27;" in pages.text(HOSTILE)
    assert pages.text("a\x1b[31mb\x07c\nd\te") == "abc\nd\te"
    assert FORBIDDEN_CONTROLS.search(pages.text(HOSTILE)) is None
    assert pages.text(7) == "7" and pages.text(Path("/x<y")) == "/x&lt;y"


@pytest.mark.parametrize("name", ["index", "confirm", "result", "refused"])
def test_every_page_escapes_hostile_values(name):
    page = every_page()[name]
    audit = Audit(page)
    assert FORBIDDEN_CONTROLS.search(page) is None
    assert {tag for tag, _ in audit.tags} <= ALLOWED_TAGS
    for tag, attributes in audit.tags:
        assert not any(attribute.startswith("on") or attribute in ("style", "srcdoc") for attribute in attributes)
        assert "javascript:" not in " ".join(str(value) for value in attributes.values()).lower()
    assert "&lt;script&gt;" in page
    assert "<script" not in page.lower() and "evil.example" not in [m.get("content") for m in audit.metas]
    refreshes = [meta for meta in audit.metas if meta.get("http-equiv")]
    assert refreshes == ([{"http-equiv": "refresh", "content": "10"}] if name == "index" else [])


def test_index_and_events_show_the_escaped_text_not_nothing():
    page = pages.index(hostile_snapshot(), OWNER, CSRF)
    assert page.count("&lt;/pre&gt;&lt;/div&gt;&lt;script&gt;alert(1)&lt;/script&gt;") >= 10
    assert "&amp;amp;" in page  # a literal "&amp;" in data stays literal text


@pytest.mark.parametrize("name", ["index", "confirm", "result", "refused"])
def test_no_script_tag_on_any_page(name):
    page = every_page()[name]
    assert not re.search(r"<\s*(script|iframe|object|embed|img|svg|base)\b", page, re.I)
    assert not re.search(r"\son[a-z]+\s*=", page, re.I) and " style=" not in page
    plain = pages.index(make_snapshot(items=[make_item()]), gate.Identity("o@example.com", "n1", "mac"), CSRF)
    assert "<script" not in plain


def forms_in(snapshot):
    audit = Audit(pages.index(snapshot, gate.Identity("o@example.com", "n1", "mac"), CSRF))
    return audit


def test_forms_only_for_applicable_actions():
    flow = forms_in(make_snapshot(items=[make_item()]))
    lane = forms_in(make_snapshot(items=[make_item(kind="lane", key=("lane", "proc", "auth"))]))
    group = forms_in(make_snapshot(groups=[GroupRow("proc", Path("/r"), "proc", 0, "", "")]))
    assert sorted(flow.hidden_values("key")) == ["D", "R", "S", "a", "m", "q", "s", "t", "w"]
    assert sorted(lane.hidden_values("key")) == ["d", "m", "w"]
    assert sorted(group.hidden_values("key")) == []


def test_model_form_asks_role_only_for_flows():
    flow = [name for name, _ in forms_in(make_snapshot(items=[make_item()])).names]
    lane = [name for name, _ in forms_in(
        make_snapshot(items=[make_item(kind="lane", key=("lane", "proc", "auth"))])).names]
    assert "input.role" in flow and "input.role" not in lane
    assert "input.model" in flow and "input.model" in lane


def test_inputs_are_required_except_optional_ones():
    audit = forms_in(make_snapshot(items=[make_item()]))
    assert dict(audit.names) == {"input.role": True, "input.model": True, "input.step": True, "input.level": True, "input.preset": True,
                                "input.state": True, "input.remember": False, "input.note": False}


def test_every_form_posts_to_action_with_csrf_key_and_the_target_key():
    item = make_item()
    audit = forms_in(make_snapshot(items=[item]))
    assert audit.hidden_values("csrf") == [CSRF] * 9
    assert {json.dumps(json.loads(target)) for target in audit.hidden_values("target")} == {json.dumps(list(item.key))}
    forms = [attributes for tag, attributes in audit.tags if tag == "form"]
    assert forms and all(form.get("method") == "post" and form.get("action") == "/action" for form in forms)


def test_hidden_target_survives_hostile_keys_round_trip():
    hostile = hostile_item()
    audit = forms_in(make_snapshot(items=[hostile]))
    (decoded,) = {tuple(json.loads(value)) for value in audit.hidden_values("target")}
    assert decoded == hostile.key


def test_index_shows_provider_usage_and_deferred_at_night():
    provider = aggregate.ProviderWeek("claude", NIGHT, NIGHT, 0.42, NIGHT, None, None, ())
    page = pages.index(make_snapshot(mode="night", usage=aggregate.WeekUsage(NIGHT, (provider,)),
                                     deferred=[{"at": "01:30", "task": "demo", "event": "failed", "message": "boom"}]),
                       gate.Identity("o@example.com", "n1", "mac"), CSRF)
    assert "mode: night" in page and "claude: week 42.0%" in page and "1 deferred" in page and "boom" in page


def test_index_shows_the_signed_in_actor_and_the_events():
    page = pages.index(make_snapshot(events=[{"source": "flow", "scope": "r/demo", "at": "", "text": "dial changed"}]),
                       gate.Identity("o@example.com", "n1", "mac"), CSRF)
    assert "o@example.com" in page and "dial changed" in page and "r/demo" in page


def test_confirm_page_shows_the_exact_command_and_posts_the_proof():
    target = make_item()
    invocation = actions.Invocation(["/bin/sakura", "flow", "stop", "--actor=overlay:o:n1", "--", "demo"], Path("/r"))
    page = pages.confirm(actions.ACTIONS["s"], target, invocation, {}, CSRF, "99.nonce.mac")
    audit = Audit(page)
    assert shlex.join(invocation.argv) in "".join(audit.text_chunks)
    assert audit.hidden_values("proof") == ["99.nonce.mac"] and audit.hidden_values("csrf") == [CSRF]
    assert audit.hidden_values("key") == ["s"] and json.loads(audit.hidden_values("target")[0]) == list(target.key)


def test_confirm_page_carries_the_inputs_forward():
    audit = Audit(pages.confirm(actions.ACTIONS["w"], make_item(), hostile_invocation(), {"text": "run it"}, CSRF, "p"))
    assert audit.hidden_values("input.text") == ["run it"]


def test_result_page_shows_command_exit_code_and_output():
    done = actions.ActionResult(actions.Invocation(["/bin/sakura", "flow", "stop", "--", "demo"], Path("/r")), 3,
                                "blocked\nsecond line\n")
    page = pages.result(done)
    assert "/bin/sakura flow stop -- demo" in page and "exit code 3" in page and "blocked\nsecond line" in page
    assert "exit code 0" in pages.result(actions.ActionResult(done.invocation, 0, ""))


def test_css_fits_phone_width():
    css = pages.STYLESHEET.read_text()
    for page in every_page().values():
        assert any(meta.get("name") == "viewport" and "width=device-width" in meta.get("content", "")
                   for meta in Audit(page).metas)
    widths = [int(value) for value in re.findall(r"(?<!max-)(?<!min-)width:\s*(\d+)px", css)]
    assert all(width <= 320 for width in widths) and not re.findall(r"min-width:\s*(\d{4,}|[4-9]\d{2})px", css)
    assert "box-sizing: border-box" in css and "overflow-wrap" in css
    assert re.search(r"pre\s*\{[^}]*white-space:\s*pre-wrap", css)
    assert "max-width" in css and "@media (prefers-color-scheme: dark)" in css
    assert "url(" not in css and "@import" not in css


# -- the Inbox section --------------------------------------------------------------------------------


def waiting_page(*wait_list, **over):
    return pages.index(make_snapshot(waits=list(wait_list), **over), gate.Identity("o@example.com", "n1", "mac"), CSRF)


def wait_detail(wait):
    return pages.wait_view(make_snapshot(), replace(wait, is_read=True), CSRF)


def test_Index_Waits_FirstSectionGroupedByLane():
    page = waiting_page(make_wait("blocked", "blocked", title="Blocked task"),
                        make_wait("approval", "spec", title="Approve the spec"),
                        make_wait("owner_row", "R1", title="Check row R1"), items=[make_item()],
                        groups=[GroupRow("proc", Path("/r"), "proc", 0, "", "")])

    assert page.index("<section>") == page.index("<section><h2>Inbox")
    assert page.count("<h4>demo</h4>") == 1
    assert all(f"{kind}, demo" in page for kind in ("Approval", "Blocked"))
    assert page.index("Approve the spec") < page.index("Flow tasks") and page.index("Check row R1") < page.index("proc")


def test_Index_WaitValues_Escaped():
    page = waiting_page(*hostile_waits())

    assert page.count("&lt;/pre&gt;&lt;/div&gt;&lt;script&gt;alert(1)&lt;/script&gt;") >= 9
    assert "<script" not in page.lower() and FORBIDDEN_CONTROLS.search(page) is None


def test_Detail_OwnerRowForm_PostsMarkWithVerdict():
    wait = make_wait("owner_row", "R1", detail="open the page and look")
    audit = Audit(wait_detail(wait))

    assert audit.hidden_values("key") == ["k", "unread", "snooze"] and audit.hidden_values("csrf") == [CSRF] * 3
    assert [json.loads(target) for target in audit.hidden_values("target")] == [json.loads(wait.id)] * 3
    verdicts = [attributes for tag, attributes in audit.tags if tag == "button" and attributes.get("name")]
    assert [(button.get("name"), button.get("value")) for button in verdicts] == [("input.verdict", "ok"),
                                                                                    ("input.verdict", "ko")]
    assert dict(audit.names) == {"input.note": False} and "open the page and look" in "".join(audit.text_chunks)


def test_Detail_ApprovalForm_OffersRememberOnlyAtTheChecksGate():
    checks_page = Audit(wait_detail(make_wait("approval", "checks")))
    spec_page = Audit(wait_detail(make_wait("approval", "spec")))

    assert checks_page.hidden_values("key") == spec_page.hidden_values("key") == ["a", "unread", "snooze"]
    assert dict(checks_page.names) == {"input.remember": False} and dict(spec_page.names) == {}


def test_Detail_BlockedWaitWithoutLane_ShowsReasonAndAttentionControls():
    audit = Audit(wait_detail(make_wait("blocked", "blocked", detail="push stalled")))

    assert audit.hidden_values("key") == ["unread", "snooze"] and "push stalled" in "".join(audit.text_chunks)


def test_Index_NoWaits_ExplainsWhatAppears():
    page = waiting_page(items=[make_item()])

    assert page.index("<section>") == page.index("<section><h2>Inbox") and "Questions, approvals, reviews" in page
    assert "<h3>Needs you</h3>" not in page


def test_Index_FlowItemProgress_PlainWordsOrThePhase():
    with_plan = waiting_page(items=[make_item(progress=progress.Progress(3, 7, "build"))])
    without_plan = waiting_page(items=[make_item(progress=progress.Progress(0, 0, "contract"))])

    assert "<dt>progress</dt><dd>3 of 7 done</dd>" in with_plan
    assert "<dt>progress</dt><dd>contract</dd>" in without_plan


# -- outside lanes on the phone ------------------------------------------------------------------------------


def outside_wait(kind, name, *, detail="a detail", title="a title", choices=(), digest="0123456789ab",
                 author="outside:claude", project="work"):
    identity = json.dumps(["wait", "outside", "measure-help", kind, f"{name}@{digest}"], separators=(",", ":"))
    return waits.Wait(identity, kind, f"/work/{project}", project, "measure-help", title, detail,
                      "2026-11-16T10:00:00+00:00", "outside", author, tuple(choices))


ROW_DETAIL = "1. Run measure.py --help\n2. Read the usage line\nExpected: usage: measure\nNote: on a clean clone"


def outside_item(**over):
    fields = dict(kind="outside", key=("outside", "measure-help"), repo=Path("/work/work"), worktree=None,
                  branch="", last_commit="", stack="", agent_state="waiting", pr="", models={}, pending=[],
                  usage=None, next_command="", title="Add --help", author="outside:claude",
                  links=(("PR 12", "https://github.com/x/y/pull/12"),), unread=2,
                  progress=progress.Progress(1, 2, "ready"))
    fields.update(over)
    return make_item(**fields)


def test_Detail_OutsideRow_StepsNumberedAndFromLine():
    page = wait_detail(outside_wait("owner_row", "help-prints", title="Help prints", detail=ROW_DETAIL))

    assert ("<ol><li>Run measure.py --help</li><li>Read the usage line</li></ol>") in page
    assert "Expected: usage: measure" in page and "Note: on a clean clone" in page
    assert "From claude in work" in page
    audit = Audit(page)
    buttons = [attributes for tag, attributes in audit.tags if tag == "button" and attributes.get("name")]
    assert [(b.get("name"), b.get("value")) for b in buttons] == [("input.verdict", "ok"), ("input.verdict", "ko")]
    assert audit.hidden_values("key") == ["k", "unread", "snooze"] and audit.names == []


def test_Detail_Question_OneButtonPerChoice():
    page = wait_detail(outside_wait("question", "flag-short", title="Also accept -h?", choices=("yes", "no"),
                                     detail="Also accept -h?\nSecond line"))
    audit = Audit(page)

    choices = [attributes for tag, attributes in audit.tags if tag == "button" and attributes.get("name")]
    assert [(b.get("name"), b.get("value")) for b in choices] == [("input.choice", "yes"), ("input.choice", "no")]
    assert "<dd>Question</dd>" in page and "From claude in work" in page
    assert audit.hidden_values("key") == ["r", "r", "unread", "snooze"] and "Reply in words" in page
    assert audit.names == []  # no field: a reply in words has a page of its own


def test_Detail_QuestionWithoutChoices_OnlyReplyInWords():
    audit = Audit(wait_detail(outside_wait("question", "why", detail="Why?")))

    assert [a for tag, a in audit.tags if tag == "button" and a.get("name")] == []
    assert "Reply in words" in "".join(audit.text_chunks)


def test_Index_OutsideLane_ProgressPhaseLinksUnread():
    page = waiting_page(items=[outside_item()])

    assert "Add --help" in page and "From claude in work" in page
    assert "<dt>progress</dt><dd>1 of 2 done</dd>" in page and "2 not read yet" in page
    assert '<a href="https://github.com/x/y/pull/12" rel="noreferrer">PR 12</a>' in page
    audit = Audit(page)
    assert sorted(audit.hidden_values("key")) == ["d", "w"] and audit.names == []
    no_rows = waiting_page(items=[outside_item(progress=progress.Progress(0, 0, "draft"), unread=0, links=())])
    assert "<dt>progress</dt><dd>draft</dd>" in no_rows and "not read yet" not in no_rows


def test_Index_AgentMarkup_Escaped():
    markup = '<script>alert(1)</script><img src=x onerror=alert(1)>"\''
    row = outside_wait("owner_row", "r", title=markup, detail=f"1. {markup}\nExpected: {markup}\nNote: {markup}",
                       author=f"outside:{markup}", project=markup)
    question = outside_wait("question", "q", title=markup, detail=markup, choices=(markup,))
    item = outside_item(title=markup, author=f"outside:{markup}", links=((markup, f"https://x.test/{markup}"),))
    page = waiting_page(row, question, items=[item])
    audit = Audit(page)

    assert "<script" not in page.lower() and "<img" not in page.lower() and FORBIDDEN_CONTROLS.search(page) is None
    assert {tag for tag, _ in audit.tags} <= ALLOWED_TAGS
    for tag, attributes in audit.tags:
        assert not any(name.startswith("on") for name in attributes)
    for wait in (row, question):
        detail = wait_detail(wait)
        assert "&lt;script&gt;alert(1)&lt;/script&gt;" in detail and "<script" not in detail
        assert {tag for tag, _ in Audit(detail).tags} <= ALLOWED_TAGS


def test_Index_OutsideLinkThatIsNotHttps_ShownAsTextNeverAsLink():
    item = outside_item(links=(("bad", "javascript:alert(1)"),))
    page = waiting_page(items=[item])

    assert "<a " not in page and "javascript:alert(1)" in page


def test_Compose_NamesTheTargetWithOneTextareaAndNoRefresh():
    wait = outside_wait("question", "why", title="Why?", detail="Why?")
    page = pages.compose(actions.ACTIONS["r"], wait_actions.target_of(wait), CSRF)
    audit = Audit(page)

    assert audit.names == [("input.text", True)] and audit.hidden_values("key") == ["r"]
    assert audit.hidden_values("csrf") == [CSRF] and "Why?" in page
    assert [m for m in audit.metas if m.get("http-equiv")] == []
    assert json.loads(audit.hidden_values("target")[0]) == json.loads(wait.id)

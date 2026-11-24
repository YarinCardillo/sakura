"""The digest's data: bounded items, the model's output file and the run's JSON file."""

import json

import pytest

from sakura.digest import schema

LINK = {"label": "PR 12", "url": "https://example.org/pr/12"}


def _output(**changes) -> dict:
    document = {"news": [{"source": "work", "title": "Release", "text": "Version 2 shipped.", "links": [LINK]}],
                "checks": [{"id": "check-pr", "title": "Check PR 12", "why": "CI is red.", "links": [LINK]}],
                "decisions": [{"id": "pick", "question": "Ship today?", "choices": ["yes", "no"], "links": []}]}
    return {**document, **changes}


def test_Schema_ModelOutputInsideLimits_Accepted():
    output = schema.output_from_json(_output())
    assert output.news[0].links == (schema.Link("PR 12", "https://example.org/pr/12"),)
    assert output.urls() == {"https://example.org/pr/12"}
    assert schema.output_from_json({}) == schema.ModelOutput()


@pytest.mark.parametrize("changes", [
    {"news": [{"source": "work", "title": "t", "text": "x" * 401, "links": []}]},
    {"news": [{"source": "Work", "title": "t", "text": "x"}]},
    {"news": [{"source": "work", "title": "t" * 121, "text": "x"}]},
    {"news": [{"source": "work", "title": "t", "text": "x"}] * 13},
    {"checks": [{"id": "c", "title": "t", "why": "line one\nline two"}]},
    {"checks": [{"id": "c", "title": "t", "why": "w"}, {"id": "c", "title": "u", "why": "w"}]},
    {"decisions": [{"id": "d", "question": "q", "choices": ["a"] * 2}]},
    {"decisions": [{"id": "d", "question": "q", "choices": [str(number) for number in range(7)]}]},
    {"decisions": [{"id": "d", "question": "q", "choices": ["x" * 81]}]},
    {"news": [{"source": "work", "title": "t", "text": "x", "links": [LINK] * 5}]},
    {"news": [{"source": "work", "title": "t", "text": "x", "links": [{"label": "l", "url": "javascript:alert(1)"}]}]},
    {"news": [{"source": "work", "title": "t", "text": "x", "links": [{"label": "l", "url": "https://a b"}]}]},
    {"news": [{"source": "work", "title": "t", "text": "x", "extra": "field"}]},
    {"extra": []},
])
def test_Schema_ModelOutputOverLimits_Refused(changes):
    with pytest.raises(schema.SchemaError):
        schema.output_from_json(_output(**changes))


def test_Schema_NotJson_Refused():
    with pytest.raises(schema.SchemaError, match="not JSON"):
        schema.parse_model_output("{broken")
    with pytest.raises(schema.SchemaError, match="must be an object"):
        schema.parse_model_output("[]")


@pytest.mark.parametrize("hidden", ["‮", "​", "", "\U000e0041"])
def test_Schema_HiddenCharacters_Refused(hidden):
    with pytest.raises(schema.SchemaError):
        schema.output_from_json(_output(news=[{"source": "work", "title": f"a{hidden}b", "text": "x"}]))
    with pytest.raises(schema.SchemaError, match="hidden"):
        schema.output_from_json(_output(news=[{"source": "work", "title": "t", "text": f"a{hidden}b"}]))
    with pytest.raises(schema.SchemaError):
        schema.parse_item({"id": "i", "title": f"a{hidden}b"})


def test_Schema_Item_BoundedAndTimesNormalized():
    item = schema.parse_item({"id": "pr-1", "title": "T", "url": "https://example.org/1",
                              "at": "2026-11-17T08:12:00+02:00", "text": "line\nbreak"})
    assert item.at == "2026-11-17T06:12:00+00:00" and item.text == "line\nbreak"
    assert schema.parse_item({"id": "i", "title": "t"}) == schema.Item("i", "t")
    for bad in ({"id": "i"}, {"id": "i", "title": "t" * 201}, {"id": "i", "title": "t", "text": "x" * 4001},
                {"id": "i", "title": "t", "at": "yesterday"}, {"id": "i", "title": "t", "at": "2026-11-17T06:12:00"},
                {"id": "i", "title": "t", "url": "file:///etc/passwd"}, {"id": "i", "title": "t", "x": 1}, [1]):
        with pytest.raises(schema.SchemaError):
            schema.parse_item(bad)


def _digest() -> schema.Digest:
    return schema.Digest("2026-11-17", "2026-11-17T06:30:00+00:00",
                         schema.Window("2026-11-16T06:30:00+00:00", "2026-11-17T06:30:00+00:00"), "sonnet",
                         "digest-2026-11-17",
                         (schema.SourceReport("work", True, 3, 1), schema.SourceReport("feed", False, 0, 0, "timed out")),
                         schema.output_from_json(_output()))


def test_Schema_RoundTrip_Version1():
    digest = _digest()
    document = schema.digest_to_json(digest)
    assert document["schema"] == 1 and set(document) == {
        "schema", "date", "generated_at", "window", "model", "lane", "sources", "news", "checks", "decisions"}
    assert schema.digest_from_json(json.loads(json.dumps(document))) == digest


def test_Schema_OtherVersionOrBadLane_Refused():
    document = schema.digest_to_json(_digest())
    with pytest.raises(schema.SchemaError, match="schema must be 1"):
        schema.digest_from_json({**document, "schema": 2})
    with pytest.raises(schema.SchemaError, match="lane"):
        schema.digest_from_json({**document, "lane": "other"})


@pytest.mark.parametrize("sneaky", ["͏", "\udc80", "\ud83d"])
def test_Schema_InvisibleJoinerOrLoneSurrogate_RefusedEverywhere(sneaky):
    for document in (_output(news=[{"source": "work", "title": f"a{sneaky}", "text": "x"}]),
                     _output(news=[{"source": "work", "title": "t", "text": f"a{sneaky}b"}]),
                     _output(decisions=[{"id": "d", "question": f"q{sneaky}", "choices": []}]),
                     _output(decisions=[{"id": "d", "question": "q", "choices": [f"yes{sneaky}", "yes"]}])):
        with pytest.raises(schema.SchemaError):
            schema.output_from_json(document)
    with pytest.raises(schema.SchemaError):
        schema.parse_item({"id": "i", "title": "t", "text": f"a{sneaky}"})
    with pytest.raises(schema.SchemaError):
        schema.parse_model_output('{"news": [{"source": "work", "title": "a\\ud800", "text": "x"}]}')


@pytest.mark.parametrize("moment", ["0001-01-01T00:00:00+01:00", "9999-12-31T23:59:59-05:00", "0001-01-01T00:00:00+14:00"])
def test_Schema_TimeThatCannotBeConvertedToUtc_RefusedNotRaised(moment):
    with pytest.raises(schema.SchemaError, match="time"):
        schema.parse_item({"id": "i", "title": "t", "at": moment})
    with pytest.raises(schema.SchemaError, match="time"):
        schema.time_text(moment)

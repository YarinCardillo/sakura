from datetime import datetime, timezone

import pytest

from sakura import questions

NOW = datetime(2026, 11, 16, 10, 0, tzinfo=timezone.utc)


def make(question_id="flag-short", text="Also accept -h?", choices=("yes", "no")):
    return questions.make(question_id, text, choices, NOW)


def test_Questions_SevenChoices_RefusedNamingSix():
    with pytest.raises(questions.QuestionRefused, match="6"):
        make(choices=[f"c{number}" for number in range(7)])


def test_Questions_ReplyChoiceNotOffered_Refused():
    assert questions.reply_problem(make(), "maybe", "") is not None
    assert questions.reply_problem(make(), "yes", "") is None


def test_Questions_SameContent_SameDigest():
    assert make().digest == make().digest
    assert make().digest != make(text="Also accept --short?").digest
    assert make().digest != make(choices=("yes",)).digest


def test_Questions_ReplyNeedsChoiceOrTextNotBoth():
    assert questions.reply_problem(make(), "", "") is not None
    assert questions.reply_problem(make(), "yes", "words") is not None
    assert questions.reply_problem(make(), "", "in words") is None


def test_Questions_ReplyTextOverEightThousand_Refused():
    assert "8000" in questions.reply_problem(make(), "", "x" * 8001)


def test_Questions_BadIdOrDuplicateChoiceOrHostileText_Refused():
    for bad in (dict(question_id="Flag"), dict(question_id="-x"), dict(question_id="a" * 41),
                dict(choices=("yes", "yes")), dict(choices=("a‮b",)), dict(text="x" * 2001),
                dict(text="bad \x1b"), dict(choices=("x" * 81,))):
        with pytest.raises(questions.QuestionRefused):
            make(**bad)


def test_Questions_NoChoices_AskedAsTextOnly():
    question = make(choices=())
    assert question.choices == () and question.asked_at == "2026-11-16T10:00:00+00:00"
    assert questions.reply_problem(question, "yes", "") is not None

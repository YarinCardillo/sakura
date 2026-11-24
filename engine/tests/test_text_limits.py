import pytest

from sakura import text_limits


@pytest.mark.parametrize("value", ["a\x00b", "a\x1bb", "a\nb", "a\x7fb", "a‮b", "a​b", "a b"])
def test_TextLimits_ControlOrBidiCharacter_Refused(value):
    assert text_limits.one_line_problem(value, 200) is not None


def test_TextLimits_TextWithControlCharacter_Refused_NewlineAndTabPass():
    assert text_limits.text_problem("a\x1bb", 50) is not None
    assert text_limits.text_problem("line one\nline two\tend\r\n", 50) is None


def test_TextLimits_OverLimit_NamesTheLimit():
    assert "200" in text_limits.one_line_problem("x" * 201, 200)
    assert "50" in text_limits.text_problem("x" * 51, 50)


def test_TextLimits_BlankOrEmpty_Refused():
    assert text_limits.one_line_problem("", 10) is not None
    assert text_limits.text_problem("   \n", 10) == "empty"


def test_TextLimits_AtTheLimit_Accepted():
    assert text_limits.one_line_problem("x" * 200, 200) is None
    assert text_limits.text_problem("x" * 50, 50) is None


def test_TextLimits_ContentDigest_StableForSameFieldsInAnyOrder():
    first = text_limits.content_digest({"a": 1, "b": ["x", "y"]})
    assert first == text_limits.content_digest({"b": ["x", "y"], "a": 1})
    assert len(first) == 12 and first != text_limits.content_digest({"a": 2, "b": ["x", "y"]})


@pytest.mark.parametrize("value", ["a\U000e0041b", "a\U000e007fb", "x️", "x\U000e0100", "a‍b", "a⁠b",
                                   "a­b", "a﻿b", "a؜b", "ab", "a\U000f0000b", "a͸b",
                                   "a b", "a b"])
def test_TextLimits_TagCharacter_Refused(value):
    assert text_limits.hidden_character_problem(value) is not None
    assert text_limits.one_line_problem(value, 50) is not None


def test_TextLimits_ZeroWidthJoinerAndFormatChars_Refused():
    for value in ("family \U0001f468‍\U0001f469", "a​b", "a‎b", "a‪b", "a⁦b"):
        assert text_limits.hidden_character_problem(value) is not None


@pytest.mark.parametrize("value", ["plain words", "café naïve", "日本語のテキスト",
                                   "مرحبا", "αβγ", "emoji \U0001f600", "tab\there",
                                   "two\nlines", "a b"])
def test_TextLimits_PlainUnicodeLetters_Accepted(value):
    assert text_limits.hidden_character_problem(value) is None
    assert text_limits.text_problem(value, 50) is None
    assert "\n" in value or "\t" in value or text_limits.one_line_problem(value, 50) is None


def test_TextLimits_WithoutHiddenCharacters_RemovesWhatTheCheckRefuses():
    assert text_limits.without_hidden_characters("a‮b​c\U000e0041de f\n\tg") == "abcde f\n\tg"
    assert text_limits.hidden_character_problem(text_limits.without_hidden_characters("x y️")) is None


@pytest.mark.parametrize("value", ["a\udc80b", "a\ud800", "a͏b", "͏"])
def test_TextLimits_LoneSurrogateOrGraphemeJoiner_Refused(value):
    assert text_limits.hidden_character_problem(value) is not None
    assert text_limits.one_line_problem(value, 20) is not None
    assert text_limits.without_hidden_characters(value) == value.replace("͏", "").replace("\udc80", "").replace("\ud800", "")

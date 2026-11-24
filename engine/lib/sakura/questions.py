"""A question an agent asks the person, and the checks on the person's reply.

A question has an id, its text, up to six choices and a digest of that content. A reply names the digest it
answers, so an answer always refers to the exact question the person saw.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import datetime
from typing import Sequence

from sakura import text_limits

QUESTION_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")
MAX_TEXT, MAX_CHOICES, MAX_CHOICE, MAX_REPLY = 2000, 6, 80, 8000


class QuestionRefused(ValueError):
    """Raised, with a one-line reason, for a question that breaks a limit."""


@dataclass(frozen=True)
class Question:
    """One question: `digest` is a hash of its id, text and choices; `asked_at` is a UTC ISO string."""

    id: str
    text: str
    choices: tuple[str, ...]
    digest: str
    asked_at: str


def make(question_id: str, text: str, choices: Sequence[str], now: datetime) -> Question:
    """Return the question, or raise QuestionRefused naming the limit it breaks."""
    if not QUESTION_ID.fullmatch(question_id):
        raise QuestionRefused(f"question id {question_id!r} must match {QUESTION_ID.pattern}")
    problem = text_limits.text_problem(text, MAX_TEXT) or text_limits.hidden_character_problem(text)
    if problem:
        raise QuestionRefused(f"question text: {problem}")
    offered = tuple(choices)
    _check_choices(offered)
    digest = text_limits.content_digest({"id": question_id, "text": text, "choices": list(offered)})
    return Question(question_id, text, offered, digest, text_limits.utc_text(now))


def reply_problem(question: Question, choice: str, text: str) -> str | None:
    """Why a reply is refused: it is one of the question's choices or text, not both and not neither."""
    if bool(choice) == bool(text):
        return "give a choice or text, one of them"
    if choice:
        return None if choice in question.choices else f"{choice!r} is not one of the choices"
    return text_limits.text_problem(text, MAX_REPLY)


def _check_choices(choices: tuple[str, ...]) -> None:
    if len(choices) > MAX_CHOICES:
        raise QuestionRefused(f"{len(choices)} choices; the limit is {MAX_CHOICES}")
    if len(set(choices)) != len(choices):
        raise QuestionRefused("choices must be distinct")
    for choice in choices:
        problem = text_limits.one_line_problem(choice, MAX_CHOICE)
        if problem:
            raise QuestionRefused(f"choice {choice[:20]!r}: {problem}")

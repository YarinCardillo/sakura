"""No lane stops without plain words: every place that stops a lane with a reason has a rule that says, for the person, what
happened and what to do, with none of the engine's own words in it.

The test walks the source: every `PhaseOutcome("blocked" | "failed", reason)` of the engine. A reason written in the code is
checked by its shape against `flow/stop_words.py`; one that is a message of another module's error (`str(error)`) or already
a sentence of `ask_words` is listed apart. A shape that has no rule fails the test: write it in `flow/stop_words.py`. A reason that is an error message of another
module gets the general sentence, which names no cause.
"""

import ast
import re
from pathlib import Path

import pytest

from sakura.flow import ask_words, stop_words

LIB = Path(__file__).resolve().parents[1] / "lib" / "sakura"
ENGINE_WORDS = re.compile(r"\b(T\d+|final-\d+|[\w-]+\.(?:json|md|py)|workspace 0|arbiter|tests\.(?:focused|full)|Errno|Traceback|"
                          r"state\.json|candidate head|ext::|PhaseOutcome|GitError)\b|/")
# What a placeholder of a reason stands for when "X1" would not be what the engine writes there.
SAMPLES = {
    "contract/{}: nothing proves the new behavior": ["contract has no non-baseline row: nothing proves the new behavior"],
    "build_task/{} still needs changes after {} fix rounds: {}": ["T1 still needs changes after 2 fix rounds: focused tests failed"],
    "build_task/{}: tests.focused ({})": ["timed out: tests.focused (T1)", "stalled: tests.focused (T1)"],
    "runs/{}: {}; see {}": ["timed out: build; see /x/run.log", "stalled: review; see /x/run.log"],
    "runs/{} exited {} during {}; see {}": ["builder exited 2 during build; see /x/run.log"],
}


def _text(node: ast.expr) -> str | None:
    """The shape of a reason the code writes (`{}` for what is filled in), None for one that is only an expression."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return "".join(part.value if isinstance(part, ast.Constant) else "{}" for part in node.values)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = _text(node.left)
        return None if left is None else left + "{}"
    return None


def producers() -> tuple[dict[str, str], list[str]]:
    """({key: shape} of the reasons written in the code, [expression] of those that are not)."""
    written, dynamic = {}, []
    for path in sorted(LIB.rglob("*.py")):
        tree = ast.parse(path.read_text())
        for node in ast.walk(tree):
            is_outcome = isinstance(node, ast.Call) and getattr(node.func, "id", getattr(node.func, "attr", "")) == "PhaseOutcome"
            if not (is_outcome and len(node.args) > 1 and isinstance(node.args[0], ast.Constant)
                    and node.args[0].value in ("blocked", "failed")):
                continue
            shape = _text(node.args[1])
            where = path.relative_to(LIB).as_posix().removesuffix(".py").replace("flow/phases/", "").replace("flow/", "")
            expression = ast.unparse(node.args[1])
            if shape is not None:
                written[f"{where}/{shape}"] = shape
            elif not expression.startswith("ask_words."):
                dynamic.append(f"{where}/{expression}")
    return written, dynamic


def _samples(key: str, shape: str) -> list[str]:
    return SAMPLES.get(key) or [shape.replace("{}", "X1")]


def _is_plain(sample: str) -> bool:
    sentence = stop_words.sentence_of(sample)
    return sentence is not None and not ENGINE_WORDS.search(sentence)


# Reasons that carry the message of an error of another module: they get the general sentence, which names no cause.
ERROR_CARRYING = {"build/{}: {}", "evaluate/{}: {}"}
# Reasons written outside `PhaseOutcome`: the lane's driver, the tamper check and the keeper.
OTHER_REASONS = ["the engine stopped on an error: ValueError: boom", "state.json changed outside the engine",
                 "the engine keeper resumed this lane twice within a few minutes and it stopped again; read its log, then "
                 "resume it yourself"]
SOME_ERROR = "WorkspaceError: /home/me/work/tally.py line 3 (T1, final-1): Errno 2"

WRITTEN, DYNAMIC = producers()


def test_Producers_TheWalkFindsTheReasonsOfTheEngine():
    assert len(WRITTEN) >= 55 and len(DYNAMIC) >= 10


@pytest.mark.parametrize("key", sorted(set(WRITTEN) - ERROR_CARRYING))
def test_StuckWords_EveryReasonTheEngineWritesHasARule(key):
    for sample in _samples(key, WRITTEN[key]):
        sentence = ask_words.blocked_words(sample)[0]
        assert not ENGINE_WORDS.search(sentence) and sentence != sample, (key, sample, sentence)


@pytest.mark.parametrize("key", sorted(ERROR_CARRYING))
def test_StuckWords_AReasonThatCarriesAnErrorOfAnotherModule_GetsTheGeneralSentenceAndKeepsTheLineApart(key):
    sample = f"T1: {SOME_ERROR}"

    sentence, technical = ask_words.blocked_words(sample)

    assert sentence == stop_words.GENERIC and technical == sample and not ENGINE_WORDS.search(sentence)


@pytest.mark.parametrize("expression", sorted(set(DYNAMIC)))
def test_StuckWords_EveryReasonThatIsAnErrorMessage_ReachesThePersonInPlainWords(expression):
    sentence, technical = ask_words.blocked_words(SOME_ERROR)

    assert sentence == stop_words.GENERIC and technical == SOME_ERROR and not ENGINE_WORDS.search(sentence), expression


@pytest.mark.parametrize("reason", OTHER_REASONS)
def test_StuckWords_TheReasonsOutsideThePhasesHaveRules(reason):
    sentence = ask_words.blocked_words(reason)[0]

    assert sentence != stop_words.GENERIC and sentence != reason and not ENGINE_WORDS.search(sentence)


def test_StuckWords_ASentenceAlreadyInPlainWords_IsKeptAsItIsWithNoTechnicalLine():
    plain = ask_words.nothing_written("contract", "It already exists.")

    assert ask_words.blocked_words(plain) == (plain, "")

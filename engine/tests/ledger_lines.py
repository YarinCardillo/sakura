"""The lines a normal lane run writes to its ledger, read from the engine's own source.

Every `append_ledger(tag, text)` call of the engine is a kind of ledger line. This walks them, resolves the module
constants and the phase list, and keeps the kinds a lane run writes in its phases (a line whose text is only a
variable is whatever the caller passed and has no fixed words). `{}` stands for a value the engine fills in. The
interface's tests read the same list (`tests/fixtures/ledger-lines.txt`), so a new kind of line cannot reach the
person as "an engine event". Regenerate it with `python3 tests/ledger_lines.py`.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "ledger-lines.txt"
SOURCE = ROOT / "lib" / "sakura"
sys.path.insert(0, str(ROOT / "lib"))

from sakura.flow.engine import PHASE_ORDER  # noqa: E402

# The tags of a lane run that are not a phase of the engine's list: the plan reviews, the code reviews, the
# integration of builds, the evaluator's calibration, the lane's questions, the person's marks and the notices
# a lane sends, and what pace decided (a step moved to another account, an advisor that did not answer).
RUN_TAGS = ("plan_review", "review", "integration", "calibrate", "questions", "owner", "notify", "pace")
PHASE_LINES = ("done", "skipped")


def _constants(tree: ast.Module) -> dict[str, str]:
    found = {}
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            text = _text(node.value, {})
            if text is not None:
                found[node.targets[0].id] = text
    return found


def _text(node: ast.AST, constants: dict[str, str]) -> str | None:
    """The words of a string expression with `{}` for each value, or None when it is only a variable."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.Name):
        return constants.get(node.id)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        return _text(node.left, constants)
    if isinstance(node, ast.JoinedStr):
        return "".join(part.value if isinstance(part, ast.Constant) else "{}" for part in node.values)
    return None


def _calls(path: Path):
    tree = ast.parse(path.read_text())
    constants = _constants(tree)
    for node in ast.walk(tree):
        is_ledger = isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
        if is_ledger and node.func.attr == "append_ledger" and len(node.args) >= 2:
            yield node.args[0], node.args[1], constants


def kinds() -> list[str]:
    """`tag<TAB>words` for each kind of line a lane run writes, sorted and without repeats."""
    scoped = {*PHASE_ORDER, *RUN_TAGS}
    found = set()
    for path in sorted(SOURCE.rglob("*.py")):
        for tag_node, text_node, constants in _calls(path):
            tag, text = _text(tag_node, constants), _text(text_node, constants)
            if text is not None and text.replace("{}", "").strip(" :,()=") and tag in scoped:
                found.add(f"{tag}\t{text}")
    found.update(f"{phase}\t{line}" for phase in PHASE_ORDER for line in PHASE_LINES)
    return sorted(found)


if __name__ == "__main__":
    FIXTURE.write_text("\n".join(kinds()) + "\n")
    print(f"wrote {FIXTURE}")

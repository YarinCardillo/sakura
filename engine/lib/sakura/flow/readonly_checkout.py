"""What a check row may write: the engine runs it in a read-only copy of the committed files.

After the build every check row runs from the engine's checkout of the committed revision, source and Git folder
read-only (`flow/completion.py`). Its only writable place is the run's temp folder, `$TMPDIR` (the same folder as
`$SAKURA_FLOW_BUILD_DIR` there). The planner writes rows on the base build, where the workspace is writable, so a row
that writes next to its script passes there and fails later. `plain_writes` finds the writes a command's own text makes
(redirects, `tee`, `touch`, `mkdir` onto a plain relative path); what a script does inside is not read.
"""

from __future__ import annotations

import re
import shlex

WRITABLE_FOLDER = "$TMPDIR"
# How a refused write reads in a command's output: Seatbelt answers a denied write with EPERM or EROFS.
_DENIED_WRITE = re.compile(r"Operation not permitted|Read-only file system", re.IGNORECASE)
HINT = (f"sakura: the checkout this row ran in is read-only; it may write only under the run's temp folder "
        f"({WRITABLE_FOLDER}, also $SAKURA_FLOW_BUILD_DIR)")
_REDIRECTS = frozenset({">", ">>", ">|", "&>", "&>>"})
_DUPLICATING_REDIRECT = ">&"
_SEPARATORS = frozenset({"|", "||", "&&", ";", "&", "(", ")"})
_WRITING_COMMANDS = frozenset({"tee", "touch", "mkdir"})
_UNKNOWN_START = ("/", "~", "$", "`", "&", "-")


def plain_writes(command: str) -> list[str]:
    """The plain relative paths `command` writes by its own text; empty when it cannot be read as plain words."""
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        tokens = list(lexer)
    except ValueError:
        return []
    targets = _redirect_targets(tokens) + _command_targets(tokens)
    return list(dict.fromkeys(target for target in targets if _is_inside_checkout(target)))


def refusal(row_id: str, writes: list[str]) -> str:
    """Why a row is refused before the person is asked to approve its command."""
    return (f"row {row_id}: the command writes {', '.join(writes)} inside the checkout; a check runs in a read-only "
            f"copy of the committed files, so write only under the run's temp folder ({WRITABLE_FOLDER}, also "
            f"$SAKURA_FLOW_BUILD_DIR)")


def failure_hint(output: str) -> str:
    """`HINT` for a failed row's log when its output shows a refused write, else an empty string."""
    return HINT if _DENIED_WRITE.search(output) else ""


def _redirect_targets(tokens: list[str]) -> list[str]:
    found = []
    for index, token in enumerate(tokens[:-1]):
        if token in _REDIRECTS or (token == _DUPLICATING_REDIRECT and not tokens[index + 1].isdigit()):
            found.append(tokens[index + 1])
    return found


def _command_targets(tokens: list[str]) -> list[str]:
    found, at_command = [], True
    command = ""
    for token in tokens:
        if token in _SEPARATORS:
            at_command = True
        elif at_command:
            command, at_command = token, False
        elif command in _WRITING_COMMANDS and not token.startswith("-") and token not in _REDIRECTS:
            found.append(token)
    return found


def _is_inside_checkout(target: str) -> bool:
    return bool(target) and not target.startswith(_UNKNOWN_START) and "$" not in target and target != "."

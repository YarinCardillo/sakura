"""Read a superpowers plan (Markdown) as build tasks, enough to predict its cost before a flow exists.

A task is a `### Task <id>: <title>` section. Its `Files:` line (and the lines that continue it) names the
files it touches, backticked or comma separated; a `Class:` line names its task class (default integration).
"""

from __future__ import annotations

import re

from sakura.budget import routing
from sakura.flow.plan import BuildTask

TASK_HEADING = re.compile(r"^###\s+Task\s+(?P<id>\w+(?:\.\w+)*)\s*[:.\-]?\s*(?P<title>.*)$")
ANY_HEADING = re.compile(r"^#{1,3}\s")
FILES_LINE = re.compile(r"^\s*(?:[-*]\s+)?\**Files\s*:\s*\**\s*(?P<rest>.*)$")
CLASS_LINE = re.compile(r"^\s*(?:[-*]\s+)?\**Class\s*:\s*\**\s*(?P<value>[\w-]+)")
FILES_END = re.compile(r"^(\s*$|\s*\*\*|\s*- \[|\s*(?:[-*]\s+)?\**Class\b)")
BACKTICKED = re.compile(r"`([^`\n]+)`")
LINE_SUFFIX = re.compile(r":\d+(?:-\d+)?$")
ROLE_PREFIX = re.compile(r"^(?:[-*]\s+)?(?:Create|Modify|Test|Delete|Edit)s?:?\s*", re.IGNORECASE)


class PlanFileError(ValueError):
    """The plan text has no task, or a task names an unknown class."""


def parse(text: str) -> list[BuildTask]:
    """The tasks of a plan in file order; raises PlanFileError when there is none or a class is unknown."""
    tasks = [_task(match, lines) for match, lines in _sections(text.splitlines())]
    if not tasks:
        raise PlanFileError("no ### Task heading found in the plan")
    return tasks


def _sections(lines: list[str]) -> list[tuple[re.Match, list[str]]]:
    """Each task heading with the lines up to the next heading of level 1 to 3."""
    found: list[tuple[re.Match, list[str]]] = []
    for line in lines:
        heading = TASK_HEADING.match(line)
        if heading:
            found.append((heading, []))
        elif ANY_HEADING.match(line):
            found.append((None, []))
        elif found:
            found[-1][1].append(line)
    return [(match, body) for match, body in found if match]


def _task(heading: re.Match, body: list[str]) -> BuildTask:
    label = heading["id"]
    return BuildTask(label, heading["title"].strip(), [], [], _files(body), False,
                     task_class=_task_class(label, body))


def _task_class(label: str, body: list[str]) -> str:
    for line in body:
        found = CLASS_LINE.match(line)
        if found:
            if found["value"] not in routing.TASK_CLASSES:
                raise PlanFileError(f"Task {label}: unknown class {found['value']!r}; "
                                    f"use one of {', '.join(routing.TASK_CLASSES)}")
            return found["value"]
    return routing.DEFAULT_TASK_CLASS


def _files(body: list[str]) -> list[str]:
    chunk: list[str] = []
    for index, line in enumerate(body):
        found = FILES_LINE.match(line)
        if found:
            chunk = [found["rest"], *_continuation(body[index + 1:])]
            break
    return list(dict.fromkeys(_paths("\n".join(chunk))))


def _continuation(lines: list[str]) -> list[str]:
    kept: list[str] = []
    for line in lines:
        if FILES_END.match(line):
            break
        kept.append(line)
    return kept


def _paths(text: str) -> list[str]:
    quoted = BACKTICKED.findall(text)
    if quoted:
        return [LINE_SUFFIX.sub("", item.strip()) for item in quoted]
    pieces = (ROLE_PREFIX.sub("", piece.strip()).rstrip(".") for piece in re.split(r"[,\n]", text))
    return [piece for piece in pieces if piece and not re.search(r"\s", piece)]

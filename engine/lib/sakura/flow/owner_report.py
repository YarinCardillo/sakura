"""The owner report: validate the reporter's plain-language text, compose its front matter, write it to the notes folder.

The report is written in the language set by the `knobs.report_language` knob; each language has its own header keys and headings.
"""

from __future__ import annotations

import os
import re
import tempfile
from dataclasses import dataclass
from datetime import date
from pathlib import Path

from sakura import engine_home
from sakura.flow.render import strip_controls

DEFAULT_LANGUAGE = "it"
ENGINE_AGENT = "engine"
REPORTER_AGENT = "reporter"
MIN_TRY_STEPS = 1
MAX_TRY_STEPS = 3
TRY_ITEM = re.compile(r"^\s*(?:[-*]|\d+[.)])\s+", re.MULTILINE)
REPORT_MODE = 0o644
MAX_OWNER_REPORT_BYTES = 64 * 1024
SAFE_SCALAR = re.compile(r"[A-Za-z0-9._-]+")
CREDENTIAL_PATTERNS = [
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\b(?:ghp|gho)_[A-Za-z0-9]{20,}"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bsk-[A-Za-z0-9_-]{20,}"),
    re.compile(r"\bxox[abp]-[A-Za-z0-9-]{10,}"),
    re.compile(r"(?<![\w-])(?=[\w-]*?(?:password|passwd|token|secret))[\w-]+[\"']?\s*[:=]\s*\S+", re.IGNORECASE),
]


@dataclass(frozen=True)
class Template:
    """The fixed shape of an owner report in one language."""

    language_name: str
    title_key: str
    description_key: str
    section_headings: tuple[str, ...]
    try_heading: str


TEMPLATES = {
    "it": Template("Italian", "Titolo:", "Descrizione:",
                   ("## Cosa fa quest'area", "## Cosa cambia", "## Prima com'era", "## Come provarlo a mano",
                    "## Cosa resta aperto"), "## Come provarlo a mano"),
    "en": Template("English", "Title:", "Description:",
                   ("## What this area does", "## What changes", "## How it was before", "## How to try it by hand",
                    "## What is still open"), "## How to try it by hand"),
}
LANGUAGES = tuple(TEMPLATES)


def prompt_format(language: str) -> str:
    """The reporter's instructions for the owner report in `language`, with its header keys and headings."""
    template = TEMPLATES[language]
    headings = "\n".join(template.section_headings)
    return (f"The owner report is for a reader who is not a technician of this area. Write it in plain "
            f"{template.language_name}, explain every\ntechnical term, and use no code unless it is needed. Never "
            f"include credentials, passwords, tokens or real customer\ndata. Format, exactly:\n"
            f"Line 1: `{template.title_key} <short title>`\n"
            f"Line 2: `{template.description_key} <one line: what changes for whoever uses the product>`\n"
            f"Line 3: blank\nThen these sections, in this order:\n{headings}\n"
            f"Under `{template.try_heading}`, give a numbered list of one to three steps. Do not write front matter; "
            f"the engine adds it.")


def path(values: dict, repo: str, task_id: str) -> Path:
    """Where the owner report of a task lives: `<dir>/<task_id>.md`, with `~` and `{repo}` expanded.

    `<dir>` is the personal `report.owner_dirs` entry for the repository, else `report.owner_dir`.
    """
    report = values["report"]
    template = report["owner_dirs"].get(repo) or report["owner_dir"].replace("{repo}", repo)
    return Path(os.path.expanduser(engine_home.with_engine_root(template))) / f"{task_id}.md"


def project(values: dict, repo: str) -> str:
    """The project named in the front matter: the configured one, else the repository name."""
    return values["report"]["project"] or repo


def validate(text: str, language: str = DEFAULT_LANGUAGE) -> list[str]:
    """Return the format errors of the reporter's owner text in `language`; empty when it can be published."""
    if len(text.encode()) > MAX_OWNER_REPORT_BYTES:
        return [f"owner report is larger than {MAX_OWNER_REPORT_BYTES} bytes"]
    template = TEMPLATES[language]
    text = strip_controls(text)
    lines = text.splitlines()
    title, description, _ = _header(lines, template)
    keys = ((template.title_key, title), (template.description_key, description))
    errors = [f"owner report header line {number} must be '{key} <text>'"
              for number, (key, value) in enumerate(keys, start=1) if not value]
    errors += [f"owner report is missing required heading {heading}"
               for heading in template.section_headings if heading not in lines]
    steps = len(TRY_ITEM.findall(_section(lines, template.try_heading)))
    if template.try_heading in lines and not MIN_TRY_STEPS <= steps <= MAX_TRY_STEPS:
        errors.append(f"{template.try_heading[3:]} needs at least {MIN_TRY_STEPS} and at most {MAX_TRY_STEPS} steps")
    if any(pattern.search(text) for pattern in CREDENTIAL_PATTERNS):
        errors.append("owner report contains what looks like a credential; remove it")
    return errors


def compose(text: str, task_id: str, project_name: str, today: date, links: list[str],
            language: str = DEFAULT_LANGUAGE) -> str:
    """Build the final document: engine-written front matter naming the reporter as its author, then the body without
    the two header lines. The title is the reporter's title alone: the task id is a field of its own."""
    title, description, body = _header(strip_controls(text).splitlines(), TEMPLATES[language])
    front = front_matter(title, description, project_name, "report", task_id, ("date", today.isoformat()), links,
                         agent=REPORTER_AGENT)
    return front + "\n".join(body).rstrip() + "\n"


def front_matter(title: str, description: str, project_name: str, kind: str, task_id: str,
                 stamp: tuple[str, str], links: list[str], agent: str = ENGINE_AGENT) -> str:
    """The engine-written front matter block, newline terminated; free text is one line and quoted, `stamp` is engine-made.

    `agent` names who wrote the document: the engine for a page it computes, the reporter for a report whose title,
    description and body are the reporter's text.
    """
    lines = ["---", f"title: {_quoted(title)}", f"description: {_quoted(description)}", f"project: {_scalar(project_name)}",
             f"kind: {kind}", f"task: {_scalar(task_id)}", f"agent: {agent}", f"{stamp[0]}: {stamp[1]}",
             f"links: [{', '.join(_quoted(link) for link in links)}]", "---"]
    return "\n".join(lines) + "\n"


def write(target: Path, document: str) -> None:
    """Replace `target` atomically (temp file in the same directory, then rename); parents are created."""
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=target.parent, prefix=f".{target.name}.", suffix=".tmp")
    try:
        with os.fdopen(descriptor, "w") as handle:
            handle.write(document)
        os.chmod(temporary, REPORT_MODE)
        os.replace(temporary, target)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def _header(lines: list[str], template: Template) -> tuple[str, str, list[str]]:
    """Return title, description and the body after them; blank lines around the header are skipped."""
    filled = [index for index, line in enumerate(lines) if line.strip()][:2]
    keys = (template.title_key, template.description_key)
    values = [_header_value(lines[index], key) for index, key in zip(filled, keys)]
    title, description = (values + ["", ""])[:2]
    return title, description, lines[filled[-1] + 1:] if len(filled) == 2 else []


def _header_value(line: str, key: str) -> str:
    return _one_line(line[len(key):]) if line.startswith(key) else ""


def _one_line(value: str) -> str:
    return " ".join(strip_controls(value).split())


def _quoted(value: str) -> str:
    escaped = _one_line(value).replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _scalar(value: str) -> str:
    cleaned = _one_line(value)
    return cleaned if SAFE_SCALAR.fullmatch(cleaned) else _quoted(cleaned)


def _section(lines: list[str], heading: str) -> str:
    if heading not in lines:
        return ""
    start = lines.index(heading) + 1
    end = next((index for index in range(start, len(lines)) if lines[index].startswith("## ")), len(lines))
    return "\n".join(lines[start:end])

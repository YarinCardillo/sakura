"""Generate and validate the task comprehension report.

The report is about the delivered head only, the engine's own (`flow/final_head.py`). The reporter works in a
repository of its own, made after delivery (`task_repository.make_role_repository`): its workspace holds nothing but
the inputs the engine copies there (the diff read from the engine's store, the rows, the evidence lists), and its cage
reads nothing of the task's workspaces, repository or earlier runs, so no file a survivor planted there reaches the
report (review 11, finding 2)."""

from __future__ import annotations

import hashlib
import html
import json
import re
from datetime import datetime
from pathlib import Path

from sakura.flow import contract, final_head, owner_report, runner, task_repository
from sakura.flow.engine import FlowContext, PhaseOutcome
from sakura.flow.gitutil import GitError, git
from sakura.flow.phases import runs
from sakura.flow.render import strip_controls

PHASE = "report"
REPORT_INPUTS = "report-inputs"
MAX_REPORT_ATTEMPTS = 2
MAX_TRY_STEPS = owner_report.MAX_TRY_STEPS
BODY_FILE = "report-body.md"
REPORTER_LABEL = "_Written by the reporter role. The engine did not check what it says._"
BUILDER_LABEL = "_The builder's words. The engine did not check what they say._"
ENGINE_LABEL = "_Computed by the engine from its own records, not from the reporter's text._"
OWNER_BODY_FILE = "owner-report.md"
FAMILIAR_HEADINGS = ["## What changed and why", "## Decisions and rulings", "## Risks", "## What to try"]
NEW_HEADINGS = ["## What this area is for", "## How it behaved before", "## Where it lives in the code",
                "## Terms", "## Guided manual check"]
TRY_ITEM = re.compile(r"^\s*(?:[-*]|\d+[.)])\s+", re.MULTILINE)
MARKDOWN_LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


def run(ctx: FlowContext) -> PhaseOutcome:
    """Prepare report inputs, validate one reporter revision, then save the final report."""
    try:
        return _run(ctx)
    except (contract.ContractError, GitError) as error:
        return PhaseOutcome("blocked", str(error))


def inputs_hash(ctx: FlowContext) -> str:
    """Bind the report to the contract, the delivered head (never the live workspace's), the ruling history and the
    reporter knobs (whether it runs, its language)."""
    state = ctx.store.read_state()
    rulings = json.dumps(ctx.store.ruling_entries(), sort_keys=True)
    knobs = ctx.config.values["knobs"]
    text = f"{state['contract_sha256']}\0{delivered_head(state)}\0{rulings}\0{knobs['reporter']}\0{knobs['report_language']}"
    return hashlib.sha256(text.encode()).hexdigest()


def delivered_head(state: dict) -> str:
    """The one revision a report is about: the head the pull request phase recorded as delivered ('' before)."""
    return str((state.get("pr") or {}).get("head") or "")


def validate_body(text: str, familiarity: str, evidence_dir: Path) -> list[str]:
    """Return report-format errors for the configured familiarity level."""
    required = [*FAMILIAR_HEADINGS, *(NEW_HEADINGS if familiarity == "new" else [])]
    errors = [f"missing required heading {heading}" for heading in required if heading not in text.splitlines()]
    if familiarity != "new":
        errors.extend(f"new-only heading {heading} is not allowed for familiar readers"
                      for heading in NEW_HEADINGS if heading in text.splitlines())
    try_section = _section(text, "## What to try")
    if len(TRY_ITEM.findall(try_section)) > MAX_TRY_STEPS:
        errors.append(f"What to try must contain at most {MAX_TRY_STEPS} steps")
    if familiarity == "new" and not _has_base_evidence_link(_section(text, "## How it behaved before"), evidence_dir):
        errors.append("How it behaved before needs a Markdown link to an existing evidence/base file, "
                      "like [before](evidence/base/<file>); a path in backticks is not a link")
    return errors


def _quoted(text: str) -> list[str]:
    """`text` as inert Markdown: HTML escaped, every line (whatever breaks it) prefixed with `> `."""
    return [f"> {html.escape(line, quote=False)}".rstrip() for line in text.strip().splitlines()]


def compose(state: dict, body: str | None, rows: list[contract.Row], rulings: list[dict], steps) -> str:
    """Compose the final report: engine-derived rulings and verification status first, then the reporter's body.

    The status comes first, and the reporter's text is shown as an HTML-escaped Markdown quote, every line
    prefixed, so nothing it wrote can become a heading, a label or a section of its own. `body` None: no reporter
    ran (`knobs.reporter` off), and the report is the engine's section alone.
    """
    title = state.get("title") or f"Task {state['task']} ready for review"
    lines = [f"# {title}"]
    spec_rulings = [entry for entry in rulings if entry.get("spec_change")]
    if spec_rulings:
        lines += ["", "## Rulings that change the spec", ""]
        lines.extend(f"- {entry['decision']} {entry['kind']} {entry['path']}: {entry['why']} "
                     f"(cost if wrong: {entry['cost_if_wrong']})" for entry in spec_rulings)
    lines += ["", "## Verification status", "", ENGINE_LABEL, ""]
    lines.append(_rows_line(rows, steps, state))
    lines += _nothing_to_change(state)
    lines += [f"Preset: {state['preset']}", f"Plan review: {'on' if steps.plan_review else 'off'}",
              f"Code review: {'on' if steps.code_review else 'off'}", _full_tests_line(state),
              f"Pull request: {state.get('pr', {}).get('url') or state.get('pr', {}).get('note') or 'not opened'}"]
    if body is not None:
        lines += ["", REPORTER_LABEL, "", *_quoted(body)]
    return "\n".join(lines) + "\n"


def _rows_line(rows: list[contract.Row], steps, state: dict) -> str:
    """How many rows are green and who proved them: the evaluator, the engine's own checks, the person's marks."""
    passed = sum(row.passes for row in rows)
    if steps.evaluate:
        return f"{passed}/{len(rows)} rows passed by the evaluator"
    by_checks = sum(row.passes for row in contract.proven_by(rows, steps, contract.CHECK))
    by_person = sum(row.passes for row in contract.proven_by(rows, steps, contract.OWNER))
    return (f"{passed}/{len(rows)} rows passed: {by_checks} by engine checks, {by_person} by the person "
            f"(no evaluator, preset {state['preset']})")


def _full_tests_line(state: dict) -> str:
    """What the full-test gate did: a suite ran, or none is configured (the gate then proves nothing)."""
    if state.get("full_tests") == "not configured":
        return "Full tests: not configured, the repository's own tests were not run by the engine"
    return f"Full tests: {state['full_tests']}" if state.get("full_tests") else "Full tests: not run"


def _nothing_to_change(state: dict) -> list[str]:
    """The engine line per build task that needed no change; the builder's reason follows as an escaped, labelled
    quote, so nothing it wrote can pose as an engine statement."""
    lines: list[str] = []
    for task_id, entry in state.get("build", {}).items():
        if entry.get("no_change"):
            lines += ["", f"Nothing to change in {task_id}.", "", BUILDER_LABEL, "", *_quoted(entry["no_change"])]
    return lines


def _run(ctx: FlowContext) -> PhaseOutcome:
    state = ctx.store.read_state()
    rows = contract.load(ctx.store, state["contract_sha256"])
    head = delivered_head(state)
    moved = _off_the_delivered_head(ctx, head)
    if moved:
        return PhaseOutcome("blocked", moved)
    if not ctx.config.values["knobs"]["reporter"]:
        return _ready(ctx, compose(state, None, rows, ctx.store.ruling_entries(), ctx.steps))
    _write_inputs(ctx, rows, state["base_commit"], head)
    failure, body, owner_text = _write_body(ctx, state)
    if failure:
        return failure
    failure = _write_owner_report(ctx, state, owner_text)
    if failure:
        return failure
    return _ready(ctx, compose(state, body, rows, ctx.store.ruling_entries(), ctx.steps))


def _ready(ctx: FlowContext, text: str) -> PhaseOutcome:
    """Save the task report, mark the task ready for review and tell the person."""
    ctx.store.report.write_text(text)
    ctx.store.update_state(lambda current: current.update(status="report_ready"))
    ctx.notifier.notify("ready_for_review", ctx.store.task_id, str(ctx.store.report))
    return PhaseOutcome("next")


def _write_owner_report(ctx: FlowContext, state: dict, owner_text: str) -> PhaseOutcome | None:
    """Publish the owner report into the configured directory; blocked when that cannot be done."""
    values = ctx.config.values
    target = owner_report.path(values, ctx.repo.name, ctx.store.task_id)
    links = [state["pr"]["url"]] if state.get("pr", {}).get("url") else []
    document = owner_report.compose(owner_text, ctx.store.task_id, owner_report.project(values, ctx.repo.name),
                                    datetime.now().astimezone().date(), links, values["knobs"]["report_language"])
    try:
        owner_report.write(target, document)
    except OSError as error:
        return PhaseOutcome("blocked", f"cannot write the owner report to {strip_controls(str(target))}: "
                                       f"{strip_controls(str(error))}")
    ctx.store.append_ledger(PHASE, f"owner report written to {strip_controls(str(target))}")
    return None


def _off_the_delivered_head(ctx: FlowContext, head: str) -> str:
    """Why the report may not be written about `head`: none was delivered, or it is not the head the rows and the
    full tests were verified at, the engine's own head (`flow/final_head.py`). No workspace or branch is read: what a
    survivor adds there is not what the report is about. ''."""
    state = ctx.store.read_state()
    if not head or delivered_head(state) != head:
        return "no delivered head is recorded; the report is written only about the head the pull request phase delivered"
    if state.get("full_green_head") != head:
        return f"the delivered head {head[:12]} is not the head that was verified; nothing is reported"
    if final_head.proved(ctx)[1] != head:
        return f"the delivered head {head[:12]} is not the engine's head; nothing is reported"
    return ""


def _write_inputs(ctx: FlowContext, rows: list[contract.Row], base: str, head: str) -> None:
    """The reporter's inputs; the diff is read from the engine's store, the bytes that were proved and delivered."""
    target = ctx.store.root / REPORT_INPUTS
    target.mkdir(parents=True, exist_ok=True)
    store = final_head.proved(ctx)[0]
    (target / "diff.patch").write_text(git(store, "diff", "--no-ext-diff", "--no-textconv", f"{base}..{head}"))
    (target / "rows.json").write_text(json.dumps([row.__dict__ for row in rows], indent=2) + "\n")
    regular = _files(ctx.store.evidence_dir, exclude_base=True)
    baseline = _files(ctx.store.evidence_dir / "base")
    (target / "evidence.txt").write_text("\n".join(regular) + ("\n" if regular else ""))
    (target / "base-evidence.txt").write_text("\n".join(baseline) + ("\n" if baseline else ""))


def _write_body(ctx: FlowContext, state: dict) -> tuple[PhaseOutcome | None, str, str]:
    """Run the reporter until both bodies validate; returns (failure, English body, owner text)."""
    errors = ""
    for _ in range(MAX_REPORT_ATTEMPTS):
        failure, body, owner_text, found = _attempt(ctx, state, errors)
        if failure or not found:
            return failure, body, owner_text
        errors = "; ".join(found)
    return PhaseOutcome("blocked", f"report or owner report invalid after {MAX_REPORT_ATTEMPTS} attempts: {errors}"), "", ""


def _attempt(ctx: FlowContext, state: dict, errors: str) -> tuple[PhaseOutcome | None, str, str, list[str]]:
    """One reporter run, in a repository of its own made for it and removed afterwards (see the module text)."""
    body_path, owner_path = ctx.store.root / BODY_FILE, ctx.store.root / OWNER_BODY_FILE
    body_path.unlink(missing_ok=True)
    owner_path.unlink(missing_ok=True)
    values = {"task_id": ctx.store.task_id, "familiarity": state["familiarity"],
              "inputs_dir": str(ctx.store.root / REPORT_INPUTS), "output_path": str(body_path),
              "owner_output_path": str(owner_path),
              "owner_format": owner_report.prompt_format(ctx.config.values["knobs"]["report_language"]),
              "previous_error": f"\nCorrect these validation errors: {errors}" if errors else ""}
    repository, workspace = task_repository.make_role_repository(ctx.store, ctx.config.values, ctx.repo, PHASE)
    try:
        request = runner.RunRequest("reporter", PHASE, runner.load_template(PHASE), values, workspace,
                                    {"SAKURA_FLOW_FAMILIARITY": state["familiarity"]}, repository=repository)
        result = ctx.runner.run(request)
    finally:
        task_repository.remove_role_repository(ctx.store, repository)
    failure = runs.failure_outcome(result, "reporter", PHASE)
    if failure:
        return failure, "", "", []
    return None, *_read_bodies(body_path, owner_path, state["familiarity"], ctx.store.evidence_dir,
                               ctx.config.values["knobs"]["report_language"])


def _read_bodies(body_path: Path, owner_path: Path, familiarity: str, evidence_dir: Path,
                 language: str) -> tuple[str, str, list[str]]:
    found = []
    body = owner_text = ""
    if body_path.is_file():
        body = body_path.read_text()
        found += validate_body(body, familiarity, evidence_dir)
    else:
        found.append(f"{BODY_FILE} was not written")
    if owner_path.is_file():
        owner_text = owner_path.read_text()
        found += owner_report.validate(owner_text, language)
    else:
        found.append(f"{OWNER_BODY_FILE} was not written")
    return body, owner_text, found


def _files(directory: Path, exclude_base: bool = False) -> list[str]:
    if not directory.is_dir():
        return []
    files = [path for path in directory.rglob("*") if path.is_file()]
    if exclude_base:
        files = [path for path in files if "base" not in path.relative_to(directory).parts]
    return sorted(path.relative_to(directory.parent).as_posix() for path in files)


def _section(text: str, heading: str) -> str:
    lines = text.splitlines()
    try:
        start = lines.index(heading) + 1
    except ValueError:
        return ""
    end = next((index for index in range(start, len(lines)) if lines[index].startswith("## ")), len(lines))
    return "\n".join(lines[start:end])


def _has_base_evidence_link(section: str, evidence_dir: Path) -> bool:
    base = (evidence_dir / "base").resolve()
    for href in MARKDOWN_LINK.findall(section):
        candidate = Path(href)
        if not candidate.is_absolute():
            candidate = evidence_dir.parent / candidate if href.startswith("evidence/base/") else evidence_dir / candidate
        try:
            path = candidate.resolve()
            path.relative_to(base)
        except ValueError:
            continue
        if path.is_file():
            return True
    return False

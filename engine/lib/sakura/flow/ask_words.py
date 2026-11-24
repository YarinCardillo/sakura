"""The words of an ask, for the person: what a gate title and body say, in plain terms and never in engine ones.

The engine's gate names and step ids stay in the state, the ledger and the commands. Each kind of ask gets its wording
here from the facts passed in, without reading files or consulting Leeway.
"""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence
from datetime import datetime

from sakura.flow import stop_words

SPEC_TITLE = "Read the spec and approve it"
BUDGET_START_TITLE = "Start the build?"
BUDGET_WAIT_TITLE = "Start now?"  # when only the gate is known: the account the lane waits on is not
OVERLAP_TITLE = "Delivery overlaps another lane"
FINAL_REVIEW_TITLE = "Deliver although the review still wants changes?"
OPUS_TITLE = "Use the strongest model for a task?"
OPUS_DETAIL = "A build task needs the Opus model, which costs more of your week than the others."
OTHER_TITLE = "Approve to continue"
BLOCKED_TITLE = "This lane is stuck"
STATE_UNREADABLE = "Sakura cannot read the lane state. Check the host logs."
CONFIG_UNREADABLE = "The lane configuration could not be loaded. Check the host logs."
OWNER_ROW_TITLE_CHARACTERS = 80
NO_ACCOUNT_LABEL = "the account"
TIME_FORMAT = "%a %d %b, %H:%M"
CHECKS_PROVE = "They prove the change works: each one runs on this Mac, in the lane's own workspace."
CHECKS_NOT_PASSED = "Some of the lane's checks have not passed yet, so it cannot be delivered."
TESTS_NOT_PASSED = "The project's tests have not passed on the lane's latest work, so it cannot be delivered."
ALL_TRIED_DETAIL = "You have tried everything. Resume the lane to deliver it."
NOT_WAITING_ANY_MORE = "no longer waiting: this lane is not waiting for your approval any more; nothing was done"
# What the engine says when it blocks a lane, by the start of its line, and what a person reads instead.
LOOKUP_FAILED = "pull request lookup failed"
GH_LOGIN_HINT = "gh auth login"
GITHUB_LOGIN = "GitHub CLI is not logged in: run gh auth login in a terminal, then resume the lane."
LOOKUP_FAILED_WORDS = ("Looking up the lane's pull request on GitHub failed: check that the GitHub CLI works in a terminal "
                       "(gh auth status), then resume the lane.")
CONTRACT_REFUSED = re.compile(r"the code reviewer did not approve the contract flows: (?P<why>.+)", re.DOTALL)
CONTRACT_REFUSED_WORDS = "The reviewer did not approve the contract's checks."
PLAN_REFUSED = re.compile(r"plan review still asked for changes after (?P<rounds>\d+) fix rounds? \(\d+ reviews?\): (?P<why>.+)",
                          re.DOTALL)
MAX_REASON_SENTENCES = 2
MAX_REASON_CHARACTERS = 260
FILE_PATH = re.compile(r"(?<![\w.-])(?:[\w.-]*/)+([\w-]+\.\w+)")  # a path that ends in a file name: only that name is kept
TASK_ID = re.compile(r"\bT\d+\b")
RUN_NAME = re.compile(r"\b(?:build|review|plan|intake|contract|checks|final)-\d+(?:\.\d+)*\b")
SENTENCE_START = re.compile(r"(^|[.!?] )([a-z])(?=[a-z']*[,.!?;:]?(?: |$))")
STUCK_BUILD = re.compile(r"\S+ still needs changes after (?P<rounds>\d+) fix rounds?: (?P<rest>.+)")
NO_COMMIT = "builder made no commit"
COMMIT_REFUSED = ": the commit check refused its commits: "
HOST_PATH = re.compile(r"(?:~|/)(?:[^\s;:,)/]+/)+([^\s;:,)/]+)")  # a path of this Mac: only its last part is kept
NOTHING_NEW_LEAD = "The contract found nothing new to prove: the project may already do what the lane asks."
RESUME_WITH_NOTE = "Resume the lane with a note on what to prove, or stop it."
NOTHING_NEW_WORDS = f"{NOTHING_NEW_LEAD} {RESUME_WITH_NOTE}"
BLOCKED_WORDS = (("contract has no non-baseline row", NOTHING_NEW_WORDS),
                 ("contract rows are still open", CHECKS_NOT_PASSED),
                 ("full tests have not passed", TESTS_NOT_PASSED),
                 ("every owner row is marked", ALL_TRIED_DETAIL))


def checks_title(commands: Sequence[str]) -> str:
    """`Run these 3 checks?`, `Run this check?` for one and `Run the project's checks?` when none is listed."""
    if not commands:
        return "Run the project's checks?"
    return "Run this check?" if len(commands) == 1 else f"Run these {len(commands)} checks?"


def checks_detail(commands: Sequence[str]) -> str:
    """One plain sentence on what the checks prove, then the commands as a list: the person approves commands, so
    every one is shown."""
    return "\n".join([CHECKS_PROVE, "", *(f"- {command}" for command in commands)])


def owner_row_title(flow_text: str) -> str:
    """The title of a check the person makes by hand: what to try, from the first line of the row's own text."""
    first = " ".join(flow_text.split("\n", 1)[0].split())
    return f"Try it yourself: {first[:OWNER_ROW_TITLE_CHARACTERS]}" if first else "Try it yourself"


def blocked_detail(reason: str) -> str:
    """What a person reads for a lane the engine blocked (`blocked_words`, the sentence)."""
    return blocked_words(reason)[0]


def blocked_words(reason: str) -> tuple[str, str]:
    """A lane the engine blocked, for the person: a plain sentence (what failed and what to do, with no path of this Mac in
    it) and the engine's own line apart, as technical detail ("" when the sentence is the line)."""
    sentence = next((words for start, words in BLOCKED_WORDS if reason.startswith(start)), None)
    if sentence is None and (stuck := STUCK_BUILD.fullmatch(reason)):
        sentence = _stuck_build_sentence(stuck["rounds"], stuck["rest"])
    if sentence is None:
        sentence = _reviewer_sentence(reason)
    if sentence is None and reason.startswith(LOOKUP_FAILED):
        sentence = GITHUB_LOGIN if GH_LOGIN_HINT in reason else LOOKUP_FAILED_WORDS
    if sentence is None:
        sentence = stop_words.sentence_of(reason)
    if sentence is None:
        sentence = reason if stop_words.is_plain(reason) else stop_words.GENERIC  # no reason reaches a person in the engine's words
    return sentence, "" if sentence == reason else reason


# What each step of a lane does, in the words the app uses for it.
STEP_WORDS = {"intake": "writing the spec", "contract": "writing the contract", "plan": "planning",
              "plan_review": "reviewing the plan", "review": "reviewing the code", "build": "building",
              "arbiter": "ruling on a request for a protected change", "evaluate": "checking the work",
              "report": "writing the report"}
MAX_FILES_SHOWN = 5


def changed_workspace(role: str, phase: str, files: Sequence[str], is_committed: bool) -> str:
    """A step that must only read the project changed it: who, in which step, which files (or that it added a commit), and
    why the lane stopped. `files` are paths in the project, `role` and `phase` the engine's names for the step."""
    who = role.replace("_", " ")
    step = STEP_WORDS.get(phase, phase.replace("_", " "))
    shown = sorted(files)
    named = ", ".join(shown[:MAX_FILES_SHOWN]) + (f" and {len(shown) - MAX_FILES_SHOWN} more" if len(shown) > MAX_FILES_SHOWN else "")
    if len(shown) > 1 and len(shown) <= MAX_FILES_SHOWN:
        named = f"{', '.join(shown[:-1])} and {shown[-1]}"
    did = " and ".join(part for part in (f"changed {named}" if shown else "", "added a commit" if is_committed else "") if part)
    return (f"The {who} should only read the project, but it {did} while {step}. The lane stopped so that nothing unreviewed "
            "goes on: resume it to try again.")


MAX_FINDINGS_SHOWN = 10
MAX_FINDING_CHARACTERS = 1500


def plain_findings(findings: Sequence[object]) -> tuple[str, ...]:
    """Short findings that keep project file names and check ids while dropping host folders, run names and task ids."""
    shown = []
    for finding in findings[:MAX_FINDINGS_SHOWN]:
        plain = " ".join(str(finding).split())
        plain = HOST_PATH.sub(_without_folders, plain)
        plain = _plain_review_text(plain)
        if plain:
            shown.append(plain if len(plain) <= MAX_FINDING_CHARACTERS else _cut_at_a_word(plain, MAX_FINDING_CHARACTERS))
    return tuple(shown)


def _cut_at_a_word(text: str, limit: int) -> str:
    """`text` cut to at most `limit` characters at a word boundary, with an ellipsis after the last whole word."""
    return text[:limit].rsplit(" ", 1)[0].rstrip(",;: ") + "..."


def nothing_written(what: str, said: str) -> str:
    """A planner that ended without writing the `what` (the spec, the contract, the plan) of its step: what happened, what it
    said at the end when it said anything, and what the person may do."""
    plain = reviewer_reason(said) if said.strip() else ""
    told = f'The planner ended without writing the {what}. It said: "{plain}"' if plain else (
        f"The planner ended without writing the {what} and gave no reason.")
    return f"{told} Resume the lane to ask it again, or stop it."


def nothing_new_to_prove(said: str) -> str:
    """A contract with no new row to prove: the plain sentence, with what the planner said at the end when it said anything."""
    plain = reviewer_reason(said) if said.strip() else ""
    if not plain:
        return NOTHING_NEW_WORDS
    return f'{NOTHING_NEW_LEAD} The planner said: "{plain}" {RESUME_WITH_NOTE}'


def fix_without_commit(said: str) -> str:
    """The builder of a fix for a review ended without changing anything: what happened, what it said at the end when it said
    anything, and what the person may do."""
    plain = reviewer_reason(said) if said.strip() else ""
    told = (f'The final review asked for changes, but the builder ended without making any. It said: "{plain}"' if plain else
            "The final review asked for changes, but the builder ended without making any and gave no reason.")
    return f"{told} Resume the lane to ask it again, or stop it."


def merge_conflict(titles: Sequence[str], files: Sequence[str]) -> str:
    """Tasks of one lane whose work the engine could not join, even after a round of the builder: which tasks (by title)
    changed which files, and what the person may do."""
    named = [f'"{title}"' for title in titles]
    who = f"{', '.join(named[:-1])} and {named[-1]}" if len(named) > 1 else named[0]
    return (f"The lane's tasks {who} both changed {', '.join(files)}, and the engine could not join their work, even after "
            "a round of the builder. Resume the lane to try another round, or stop it.")


def late_work(count: int, status: str, is_pushed: bool) -> str:
    """What a person reads when a message of theirs to a builder left `count` commits in a lane that had gone on without them:
    what is not in the lane's branch and what to do. `status` is the lane's (`report_ready`, `stopped`, `failed` or another),
    `is_pushed` whether it pushed its branch."""
    what, verb = ("1 commit", "it is") if count == 1 else (f"{count} commits", "they are")
    if status == "report_ready":
        return (f"Your message to the builder produced {what} after the lane was delivered: {verb} not in the delivered "
                "branch. Resume the lane to deliver again.")
    if status in ("stopped", "failed"):
        branch = "pushed" if is_pushed else "lane's"
        return (f"Your message to the builder produced {what} after the lane {status}: {verb} not in the {branch} branch. "
                f"The lane {status}, so nothing else was done with {'it' if count == 1 else 'them'}.")
    return (f"Your message to the builder produced {what} after the lane pushed its branch: {verb} not in the pushed "
            "branch. Resume the lane to push it before it goes on to the pull request.")


def _stuck_build_sentence(rounds: str, findings: str) -> str:
    """A build task that still needed changes after its fix rounds: what the builder hit, without the task's id."""
    plural = "fix round" if rounds == "1" else "fix rounds"
    refused = HOST_PATH.sub(_without_folders, findings.partition(COMMIT_REFUSED)[2]).replace('"', "'")
    if findings.startswith(NO_COMMIT) and refused:
        return (f"The builder's commits were refused by the commit check, as its tool printed: \"{refused}\". Look at it in "
                "the builder's chat, then resume the lane.")
    if findings == NO_COMMIT:
        return (f"The builder made no commit after {rounds} {plural}. Look at what it said in its chat, then resume the lane.")
    return f"The builder's work still needed changes after {rounds} {plural}: {reviewer_reason(findings, starts_sentence=False)}"


def _reviewer_sentence(reason: str) -> str | None:
    """The sentence for a lane a reviewer's verdict stopped (the contract's checks or the plan): what the reviewer said, kept
    short and plain, or None when `reason` is not such a verdict."""
    if refused := CONTRACT_REFUSED.fullmatch(reason):
        return f"{CONTRACT_REFUSED_WORDS} {reviewer_reason(refused['why'])}"
    if refused := PLAN_REFUSED.fullmatch(reason):
        rounds = refused["rounds"]
        return (f"The plan reviewer still wanted changes after {rounds} {'fix round' if rounds == '1' else 'fix rounds'}. "
                f"{reviewer_reason(refused['why'])}")
    return None


def reviewer_reason(findings: str, *, starts_sentence: bool = True) -> str:
    """At most two sentences that keep check ids and omit folder paths, run names and task ids."""
    plain = _plain_review_text(plain_folders(" ".join(findings.split())), starts_sentence=starts_sentence)
    kept: list[str] = []
    for piece in re.split(r"(?<=[.;]) +", plain)[:MAX_REASON_SENTENCES]:
        if kept and len(" ".join([*kept, piece])) > MAX_REASON_CHARACTERS:
            break
        kept.append(piece)
    shortened = " ".join(kept).rstrip("; ")
    if len(shortened) > MAX_REASON_CHARACTERS:
        shortened = shortened[:MAX_REASON_CHARACTERS].rsplit(" ", 1)[0].rstrip(",;: ") + "..."
    return shortened if shortened.endswith((".", "...")) else shortened + "."


def _plain_review_text(text: str, *, starts_sentence: bool = True) -> str:
    plain = TASK_ID.sub("a task", RUN_NAME.sub("a run", text))
    return SENTENCE_START.sub(lambda match: match[0].upper() if match[1] or starts_sentence else match[0], plain)


def _without_folders(match: re.Match) -> str:
    return match[1]


def plain_folders(text: str) -> str:
    """`text` with every path of this Mac or of the project reduced to its last part."""
    return FILE_PATH.sub(_without_folders, HOST_PATH.sub(_without_folders, text))


def owner_rows_detail(count: int) -> str:
    """What a lane waiting for the person's own checks says: how many are left, not the commands that mark them."""
    return f"Waiting for you to try {count} {'thing' if count == 1 else 'things'} yourself."


def confirmation_summary(label: str, subject: str) -> str:
    """The line a confirmation shows: the action and what it is about, in the words the person already read."""
    return f"{label}: {subject}"


def overlap_detail(conflicts: Sequence[Mapping]) -> str:
    """One sentence for a delivery that overlaps lanes delivered before it: those lanes (each by the name a person knows),
    then the files they changed that this lane also changes, grouped by file when the lanes did not all change the same."""
    lanes = [str(conflict["lane"]) for conflict in conflicts]
    names = _listing([f'"{lane}"' for lane in lanes])
    every = sorted({name for conflict in conflicts for name in conflict["files"]})
    if not every:
        return f"{names} delivered before this lane and changed files this lane also changes, which are not shown."
    if all(sorted(conflict["files"]) == every for conflict in conflicts):
        both = "" if len(lanes) == 1 else "both " if len(lanes) == 2 else "all "
        return f"{names} delivered before this lane and {both}changed {_listing(every)}, which this lane also changes."
    grouped = [f'{name} ({_listing([chr(34) + str(c["lane"]) + chr(34) for c in conflicts if name in c["files"]])})'
               for name in every]
    return f"{names} delivered before this lane and changed files this lane also changes: {'; '.join(grouped)}."


def _listing(items: Sequence[str]) -> str:
    """`a`, `a and b`, `a, b and c`."""
    return items[0] if len(items) == 1 else f"{', '.join(items[:-1])} and {items[-1]}"


def final_review_detail(rounds: int) -> str:
    """What the final review said when it ran out of rounds: the person decides whether to deliver as it is."""
    spent = f" after {rounds} round{'s' if rounds != 1 else ''}" if rounds else ""
    return f"The reviewer still asked for changes{spent}. You decide whether to deliver the branch as it is."


def budget_wait(record: Mapping, label: str) -> tuple[str, str]:
    """Title and body of the ask of a lane the budget holds back, from the guard's waiting record (`action`, `rule`,
    `until`) and the label of the account it is about (empty when it is not known)."""
    who = label or NO_ACCOUNT_LABEL
    if record.get("action") == "ask":
        return f"Continue without knowing how much of {who}'s week is used?", _unknown_usage(record, who)
    when = _moment(record.get("until"))
    ending = f" Sakura starts the lane again on {when}; approve to start it now." if when else ""
    return f"Start now on {who}?", f"{_why_held(record.get('rule', ''), who)}{ending}"


def _why_held(rule: str, who: str) -> str:
    return {
        "pace": f"{who} is forecast to end its week above the pace line you set.",
        "week": f"{who} has used its week down to the reserve you keep.",
        "five_hour": f"{who}'s 5-hour window is nearly full.",
        "pacing": f"{who} is running ahead of its week, so this lane waits for the night.",
    }.get(rule, f"{who} is over a limit you set.")


def _unknown_usage(record: Mapping, who: str) -> str:
    if record.get("rule") == "pace":
        return f"No account can run this step within the pace line you set; {who} was the closest."
    return f"Sakura cannot read how much of {who}'s week is used, so it asks before the lane goes on."


def _moment(text: object) -> str:
    try:
        return datetime.fromisoformat(str(text)).astimezone().strftime(TIME_FORMAT) if text else ""
    except ValueError:
        return ""

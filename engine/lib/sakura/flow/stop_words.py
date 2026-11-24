"""Plain words for the reasons the engine stops a lane with: what happened and what the person may do, with no task id, file
name the person never wrote, role the app does not show or Python error in it. The engine's own line is kept apart, as
technical detail (`flow/ask_words.py`, `blocked_words`).

`RULES` is the table of the reasons the engine writes itself, by their shape; a reason that fits none of them (a message from
an error of another module) gets `GENERIC`. The test `tests/test_flow_stuck_words.py` walks every place that stops a lane with a
reason and fails for one that has no rule.
"""

from __future__ import annotations

import re

RETRY = "Resume the lane to try again, or stop it."
GENERIC = ("The lane stopped for a reason Sakura could not put into plain words: its own line is under the technical details. "
           + RETRY)

# (the shape of the engine's reason, the sentence a person reads); the first that fits wins.
RULES: tuple[tuple[re.Pattern, str], ...] = tuple((re.compile(shape, re.DOTALL), words) for shape, words in (
    # the lane's own run: its folders, its builders and the agents it launches
    (r"phase \S+ not available",
     "This lane is at a step this version of Sakura cannot run. Update Sakura, then " + RETRY.lower()),
    (r"workspace \S+ is missing; the (?:contract|build) phase must run first",
     "The lane's working folder is gone, so it cannot go on. Stop the lane and start it again."),
    (r"no build task is ready; the plan has a dependency cycle",
     "The tasks of the plan wait for each other, so none of them can start. Stop the lane and start it again."),
    (r"\S+ keeps asking for rulings after \S+ arbiter rounds",
     "A builder keeps asking to change a protected file, even after it was answered several times. Look at what it asked in "
     "its chat, then resume the lane, or stop it."),
    (r"stalled waiting for heavy-work slot: tests\.focused \(.*\)",
     "The project's quick tests could not start because the machine was busy with other work. " + RETRY),
    (r"[\w ]+: tests\.focused \(.*\)",
     "The project's quick tests did not finish while a task was checked. " + RETRY),
    (r"pull request not merged",
     "The pull request is not merged yet, so the lane's folders were kept. Merge it, then resume the lane."),
    (r"stack process .* no longer matches its recorded command",
     "A program the lane started is no longer the one Sakura started, so Sakura left it running. Stop it yourself, then "
     "resume the lane."),
    (r"output cap exceeded: .* was terminated; see .*",
     "An agent wrote more than Sakura allows, so it was stopped. " + RETRY),
    (r"log replaced during the run: .* was terminated; see .*",
     "An agent's log was changed while it ran, so it was stopped. " + RETRY),
    (r".* left processes running after it exited during .*; see .*",
     "An agent left programs running after it ended, so Sakura stopped them and did not use its result. " + RETRY),
    (r"(?:timed out|stalled): \S+; see .*", "An agent took too long or stopped answering. " + RETRY),
    (r".* exited \d+ during \S+; see .*",
     "An agent ended with an error. Look at what it said in its chat, then resume the lane, or stop it."),
    (r".* could not do its job in \S+: .*; see .*",
     "An agent said it could not do its job. Look at what it said in its chat, then resume the lane, or stop it."),
    # the planner's work: spec, contract, plan, and the report
    (r"contract flows are wrong on the base build: .*",
     "The checks the planner wrote do not behave as they should on the project as it stands. Resume the lane to ask the "
     "planner again, or stop it."),
    (r"contract invalid after \S+ attempts: .*", "The planner's checks were not valid after several tries. " + RETRY),
    (r"the contract went back to the planner more than once",
     "The planner's checks were sent back for changes and still were not right. " + RETRY),
    (r"[\w ]+: focused tests",
     "The project's quick tests did not finish while the checks were prepared. " + RETRY),
    (r"intake failed: .*", "The planner could not write a usable spec. " + RETRY),
    (r"plan files changed during plan review", "The plan was changed while it was being reviewed, so it was not used. " + RETRY),
    (r"plan invalid after \S+ attempts: .*", "The planner's plan was not valid after several tries. " + RETRY),
    (r"plan invalid at approval: .*", "The plan is not valid. " + RETRY),
    (r"report or owner report invalid after \S+ attempts: .*",
     "The reporter's report was not valid after several tries. " + RETRY),
    (r"cannot write the owner report to .*",
     "Sakura could not save the owner report. Check that its folder can be written to, then resume the lane."),
    # the checks: the evaluator, the fix rounds for the checks, and the project's full tests
    (r"calibration copies no test file of the head: .*",
     "The checker could not set itself up against the project's tests. " + RETRY),
    (r"evaluator miscalibrated: .*",
     "The checker did not behave as it should when it was tried on the project, so its results were not used. " + RETRY),
    (r"fix rounds exhausted; rows still need work: .*",
     "The checks still fail after the builder's fix rounds. Look at what the builder said in its chat, then resume the lane, "
     "or stop it."),
    (r"workspace \S+ cannot be moved to the engine's head: .*",
     "The lane's folder could not be brought up to date with its work. " + RETRY),
    (r"\S+ code review needs revision: .*",
     "The reviewer still wants changes to a fix for the checks. Read its findings in the lane's chat, then resume the lane, "
     "or stop it."),
    (r"\S+ contract commands changed and wait for the person's approval; see sakura flow status",
     "The commands of the checks changed and need your approval before the lane goes on. Approve them, then resume the lane."),
    (r"[\w-]+: builder made no commit",
     "The builder of a fix for the checks ended without changing anything. Resume the lane to ask it again, or stop it."),
    (r"[\w-]+: evaluator (?:modified the evaluated workspace|workspace has ).*",
     "The checker changed the project while it checked it, so its result was not used. " + RETRY),
    (r"[\w-]+: focused tests failed", "The project's quick tests fail on a fix for the checks. " + RETRY),
    (r"[\w-]+: pending candidate head does not match workspace",
     "The fix the builder made is not the one Sakura was following, so it was not used. " + RETRY),
    (r"full tests not green: .*",
     "The project's tests fail on the lane's work. Look at the failure, then resume the lane, or stop it."),
    (r"stalled", "The project's tests stopped answering. " + RETRY),
    (r"tests\.full not run: .*", "The project's full tests could not run. " + RETRY),
    (r".*: full tests \(.*\); see .*", "The project's full tests did not finish. " + RETRY),
    # delivery: the push and the pull request
    (r"refusing to push to .*: the ext:: transport runs a program",
     "Sakura refused to push to this address because it would run a program on this Mac. Change the project's remote to a "
     "normal address, then resume the lane."),
    (r"push stalled; see .*",
     "Pushing the branch took too long. Check the connection to the remote, then resume the lane."),
    (r"git push failed: .*",
     "Git could not push the branch to the remote. Check that you can push to it from a terminal, then resume the lane."),
    (r"push not verified",
     "The remote does not show the pushed branch yet. Resume the lane to check again, or stop it."),
    (r"not pushed: .*", "The branch is not pushed yet. Resume the lane to push it."),
    (r"no push URL was recorded when the task started .*",
     "This project has no remote to push to, so the lane cannot be delivered. Add a remote to the project and start the lane "
     "again."),
    (r"pull request lookup returned (?:an invalid result|invalid JSON)",
     "GitHub's answer about the lane's pull request could not be read. Resume the lane to look again, or stop it."),
    (r"pull request creation stalled; see .*",
     "Opening the pull request took too long. " + RETRY),
    (r"gh pr create failed: .*",
     "The GitHub CLI could not open the pull request. Check it with gh auth status in a terminal, then resume the lane."),
    (r"gh pr create returned no pull request URL",
     "GitHub did not give back the address of the new pull request. Resume the lane to look again, or stop it."),
    (r"an existing pull request for this head is not a draft",
     "A pull request for this work already exists and is not a draft, so Sakura left it alone. Close it or turn it into a "
     "draft, then resume the lane."),
    # the engine itself
    (r"the engine stopped on an error: .*", "Sakura stopped on an unexpected error. " + RETRY),
    (r"state\.json changed outside the engine",
     "Something changed the lane's records outside Sakura, so it stopped to be safe. Stop the lane, or resume it if you made "
     "the change yourself."),
    (r"the engine keeper resumed this lane twice within a few minutes and it stopped again; .*",
     "Sakura restarted this lane twice and it stopped again each time, so it left it to you. Look at what it said in its chat, "
     "then resume the lane, or stop it."),
))


# The sentences other modules write for the person themselves (`flow/ask_words.py`): a reason that starts so is already plain.
PLAIN_REASONS = re.compile(r"(?:The planner ended without writing |The final review asked for changes, but |The contract found nothing new to prove|"
                           r"The [\w ]+ should only read the project, but |Your message to the builder produced )")


def sentence_of(reason: str) -> str | None:
    """The plain sentence for an engine reason that has a rule, else None."""
    return next((words for shape, words in RULES if shape.fullmatch(reason)), None)


def is_plain(reason: str) -> bool:
    """Whether `reason` is already a sentence for the person, written by `flow/ask_words.py`."""
    return PLAIN_REASONS.match(reason) is not None

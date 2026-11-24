"""Command line of `sakura stack`: the project's local stack (Docker and the like), the person's own action.

`stack.up` and `stack.down` come from host config only (personal or project file in the engine home, or a flag);
no agent can start them: the engine offers roles no stack tool, request or approval. The person runs
`sakura stack up|down [--repo PATH]`: the command and its layer are printed, and it runs outside every sandbox, with
the person's rights, from the engine's read-only checkout of the repository's committed HEAD
(`flow/stack_checkout.py`); uncommitted changes are not in it. An agent that needs the stack says so in its text.
"""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

from sakura import engine_home, programs, repository
from sakura.flow import brakes, config, controls, exits, gitutil, stack_checkout, task_repository

STACK_TIMEOUT_SECONDS = 900
REFUSAL = ("sakura stack cannot run inside an engine role or an agent cage; the stack is the person's own command, "
           "so say in your report that the work needs it")


def main(argv: list[str]) -> int:
    """Run `sakura stack up|down [--repo PATH]` and return its exit code."""
    parser = argparse.ArgumentParser(prog="sakura stack")
    parser.add_argument("action", choices=("up", "down"))
    parser.add_argument("--repo", default=".")
    args = parser.parse_args(argv)
    if os.environ.get(brakes.ROLE_ENV) or engine_home.sandboxed():
        print(REFUSAL)
        return exits.USAGE
    try:
        # A task workspace stands for the person's repository; nothing in it says which (review 10, finding 4).
        repo = task_repository.operator_repository(Path(args.repo))
        loaded = config.load(repo)
    except (repository.RepositoryError, config.FlowConfigError, engine_home.EngineHomeError,
            task_repository.TaskRepositoryError) as error:
        print(error)
        return exits.USAGE
    verb = args.action
    command = loaded.values["stack"][verb]
    if not command:
        print(f"no stack.{verb} command is configured for {repo.name}")
        return exits.USAGE
    try:
        gitutil.trust_repository(repo)
        revision = gitutil.git(repo, "rev-parse", "HEAD").strip()
        source = stack_checkout.checkout(repo, revision)
    except (gitutil.GitError, stack_checkout.StackCheckoutError) as error:
        print(f"stack.{verb} not run: {controls.escaped(str(error))}")
        return exits.USAGE
    print(f"stack.{verb} ({loaded.origins.get(f'stack.{verb}', 'defaults')} layer): {controls.escaped(command)}")
    print(f"it runs outside the cage, with your rights, from the engine's read-only checkout of {revision[:12]} "
          f"(the committed HEAD of {repo}; uncommitted changes are not in it)", flush=True)
    return _run(verb, command, source)


def _run(verb: str, command: str, source: Path) -> int:
    try:
        done = subprocess.run([programs.path("sh"), "-c", command], cwd=source, stdin=subprocess.DEVNULL, check=False,
                              timeout=STACK_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        print(f"stack.{verb} timed out after {STACK_TIMEOUT_SECONDS} seconds")
        return exits.FAILED
    print(f"stack.{verb} exited {done.returncode}")
    return exits.OK if done.returncode == 0 else exits.FAILED

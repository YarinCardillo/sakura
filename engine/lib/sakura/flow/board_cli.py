"""`sakura board show [--repo PATH]`: the posts on a project's board, oldest first, each with its author."""

from __future__ import annotations

import argparse
from pathlib import Path

from sakura import engine_home, repository
from sakura.flow import board, exits, render, task_repository


def main(argv: list[str]) -> int:
    """Print the board of the repository at `--repo` (default: here); 2 when it is not a repository."""
    parser = argparse.ArgumentParser(prog="sakura board")
    parser.add_argument("action", choices=("show",))
    parser.add_argument("--repo", default=".")
    args = parser.parse_args(argv)
    try:
        repo = task_repository.operator_repository(Path(args.repo))
        posts = board.read(engine_home.project_dir(repo))
    except (repository.RepositoryError, task_repository.TaskRepositoryError, engine_home.EngineHomeError,
            board.BoardRefused, OSError) as error:
        print(error)
        return exits.USAGE
    if not posts:
        print(f"no posts on the board of {repo.name}")
    for post in posts:
        print(f"[{post.seq}] {post.at} {_who(post.author)}: {_one_line(post.text)}")
    return exits.OK


def _who(author: board.Author) -> str:
    return f"role {author.task}/{author.role}" if author.kind == "role" else author.kind


def _one_line(text: str) -> str:
    """The text on one line, so a post cannot look like two."""
    return " ".join(render.strip_controls(text).split())

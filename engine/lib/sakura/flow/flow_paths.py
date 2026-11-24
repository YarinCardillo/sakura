"""Where a contract flow may live: a host-owned, root-scoped allowlist and hard denials.

The planner's flow files are committed by the engine itself as the first step of its head and pushed with the task,
so the boundary of what they may be is an authority boundary (review 12, finding 1). It never comes from the
repository: `brakes.test_patterns` is the repository's and may only widen what is protected, so it is not used here.
The allowlist is `contract.flow_paths`, the engine's defaults `tests/**`, `test/**` and `e2e/**`, which only the
person's host config may change (a repository layer may not set the key), and every pattern must be rooted: its
first path component is a plain folder name; it matches exactly. Whatever the allowlist says, the hard denials always
apply: CI and automation folders and files, Git metadata, the engine's own folder, and package, build and test-runner
configuration, wherever they are in the tree, matched ignoring case, since a case-insensitive volume or checkout
reads `.GitHub/workflows` as `.github/workflows`. For the same reason a flow may not collide, ignoring case, with a
path of the tree it is added to or with another flow (`collisions`).
"""

from __future__ import annotations

from sakura.flow import globs

DEFAULT = ("tests/**", "test/**", "e2e/**")
# What the engine reads of agent-written test and flow files into its own memory (review 13, finding 3; review 15,
# finding 5): the planner's flows and calibration's copies of the head's test files alike. A file over a limit is
# refused by its size before any of it is read.
MAX_FLOW_FILES = 200
MAX_FLOW_FILE_BYTES = 1 << 20
MAX_FLOW_TOTAL_BYTES = 8 << 20
# Paths no contract flow may be, at any depth: what runs on a remote, what Git or a tool reads as configuration, and
# what installs or builds the project.
DENIED = (
    # CI and automation
    "**/.github/**", "**/.gitlab-ci.yml", "**/.gitlab/**", "**/.circleci/**", "**/.buildkite/**", "**/.travis.yml",
    "**/.drone.yml", "**/.woodpecker/**", "**/.woodpecker.yml", "**/azure-pipelines*.yml", "**/.azure-pipelines/**",
    "**/bitbucket-pipelines.yml", "**/Jenkinsfile", "**/.pre-commit-config.yaml", "**/.husky/**", "**/.githooks/**",
    # Git metadata and the engine's own folder
    "**/.git", "**/.git/**", "**/.gitattributes", "**/.gitmodules", "**/.gitignore", "**/.mailmap", "**/.sakura/**",
    # Package, build and test-runner configuration
    "**/package.json", "**/package-lock.json", "**/yarn.lock", "**/pnpm-lock.yaml", "**/pnpm-workspace.yaml",
    "**/.npmrc", "**/.yarnrc*", "**/pyproject.toml", "**/setup.py", "**/setup.cfg", "**/requirements*.txt",
    "**/Pipfile*", "**/poetry.lock", "**/uv.lock", "**/tox.ini", "**/noxfile.py", "**/pytest.ini", "**/conftest.py",
    "**/Cargo.toml", "**/Cargo.lock", "**/go.mod", "**/go.sum", "**/Gemfile*", "**/*.gemspec", "**/pom.xml",
    "**/build.gradle*", "**/settings.gradle*", "**/gradle.properties", "**/Makefile", "**/makefile", "**/*.mk",
    "**/CMakeLists.txt", "**/Dockerfile*", "**/docker-compose*.yml", "**/compose*.yml", "**/*.config.js",
    "**/*.config.cjs", "**/*.config.mjs", "**/*.config.ts", "**/tsconfig*.json", "**/.babelrc*", "**/.eslintrc*",
)


_DENIED_FOLDED = [pattern.casefold() for pattern in DENIED]


def size_refusal(name: str, size: int, left: int) -> str:
    """Why a file of `size` bytes may not be read when `left` bytes are left of the total limit, or ''."""
    if size > MAX_FLOW_FILE_BYTES:
        return f"{name} is {size} bytes, more than the per-file limit of {MAX_FLOW_FILE_BYTES} bytes"
    if size > left:
        return f"{name} is {size} bytes, more than what is left of the total limit of {MAX_FLOW_TOTAL_BYTES} bytes"
    return ""


def count_refusal(count: int) -> str:
    """Why `count` files may not be read, or ''."""
    return f"{count} files, more than the limit of {MAX_FLOW_FILES}" if count > MAX_FLOW_FILES else ""


class FlowPathError(ValueError):
    """A host allowlist pattern that is not rooted."""


OUTSIDE_FOLDERS = "is outside the contract flow folders"


def refusal(name: str, allowed: list[str] | tuple[str, ...]) -> str:
    """Why `name` may not be a contract flow file under the host allowlist `allowed`, or ''."""
    if globs.matches_any(name.casefold(), _DENIED_FOLDED):
        return f"{name} is a CI, Git, engine or build configuration path, which no contract flow may be"
    if not globs.matches_any(name, list(allowed)):
        return f"{name} {OUTSIDE_FOLDERS} ({', '.join(allowed)}; host config contract.flow_paths)"
    return ""


def collisions(names: list[str], tracked: list[str]) -> list[str]:
    """Each flow name of `names` that is, ignoring case, a path of `tracked` (the tree it is added to) or another flow
    name, with what it collides with."""
    in_tree = {path.casefold(): path for path in tracked}
    seen: dict[str, str] = {}
    found = []
    for name in sorted(names):
        key = name.casefold()
        if key in in_tree:
            found.append(f"{name} collides, ignoring case, with the tracked {in_tree[key]}")
        elif key in seen:
            found.append(f"{name} collides, ignoring case, with the flow {seen[key]}")
        seen.setdefault(key, name)
    return found


def require_rooted(patterns: object) -> None:
    """Raise FlowPathError unless `patterns` is a list of rooted patterns (`<folder>/...`, no wildcard first)."""
    if not isinstance(patterns, list) or not patterns or any(not isinstance(item, str) for item in patterns):
        raise FlowPathError("contract.flow_paths must be a non-empty list of patterns")
    for pattern in patterns:
        first = pattern.split("/", 1)[0]
        if (pattern.startswith("/") or "/" not in pattern or not first or first in (".", "..")
                or any(char in first for char in "*?[")):
            raise FlowPathError(f"contract.flow_paths pattern {pattern!r} must start with a plain folder name, "
                                "like tests/**")

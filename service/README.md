# agentbox

Workspaces for coding agents on one Mac, in one of two places chosen per lavoro (task): on the Mac itself (a
git worktree of your repository, the agent running natively as you; the default) or in a box (a confined,
limited container; limits come from Docker, never from a request to the agent). See "The two places" below. Contract for the desktop app: `docs/interface.md`.

## Host requirements

- `jq` (`brew install jq`), git, perl, the system bash (3.2).
- For tasks in a box: Docker Desktop installed and RUNNING (every command that needs it fails fast with one
  message if it is not). python3 runs inside the image.
- For lavori on the Mac: no Docker. `python3` (for `box tree`, `box file`, `box search` and the result of a
  check), and `npm` with `node` for `box acp` (the adapter is installed once on first use).
- Put `bin/box` on your PATH (a symlink is fine), or call it by path.
- Data lives in `$AGENTBOX_HOME`, by default the application data folder of the system (`~/Library/Application Support/Sakura` on macOS, `~/.local/share/sakura` on Linux).

## The two places

`box new <task> <project> [--where mac|box]`; the project names the default (`box project set <project> --where
box`), and a project that does not say gets `mac`. The place is recorded when the lavoro is created and never
changes. The two never mix: nothing of a lavoro on the Mac is ever mounted in a container, and a task in a box is
never read with git on the host.

- `mac`: a git worktree of your repository at `$AGENTBOX_HOME/work/<task>` on a new branch `sakura/<task>`
  (`--branch-prefix` changes the prefix). **The agent runs as you, with your privileges: nothing confines it.**
  `box` gives it order, not a wall. In short:
  - `box new` runs the project's `--setup` command once in the worktree (`box log <task> --setup` shows its
    output).
  - `box acp <task> [<account>]` is the conversation: the Claude ACP adapter run natively in the worktree with one
    of your existing logins (`$AGENTBOX_HOME/config` names them, one `ACCOUNT_<name>=<config directory>` line
    each; the first run writes `ACCOUNT_claude` for `~/.claude` when it exists, and `BOX_FIRST_ACCOUNTS` names
    others to look for; `box accounts --json` lists them; a project names its default with
    `--account`). `box stop <task>` ends it.
  - `box play <task> start|stop|status [--json]` and `box play <task> log` run the project's `--run` command with
    a range of ports of its own (`SAKURA_PORT`, from 56000 up; `--ports`, `--run-mode one|many`).
  - `box verify <task> <verb>` queues a check; the runner runs it natively in the worktree, one at a time across
    all lavori, and only on a worktree without uncommitted changes.
  - `box tree`, `box file`, `box search`, `box graph`, `box log`, `box diff`, `box facts` read it with plain git.
  - Publishing is the agent's job (it has your git and `gh`): ask it to push and open a draft pull request.
  - `box rm <task>` runs the project's `--archive` command, removes the worktree and keeps the branch; it
    refuses while files are uncommitted or commits are neither pushed nor merged, unless `--force`.
  - `box shell <task>` opens your own login shell in the folder of the lavoro (not confined; `box rm` refuses
    while it is open, `--force` ends it).
  - `box take`, `box run`, `box agent`, `box attach` do not apply (one line, exit code 80).
- `box`: everything below. The steps that follow describe a task in a box; `box up`, `box login` and Docker are
  needed only for those.

Details, JSON shapes and exit codes of both: `docs/interface.md`.

## Start here: the ten things you need, in order (a task in a box)

1. `box up`: builds the image `agentbox:4` if missing, starts the internal network and the allow-list proxy.
   The tag is named once, in `lib/common.sh`; a change to `image/Dockerfile` gets a new tag, so the image under
   running boxes is never replaced. Boxes started before keep their image until they are started again.
2. `box login <account>`: once per account (for example `personal`). Runs `claude auth login`, you sign in, the login is
   stored in the Docker volume `agentbox-acct-<account>`, not on the Mac.
3. `box runner --daemon`: starts the one runner that executes heavy checks, detached. Log:
   `$AGENTBOX_HOME/runner.log`. `box runner --stop` stops it. Keep it running; without it `verify` refuses.
   `box runner --install` makes the runner start at login (LaunchAgent) and `--uninstall` undoes it; stop a runner
   started by hand first.
4. A project is WHAT is worked on: a repository on this Mac, an optional subfolder, the base branch, the install
   command, the commands of the checks, the protected paths and the test file pattern. Nobody edits a file to
   work on another repository or folder: `box project detect <folder> --json` proposes a project from a folder
   you choose (it writes nothing), and `box project add <name> --repo <path> [--subdir <dir>] [--base <branch>]
   [--install <cmd>] [--check verb=command]... [--protected <globs>] [--test-files <globs>]` adds it, once.
   `box project set`, `box project rm`, `box projects --json` and `box branches <project> --json` are the rest.
   (What KIND of box a task gets, the preset, is a separate thing and not built yet.)
   `box new <task> <project> [--base <branch>] --where box`: clones the project's repository into
   `$AGENTBOX_HOME/work/<task>` on branch `box/<task>`, from the project's base branch or the one you name,
   and installs dependencies in a container. Task names: lowercase letters, digits, single hyphens.
5. `box agent <task> <account> [--detach] [-- <claude args>]`: Claude Code in the workspace. With `--detach` it
   runs in the background; `box attach <task>` joins it (detach with ctrl-backslash twice), `box stop <task>`
   stops it. `box shell <task>` opens a plain shell for you. `box acp <task> <account>` is the same box for a
   client that speaks the Agent Client Protocol on stdin and stdout (the conversation view of Sakura); one of the
   two per task at a time. Details and exit codes: `docs/interface.md`.
6. `box verify <task> <verb> [--no-wait]`: queues `test`, `build` or `rules` for the runner, waits and prints the
   result for the current commit. It never waits forever. Exit codes of its own: 75 the runner is not running,
   76 the request was quarantined, 77 the deadline passed. Anything else is the verb's exit code.
7. `box status [--json]`: tasks, branches, commits ahead, dirty files, containers, queue, last result per
   verb. `box facts <task> --json`, `box log`, `box diff`, `box result-log`, `box journal` give the details.
8. `box take <task>`: fetches the task branch into the source repository (through a bundle written in a sandbox).
   Nothing leaves a workspace any other way.
9. `box rm <task> [--force]`: removes the task; refuses while commits are not taken or files are dirty.
10. Where things live: see below.

### Where things live

| What | Where |
|-|-|
| Workspaces (plain git clones, open in any editor) | `$AGENTBOX_HOME/work/<task>` |
| Task records, queue, journal, runner log, quarantine | `$AGENTBOX_HOME/{tasks,queue,journal.jsonl,runner.log,quarantine}` |
| Results of verifications | `$AGENTBOX_HOME/results/<task>/<id>.json` and `.log` |
| Where agents ask for a check | `$AGENTBOX_HOME/drop/<task>` (empty file named like the verb) |
| Projects (repository, subfolder, base, install, checks) | `$AGENTBOX_HOME/projects/<name>.env`, written by `box project add` and `set`, read as data |
| Shipped profiles | `profiles/<name>.env` in this repository: on upgrade each becomes a project, once (`selftest` only when something names it); not read afterwards |
| Rules the agent reads | `rules/agent.md`, mounted as the agent's CLAUDE.md |
| Lavori on the Mac: logins, setup logs, Play state and logs, the ACP adapter | `$AGENTBOX_HOME/config`, `setup/<task>.log`, `play/<task>.*`, `tools/claude-agent-acp-<version>/` |
| Allow-list of hosts (one anchored line per name, `^name\.tld:443$`: tunnels to port 443 only) | `proxy/allow.txt` |
| Docker objects (all start with `agentbox`) | image `agentbox:4`, networks `agentbox-net`, `agentbox-net-<task>` and `agentbox-egress`, container `agentbox-proxy`, volumes `agentbox-nm-<task>`, `agentbox-pristine.<task>.<key>`, `agentbox-npm-cache-<task>`, `agentbox-acct-<account>`, `agentbox-cfg-*` (every derived name is built in `lib/names.sh`) |

`box run <task> -- <cmd>` runs one command in a confined container. `box projects --json` and
`box accounts --json` list what exists.

## Rules: what is trusted and what is not

These rules are about tasks in a box. A lavoro on the Mac is your own worktree and your own agent, running as
you: nothing in it is treated as hostile and none of the machinery below applies to it. What `box` guarantees
there is separation from the boxes: no container ever mounts it, and host git is pointed at it only through the
git directory `box` recorded when it created the worktree (`docs/interface.md`, "The two places").

NOT trusted:
- Everything under `work/<task>`, including its `.git`, its config and hooks. The agent writes it. After
  `box new` created it, the host never runs git there. Facts (HEAD, status, log, diff, package files) are read by
  a sandbox container: no network, no capabilities, read-only root, the workspace mounted read-only. What comes
  back is validated (shas, names, times) and re-encoded before it is shown or used.
  Commits are never read from the agent's own object store: the base comes from the profile's source
  repository (read-only) and the agent's commits are imported into a per-task verified repository with every
  object re-hashed (`index-pack --strict`); a forged object makes the import fail and the result counts as a
  change to the checks. Replace objects, grafts, commit-graph and multi-pack-index files of the workspace are
  ignored. Details: `docs/interface.md`.
- The drop directory. A request is an empty file whose NAME is the verb; contents are never read. Symlinks,
  directories, files that are not empty, odd names and verbs not in the profile are moved out by rename and
  never followed: each into a folder of `quarantine/` that `box` reserved for it alone (`drop-<task>.XXXXXX`),
  under the fixed name `entry`, so nothing an agent made is ever the destination of a move. A plain file is
  kept there, unread; a link, a FIFO, a device or a directory is removed where it was put aside, and only its
  name and kind are written down (`about`). Whether an entry is an empty regular file is read in a container
  that mounts only that directory (lstat of the names the host picked; it lists nothing), and only for names
  that look like verbs. Each time round the runner looks at no more than 25 entries of a task's drop directory
  (`BOX_DROP_INTAKE_BUDGET`; every entry counts, whatever it is), never lists the directory whole, and serves
  the queue before it looks again: a task that fills its drop directory does not hold up the checks of the
  others.
- Logs of a verification. A job runs agent-written test code, so its log is text to display, never to execute.
- `box diff` output: raw patch bytes, display as text only.

Trusted (the host writes them, no container can):
- Task records `tasks/<task>.env` (project, base branch, base sha), the queue, the results directory (containers
  see it read-only), the journal, the projects, the proxy allow-list.
- The source repository named by the project (`take` writes only the branch `box/<task>` into it). It is the
  one place where `box` runs git on the host, always through one wrapper that lets nothing of the repository's
  configuration run (no hooks, no file system monitor, no pager, no helper), and only after checking, by device
  and inode, that the folder is not in the data directory, and that it belongs to you.

What an agent container can do: see its workspace, the dependency volume, the npm cache, its own configuration
volume plus the credential files of its account, the drop directory (write) and its results (read). Each task has its own internal network (only the proxy is attached to all of them) and its own npm cache volume. 2 CPUs, 3 GB, no capabilities, read-only root, no Docker
socket, no home directory of yours. The network is only the proxy, which allows the package registry, GitHub and
the model APIs, as HTTPS tunnels to port 443 only; ordinary HTTP through the proxy, IP literals, port 22 and
every other port, `host.docker.internal` and overlay network addresses are refused.

What a runner job can do: run only a verb the profile names, on a clean checkout of one commit, with pristine
dependencies mounted read-only (built from `package.json` and the lockfile, install scripts off), 4 CPUs and
6 GB, with a deadline. One request at a time for everyone.

The small containers `box` starts for a step of its own (the checkout of the commit of a request, the copy of
the dependencies, the first mount of a volume, the look at which credential files an account has) all get one
policy: 1 CPU, 1 GB with no extra swap, 256 processes, no capabilities, no network, removed when they end, each
under its deadline. A commit can be small and still check out into a very large tree; the checkout then fails
inside its own limits and the request with it.

Limits to know: the runner's dependency volumes (pristine, mounted read-only into jobs) belong to one task and
to what they were built from (package.json, the lockfile, the install and rebuild commands): no task runs on
dependencies another task built or on ones built another way, which costs one install and one copy of the
dependencies per task. Their name holds a dot, which no task name can, so no task can own one. Volumes `agentbox-nm-lock-<hash>` left by an older
`box` are not used any more (`box status` lists them as orphans; remove them with `docker volume rm`). Tasks
that used `box agent` before the per-task network existed keep running on the old shared network until restarted.
Boxes of one account share only the credential files of the account; each task has its own configuration
directory (see `docs/interface.md`, account configuration; the real token refresh is not yet verified).

One proxy serves every task, with 64 connections in all and no share kept for each task: a task that opens
and holds 64 tunnels to allowed names takes the way out from the others, whose installs and model requests
then fail or wait for their deadlines, while that task stays within its own CPU, memory and process limits.
This is accepted as an availability limit of a machine with one person on it, who can see and stop the task
(`box status`, `box rm`); it lets nothing out that was not allowed. There are no proxies per task.

## Tests

`bash tests/run.sh` runs the bash tests with a fake docker and a fake npm (no Docker needed); the tests of the
lavori on the Mac run real git on throwaway repositories and real processes. `tests/confinement.sh m1 s1 s2`
runs the live confinement and attack checks against real containers (runner daemon must be running, `s1` and
`s2` are tasks of the `selftest` project, which the shipped profile of that name becomes when `box new s1 selftest`
first names it; its source is a throwaway clone at `$AGENTBOX_HOME/selftest-source`).

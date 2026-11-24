# sakura

sakura adds deterministic local workflow gates to any Git repository. It blocks commits and pushes to protected branches, validates branch names, can require signed commits, checks Alembic heads, requires a passing fast-test stamp for the pushed tree, and keeps temporary integration branches local. Agent hooks require draft pull requests, ask before making a pull request ready, reject verification and Git-config bypasses, and fingerprint their own guarded configuration after shell commands.

These are local guardrails, not an authorization boundary. A user with the same shell access can alter Git configuration or forge local state, including state held in the engine home. The authority is server-side branch protection and required CI, configured on the remote host.

## Profiles

Settings are merged in this order. Later layers can tighten a setting. To weaken an earlier setting, the later file must contain `relax = ["setting.name"]`; `sakura status` reports every such relaxation.

1. Built-in defaults
2. `~/.config/sakura/profile.toml`
3. `<repo>/.sakura/profile.toml`
4. `<repo>/.sakura/local.toml`, excluded from Git during install

```toml
integration_branch = "main"
protected_branches = ["main", "release"]
require_signed_commits = true

[tests]
fast = ["python3 -m pytest -q"]
full = ["python3 -m pytest -q", "npm test"]

[migrations]
alembic_versions = "migrations/versions"

[integration]
setup = ["python3 -m pytest -q"]
```

Defaults are `main`, `master`, and the integration branch as protected branches; the standard `feat|fix|chore|refactor|docs|test|perf|security|hotfix` branch pattern; unsigned commits allowed; draft pull requests and user confirmation before ready required; and empty test, migration, and integration setup settings.

## Install and use

From a Git worktree:

```sh
/path/to/sakura/bin/sakura install
sakura status
sakura test fast
sakura integra 123 456
sakura integra --full 123
```

Install sets a per-worktree `core.hooksPath`, writes Claude Code and Codex hook configuration, and excludes `.sakura/local.toml`. `sakura test fast` records a stamp in the engine home only after every configured command passes. The pre-push hook checks that exact tree. If `tests.fast` is empty, the stamp check is skipped and the test command says so.

Claude Code asks for user approval before `gh pr ready`. Codex does not honor that decision, so sakura denies the command and tells the agent to ask the user to run it themselves.

`sakura box ...` hands its arguments, unchanged, to the Sakura service command: the file named by `SAKURA_BOX_BIN`, else `box` on `PATH`. Without either it exits 127 and says so. `sakura flow`, `lane`, `quota`, `usage`, `cockpit` and the other commands run in the engine.

`bin/agentkit` is the old name. It prints one deprecation line on stderr and runs `bin/sakura` with the same arguments.

`sakura integra` fetches `pull/N/head`, starts from the configured integration branch, merges each head on `sakura-integration/local`, prints conflicting files and aborts on conflicts, runs configured integration setup, and assigns a nonexistent push remote. `--full` runs the configured full test suite afterwards.

## Machine load governor

Use `sakura run --heavy -- <command...>` for tests, builds, scans, and other commands that use substantial machine resources. Heavy commands across worktrees share file-lock slots under `~/.cache/sakura/slots/`. The default slot count is one third of the available cores, with at least one slot. Set `SAKURA_HEAVY_SLOTS` to override it, or set `load.slots` in flow configuration (`0` keeps automatic sizing). Add `--critical` or set `SAKURA_FLOW_CRITICAL=1` to move a queued command ahead of normal waiters.

Flow agents that edit code run at lower priority. App stacks idle longer than `load.idle_stack_seconds` are stopped while their worktrees and logs are kept. `sakura flow status <task>` shows slot use, queued commands, and the system load average.

Repository-defined Claude hook commands are never run automatically. If a user wants to permit the current `.claude/settings.json`, they run `sakura trust`, which records its content hash in the engine home. Any change requires a new explicit trust decision.

## Uninstall

Run `sakura uninstall [path]` to remove this worktree's `core.hooksPath`. Review and remove the generated `.claude/settings.local.json` and `.codex/hooks.json` entries if they are no longer wanted. The local overlay stays excluded and is not deleted.

## Engine home

The engine keeps task state and its own config outside every repository, because a workspace and its `.git` can be written by the agents working in it. The engine home is `~/Library/Application Support/Sakura/engine`, with `~` read from the account database, not from `$HOME`:

- `config.toml`: the personal flow config.
- `<project>/config.toml`: per-project flow config. `<project>` is the main worktree's name plus 8 hex digits of the SHA-256 of its absolute path, the same from every linked worktree.
- `<project>/flow/<task>/`: a task's state, spec, contract, plan, ledger, runs and evidence; `<project>/flow/calibration.json`.
- `<project>/kit/`: test stamps, guard fingerprints, trusted hook hashes and role sessions.

`SAKURA_ENGINE_HOME` moves the config and task state to another folder, which must exist, belong to you and have mode 0700; anything else is refused. It also moves the records that decide nothing about trust: the cockpit's decided asks, action log and access log and the lane events, which go to `<engine home>/state/`. It never moves the trust records in `<project>/kit/`, nor the config that Git hooks and `sakura test` read, nor the cockpit's proof secret and used tokens, nor the gate marks, hook marks and accepted digests: an agent can set a variable on its own command, a `git commit` included, so those always come from the account's own folders. To run the engine without touching them (a proof run, a test), start it with its own `HOME` and the test interpreter of `tests/fixtures/trust_root_python`, which names a throwaway trust root; where a Claude login is needed, point `CLAUDE_CONFIG_DIR` at the real Claude folder. The heavy-work slots in `~/.cache/sakura/slots` are one set of locks for the whole machine and are shared.

Engine roles may write only inside their own task folder; the rest of the engine home is operator state. `sakura flow migrate-state [--repo PATH]` copies the reports and logs of agentkit-era tasks (`ledger.md`, `report.md`, `local-report.md`, `runs/**/*.log` under `<git common dir>/flow`) into `<project>/migrated/`, an archive the engine never reads, and prints the whole plan before copying. Nothing that could carry authority is migrated: task state, contracts, plans, rulings, evidence and calibration stay behind (a task starts again from nothing), and so do the trust records of `<git common dir>/agentkit` (re-create them with `sakura test`, `sakura trust` and `sakura trust-codex`) and the old host folders `~/.config/agentkit` and `~/.local/state/agentkit` (copy what you still want by hand). Symlinks, non-regular files and non-text files are refused, and a file changed after the plan (its size or digest) is not copied. It never deletes or overwrites; a file already present is reported as kept, so a second run copies nothing.

## Flow engine

`sakura flow` drives a task through spec approval, contract, plan, build, evaluation, a verified draft pull request, a comprehension report, and cleanup after merge. The task directory and ledger remain available after cleanup.

```sh
sakura flow start specs/change.md
sakura flow approve change spec
sakura flow resume change
sakura flow status change
sakura flow sessions change
sakura flow report change
sakura flow cleanup change
sakura flow stop change
sakura flow steer change "Keep the public API compatible"
```

`start` takes the task as a file, or as its text (`sakura flow start -m "Add a --version option"`, or the text as the argument); it prints whether it took a file or the text, and starting the same text again resumes the same task. Every waiting line names the file to read and the exact `approve` command. `sessions` lists every role run with its host, state and the command that reopens it interactively within the role's original limits; `--json` adds, per run, the account it launched under, what it is doing now, how a message reaches it and the run it continues. `sakura flow feed <task> [--since CURSOR] --json` prints one page of what the task's roles wrote and which tools they called, as JSON with a cursor that continues where the page ended; with `--run <run>` it prints one agent's conversation instead, including the text it is writing now. `sakura flow steer <task> --run <run> <text> --json` writes to one agent. These three are described in [docs/agent-conversations.md](docs/agent-conversations.md). Use `--new` to start another task from the same input. Use `--after <task>` to stack a task on another task branch. `resume` continues from saved state. When a task reaches `report_ready`, merge its draft pull request on GitHub, then run `resume` or `cleanup`; cleanup checks the pull request state before removing worktrees, local build branches and the task's own repository.

With tmux, `start`, `resume` and `approve --continue` accept `--detach`: the engine runs in an `engine` window of the task's session and the command returns at once. `sakura flow attach <task>` rebuilds the session (ledger window, a live window per unfinished run) and switches to it. `sakura flow open <task> <run>` reopens a finished run's conversation in a window of that session, in its original working directory and with its original limits; it refuses a run that is still active or whose Claude transcript is gone.

Flow settings live in `<engine home>/config.toml` (personal), `.sakura/flow.toml` (shared), `<engine home>/<project>/config.toml` (per project, host only) and the untracked `.sakura/local.toml`, merged in that order after defaults. Shared and local files use a `[flow]` table:

```toml
[flow]
preset = "standard"
dial = "balanced"
familiarity = "familiar"
approve_spec = true

[flow.roles]
planner = "claude:opus"
plan_reviewer = "claude:sonnet"
builder = "claude:sonnet"
code_reviewer = "claude:sonnet"
evaluator = "claude:opus"
reporter = "claude:sonnet"

[flow.builders]
min = 1
max = 4

[flow.tests]
focused = ["python3 -m pytest -q tests/test_change.py"]
full = ["python3 -m pytest -q"]
e2e = []

[flow.app]
start = ""
stop = ""
ready_url = "" # Must use the app's origin (http://127.0.0.1:$SAKURA_FLOW_PORT/...); only the path is free
ready_seconds = 60
origin = "" # Host files only: an explicit HTTP loopback origin, for example http://localhost:4200; the port must be free on 127.0.0.1 and ::1

[flow.contract]
required_ids = [] # Optional row ids that must be present before base proof starts
flow_paths = ["tests/**", "test/**", "e2e/**"] # Host files only: where the planner's contract flows may be added

[flow.workspace]
root = "~/worktrees/{repo}"
branch = "feat/flow-{task}"
setup = []

[flow.timeouts]
idle_seconds = 600
build = 5400
evaluate = 2700
```

The main config groups are `roles`, `builders`, `spec`, `app`, `tests`, `workspace`, `brakes`, `notify`, `report`, `limits`, and `timeouts`. `contract.flow_paths` bounds the new files the planner may add as contract flows, which the engine commits on the base before the build and which ship with the change: every pattern starts with a plain folder name and matches exactly, only your host config may change it (a repository layer that sets it is refused; `brakes.test_patterns` does not widen it), and whatever it says a flow may never be a CI or automation file, Git metadata, the engine's `.sakura` folder or a package, build or test-runner file, at any depth and ignoring case, nor collide, ignoring case, with a tracked path. The code reviewer reviews the flows commit itself, exactly those bytes against the base, before it enters the engine's head (with code review on in the preset), and every later code reviewer is shown it as part of the change. The engine reads at most 200 flow files, 1 MiB each and 8 MiB in all. Command flags can override `preset`, `dial`, `familiarity`, roles, and individual settings for one task. Weakening a setting requires naming it in `relax`; brakes cannot be relaxed.

`workspace.root` defaults to `{engine}-workspaces/{repo}`: a folder next to the engine home, not inside it, because an agent's cage denies the whole engine home and a workspace in it would be unreachable. Set `workspace.root` in the host config to keep worktrees elsewhere. Every workspace of a task (its lanes, workspace 0 where they are merged, the evaluator's base) is a worktree of the task's own bare repository, `<workspace root>/<task>.git`, which the engine makes when the task starts: the base commit and its history copied from your repository (no other branch), and a config the engine writes (the object format and your `user.name` and `user.email`, no remote and no credential). Your repository's Git folder, its local config included, is never inside an agent's cage; the task branch reaches you through the remote it is pushed to, with the URL and the credential settings of your repository's config, and cleanup removes the task's repository.

The `notify` group is accepted only from personal or untracked local config. Set `ntfy_keychain` to the macOS Keychain item name and `ntfy_url` to an HTTPS endpoint, or a loopback HTTP endpoint. The engine reads the secret from Keychain when sending a notification. `limits.claude_max_budget_usd` sets the per-run Claude budget.

The `gates` group, also personal-only, sets the day and night policy. By day (`gates.day`, local `HH:MM-HH:MM`, default `08:00-23:00`, may wrap midnight) escalations, failures, red gates, `ready_for_review` and `awaiting_approval` page through ntfy. By night nothing pages: each would-be page is stored as one file under `~/.local/state/sakura/flow/deferred/` and ledgered as `notify deferred (night)`. `gates.mode` is `auto` (use the window), `day` or `night`; `sakura flow mode [day|night|auto]` overrides it machine-wide and prints the effective mode.

Every flow command run inside a repository records it in `repos.json` in the engine home (not from an engine role session), which is where the cockpit looks for tasks. A fresh `SAKURA_ENGINE_HOME` starts with an empty registry; the default engine home adopts, once, the entries that still exist of the older `~/.local/state/sakura/flow/repos.json` and leaves that file as it is. `sakura flow register <path>` adds a repository without running a task there.

The `surface` group controls the tmux view of a task. `surface.tmux` (default `true`) turns it on; the engine's sessions live on its own tmux server (`tmux -L sakura`, started without reading your tmux.conf so a session-restoring plugin cannot start your own agents in it), never on your default server, so look at them with `sakura flow attach <task>` or `tmux -L sakura ls`; without tmux on `PATH` the flow runs headless and ledgers one `surface unavailable` line. `surface.session_prefix` (default `flow-`) prefixes the name of each task session and `surface.cockpit_session` (default `flow`) is the name of the tmux session `sakura cockpit --open` creates, with the cockpit server in its `cockpit` window; both names accept 1 to 32 characters of `A-Z a-z 0-9 . _ -`.

The `report` group sets where the owner report goes. Every flow ends with a second report written in plain language for a non-technical reader, next to the English `report.md`. Two preset knobs, beside `reviewer` and `evaluator`, decide whether that happens and in which language: `reporter` (default `true` in every built-in preset) runs the reporter role, and `report_language` picks the owner report's template, `it` (the default, Italian headings) or `en`. Set them in a preset file or in the `[knobs]` table of a host config file; turning the reporter off is a relaxation, as for the reviewer and the evaluator, and a repository layer may set neither. With `reporter = false` no reporter runs: `report.md` keeps only the engine-computed section, no owner report is written, and the task still ends `report_ready`. The old `report.language` key is refused with a message naming `knobs.report_language`. The reporter writes only the body into the task store; the engine composes the front matter (`title`, `description`, `project`, `kind`, `task`, `agent`, `date`, `links`) and writes `<task>.md` into `report.owner_dir` atomically, replacing the file of an earlier run of the same task. `owner_dir` defaults to `{engine}/reports/{repo}` (`{engine}` is the engine home, `{repo}` the repository name; a leading `~` is the HOME the process sees) and must be absolute after expansion; the directory is created when missing. `owner_dir` is personal-only, like `notify.*` and `gates.*`: it may be set in `~/.config/sakura/flow.toml` (or with `--set report.owner_dir=...` on the command line), and a repository's shared or local flow file that sets it is a configuration error, because the file would otherwise let a cloned repository write agent-authored Markdown into the person's agent config directories. To give one repository its own directory, add a table to the personal file: `[report.owner_dirs]` with the repository name as key and an absolute directory as value (for example `myrepo = "~/notes/myrepo/report"`); an entry wins over `owner_dir` for that repository. `report.project` defaults to the repository name. The report phase is `blocked`, and the status stays short of `report_ready`, when the text is invalid after the last attempt (missing sections, more than three manual steps, something that looks like a credential) or the directory cannot be written.

Every flow also keeps a live status page, written by the engine and never by an agent, so it costs no agent run. `<task>.md` in `report.status_dir` (default `{owner_dir}/stato`, so it follows the repository's effective owner directory; absolute after expansion, personal-only like `owner_dir`) is rewritten after every state change of the engine: the board of build tasks by state (in corso, in attesa dell'owner, bloccato, da fare, fatto, with counts), the last eight updates, the current phase with what the owner must do at a gate, one plain line per task, the arbiter rulings, and a link to the owner report once it exists. The planner may give each build task an `owner_line` in `plan.json` (one plain Italian line); without it the page shows the task title. A write is skipped when only the `updated` stamp would change. The page never blocks a flow: a failed write is ledgered once as `status_page_failed` and the flow continues.

Presets control optional review stages:

| Preset | Plan review | Code review | Evaluator |
|---|---|---|---|
| `vanilla` | Off | Off | Off |
| `standard` | On | On | Off |
| `verified` | On | On | On |

The resource dial controls model effort, maximum parallel builders, and fix rounds:

| Dial | Claude model | Effort | Builders | Fix rounds |
|---|---|---|---:|---:|
| `save` | Haiku | Low | 1 | 1 |
| `lean` | Sonnet | Low | 2 | 2 |
| `balanced` | Sonnet | Medium | 3 | 2 |
| `strong` | Opus | High | 4 | 3 |
| `power` | Opus | Extra high | 6 | 4 |

The Claude model of the dial applies to a Claude role the budget router does not place (see "Task classes and model routing" under Usage). A role the router places runs on the cheaper of the dial's model and its route's model, so `save` keeps integration builders on Haiku and `strong` and `power` do not lift builders, reviewers or the reporter to Opus: only the planner and the evaluator default to Opus. A role the owner sets explicitly is never rerouted.

For every Codex role, reviewers included, the owner must run `sakura trust-codex --record` after the engine is installed and ready, before enabling those roles: Codex skips the engine's inline hooks unless their trust is bypassed, and the engine only bypasses it once the workspace hooks are exactly its own. The command records trust for the current repository hooks. The commit brake is a Git hook, so it also applies when Codex attempts a commit.

Unattended roles load no personal settings, plugins, MCP servers, skills, hooks or exec rules (Codex runs with `--ignore-user-config --ignore-rules`); the person's instruction files (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`) still load, by acceptance: they are the person's standards and carry no tools. Because the Codex config is ignored, the engine hooks are passed on the command line (`-c hooks.PreToolUse=...`, `-c hooks.PostToolUse=...`), not read from Codex's project config; Codex only runs them with `--dangerously-bypass-hook-trust`, which the engine adds after its own trust check. A Claude or Codex run of any role (reviewers included) that used any tool without a single engine hook call since it started, on any attempt, is blocked with the ledger line `<provider> hooks did not run`. The proof is a mark the hook writes under `~/.local/state/sakura/flow-hook-marks/`, which no role may write. Claude roles load no settings source at all (`--setting-sources ""`: no user, project or local settings, because hooks merge across sources and a tracked project hook would otherwise run unmediated next to the engine's) and receive the engine hooks inline (`--settings` with `disableAllHooks` set to false), so a repository file such as a tracked `.claude/settings.json` can neither disable the engine brakes nor add hooks, and no role depends on `.claude/settings.local.json`. No role can run `tmux`: roles, repository commands and the lane setup, down and status commands get no `TMUX` or `TMUX_PANE`, a private empty `TMUX_TMPDIR`, and the hook brake denies a `tmux` program, a glob that can expand to it, and the word `tmux` (socket paths included) in any command that is not only plain readers such as `echo` or `grep`.

### Operator commands while a flow runs

`sakura flow steer <task> "<text>"`, `sakura flow dial <task> <step>` and `sakura flow role <task> <role> <provider:model>` never touch a running call. Each one is validated, written to the task's mailbox, and applied by the driver at the next safe boundary (between calls); the running call keeps its settings. Every command carries an actor (`--actor`, default the local user) that the ledger records. Mailbox entries are authenticated like `state.json`: a file written into the mailbox by hand is refused. `sakura flow approve <task> <gate> --continue` approves a gate and keeps driving the flow (with `--detach`, in the task's tmux session). Engine roles cannot run any operator command: the CLI refuses them inside a role session and the hook brakes deny them.

### Manual lanes (shelved)

`sakura lane` is shelved: it prints one line and exits 2, the cockpit no longer lists these lanes, and a lane is a flow task now (`sakura flow start`). The module and its tests stay in `lib/sakura/lanes/` for reference. What follows describes the shelved command.

A lane is a worktree with its own tmux window, an interactive agent and, optionally, an application stack, for work the owner drives by hand next to the engine. Lane groups live in `~/.config/sakura/lanes.toml`, one table per group with `repo`, `session`, `worktree_root`, `branch` (with `{name}`), `base`, `setup` (commands run once in a new worktree), `up`, `down`, `status` (exit 0 means the stack is up) and `agent` (the command started in the lane window).

```sh
sakura lane open fix-login --group work            # worktree, setup, stack up, agent window
sakura lane status --group work                    # lanes, stack, branch, last commit, agent state
sakura lane say fix-login --group work "run the unit tests"
sakura lane model fix-login --group work sonnet
sakura lane done fix-login --group work --remove    # stop the stack, close the window, remove the worktree
```

`lane open` holds a per-group lock from lookup to window creation, so a double request opens one window, and reruns setup until it completes once. `lane done` checks for uncommitted changes, unpushed commits and, with `--remove`, a branch not merged into `base` before it stops or closes anything; a refusal changes nothing. Text or a model sent to a lane agent that is working is refused.

### Usage

`sakura usage` (or `--json`) prints this week's consumption per provider from local logs only: Claude and Codex utilization of the weekly limit, the projection to the reset, and the tokens per flow role and lane. A figure the logs cannot give is printed as `not available`.

#### Usage archive

`sakura usage archive` keeps a local SQLite archive (`~/.local/state/sakura/usage.db`, private to the user) of every assistant turn in the Claude and Codex transcripts, plus the facts of recent pull requests from `gh`. It reads each transcript incrementally (only whole new lines) and is safe to run on a schedule; a second concurrent run exits with `usage archive already running`. Per turn it stores only token counts, ids, model, time, working directory and branch. Message text, prompts and tool input or output are never stored. If `gh` is missing or logged out, transcripts are still archived and the pull request step is skipped with one line on stderr.

`sakura usage report --by-pr [--repo OWNER/NAME] [--days N]` joins turns to pull requests by branch name and prints tokens, cost where known, and the size of each pull request.

#### Plan quota snapshots and the estimate

Each `sakura usage archive` run also takes one quota snapshot per Claude source: a single minimal headless `claude -p` request (no tools, no session saved, no settings sources, a throwaway working directory, `CLAUDE_CONFIG_DIR` set to the account's config dir, never `ANTHROPIC_API_KEY`) whose `rate_limit_event` carries the five-hour and weekly utilization and reset times. Claude Code reads its own credentials; sakura never does. From `claude auth status --json` only `subscriptionType` is kept, so no email or organisation is stored. A failed probe stores nothing and prints one line on stderr. Each source's config dir defaults to the parent of its `root` (or set `config_dir`). Transcript lines for "limit reached" are archived too (window and reset time only).

`sakura usage quota [--json]` prints the latest snapshot per account, the plan periods, the calibration (weighted tokens per 1% of the week, per account, plan and model class, with the intervals it rests on), the rises that had no local tokens (use from another device), and the limit events with the weighted tokens used in their window (a cap estimate). `sakura usage estimate --account NAME --tokens opus=1000,sonnet=500 [--json]` prints `about N.N% of the week (basis)`; the tokens are weighted (input 1, cache write 2, cache read 0.1, output 5). Right after a plan switch with no calibration yet, the previous plan's ratio is scaled by `plan_weights` and the basis says so. `sakura usage estimate --plan FILE.md [--account NAME] [--json]` reads a Markdown plan (`### Task` sections, their `Files:` line and an optional `Class:` line) and prints the flow's cost as a share of the week and the 5-hour window with a band, then whether it fits above the reserve. A flow computes the same line (for `budget.account`, or, when it is empty, the usage.toml Claude source whose config directory is the engine's `CLAUDE_CONFIG_DIR`, else `~/.claude`) when its plan is accepted, shows it on its status page, and waits at the `budget` gate (`sakura flow approve <task> budget`, which belongs to that plan) when the band passes `budget.ask_flow_week_pct`, does not fit above `budget.reserve_week_pct`, or the quota is unknown (no snapshot, or one older than `budget.snapshot_max_age_minutes`); above `budget.warn_flow_week_pct` it only notifies. Without `~/.config/sakura/usage.toml` no usage archive is configured: the budget is off (one ledger line, `state.budget.preflight` marked `off`, no ask), and `usage estimate --plan` says so and exits 1; an explicit `budget.account` without that file asks.

Live budget guard: before each Claude or Codex role run and at each phase boundary (not `pr`, not `cleanup`) a flow reads the newest quota snapshot of its account, not an estimate. A Claude snapshot older than `budget.snapshot_max_age_minutes`, or none, is unknown and the flow asks instead of running. The week (live figure, plus the holds of the other flows, plus what the flow itself still has to spend) at `100 - budget.reserve_week_pct` pauses it until the weekly reset; the 5-hour window at `100 - budget.reserve_five_hour_pct` defers it to that window's reset; a flow started with `sakura flow start --when-cheap` also defers to the `budget.night` window when the trend of the snapshots projects the week past the reserve before its reset. A window whose reset has passed counts as 0%.

A started flow holds its preflight `high` in the `budget_holds` table from the start of build until its report is ready or it is cleaned up, so two flows starting together see each other. The hold records the live week figure it started from: the flow's own pending share is its `high` less what the week gained since, so its spend is not counted twice, and the share of its evaluator and reporter stays counted until the report phase ends, so those phases are checked against the reserve too. A weekly pause gives the hold back, and a hold from before the last weekly reset starts afresh. Without `~/.config/sakura/usage.toml` the guard is off.

A stopped flow keeps its whole state (`waiting: budget` with `until`, status awaiting approval at the `budget-wait` gate), writes a `Ruling:` ledger line with the numbers and an event, and resumes by itself or on `sakura flow approve <task> budget-wait`. The `budget` gate is the preflight's and `budget-wait` the guard's; approving one never approves the other. An approval lifts only the rule that stopped the flow, until that wait would have ended: a pause the weekly reserve until that reset, a defer the 5-hour reserve (or the night pacing) until its reset, an ask the age limit of the snapshot for `budget.snapshot_max_age_minutes`. Every other rule keeps applying, so a released flow can still stop on a different rule, and an ask for a quota with no snapshot or no weekly figure at all cannot be released that way.

`sakura flow wake`, run after the quota probes of every `sakura usage archive`, re-runs the guard of each due wait in every registered repository and resumes the ones that may run, detached. It only resumes flows when a tmux server is already running (otherwise it prints `wake: no tmux server, N flows due` and touches nothing), because a server started from the background archive job would inherit that job's priority and environment, and it ledgers which checkout (`bin/sakura`) resumed each flow.

Codex has no probe. The archive run reads the plan rate limits Codex writes into its rollout files and stores them as snapshots of the account `codex:<name>`, kept apart from Claude calibration and from the weekly report. A Codex role is guarded by the latest such snapshot read as a lower bound at any age (inside a window the percentage only rises), and never asks; a flow whose Codex source cannot be matched to its budget account runs its Codex roles unguarded and says so once in the ledger.

Freshness: the default `budget.snapshot_max_age_minutes = 30` sits against a 15-minute archive interval. One skipped or failed archive run leaves every flow near the edge of asking, so raise the limit if the probe or the schedule is unreliable.

The Claude Code tier (5x, 20x) is not exposed, so plan periods come from the config:

```toml
claude_bin = "claude"          # path of the claude CLI, if it is not on PATH

[[plans]]
account = "personal"
plan = "max20x"
since = "2026-09-01"           # ISO date or datetime, UTC
estimated = false              # true when the start date is a guess

[plan_weights]                 # relative weekly capacity, used only to scale across a plan switch
max20x = 20
max5x = 5
pro = 1
```

Optional config in `~/.config/sakura/usage.toml` (all keys optional; without it, Claude `~/.claude/projects` and Codex `$CODEX_HOME/sessions` or `~/.codex/sessions` are scanned):

```toml
db = "~/.local/state/sakura/usage.db"
pr_repos = ["owner/name"]
pr_days = 60

[[sources]]
provider = "claude"   # or "codex"
account = "personal"
root = "~/.claude/projects"
```

#### Task classes and model routing

The planner may give each build task a `task_class` in `plan.json`: `mechanical`, `integration` (the default, and the class of a plan written before classes existed) or `design`; a Markdown plan read by `usage estimate --plan` names it on a `Class:` line. The `[budget.routes]` table (`[flow.budget.routes]` in a shared or local file) maps a class, and the roles `reviewer`, `reporter` and `intake` (the intake phase of the planner), to `haiku` or `sonnet`, optionally followed by `:low|medium|high|xhigh` for the effort:

```toml
[budget.routes]
mechanical = "haiku"
integration = "sonnet"
design = "sonnet:high"
reviewer = "sonnet"
reporter = "sonnet"
intake = "haiku"
```

Only a Claude role left at its default is routed; a role set in any config layer, a flag or the cockpit keeps its model, and the planner and the evaluator keep theirs. Opus is never a table value: it is reached only by escalation. Every two failed review rounds of a build task move its model one step up (Haiku, Sonnet, Opus), and reaching Opus waits at `sakura flow approve <task> opus:<build task>`, whose reason carries the extra cost of an Opus run as a share of the week. The approval belongs to the plan it was given for, and a task whose plan changed before it was merged starts with no failures, so a task that reuses an id after a re-plan inherits neither. An Opus model for any role other than the planner and the evaluator, set in the shared or local repository layer, is a relaxation that needs `relax = ["roles.<role>"]`; the personal layer and flags are the person's own choice.

#### Budget settings

The `[budget]` group (personal, shared or local flow file; read as `[flow.budget]` in the shared and local files) has these keys, all optional:

| Key | Default | Meaning |
|---|---|---|
| `account` | empty | usage.toml Claude source the flow draws on; empty is the source whose config directory is the engine's `CLAUDE_CONFIG_DIR` |
| `reserve_week_pct` | 25 | share of the week no flow may consume |
| `reserve_five_hour_pct` | 30 | share of the 5-hour window no flow may consume |
| `warn_flow_week_pct` | 5 | a flow whose high estimate passes this only notifies |
| `ask_flow_week_pct` | 10 | a flow whose high estimate passes this waits at the `budget` gate |
| `outside_share_alert` | 0.4 | share of a day's rise not explained by local use that opens an `outside` alert |
| `night` | `23:00-07:00` | local window a `--when-cheap` flow waits for |
| `snapshot_max_age_minutes` | 30 | age past which a snapshot is unknown |

The personal layer may set any of them. A shared or local layer may only tighten the thresholds: raising a reserve is free, lowering one (or raising `warn_flow_week_pct`, `ask_flow_week_pct`, `outside_share_alert` or `snapshot_max_age_minutes`) is a relaxation that must be listed in that layer's `relax`, like every other relaxation. Without `usage.toml` the budget is off (no preflight ask, no guard, no alerts); an explicit `account` without it asks.

#### Early warnings and the weekly report

Each archive run evaluates the account alerts and stores them in the `budget_alerts` table of the archive (`id`, `account`, `kind`, `title`, `reason`, `since`, `resolved_at`, `url`, `pushed_at`): `pace` (the weekly percentage projected at the reset from the snapshot trend passes the reserve, so use outside Claude Code counts), `outside` (a day whose weekly rise local turns do not explain by more than `outside_share_alert`) and `overage` (the latest snapshot reports extra usage in use); the Codex account of the flow gets `pace` and `overage` too. A flow that asks or waits opens a `request` alert, closed when it is released, resumed or finished. Account alerts are pushed through the personal `notify` settings (ntfy by day, a deferred event by night); a push that did not go out is retried on the next run while the alert is open (`pushed_at` is empty until it does). Every percentage carries its basis and, where one exists, a band. The preflight warning is notified as `budget_warning`.

`sakura usage weekly [--week YYYY-Www] [--account NAME] [--json]` shows where the quota of one weekly window went: by project, role and model class, the share used outside Claude Code, and the three most expensive tasks with what a cheaper model class would have cost. A weekly window is the account's own quota window (its reset minus seven days), and `--week` selects the window that starts in that ISO week. After a window ends, the archive run writes its page once, `consumi-<account>-<year>-W<nn>.md`, into the owner report directory of the repository `sakura`.

### Where to watch and act

The notes app is the control center: every flow writes its live status page and its owner report there (see the `report` group above). Actions live in a dashboard, which uses the cockpit's JSON API and command line (`docs/cockpit-api.md`): `sakura cockpit snapshot --json` and `sakura cockpit act <key> <target> --json`, with a two-step confirmation for approve, stop and close lane. The cockpit's own HTML pages (`sakura cockpit --open`, served on the overlay only to the person's allow-listed login and devices) are a fallback for an emergency. On the cockpit's own machine the HTTP face is read-only, because an engine process there could otherwise pose as the person's browser; actions on that machine go through the command line, where engine roles are refused.

The phone page is the cockpit's pages on your overlay, for your own devices only. Set it up from the app: install Tailscale on the Mac and the phone, sign in to both with the same account, then allow your phone from the list of your devices in the app's phone page settings.
The same by hand: `sakura cockpit phone --json` shows the state, the link and your devices; `sakura cockpit phone allow <device id>`, `remove <device id>` and `port <number>` change `~/.config/sakura/cockpit.toml` (written for you alone, mode 0600).
Then `sakura cockpit --open` serves the page on port 8787 unless you set another; a change of the allowed devices applies at once, and a new port makes the running server bind again.

Before a real pilot, run the standard contract and the real-CLI smoke rows R35, R36 and R54 with `python3 acceptance/run_contract.py --smoke --rows R35,R36,R54`. These rows use real Claude and Codex credentials and can incur model charges.

The acceptance harness and the canaries refuse to run unless `SAKURA_ENGINE_HOME` names a fresh private folder (mode 0700) that is not the default engine home.

Flow uses local Git worktrees, `gh` for pull requests, and the configured role CLIs. It does not merge pull requests, mark them ready, automatically rebase stacked tasks, or watch for merges in the background. A completed report is available with `sakura flow report <task>`; cleanup runs only after GitHub reports the pull request merged. Operators can stop or steer a task and resume it later.

## Known limits

The kit is intentionally local and does not replace server-side branch protection, required CI, required status checks, or review rules on the remote host. A user with shell access can forge local state, including test stamps and approvals. It starts no background service of its own. The one network service is `sakura cockpit serve`, which the owner starts on purpose (`sakura cockpit --open` does it in a tmux window): it binds only to the machine's overlay address (never a wildcard or loopback address) and answers only requests that pass its access gate, which checks the overlay identity against the person's allow-lists, the `Host` and `Origin` headers and a CSRF token. See `docs/cockpit-api.md`.

An agent's command, a check or a test run counts as finished only when it exited, its output ended and the engine found nothing left of its process group or carrying its run's nonce: the command and every descendant the engine can trace have ended. A child that deliberately detaches (a new session, an empty environment, its output closed) can outlive the command, still inside the cage. It cannot change what is reviewed, proved, delivered or reported, with one open exception: on a project whose host config fixes `app.origin`, every application session of a task listens on that one port, so a survivor of an earlier session of the same task, which was granted it, can answer proof requests for the instant the live application's socket is handed over, and a receipt then names code that did not serve (review 17, finding 3; whether to refuse that reuse or accept the risk is the person's decision, see the plan): once a role's step is done, nothing it could write feeds a later step. The engine merges the exact commits the reviewers approved into its own head, in its own sealed store, and tests, receipts, pushes and reports only that head; a reviewer reads a sealed copy of the change in a repository of its own; the reporter works in a repository of its own that holds only copies of the sealed inputs; the engine's helpers outside the cage (the evaluator's row tool, the agent hooks, the install step) never take their repository from the workspace; the contract's base run and the evaluator's calibration run their application and rows from a read-only checkout of the sealed base plus the flow bytes the engine read, the commit it then makes the first step of its head; every application starts on a port no earlier session of the task held and counts only while every listener on that port is in its own process group, checked again around every row; every writable folder a later step reuses is pinned from creation and refused once replaced; and the engine writes its hook files into a checkout only through folders it opened without following a link. A commit it adds or a branch it moves is ignored. It can change the live workspaces and the task's repository (what a role working there later sees, not what is proved), the files the cage lets it write and the local ports it was given, which no later application uses. This is an accepted risk of running agents as your own macOS user; running each task as a dedicated macOS user (or in a VM) is the fix, not built yet.

Agent hook snapshots are latched fail-closed after a guarded-file change, but hook runtimes do not offer an execution-wide mutex. Parallel tool calls can still race a snapshot check, so server-side controls remain the authority for irreversible actions.

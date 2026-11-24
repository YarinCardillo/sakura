# Codex and Claude run every role

No role belongs to a provider. Each step of a lane (plan, plan review, build, task review, final review, checks, report) runs on the account, model and effort the person chose for it, on Claude or on Codex, and the preset only fills those in until they do. A preset (a built-in one too) names a model per tier, like `claude:sonnet` or `codex:gpt-6.2-sol`; nothing else in the engine prefers a provider.

When the accounts hold none of the provider a role is set to (the person cancelled one), the role runs on the first account that exists, with that provider's own default model, and says so: the ledger line is `No Claude account exists, so the builder runs on the Codex account (Codex).` and `engine rules --json` shows it as the step's `agent.note`. A step the person pinned to an account keeps it, and a conversation continues on its own account.

## How a Codex role is caged

Every Codex role starts `codex exec` with approvals never, the engine's own hooks passed inline (`--ignore-user-config --ignore-rules`, hook trust recorded with `sakura trust-codex --record`) and a permission profile made from the same lists as the Claude cage (`flow/cage.py`, `codex_args`):

| What | Reviewers (`plan_reviewer`, `code_reviewer`) | Every other role |
|-|-|-|
| Profile | `sakura-review`, read-only | `sakura-work`, workspace-write |
| Reads | the minimal system paths, the workspace and its Git folder, the engine install, the run's tmp folder, the toolchains | the same |
| Writes | none | the workspace, its Git folder, the run's tmp folder; a row run: its own build folder and tmp only |
| Read-only inside the writable Git folder | | `hooks`, `config`, `config.worktree`, the project record, the alternates files, the object pins |
| Denied over everything | secrets, the engine homes, other users, system folders with credentials | the same |
| Network | none | the Claude cage's hosts, through Codex's proxy |
| Engine tools | `board_read`, `board_post`, `ask_person`, `knowledge_propose` through the engine's tool server (`-c mcp_servers.sakura`), as the host's grants say | the same, and `run_row` for the evaluator |
| Hooks and records | the same hook dispatcher as Claude, the hook-ran check, run logs, run records, ledger | the same |

## The digest summarizer

The morning digest's model is a role value too: `model = "claude:haiku"`, `"codex:gpt-6.2-sol"`, `"codex"`, or a bare name, which is a Claude model as it always was; `account` is a Leeway account id of either program. A Codex summarizer runs on the `sakura-work` profile narrowed to its job: it reads its input folder, writes its output folder and tmp only, has no network, no tool server and no hooks (its folder is no repository, so no hook trust is needed). Claude's `--tools Read,Write` becomes Codex's shell tool turned off (`features.shell_tool=false`, measured: its file edits still work), so the summarizer has no shell, which matters because what it reads is untrusted.

## What Codex cannot give, and what stands in

1. **Per-tool decisions.** Claude Code sends every tool call outside its allow rules to the engine's permission tool. Codex has no such channel: with approvals never, whatever its profile does not allow fails. The profile and the hooks carry the decision.
2. **Denied programs and Apple Events.** The Claude cage denies `osascript`, `open`, `launchctl`, `security`, `kill`, `ps`, the pasteboard and `git push` in its Seatbelt profile and by rule. Codex's profile does not (measured with 0.171: they start). A Codex role's hook refuses a command that starts one of them (`flow/denied_commands.py`). That is a check of the words of the command: a program started through an expansion or a script the role wrote is not seen. This guarantee is weaker for Codex.
3. **Loopback ports.** The Claude cage opens the exact ports the engine allocated. Codex can only open all local ports or none, so a Codex role gets none unless the person approved local sockets (`checks.local_sockets`) or the cage policy sets `local_network`. A Codex evaluator proves rows through the `run_row` tool, which the engine runs under its own Seatbelt profile with the exact ports.
4. **Glob rules.** Codex can deny a glob path but not keep it read-only, and `workspace/**/.git` would also hide the workspace's own marker. So the `.git` marker globs and the worktree pointer files (`worktrees/*/gitdir`, `commondir`) are not protected inside the Codex profile. The engine's own checks stand in: it verifies the Git folder of the workspace before every run, trusts only the folders it recorded, and refuses a marker or record it did not name. Only the project record's glob is denied.
5. **Live steering.** A message to a running Claude role is delivered live. A Codex role takes it when its run ends, as a continuation of the same conversation (`codex exec resume`).
6. **Hook fingerprint.** The guard saves a fingerprint of the guarded files (the engine's own install among them) at the first call of a run and refuses every later call when it changed. That is the same for both programs; a lane that runs while the engine's source is edited stops closed.
7. **Model effort and default.** The dial's Claude model applies to Claude roles; a Codex role with no model uses Codex's own default for the account.

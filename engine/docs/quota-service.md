# Quota service

One local service that knows the live quota of every Claude and Codex account on this Mac, forecasts each
weekly window, and says where a piece of work may run now.

## Commands

All commands take `--config PATH` before the subcommand to use another owner rules file.

- `sakura quota status [--json] [--refresh]`: accounts with 5-hour and weekly percentages, reset times and the
  weekly forecast. `*` after a name means the source did not answer and the last snapshot is shown.
- `sakura quota advise --repo PATH --role builder|reviewer|explorer|planner|evaluator --task-class mechanical|integration|design [--agent claude|codex]`:
  JSON `{verdict, agent, model, effort, config_dir, reason, retry_at, account}`. `go`: run it. `switch`: run it,
  but on the returned `config_dir`. `wait`: nothing is usable, retry at `retry_at`. `deny`: nothing is allowed.
- `sakura quota serve [--port N] [--refresh-minutes 10]`: `GET /status`, `POST /refresh` (asks every source now) and
  `POST /advise` with the same fields as the command (`repo`, `role`, `task_class`, `agent`), on 127.0.0.1 only.
  Requests with a non-loopback Host or any Origin header are refused with 403 (DNS rebinding and cross-site calls).

## Rules

- Ceiling of an account: 100 minus its `reserve_week_pct` in `~/.config/sakura/accounts.toml`, 25 when it sets none.
  A candidate is usable while its forecast after one more round of work is at or under the ceiling and its
  5-hour window is under 95%.
- Which accounts serve which repository, in order of preference, is `~/.config/sakura/quota.toml`
  (see `docs/examples/quota.toml`). Without the file every account serves every repository.
- Builders, reviewers and explorers get the cheapest fitting model (`[budget.routes]`, never Opus); planner and
  evaluator get the strongest. Without `--agent`, builders and reviewers try Codex first.
- Sources: a `claude -p` probe per Claude config directory (one Haiku request), the Codex rollout logs under the Codex home for Codex
  (`codex:<id>` snapshots in the archive). Two directories that report the same weekly window are one account. Readings and
  history are stored in the usage archive. `source` is `probe`, `codex-log` or `codexbar` (fallback only).

## LaunchAgent (not loaded by the repository)

```sh
sed -e "s#__SAKURA_BIN__#$PWD/bin/sakura#" -e "s#__HOME__#$HOME#g" \
  docs/launchd/sakura-quota.plist.template > ~/Library/LaunchAgents/local.sakura.quota.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.sakura.quota.plist
```

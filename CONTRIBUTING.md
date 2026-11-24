# Contributing

Issues and pull requests are welcome. For anything larger than a fix, open an issue first and say
what you want to change and why: it saves us both a rewrite.

Before you send a pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
python3 -m pytest -q engine/tests
python3 -m unittest discover -s scripts/tests
```

If you changed `service/`, also run `bash service/tests/run.sh`. If you changed the `boxes` build, check it
too: `cargo check --workspace --features sakura-app/boxes`.

The Rust harness runs one test at a time for predictable UI process timing. Tests that exercise
concurrent requests still run their own concurrency scenarios. Two tests in
`engine/tests/test_flow_evaluator_command_seam.py` drive a browser through Playwright and fail on a machine
where Playwright and its browser cache are not set up.

- A fix comes with a test that fails without it. Tests are not changed to make them pass.
- The window talks to the engine only through the `sakura` command: `sakura cockpit snapshot --json`, `flow feed`
  and `flow changes` to read, and `sakura cockpit act` to change. A change that the window needs and the engine does not offer is a new engine
  action first, documented in `engine/docs/cockpit-api.md`, then a control in the window.
- The window does not read credentials. A secret belongs to the engine, which reads it from the Keychain at the
  moment of use.
- Words a person reads are in `crates/sakura-app/locales`, in English and Italian, never in the code. Use one name
  per thing: Inbox, Master, Lanes, Engine, lane, ask, Approve, Decline, Reply, Preset, Pace, Changes, Files,
  Details, Automations, Accounts. Write plain English for developers, with no em dashes, middle dots or italics.
- Keep private things out: no home folder paths, host names, addresses or tokens in code, tests,
  documents or commit messages. Use placeholders (`/home/me`, `example.com`).
- The engine is Python 3.11 or later with the standard library only. The service is written for bash 3.2
  (the one macOS ships) and must also pass on Linux.

By contributing you agree that your work is licensed under AGPL-3.0-only, as the rest.

Security problems go through [SECURITY.md](SECURITY.md), not through a public issue.

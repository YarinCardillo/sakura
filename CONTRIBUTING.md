# Contributing

Issues and pull requests are welcome. For anything larger than a fix, open an issue first and say
what you want to change and why: it saves us both a rewrite.

Before you send a pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
bash service/tests/run.sh
python3 -m unittest discover -s scripts/tests
```

The Rust harness runs one test at a time for predictable UI process timing. Tests that exercise
concurrent requests still run their own concurrency scenarios.

- A fix comes with a test that fails without it. Tests are not changed to make them pass.
- The app runs only the `box` command. It does not run git or Docker itself and does not read
  credentials; a change that needs one of these belongs in the service.
- Words a person reads are in `crates/sakura-app/locales`, in English and Italian, never in the code.
- Keep private things out: no home folder paths, host names, addresses or tokens in code, tests,
  documents or commit messages. Use placeholders (`/home/me`, `example.com`).
- The service is written for bash 3.2 (the one macOS ships) and must also pass on Linux.

By contributing you agree that your work is licensed under AGPL-3.0-only, as the rest.

Security problems go through [SECURITY.md](SECURITY.md), not through a public issue.

lavoro: a task, the Italian word the code uses for one piece of work.

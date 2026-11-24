# Digest collectors

The morning digest reads what your own programs print. A program in any language, or an existing tool, is a source.
These two are examples to copy and edit; Sakura ships no code per service.

The protocol, in five lines:
- Sakura runs the program with the arguments you configured, stdin closed, in a folder of its own, and a timeout.
- It sets `SAKURA_DIGEST_SINCE` (an ISO time: your last good run, or the look-back) and `SAKURA_DIGEST_CURSOR` (what the program printed last time, or empty).
- The program prints one JSON object per line: `{"id": "pr-123-review-4", "title": "Review on #123", "url": "https://...", "at": "2026-11-17T06:12:00Z", "text": "..."}`. Only `id` and `title` are required, `at` needs a time zone, and a line that is not valid is counted and skipped.
- Its last line may be `{"cursor": "..."}`; Sakura keeps it and hands it back on the next run, moving it only after the whole run succeeded.
- A non-zero exit or a timeout is reported in the digest, and the other sources still run. Tokens are your program's business: Sakura never sees them.

The limits: at most 200 items per source, a title of 200 characters, a text of 4000, 1 MB of output.

## Configure

In `digest.toml` in the engine home (mode 0600), with your own repository, pages and ids:

```toml
[[source]]
id = "work"
run = ["~/digest/github.sh", "owner/name", "main"]

[[source]]
id = "channel"
run = ["~/digest/notion.py", "--page", "PAGE_ID", "--database", "DATABASE_ID", "--user-id", "USER_ID",
       "--keychain-item", "notion-digest"]
```

## The examples

- `github.sh` needs `gh` (signed in) and `python3`. It reports pull requests and issues, reviews and comments on your own open pull requests, your assignments, commits and failing checks on one branch, and whether each of your open pull requests is behind its base or conflicts with it. That last check reads what GitHub already computes (`mergeable`, `mergeStateStatus` and the compare API's `behind_by`): no clone, no checkout, nothing written, and a pull request GitHub is still computing is left out of that run.
- `notion.py` needs only Python 3. It reports new blocks on one page used as a channel and the tasks assigned to you that changed. The token comes from the environment variable `--token-env` names (default `NOTION_TOKEN`) or from the Keychain item `--keychain-item` names, when the program runs.

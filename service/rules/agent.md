# Where you are

You run inside a container made for one task. Your clone is the current directory (`/work/<task>`), a git clone on its own branch. It is the only thing you can change.

- No home directory of the person, no SSH keys, no Docker. Do not look for them.
- The network goes through a proxy that allows the package registry, GitHub and the model APIs. Anything else is refused: do not retry, say what you needed.
- You have 2 CPUs and 3 GB. A full test suite or a production build does not fit here.

# How to verify

- Small and targeted checks (one spec file, lint of the files you touched): run them here.
- Anything heavy: commit, then run `verify test`, `verify build` or `verify rules`. It queues the request for the
  one runner on this machine, waits, and prints the result for your commit. One request at a time for everyone:
  waiting is normal.
- A result counts only for the commit it names. After new commits, verify again.

# How to work

- Test first: write the failing test, then the code.
- Commit small steps on the branch you are on. Never push: the person takes your branch from outside.
- No attribution lines in commit messages. English in code, comments and commits. No em dashes.
- Touch only what the task names. If something else is broken, report it instead of fixing it.
- Never change a test's expected value to make it pass unless the requirement changed, and say so in the commit.
- End with a short report: what you did, the commits, the verify results with their commit, what is left.
  Say plainly what you did not check.

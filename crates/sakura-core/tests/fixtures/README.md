# Fixtures

`engine-real-*` (snapshot, rules, `preset list --json`, `cockpit act --help`, and the engine's table of actions with the shapes it
accepts for each input) are the engine's own output, captured by
`make-engine-real.py` against a throwaway engine home, a throwaway Git repository (`orchard`, two lanes, one decided ask),
a throwaway Leeway book, a keeper that resumed a lane, connectors, Scheduled tasks, a digest and two automations.
The script supplies example Leeway readings and connector checks in memory. Run it with a throwaway `HOME` and set
`ENGINE_KIT` and `LEEWAY_HELPER` to the engine checkout and the helper program. The script gives Leeway its own
`--home` and removes the generated helper wrapper after the capture. Regenerate after an engine change, then run the tests. A shape the
app invents or misreads fails `tests/engine_real.rs` and the app's `engine_contract` test. The other `engine-*.json`
files are older, hand-written ones.

The Scheduled captures include manual and event-triggered history, a saved conversation, the resulting Inbox
finding, and problem replies. The connector receipt comes from the public registry mutation that supplies the CLI reply.

`engine-real-feed.json` (a lane that only built) and `engine-real-feed-kinds.json` (a lane with runs, ledger lines and
board posts) are the engine's own `flow feed` output, captured by the same script after the snapshot, in the same
throwaway home: the engine stamps each entry with its time and lists every kind in time order.
`engine-real-snapshot-outside.json` is a second snapshot of the same home, taken after an outside lane (with a titled
link, a row of one step and a question) and a day's morning digest (two news, a check and a decision, each with a titled
link) were added, so it holds a `digest` item with its `reading`, waits with `links` and `digest`, and the one-step row
as the engine's sentence; the first snapshot keeps its two lanes. The build run of the feed fixtures ends with a shell
call, so a tool entry with its `command` is in them.
`engine-real-master-spec.json` and `engine-real-master-decide.json` are the engine's own `master spec` and
`master decide` output for a sample home, with the folder of the engine's own program replaced by a plain one.

`engine-agents.json` (`flow sessions --json` of a lane with a resumed builder, two reviewers and an orphaned run) and
`engine-agent-feed.json` (`flow feed --run --json` of a running, resumed builder with a live tail) are written by hand from
the engine's contract for lane agents, until ones cut from a real run replace them.

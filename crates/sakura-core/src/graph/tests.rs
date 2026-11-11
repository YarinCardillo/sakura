use super::*;
use crate::box_cli::test_support::fake_box;

/// The answer `box` documents, with real-looking ids.
const ANSWER: &str = r#"{ "project": "app", "base": "main", "limit": 100,
  "lanes": [ { "lavoro": null, "branch": "main", "where": null, "head": "b3", "base": null, "readable": true },
             { "lavoro": "m1", "branch": "sakura/m1", "where": "mac", "head": "m1b", "base": "b2", "readable": true },
             { "lavoro": "lost", "branch": "sakura/lost", "where": "box", "head": null, "base": null, "readable": false } ],
  "commits": [ { "sha": "m1b", "parents": ["m1a"], "subject": "Second", "time": "2026-11-10T16:00:00Z",
                 "lavoro": "m1", "branches": ["sakura/m1"] },
               { "sha": "b3", "parents": ["b2"], "subject": "Third on main", "time": "2026-11-10T15:00:00Z",
                 "lavoro": null, "branches": ["main"] },
               { "sha": "m1a", "parents": ["b2"], "subject": "First", "time": "2026-11-10T14:00:00Z",
                 "lavoro": "m1", "branches": [] },
               { "sha": "b2", "parents": ["b1"], "subject": "Second on main", "time": "2026-11-10T13:00:00Z",
                 "lavoro": null, "branches": [] },
               { "sha": "b1", "parents": ["older"], "subject": "First on main", "time": "2026-11-09T13:00:00Z",
                 "lavoro": null, "branches": [] } ] }"#;

fn answer() -> Graph {
    serde_json::from_str(ANSWER).unwrap()
}

fn lane(lavoro: Option<&str>, head: &str) -> Lane {
    Lane {
        lavoro: lavoro.map(str::to_string),
        branch: lavoro.map_or("main".into(), |name| format!("sakura/{name}")),
        place: lavoro.map(|_| "mac".to_string()),
        head: Some(head.to_string()),
        base: None,
        readable: true,
    }
}

fn commit(sha: &str, parents: &[&str], lavoro: Option<&str>) -> Commit {
    Commit {
        sha: sha.into(),
        parents: parents.iter().map(|parent| parent.to_string()).collect(),
        subject: format!("Commit {sha}"),
        time: "2026-11-10T16:00:00Z".into(),
        lavoro: lavoro.map(str::to_string),
        branches: Vec::new(),
    }
}

fn graph(lanes: Vec<Lane>, commits: Vec<Commit>) -> Graph {
    Graph {
        project: "app".into(),
        base: "main".into(),
        lanes,
        commits,
    }
}

/// The sha and the column of every row.
fn placed<'a>(graph: &'a Graph, layout: &Layout) -> Vec<(&'a str, usize)> {
    let place = |row: &Row| (graph.commits[row.commit].sha.as_str(), row.column);
    layout.rows.iter().map(place).collect()
}

#[test]
fn the_answer_is_read_as_box_documents_it() {
    let graph = answer();
    assert_eq!(
        (graph.project.as_str(), graph.base.as_str()),
        ("app", "main")
    );
    assert_eq!(graph.lanes[0].lavoro, None);
    assert_eq!(graph.lanes[1].place.as_deref(), Some("mac"));
    assert_eq!(graph.lanes[1].base.as_deref(), Some("b2"));
    assert!(!graph.lanes[2].readable);
    assert_eq!(graph.lanes[2].head, None);
    assert_eq!(graph.commits[0].lavoro.as_deref(), Some("m1"));
    assert_eq!(graph.commits[0].branches, ["sakura/m1"]);
}

#[test]
fn a_lavoro_runs_in_its_own_column_and_leaves_the_base_at_the_commit_it_started_from() {
    let graph = answer();
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(
        placed(&graph, &layout),
        [("m1b", 1), ("b3", 0), ("m1a", 1), ("b2", 0), ("b1", 0)]
    );
    assert_eq!(layout.columns, 2);
    let strokes: Vec<&[Stroke]> = layout.rows.iter().map(|row| &row.strokes[..]).collect();
    assert_eq!(strokes[0], [Stroke::ToBelow]);
    assert_eq!(strokes[1], [Stroke::Through(1), Stroke::ToBelow]);
    assert_eq!(
        strokes[2],
        [Stroke::Through(0), Stroke::FromAbove, Stroke::ToBelow]
    );
    assert_eq!(
        strokes[3],
        [Stroke::FromAbove, Stroke::ToBelow, Stroke::BendUp(1)]
    );
    assert_eq!(strokes[4], [Stroke::FromAbove], "its parent is not listed");
}

#[test]
fn a_row_names_the_lanes_it_is_the_head_of() {
    let layout = Layout::of(&answer()).expect("a graph that can be drawn");
    assert_eq!(layout.rows[0].heads, [1]);
    assert_eq!(layout.rows[1].heads, [0]);
    assert!(layout.rows[2].heads.is_empty());
}

#[test]
fn a_merge_bends_from_the_base_into_the_lavoro_s_column() {
    let lanes = vec![lane(None, "merge"), lane(Some("m1"), "m1a")];
    let commits = vec![
        commit("merge", &["b1", "m1a"], None),
        commit("m1a", &["b1"], Some("m1")),
        commit("b1", &[], None),
    ];
    let layout = Layout::of(&graph(lanes, commits)).expect("a graph that can be drawn");
    assert_eq!(
        layout.rows[0].strokes,
        [Stroke::ToBelow, Stroke::BendDown(1)]
    );
    assert_eq!(
        layout.rows[1].strokes,
        [Stroke::Through(0), Stroke::FromAbove, Stroke::ToBelow]
    );
    assert_eq!(
        layout.rows[2].strokes,
        [Stroke::FromAbove, Stroke::BendUp(1)]
    );
}

#[test]
fn a_parent_listed_before_its_child_is_still_drawn_under_it() {
    let lanes = vec![lane(None, "b2")];
    let commits = vec![
        commit("b1", &[], None),
        commit("b2", &["b1"], None),
        commit("b0", &[], None),
    ];
    let graph = graph(lanes, commits);
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(placed(&graph, &layout), [("b2", 0), ("b1", 0), ("b0", 0)]);
    assert_eq!(layout.rows[0].strokes, [Stroke::ToBelow]);
    assert!(layout.rows[2].strokes.is_empty(), "no edge to it");
}

#[test]
fn two_lavori_that_never_run_side_by_side_share_a_column() {
    let lanes = vec![
        lane(None, "b3"),
        lane(Some("late"), "l1"),
        lane(Some("early"), "e1"),
        lane(Some("beside"), "s1"),
    ];
    let commits = vec![
        commit("l1", &["b3"], Some("late")),
        commit("b3", &["b2"], None),
        commit("s1", &["b1"], Some("beside")),
        commit("b2", &["b1"], None),
        commit("e1", &["b1"], Some("early")),
        commit("b1", &[], None),
    ];
    let graph = graph(lanes, commits);
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(
        placed(&graph, &layout),
        [
            ("l1", 1),
            ("b3", 0),
            ("s1", 1),
            ("b2", 0),
            ("e1", 2),
            ("b1", 0)
        ]
    );
    assert_eq!(layout.columns, 3, "four lanes in three columns");
}

#[test]
fn a_graph_with_no_commits_has_no_rows() {
    let layout = Layout::of(&graph(vec![lane(None, "x")], Vec::new())).expect("an empty graph");
    assert_eq!(layout, Layout::default());
}

#[test]
fn a_time_is_read_as_seconds_since_the_epoch() {
    assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_time("2026-11-10T16:00:00Z"), Some(1_794_326_400));
    assert_eq!(parse_time("2024-02-29T12:30:15Z"), Some(1_709_209_815));
    for bad in [
        "",
        "2026-11-10",
        "2026-13-10T16:00:00Z",
        "2026-11-10T16:00:00+01:00",
        "x",
    ] {
        assert_eq!(parse_time(bad), None, "{bad:?}");
    }
}

#[test]
fn an_age_is_said_in_the_coarsest_unit_that_fits() {
    let hour = 3600;
    assert_eq!(Age::between(1000, 1030), Age::Now);
    assert_eq!(Age::between(1000, 990), Age::Now, "a clock a little behind");
    assert_eq!(Age::between(0, 5 * 60 + 10), Age::Minutes(5));
    assert_eq!(Age::between(0, 3 * hour), Age::Hours(3));
    assert_eq!(Age::between(0, 47 * hour), Age::Hours(47));
    assert_eq!(Age::between(0, 49 * hour), Age::Days(2));
}

#[test]
fn a_name_that_reads_as_a_flag_or_a_path_is_refused() {
    for good in ["app", "example-app", "sakura_desk", "v1.2"] {
        assert_eq!(name(good), Ok(good));
    }
    for bad in ["", "--json", "-x", ".hidden", "a/b", "a b", "cafés"] {
        assert!(name(bad).is_err(), "{bad:?}");
    }
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn reading_builds_the_documented_argument_vector() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("graph.json"), ANSWER).unwrap();
    let cli = fake_box(
        dir.path(),
        r#"d="$(dirname "$0")"; printf '%s\n' "$*" >> "$d/calls.log"; cat "$d/graph.json""#,
    );
    let graph = read_graph(&cli, "app", None).await.unwrap();
    assert_eq!(graph.commits.len(), 5);
    read_graph(&cli, "app", Some(5000)).await.unwrap();
    assert!(read_graph(&cli, "--all", None).await.is_err());
    let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["graph app --json", "graph app --json --limit 1000"]
    );
}

/// `count` commits of the base that all name the last one as their parent.
fn fan(count: usize) -> Graph {
    let last = format!("c{}", count - 1);
    let commits = (0..count).map(|index| {
        let parents: &[&str] = if index + 1 < count { &[&last] } else { &[] };
        commit(&format!("c{index}"), parents, None)
    });
    graph(vec![lane(None, "c0")], commits.collect())
}

#[test]
fn a_graph_over_its_budget_is_refused_before_anything_is_laid_out() {
    assert!(
        fan(20_000).checked().is_err(),
        "more commits than a graph holds"
    );
    let wide = fan(9_000)
        .checked()
        .expect("within the commits and edges a graph holds");
    assert!(
        Layout::of(&wide).is_err(),
        "its edges would cross forty million rows"
    );
    let mut crowded = fan(3);
    crowded.commits[0].parents = (0..5000).map(|n| format!("p{n}")).collect();
    assert!(crowded.checked().is_err(), "more parents than a commit has");
    assert!(Layout::of(&answer()).is_ok());
}

#[test]
fn a_time_no_calendar_holds_is_no_time_and_an_age_never_overflows() {
    for hostile in [
        "9223372036854775807-11-10T16:00:00Z",
        "-9223372036854775808-01-01T00:00:00Z",
        "10000-01-01T00:00:00Z",
        "1969-12-31T23:59:59Z",
        "2026-02-31T00:00:00Z",
        "2026-11-10T16:00:00:00Z",
    ] {
        assert_eq!(parse_time(hostile), None, "{hostile:?}");
    }
    assert_eq!(parse_time("9999-12-31T23:59:59Z"), Some(253_402_300_799));
    let longest = Age::Days(u64::try_from(i64::MAX / 86_400).unwrap());
    assert_eq!(Age::between(i64::MIN, i64::MAX), longest);
    assert_eq!(Age::between(i64::MAX, i64::MIN), Age::Now);
}

#[test]
fn a_branch_merged_into_the_base_keeps_a_column_though_its_commits_are_the_base_s() {
    // What box says of a merged task: every commit of the diamond has no task.
    let commits = vec![
        commit("m", &["a", "b"], None),
        commit("a", &["c"], None),
        commit("b", &["c"], None),
        commit("c", &[], None),
    ];
    let graph = graph(vec![lane(None, "m")], commits);
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(
        placed(&graph, &layout),
        [("m", 0), ("a", 0), ("b", 1), ("c", 0)]
    );
    let strokes: Vec<&[Stroke]> = layout.rows.iter().map(|row| &row.strokes[..]).collect();
    assert_eq!(strokes[0], [Stroke::ToBelow, Stroke::BendDown(1)]);
    assert_eq!(
        strokes[1],
        [Stroke::Through(1), Stroke::FromAbove, Stroke::ToBelow]
    );
    assert_eq!(
        strokes[2],
        [Stroke::Through(0), Stroke::FromAbove, Stroke::ToBelow]
    );
    assert_eq!(strokes[3], [Stroke::FromAbove, Stroke::BendUp(1)]);
}

#[test]
fn an_octopus_merge_gives_each_parent_a_column() {
    let commits = vec![
        commit("o", &["p1", "p2", "p3"], None),
        commit("p1", &["r"], None),
        commit("p2", &["r"], None),
        commit("p3", &["r"], None),
        commit("r", &[], None),
    ];
    let graph = graph(vec![lane(None, "o")], commits);
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(
        placed(&graph, &layout),
        [("o", 0), ("p1", 0), ("p2", 1), ("p3", 2), ("r", 0)]
    );
    assert_eq!(layout.columns, 3);
    let merge = [Stroke::ToBelow, Stroke::BendDown(1), Stroke::BendDown(2)];
    assert_eq!(layout.rows[0].strokes, merge);
    let root = [Stroke::FromAbove, Stroke::BendUp(1), Stroke::BendUp(2)];
    assert_eq!(layout.rows[4].strokes, root);
}

#[test]
fn a_history_that_cannot_be_is_refused_whole_and_never_drawn_in_part() {
    let base = || vec![lane(None, "a")];
    let own_parent = graph(base(), vec![commit("a", &["a"], None)]);
    let cycle = graph(
        base(),
        vec![commit("a", &["b"], None), commit("b", &["a"], None)],
    );
    let twice = graph(base(), vec![commit("a", &[], None), commit("a", &[], None)]);
    let beside_good = graph(
        base(),
        vec![
            commit("a", &["root"], None),
            commit("x", &["y"], None),
            commit("y", &["x"], None),
            commit("root", &[], None),
        ],
    );
    for (what, history) in [
        ("a commit that is its own parent", own_parent),
        ("two commits that are each other's parent", cycle),
        ("one commit listed twice", twice),
        ("a cycle beside a sound history", beside_good),
    ] {
        assert!(Layout::of(&history).is_err(), "{what} is laid out");
        assert!(history.checked().is_err(), "{what} passes the check");
    }
    assert!(answer().checked().is_ok());
}

#[test]
fn a_line_leaving_a_merge_never_takes_the_column_a_line_just_arrived_in() {
    // A task's line arrives at a merge of the base whose second parent then needs a column: taking
    // the one that was just freed would read as the task's line going on down.
    let lanes = vec![lane(None, "m"), lane(Some("t"), "t1")];
    let commits = vec![
        commit("t1", &["m"], Some("t")),
        commit("m", &["b1", "x"], None),
        commit("x", &["b1"], None),
        commit("b1", &[], None),
    ];
    let graph = graph(lanes, commits);
    let layout = Layout::of(&graph).expect("a graph that can be drawn");
    assert_eq!(
        placed(&graph, &layout),
        [("t1", 1), ("m", 0), ("x", 2), ("b1", 0)]
    );
    let merge = [Stroke::ToBelow, Stroke::BendUp(1), Stroke::BendDown(2)];
    assert_eq!(layout.rows[1].strokes, merge);
}

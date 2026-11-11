use super::detect::{add_project, detect, parse_detected, Addition, DETECTED_BRANCHES_MAX};
use super::*;
use crate::box_cli::test_support::fake_box;

/// One project as `box projects --json` documents it.
const PROJECT: &str = r#"{ "name": "web", "repo": "/home/me/code/shop", "subdir": "apps/web", "base": "main",
  "where": "mac", "branchPrefix": "sakura/", "account": "personal", "setup": "npm ci", "archive": null,
  "run": "npm run dev -- --port $SAKURA_PORT", "ports": 10, "runMode": "one", "install": "npm ci",
  "checks": [ { "verb": "test", "command": "npm test" } ],
  "protected": "package.json package-lock.json", "testFiles": "**/*.spec.ts",
  "tasks": ["m1"], "error": null }"#;

/// What `box project detect` documents.
const DETECTED: &str = r#"{ "repo": "/home/me/code/shop", "subdir": "apps/web", "name": "web",
  "currentBranch": "feature/cart", "defaultBranch": "main", "branches": ["feature/cart", "main"],
  "install": { "command": "npm ci", "reason": "package-lock.json is in the folder" },
  "checks": [ { "verb": "test", "command": "npm test", "reason": "script \"test\" of package.json: vitest run" } ],
  "protected": [ { "pattern": "package.json", "reason": "names the scripts and the dependencies" },
                 { "pattern": "package-lock.json", "reason": "pins the dependencies" } ],
  "testFiles": [ { "pattern": "**/*.spec.ts", "reason": "12 tracked file(s) match" } ],
  "notes": [] }"#;

fn list(elements: &[&str]) -> Vec<u8> {
    format!("[{}]", elements.join(",")).into_bytes()
}

fn usable(listed: &Listed) -> &Project {
    match listed {
        Listed::Usable(project) => project,
        Listed::Unusable { name, .. } => panic!("{name} is not usable"),
    }
}

fn rig(body: &str) -> (tempfile::TempDir, BoxCli) {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"d="$(dirname "$0")"; for a in "$@"; do printf '[%s]' "$a" >> "$d/calls.log"; done; echo >> "$d/calls.log"
{body}"#
    );
    let cli = fake_box(dir.path(), &script);
    (dir, cli)
}

fn calls(dir: &tempfile::TempDir) -> Vec<String> {
    let log = std::fs::read_to_string(dir.path().join("calls.log")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

#[test]
fn a_project_is_read_as_box_documents_it() {
    let projects = parse_projects(&list(&[PROJECT])).unwrap();
    let web = usable(&projects[0]);
    assert_eq!((web.name.as_str(), web.base.as_str()), ("web", "main"));
    assert_eq!(web.repo, "/home/me/code/shop");
    assert_eq!(web.subdir, "apps/web");
    assert_eq!(web.place, Place::Mac);
    assert_eq!(web.branch_prefix, "sakura/");
    assert_eq!(web.account.as_deref(), Some("personal"));
    assert_eq!(web.setup.as_deref(), Some("npm ci"));
    assert_eq!(web.archive, None);
    assert_eq!((web.ports, web.run_mode), (10, RunMode::One));
    let test = Check {
        verb: "test".into(),
        command: "npm test".into(),
    };
    assert_eq!(web.checks, [test]);
    assert_eq!(web.tasks, ["m1"]);
}

#[test]
fn a_project_that_cannot_be_used_is_listed_with_its_reason_and_nothing_else() {
    let whole = r#"{"name": "old", "error": "the file is refused: unknown key FOO"}"#;
    let broken = PROJECT.replace(
        r#""error": null"#,
        r#""error": "its repository does not exist""#,
    );
    let projects = parse_projects(&list(&[whole, &broken])).unwrap();
    let unusable = |name: &str, error: &str| Listed::Unusable {
        name: name.into(),
        error: error.into(),
    };
    assert_eq!(
        projects,
        [
            unusable("old", "the file is refused: unknown key FOO"),
            unusable("web", "its repository does not exist"),
        ]
    );
    assert_eq!(projects[1].name(), "web");
}

#[test]
fn a_list_that_is_not_one_of_projects_is_refused_whole() {
    let refused = |text: String| parse_projects(text.as_bytes()).unwrap_err().said;
    let not_projects = Said::new("settings.invalid.projects");
    let with = |from: &str, to: &str| format!("[{}]", PROJECT.replace(from, to));
    // Twice the same name, a name that is not one, a number of ports no project has.
    assert_eq!(refused(format!("[{PROJECT},{PROJECT}]")), not_projects);
    assert_eq!(
        refused(with(r#""name": "web""#, r#""name": "--force""#)),
        not_projects
    );
    assert_eq!(
        refused(with(r#""ports": 10"#, r#""ports": 0"#)),
        not_projects
    );
    assert_eq!(
        refused(with(r#""ports": 10"#, r#""ports": 101"#)),
        not_projects
    );
    assert_eq!(
        refused(with(r#""ports": 10"#, r#""ports": 4294967296"#)),
        not_projects
    );
    // A usable project says where its repository and base are.
    assert_eq!(refused(with(r#""base": "main","#, "")), not_projects);
    // A control character, a value longer than box accepts, a verb that is not one, a verb twice.
    assert_eq!(refused(with("npm ci", "npm ci\\u001b[2J")), not_projects);
    assert_eq!(
        refused(with("npm test", &"x".repeat(VALUE_MAX_CHARS + 1))),
        not_projects
    );
    assert_eq!(
        refused(with(r#""verb": "test""#, r#""verb": "Test!""#)),
        not_projects
    );
    let twice = r#"{ "verb": "test", "command": "npm test" }, { "verb": "test", "command": "x" }"#;
    assert_eq!(
        refused(with(r#"{ "verb": "test", "command": "npm test" }"#, twice)),
        not_projects
    );
    // More projects than a list holds.
    let many: Vec<String> = (0..=PROJECTS_MAX)
        .map(|n| format!(r#"{{"name": "p{n}", "error": "x"}}"#))
        .collect();
    assert_eq!(refused(format!("[{}]", many.join(","))), not_projects);
    assert!(parse_projects(b"{}").is_err());
}

#[test]
fn a_change_is_one_option_of_box_project_set() {
    let option = |change: Change| change.option().unwrap();
    assert_eq!(
        option(Change::Base("main".into())),
        ("--base", "main".into())
    );
    assert_eq!(option(Change::Place(Place::Box)), ("--where", "box".into()));
    assert_eq!(option(Change::Place(Place::Mac)), ("--where", "mac".into()));
    assert_eq!(
        option(Change::Account("personal".into())),
        ("--account", "personal".into())
    );
    assert_eq!(
        option(Change::Account(String::new())),
        ("--account", String::new())
    );
    assert_eq!(
        option(Change::BranchPrefix("yc/".into())),
        ("--branch-prefix", "yc/".into())
    );
    assert_eq!(
        option(Change::Setup("npm ci".into())),
        ("--setup", "npm ci".into())
    );
    assert_eq!(
        option(Change::Run(" npm run dev ".into())),
        ("--run", "npm run dev".into())
    );
    assert_eq!(
        option(Change::Archive(String::new())),
        ("--archive", String::new())
    );
    assert_eq!(option(Change::Ports(3)), ("--ports", "3".into()));
    assert_eq!(
        option(Change::RunMode(RunMode::Many)),
        ("--run-mode", "many".into())
    );
    let check = Change::Check {
        verb: "lint".into(),
        command: "npm run lint".into(),
    };
    assert_eq!(option(check), ("--check", "lint=npm run lint".into()));
    let removed = Change::Check {
        verb: "lint".into(),
        command: String::new(),
    };
    assert_eq!(option(removed), ("--check", "lint=".into()));
}

#[test]
fn a_change_box_would_refuse_or_misread_is_refused_before_it_is_sent() {
    let refused = |change: Change| change.option().is_err();
    assert!(refused(Change::Base(String::new())));
    assert!(refused(Change::Base("-D main".into())));
    assert!(refused(Change::Account("Not A Name".into())));
    assert!(refused(Change::BranchPrefix("sakura feature/".into())));
    assert!(refused(Change::BranchPrefix("-x".into())));
    assert!(refused(Change::Run("npm run dev\nrm -rf /".into())));
    assert!(refused(Change::Setup("x".repeat(VALUE_MAX_CHARS + 1))));
    assert!(refused(Change::Ports(0)));
    assert!(refused(Change::Ports(PORTS_MAX + 1)));
    let check = |verb: &str, command: &str| Change::Check {
        verb: verb.into(),
        command: command.into(),
    };
    assert!(refused(check("Lint", "x")));
    assert!(refused(check("lint=x", "y")));
    assert!(refused(check("lint", "a\u{7}b")));
}

#[test]
fn the_branches_of_a_project_are_read_as_box_documents_them() {
    let text = r#"{ "project": "web", "default": "main",
      "branches": [ { "name": "feature/cart", "sha": "0123456789012345678901234567890123456789", "time": "2026-11-10T08:00:00Z" },
                    { "name": "main", "sha": "abcdefabcdefabcdefabcdefabcdefabcdefabcd", "time": "2026-11-09T08:00:00Z" } ] }"#;
    let branches = parse_branches("web", text.as_bytes()).unwrap();
    assert_eq!(branches.default, "main");
    assert_eq!(branches.names, ["feature/cart", "main"]);
    let not_branches = Said::new("settings.invalid.branches");
    let refused =
        |project: &str, text: String| parse_branches(project, text.as_bytes()).unwrap_err().said;
    assert_eq!(
        refused("other", text.to_string()),
        not_branches,
        "another project's"
    );
    assert_eq!(
        refused("web", text.replace("feature/cart", "-delete")),
        not_branches
    );
    assert_eq!(
        refused("web", text.replace("feature/cart", "main")),
        not_branches,
        "twice"
    );
    let one = |n: usize| format!(r#"{{"name": "b{n}", "sha": "x", "time": "x"}}"#);
    let many: Vec<String> = (0..=BRANCHES_MAX).map(one).collect();
    let over = format!(
        r#"{{"project": "web", "default": "b0", "branches": [{}]}}"#,
        many.join(",")
    );
    assert_eq!(refused("web", over), not_branches);
}

#[test]
fn accounts_are_checked_before_they_are_listed() {
    let account = |name: &str, place: Place| Account {
        name: name.into(),
        place,
        logged_in: true,
        exists: None,
    };
    let both = vec![
        account("personal", Place::Box),
        account("personal", Place::Mac),
    ];
    assert_eq!(
        checked_accounts(both.clone()),
        Ok(both),
        "one name in each place is two accounts"
    );
    let twice = vec![
        account("personal", Place::Box),
        account("personal", Place::Box),
    ];
    let not_accounts = Said::new("settings.invalid.accounts");
    assert_eq!(checked_accounts(twice).unwrap_err().said, not_accounts);
    let bad = vec![account("--help", Place::Box)];
    assert_eq!(checked_accounts(bad).unwrap_err().said, not_accounts);
    let many = (0..=ACCOUNTS_MAX)
        .map(|n| account(&format!("a{n}"), Place::Box))
        .collect();
    assert_eq!(checked_accounts(many).unwrap_err().said, not_accounts);
}

#[test]
fn what_box_recognised_in_a_folder_becomes_the_project_to_add() {
    let detected = parse_detected(DETECTED.as_bytes()).unwrap();
    assert_eq!(
        (detected.name.as_str(), detected.default_branch.as_str()),
        ("web", "main")
    );
    assert_eq!(detected.current_branch.as_deref(), Some("feature/cart"));
    assert_eq!(detected.install.as_ref().unwrap().command, "npm ci");
    let addition = detected.addition();
    let args = addition.args().unwrap();
    assert_eq!(
        args,
        [
            "project",
            "add",
            "web",
            "--repo",
            "/home/me/code/shop",
            "--subdir",
            "apps/web",
            "--base",
            "main",
            "--install",
            "npm ci",
            "--check",
            "test=npm test",
            "--protected",
            "package.json package-lock.json",
            "--test-files",
            "**/*.spec.ts",
        ]
    );
    // A folder at the root of a repository with nothing recognised: only what is there is named.
    let bare = r#"{ "repo": "/r", "subdir": "", "name": "r", "currentBranch": null, "defaultBranch": "main",
      "branches": ["main"], "install": null, "checks": [], "protected": [], "testFiles": [],
      "notes": ["nothing was recognised in the folder"] }"#;
    let bare = parse_detected(bare.as_bytes()).unwrap();
    assert_eq!(bare.notes, ["nothing was recognised in the folder"]);
    assert_eq!(
        bare.addition().args().unwrap(),
        ["project", "add", "r", "--repo", "/r", "--base", "main"]
    );
}

#[test]
fn a_proposal_that_is_not_one_is_refused_whole() {
    let refused = |text: String| parse_detected(text.as_bytes()).unwrap_err().said;
    let not_a_proposal = Said::new("settings.invalid.detected");
    let with = |from: &str, to: &str| DETECTED.replace(from, to);
    assert_eq!(
        refused(with("/home/me/code/shop", "code/shop")),
        not_a_proposal,
        "not absolute"
    );
    assert_eq!(refused(with("apps/web", "../web")), not_a_proposal);
    assert_eq!(
        refused(with(r#""name": "web""#, r#""name": "Web App""#)),
        not_a_proposal
    );
    assert_eq!(
        refused(with(r#""defaultBranch": "main""#, r#""defaultBranch": """#)),
        not_a_proposal
    );
    assert_eq!(refused(with("npm test", "npm test\\u0000")), not_a_proposal);
    assert_eq!(
        refused(with(r#""verb": "test""#, r#""verb": "te st""#)),
        not_a_proposal
    );
    let branches: Vec<String> = (0..=DETECTED_BRANCHES_MAX)
        .map(|n| format!(r#""b{n}""#))
        .collect();
    let over = with(
        r#"["feature/cart", "main"]"#,
        &format!("[{}]", branches.join(",")),
    );
    assert_eq!(refused(over), not_a_proposal);
    // The name of the project to add can be changed by the person: it is checked again.
    let mut addition: Addition = parse_detected(DETECTED.as_bytes()).unwrap().addition();
    addition.name = "--repo".into();
    assert!(addition.args().is_err());
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn every_call_is_the_documented_argument_vector() {
    let (dir, cli) = rig(&format!(
        r#"case "$1 $2" in
  "projects --json") cat <<'EOF'
[{PROJECT}]
EOF
  ;;
  "branches web") echo '{{"project":"web","default":"main","branches":[]}}' ;;
  "accounts --json") echo '[{{"name":"personal","where":"box","loggedIn":true}}]' ;;
  "project detect") cat <<'EOF'
{DETECTED}
EOF
  ;;
  *) echo "done" ;;
esac"#
    ));
    assert_eq!(read_projects(&cli).await.unwrap().len(), 1);
    assert_eq!(read_branches(&cli, "web").await.unwrap().default, "main");
    assert_eq!(read_accounts(&cli).await.unwrap().len(), 1);
    set_project(&cli, "web", &Change::Run("npm run dev".into()))
        .await
        .unwrap();
    set_project(&cli, "web", &Change::Archive(String::new()))
        .await
        .unwrap();
    let detected = detect(&cli, "/home/me/code/shop/apps/web").await.unwrap();
    add_project(&cli, &detected.addition()).await.unwrap();
    remove_project(&cli, "web").await.unwrap();
    let calls = calls(&dir);
    assert_eq!(calls[0], "[projects][--json]");
    assert_eq!(calls[1], "[branches][web][--json]");
    assert_eq!(calls[2], "[accounts][--json]");
    assert_eq!(calls[3], "[project][set][web][--run][npm run dev]");
    assert_eq!(calls[4], "[project][set][web][--archive][]");
    assert_eq!(
        calls[5],
        "[project][detect][/home/me/code/shop/apps/web][--json]"
    );
    assert!(
        calls[6].starts_with("[project][add][web][--repo][/home/me/code/shop][--subdir][apps/web]")
    );
    assert_eq!(calls[7], "[project][rm][web]");
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn bad_arguments_never_reach_box_and_a_refusal_is_said_with_box_s_line() {
    let (dir, cli) = rig(
        r#"echo "box: the repository and the subfolder of project 'web' cannot change while tasks use it: m1" >&2; exit 1"#,
    );
    assert!(read_branches(&cli, "Not A Project").await.is_err());
    assert!(set_project(&cli, "--x", &Change::Ports(3)).await.is_err());
    assert!(set_project(&cli, "web", &Change::Ports(0)).await.is_err());
    assert!(remove_project(&cli, "").await.is_err());
    assert!(detect(&cli, "relative/folder").await.is_err());
    assert!(detect(&cli, "/a\nb").await.is_err());
    assert!(calls(&dir).is_empty());
    let refusal = remove_project(&cli, "web").await.unwrap_err();
    assert!(refusal.details.unwrap().contains("while tasks use it"));
}

//! Projects: what is worked on. A project is a repository on the service's host, an optional subfolder,
//! a base branch, where its lavori live by default and with which account. Nobody edits a file to work on
//! another folder: the window asks the person for one, shows what `box project detect` proposes, and adds
//! the project once with `box project add`. A new lavoro names a project.

use crate::box_cli::Limit;
use crate::error::{BridgeResult, ErrorCode};
use crate::models::Place;
use crate::service::Core;
use crate::validate;
use serde::{Deserialize, Serialize};

/// One project, as `box projects --json` lists it. A project `box` cannot use is still listed, with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    /// The branch its lavori start from when none is named.
    #[serde(default)]
    pub base: Option<String>,
    /// Where its lavori live when the person does not say.
    #[serde(default, rename = "where")]
    pub place: Place,
    /// The account its agents start with when none is named.
    #[serde(default)]
    pub account: Option<String>,
    /// The command Play runs in its lavori; none when the project names none.
    #[serde(default)]
    pub run: Option<String>,
    /// Why `box` cannot use it, in `box`'s own words (data).
    #[serde(default)]
    pub error: Option<String>,
}

/// What `box project detect` recognised in a folder the person chose. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    /// The root of the repository the folder is in.
    pub repo: String,
    /// The folder inside it; empty at the root.
    pub subdir: String,
    /// A name for the project that none has yet.
    pub name: String,
    pub default_branch: String,
    /// Local branches, most recently committed first.
    #[serde(default)]
    pub branches: Vec<String>,
    #[serde(default)]
    pub install: Option<Proposal>,
    #[serde(default)]
    pub checks: Vec<ProposedCheck>,
    #[serde(default)]
    pub protected: Vec<ProposedPattern>,
    #[serde(default)]
    pub test_files: Vec<ProposedPattern>,
    /// Where a proposal would not work as it is, in `box`'s words (data).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A command `box` proposes, and why (data).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Proposal {
    pub command: String,
    pub reason: String,
}

/// A check `box` proposes: its verb, its command, and why (data).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProposedCheck {
    pub verb: String,
    pub command: String,
    pub reason: String,
}

/// A file pattern `box` proposes, and why (data).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProposedPattern {
    pub pattern: String,
    pub reason: String,
}

/// The branches of a project's repository a lavoro can start from, most recently committed first.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Branches {
    /// The project's base branch.
    pub default: String,
    pub branches: Vec<Branch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Branch {
    pub name: String,
}

/// A project to add: what was detected, as the person confirmed or corrected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    pub name: String,
    pub repo: String,
    pub subdir: String,
    pub base: String,
    pub install: Option<String>,
    /// The checks kept, as (verb, command).
    pub checks: Vec<(String, String)>,
    pub protected: Vec<String>,
    pub test_files: Vec<String>,
    /// The account its agents start with.
    pub account: Option<String>,
}

impl NewProject {
    /// What `detected` proposes, whole: every check kept, the default branch as the base.
    pub fn proposed(detected: &Detected) -> Self {
        let patterns = |proposed: &[ProposedPattern]| {
            proposed
                .iter()
                .map(|proposed| proposed.pattern.clone())
                .collect()
        };
        let check = |check: &ProposedCheck| (check.verb.clone(), check.command.clone());
        Self {
            name: detected.name.clone(),
            repo: detected.repo.clone(),
            subdir: detected.subdir.clone(),
            base: detected.default_branch.clone(),
            install: detected.install.as_ref().map(|i| i.command.clone()),
            checks: detected.checks.iter().map(check).collect(),
            protected: patterns(&detected.protected),
            test_files: patterns(&detected.test_files),
            account: None,
        }
    }

    /// The arguments of `box project add` for it, every value checked.
    fn arguments(&self) -> BridgeResult<Vec<String>> {
        let mut args = vec!["project".to_string(), "add".to_string()];
        args.push(validate::project(&self.name)?.to_string());
        args.extend(["--repo".to_string(), folder(&self.repo)?]);
        args.extend(["--base".to_string(), branch(&self.base)?]);
        let mut option = |name: &str, value: Option<String>| -> BridgeResult<()> {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                args.extend([name.to_string(), validate::text(&value)?.to_string()]);
            }
            Ok(())
        };
        option("--subdir", Some(self.subdir.clone()))?;
        option("--install", self.install.clone())?;
        option("--protected", Some(self.protected.join(" ")))?;
        option("--test-files", Some(self.test_files.join(" ")))?;
        for (verb, command) in &self.checks {
            let verb = validate::verb(verb)?;
            option("--check", Some(format!("{verb}={command}")))?;
        }
        if let Some(account) = &self.account {
            args.extend([
                "--account".to_string(),
                validate::account(account)?.to_string(),
            ]);
        }
        Ok(args)
    }
}

/// A lavoro to create: its project, and what the person chose instead of the project's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLavoro {
    pub project: String,
    /// The branch it starts from; the project's base when none.
    pub base: Option<String>,
    /// Where it lives; the project's place when none.
    pub place: Option<Place>,
}

impl NewLavoro {
    /// The arguments of `box new <name>` for it, every value checked.
    pub(crate) fn arguments(&self, name: &str) -> BridgeResult<Vec<String>> {
        let mut args = vec![
            "new".to_string(),
            validate::task(name)?.to_string(),
            validate::project(&self.project)?.to_string(),
        ];
        if let Some(base) = &self.base {
            args.extend(["--base".to_string(), branch(base)?]);
        }
        if let Some(place) = self.place {
            args.extend(["--where".to_string(), place.word().to_string()]);
        }
        Ok(args)
    }
}

fn folder(path: &str) -> BridgeResult<String> {
    validate::folder(path).map(str::to_string)
}

fn branch(name: &str) -> BridgeResult<String> {
    validate::branch(name).map(str::to_string)
}

impl Core {
    /// `box projects --json`: every project. `None` when this `box` has no projects (an older one).
    pub async fn projects(&self) -> BridgeResult<Option<Vec<Project>>> {
        let read = self.cli().run_json(&["projects", "--json"], Limit::Read);
        match read.await {
            Ok(projects) => Ok(Some(projects)),
            Err(error) if error.code == ErrorCode::Unsupported => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `box project detect <folder> --json`: what `box` recognises in a folder. Writes nothing.
    pub async fn project_detect(&self, path: &str) -> BridgeResult<Detected> {
        let path = validate::folder(path)?;
        self.cli()
            .run_json(&["project", "detect", path, "--json"], Limit::Read)
            .await
    }

    /// `box project add`: adds the project as the person confirmed it.
    pub async fn project_add(&self, project: &NewProject) -> BridgeResult<()> {
        let args = project.arguments()?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.cli().run_text(&args, Limit::Action).await.map(drop)
    }

    /// `box project set <project> --account <account>`: the account new lavori of the project start with.
    pub async fn project_set_account(&self, project: &str, account: &str) -> BridgeResult<()> {
        let project = validate::project(project)?;
        let account = validate::account(account)?;
        let args = ["project", "set", project, "--account", account];
        self.cli().run_text(&args, Limit::Action).await.map(drop)
    }

    /// `box branches <project> --json`: the branches a lavoro of the project can start from.
    pub async fn branches(&self, project: &str) -> BridgeResult<Branches> {
        let project = validate::project(project)?;
        self.cli()
            .run_json(&["branches", project, "--json"], Limit::Read)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::box_cli::test_support::fake_box;
    use crate::service::FactsNotifier;
    use std::sync::Arc;

    const PROJECTS: &str = r#"[ { "name": "web", "repo": "/home/me/code/shop", "subdir": "apps/web", "base": "main",
      "where": "mac", "branchPrefix": "sakura/", "account": "personal", "setup": "npm ci", "archive": null,
      "run": null, "ports": 10, "runMode": "one", "install": "npm ci",
      "checks": [ { "verb": "test", "command": "npm test" } ],
      "protected": "package.json", "testFiles": "**/*.spec.ts", "tasks": ["m1"], "error": null },
    { "name": "broken", "error": "its repository does not exist" } ]"#;

    const DETECTED: &str = r#"{ "repo": "/home/me/code/shop", "subdir": "apps/web", "name": "web",
      "currentBranch": "feature/cart", "defaultBranch": "main", "branches": ["feature/cart", "main"],
      "install": { "command": "npm ci", "reason": "package-lock.json is in the folder" },
      "checks": [ { "verb": "test", "command": "npm test", "reason": "script \"test\" of package.json: vitest run" } ],
      "protected": [ { "pattern": "package.json", "reason": "names the scripts and the dependencies" } ],
      "testFiles": [ { "pattern": "**/*.spec.ts", "reason": "12 tracked file(s) match" } ],
      "notes": [] }"#;

    /// A core on a fake `box` that writes its arguments, one call per line, into `calls` beside it, and
    /// answers with `cases` (lines of a shell `case` on the first argument).
    fn core(cases: &str) -> (tempfile::TempDir, Core) {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            r#"d="$(dirname "$0")"
for a in "$@"; do printf '[%s]' "$a" >> "$d/calls"; done; echo >> "$d/calls"
case "$1" in
{cases}
  *) echo 'usage: box <command>' >&2; exit 2 ;;
esac"#
        );
        let cli = fake_box(dir.path(), &body);
        let notify: FactsNotifier = Arc::new(|_| {});
        (dir, Core::new(cli, notify))
    }

    fn calls(dir: &tempfile::TempDir) -> Vec<String> {
        let text = std::fs::read_to_string(dir.path().join("calls")).unwrap_or_default();
        text.lines().map(str::to_string).collect()
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn projects_are_read_as_box_lists_them_also_one_it_cannot_use() {
        let (dir, core) = core(r#"  projects) cat "$d/projects.json" ;;"#);
        std::fs::write(dir.path().join("projects.json"), PROJECTS).unwrap();
        let projects = core
            .projects()
            .await
            .unwrap()
            .expect("this box has projects");
        assert_eq!(projects.len(), 2);
        let web = &projects[0];
        assert_eq!(web.name, "web");
        assert_eq!(web.base.as_deref(), Some("main"));
        assert_eq!(web.place, Place::Mac);
        assert_eq!(web.account.as_deref(), Some("personal"));
        assert_eq!(web.error, None);
        assert_eq!(
            projects[1].error.as_deref(),
            Some("its repository does not exist")
        );
        assert_eq!(calls(&dir), ["[projects][--json]"]);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_box_without_projects_is_told_apart_from_a_failure() {
        let (_dir, old) = core("");
        assert_eq!(old.projects().await.unwrap(), None);
        let (_dir, broken) =
            core("  projects) echo 'box: cannot read the projects' >&2; exit 1 ;;");
        assert_eq!(
            broken.projects().await.unwrap_err().code,
            ErrorCode::BoxFailed
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_folder_is_detected_and_what_was_recognised_is_read_with_its_reasons() {
        let (dir, core) = core(r#"  project) cat "$d/detected.json" ;;"#);
        std::fs::write(dir.path().join("detected.json"), DETECTED).unwrap();
        let detected = core
            .project_detect("/home/me/code/shop/apps/web")
            .await
            .unwrap();
        assert_eq!(detected.repo, "/home/me/code/shop");
        assert_eq!(detected.subdir, "apps/web");
        assert_eq!(detected.default_branch, "main");
        assert_eq!(detected.branches, ["feature/cart", "main"]);
        assert_eq!(detected.checks[0].verb, "test");
        assert!(detected.checks[0].reason.starts_with("script \"test\""));
        assert_eq!(
            calls(&dir),
            ["[project][detect][/home/me/code/shop/apps/web][--json]"]
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_folder_that_could_be_read_as_an_option_is_never_passed_to_box() {
        let (dir, core) = core(r#"  project) echo '{}' ;;"#);
        for path in ["--help", "relative/folder", "", "/a\nb"] {
            let error = core.project_detect(path).await.unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{path:?}");
        }
        assert!(calls(&dir).is_empty(), "no process was started");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_refused_folder_keeps_the_line_of_box_as_details() {
        let refusal = "box: '/x' cannot be used: it borrows git objects from another repository \
                       (objects/info/alternates): run 'git repack -a -d' in it, then remove that file";
        let cases = format!("  project) echo \"{refusal}\" >&2; exit 6 ;;");
        let (_dir, core) = core(&cases);
        let error = core.project_detect("/x").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
        assert_eq!(error.said.key.0, "box.failure.borrows_objects");
        assert_eq!(error.details.as_deref(), Some(refusal));
    }

    fn detected() -> Detected {
        serde_json::from_str(DETECTED).unwrap()
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_project_is_added_with_what_was_proposed_and_the_account_chosen() {
        let (dir, core) = core("  project) echo added ;;");
        let mut project = NewProject::proposed(&detected());
        project.account = Some("personal".into());
        core.project_add(&project).await.unwrap();
        assert_eq!(
            calls(&dir),
            [
                "[project][add][web][--repo][/home/me/code/shop][--base][main][--subdir][apps/web]\
              [--install][npm ci][--protected][package.json][--test-files][**/*.spec.ts]\
              [--check][test=npm test][--account][personal]"
            ]
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_project_at_the_root_with_nothing_recognised_is_added_with_its_repository_and_base() {
        let (dir, core) = core("  project) echo added ;;");
        let project = NewProject {
            subdir: String::new(),
            install: None,
            checks: Vec::new(),
            protected: Vec::new(),
            test_files: Vec::new(),
            ..NewProject::proposed(&detected())
        };
        core.project_add(&project).await.unwrap();
        assert_eq!(
            calls(&dir),
            ["[project][add][web][--repo][/home/me/code/shop][--base][main]"]
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_project_with_a_value_box_would_misread_is_not_added() {
        let (dir, core) = core("  project) echo added ;;");
        let bad_base = NewProject {
            base: "--force".into(),
            ..NewProject::proposed(&detected())
        };
        let bad_check = NewProject {
            checks: vec![("Test".into(), "npm test".into())],
            ..NewProject::proposed(&detected())
        };
        for project in [bad_base, bad_check] {
            let error = core.project_add(&project).await.unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument);
        }
        assert!(calls(&dir).is_empty());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn the_branches_of_a_project_are_read_with_its_default() {
        let answer = r#"{ "project": "web", "default": "main", "branches": [
            { "name": "feature/cart", "sha": "9b1e0c3a5d7f2468ace013579bdf02468ace1357", "time": "2026-11-10T08:00:00Z" },
            { "name": "main", "sha": "536ea1c688aa01b2c3d4e5f60718293a4b5c6d7e", "time": "2026-11-09T08:00:00Z" } ] }"#;
        let (dir, core) = core(r#"  branches) cat "$d/branches.json" ;;"#);
        std::fs::write(dir.path().join("branches.json"), answer).unwrap();
        let branches = core.branches("web").await.unwrap();
        assert_eq!(branches.default, "main");
        let names: Vec<&str> = branches.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["feature/cart", "main"]);
        assert_eq!(calls(&dir), ["[branches][web][--json]"]);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn the_account_of_a_project_is_set_by_name() {
        let (dir, core) = core("  project) echo changed ;;");
        core.project_set_account("web", "work").await.unwrap();
        assert_eq!(calls(&dir), ["[project][set][web][--account][work]"]);
        assert!(core.project_set_account("web", "-x").await.is_err());
    }

    #[test]
    fn a_new_lavoro_names_its_project_and_only_what_differs_from_it() {
        let plain = NewLavoro {
            project: "web".into(),
            base: None,
            place: None,
        };
        assert_eq!(
            plain.arguments("sistema-login").unwrap(),
            ["new", "sistema-login", "web"]
        );
        let chosen = NewLavoro {
            base: Some("feature/cart".into()),
            place: Some(Place::Box),
            ..plain
        };
        assert_eq!(
            chosen.arguments("sistema-login").unwrap(),
            [
                "new",
                "sistema-login",
                "web",
                "--base",
                "feature/cart",
                "--where",
                "box"
            ]
        );
        let on_the_mac = NewLavoro {
            place: Some(Place::Mac),
            base: None,
            project: "web".into(),
        };
        assert_eq!(
            on_the_mac.arguments("a").unwrap(),
            ["new", "a", "web", "--where", "mac"]
        );
    }

    #[test]
    fn a_new_lavoro_with_a_name_or_a_branch_box_would_misread_has_no_arguments() {
        let lavoro = NewLavoro {
            project: "web".into(),
            base: Some("-b".into()),
            place: None,
        };
        assert!(lavoro.arguments("ok").is_err());
        let fine = NewLavoro {
            base: None,
            ..lavoro
        };
        assert!(fine.arguments("Not A Slug").is_err());
    }
}

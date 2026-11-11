//! What Settings shows and changes through `box`: the projects (`box projects`, `box project set`,
//! `box project rm`), the branches a project can start from (`box branches`) and the accounts
//! (`box accounts`). Adding a project is in [`detect`]. Every reply is checked where it is parsed:
//! one that is not what `box` documents, or that is larger than a list a person reads, is refused
//! whole and said as unreadable, never shown in part. Every value sent is checked before `box` runs.

use crate::box_cli::{BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult};
use crate::models::{Account, Place};
use crate::said::Said;
use crate::validate;
use serde::Deserialize;
use std::collections::HashSet;

pub mod detect;

/// The most projects a list holds.
pub const PROJECTS_MAX: usize = 500;
/// The longest value `box` accepts in a project, in characters.
pub const VALUE_MAX_CHARS: usize = 2000;
/// The most checks a project names.
pub const CHECKS_MAX: usize = 50;
/// The most tasks a project is said to have.
pub const TASKS_MAX: usize = 10_000;
/// The most ports a task of a project gets.
pub const PORTS_MAX: u8 = 100;
/// The most branches `box branches` lists.
pub const BRANCHES_MAX: usize = 200;
/// The most accounts a list holds.
pub const ACCOUNTS_MAX: usize = 200;

/// Whether starting the Play of a task stops the Plays of the project's other tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// One Play at a time in the project.
    #[default]
    One,
    /// Every task may run its own.
    Many,
}

impl RunMode {
    /// The word `box` knows it by.
    pub fn word(self) -> &'static str {
        match self {
            RunMode::One => "one",
            RunMode::Many => "many",
        }
    }
}

/// One check of a project: the verb `box verify` takes and the command it runs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Check {
    pub verb: String,
    pub command: String,
}

/// A project that can be used, as `box projects --json` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// The repository's folder on this Mac, and the folder of the project inside it (empty at the root).
    pub repo: String,
    pub subdir: String,
    /// The branch new tasks start from.
    pub base: String,
    /// Where new tasks live.
    pub place: Place,
    pub branch_prefix: String,
    pub account: Option<String>,
    /// The commands: preparation, archive, run; the box's install command is shown read only.
    pub setup: Option<String>,
    pub archive: Option<String>,
    pub run: Option<String>,
    pub install: Option<String>,
    pub ports: u8,
    pub run_mode: RunMode,
    pub checks: Vec<Check>,
    /// The tasks that use it: while there are any, `box` refuses to remove it.
    pub tasks: Vec<String>,
}

/// One element of the list: a project, or the name of one that cannot be used and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    Usable(Box<Project>),
    /// `error` is the sentence `box` wrote.
    Unusable {
        name: String,
        error: String,
    },
}

impl Listed {
    pub fn name(&self) -> &str {
        match self {
            Listed::Usable(project) => &project.name,
            Listed::Unusable { name, .. } => name,
        }
    }
}

/// The branches a task of a project can start from, most recently committed first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branches {
    /// The project's base.
    pub default: String,
    pub names: Vec<String>,
}

/// One change to a project: one option of `box project set`. An empty text takes the value away
/// (the account, a command, a check); an empty branch prefix puts `box`'s own back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Base(String),
    Place(Place),
    Account(String),
    BranchPrefix(String),
    Setup(String),
    Run(String),
    Archive(String),
    Check { verb: String, command: String },
    Ports(u8),
    RunMode(RunMode),
}

impl Change {
    /// The option and its value, checked: nothing `box` would refuse or read as something else.
    pub fn option(&self) -> BridgeResult<(&'static str, String)> {
        let text = |flag: &'static str, value: &str| Ok((flag, plain(value.trim())?.to_string()));
        match self {
            Change::Base(branch) => Ok(("--base", branch_name(branch)?.to_string())),
            Change::Place(place) => Ok(("--where", place_word(*place).to_string())),
            Change::Account(name) if name.is_empty() => Ok(("--account", String::new())),
            Change::Account(name) => Ok(("--account", validate::account(name)?.to_string())),
            Change::BranchPrefix(prefix) => {
                Ok(("--branch-prefix", branch_prefix(prefix)?.to_string()))
            }
            Change::Setup(command) => text("--setup", command),
            Change::Run(command) => text("--run", command),
            Change::Archive(command) => text("--archive", command),
            Change::Check { verb, command } => {
                let (verb, command) = (validate::verb(verb)?, plain(command.trim())?);
                Ok(("--check", format!("{verb}={command}")))
            }
            Change::Ports(ports) if (1..=PORTS_MAX).contains(ports) => {
                Ok(("--ports", ports.to_string()))
            }
            Change::Ports(_) => Err(not_a_value()),
            Change::RunMode(mode) => Ok(("--run-mode", mode.word().to_string())),
        }
    }
}

/// The word `box` knows a place by.
pub fn place_word(place: Place) -> &'static str {
    match place {
        Place::Mac => "mac",
        Place::Box => "box",
    }
}

/// One element as `box` prints it: a file refused as a whole has only its name and the reason.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Printed {
    name: String,
    repo: Option<String>,
    subdir: Option<String>,
    base: Option<String>,
    #[serde(rename = "where")]
    place: Option<Place>,
    branch_prefix: Option<String>,
    account: Option<String>,
    setup: Option<String>,
    archive: Option<String>,
    run: Option<String>,
    install: Option<String>,
    ports: Option<u64>,
    run_mode: Option<RunMode>,
    #[serde(default)]
    checks: Vec<Check>,
    #[serde(default)]
    tasks: Vec<String>,
    error: Option<String>,
}

impl Printed {
    /// The element checked: `None` when it is not one a list of projects holds.
    fn listed(self) -> Option<Listed> {
        let name = validate::project(&self.name).ok()?.to_string();
        if let Some(error) = self.error {
            return is_plain(&error).then_some(Listed::Unusable { name, error });
        }
        let ports = u8::try_from(self.ports?).ok()?;
        let project = Project {
            name,
            repo: self.repo?,
            subdir: self.subdir.unwrap_or_default(),
            base: self.base?,
            place: self.place?,
            branch_prefix: self.branch_prefix?,
            account: self.account,
            setup: self.setup,
            archive: self.archive,
            run: self.run,
            install: self.install,
            ports,
            run_mode: self.run_mode?,
            checks: self.checks,
            tasks: self.tasks,
        };
        project
            .is_sound()
            .then_some(Listed::Usable(Box::new(project)))
    }
}

impl Project {
    /// Whether every value is one `box` keeps in a project, within the bounds of a list.
    fn is_sound(&self) -> bool {
        let optional = [
            &self.account,
            &self.setup,
            &self.archive,
            &self.run,
            &self.install,
        ];
        let texts = [&self.repo, &self.subdir, &self.base, &self.branch_prefix];
        let verbs: HashSet<&str> = self
            .checks
            .iter()
            .map(|check| check.verb.as_str())
            .collect();
        let are_checks_sound = self.checks.len() <= CHECKS_MAX
            && verbs.len() == self.checks.len()
            && self
                .checks
                .iter()
                .all(|check| validate::verb(&check.verb).is_ok() && is_plain(&check.command));
        let are_tasks_sound = self.tasks.len() <= TASKS_MAX
            && self.tasks.iter().all(|task| validate::task(task).is_ok());
        texts.iter().all(|text| is_plain(text))
            && optional
                .iter()
                .all(|text| text.as_deref().map_or(true, is_plain))
            && !self.repo.is_empty()
            && !self.base.is_empty()
            && (1..=PORTS_MAX).contains(&self.ports)
            && are_checks_sound
            && are_tasks_sound
    }
}

/// The projects in a reply of `box projects --json`.
pub fn parse_projects(bytes: &[u8]) -> BridgeResult<Vec<Listed>> {
    let invalid = || BridgeError::failed(Said::new("settings.invalid.projects"));
    let printed: Vec<Printed> = serde_json::from_slice(bytes)
        .map_err(|error| invalid().with_details(Some(error.to_string())))?;
    if printed.len() > PROJECTS_MAX {
        return Err(invalid());
    }
    let listed: Option<Vec<Listed>> = printed.into_iter().map(Printed::listed).collect();
    let listed = listed.ok_or_else(invalid)?;
    let names: HashSet<&str> = listed.iter().map(Listed::name).collect();
    (names.len() == listed.len())
        .then_some(listed)
        .ok_or_else(invalid)
}

#[derive(Deserialize)]
struct PrintedBranch {
    name: String,
}

#[derive(Deserialize)]
struct PrintedBranches {
    project: String,
    default: String,
    branches: Vec<PrintedBranch>,
}

/// The branches of `project` in a reply of `box branches <project> --json`.
pub fn parse_branches(project: &str, bytes: &[u8]) -> BridgeResult<Branches> {
    let invalid = || BridgeError::failed(Said::new("settings.invalid.branches"));
    let printed: PrintedBranches = serde_json::from_slice(bytes)
        .map_err(|error| invalid().with_details(Some(error.to_string())))?;
    let names: Vec<String> = printed
        .branches
        .into_iter()
        .map(|branch| branch.name)
        .collect();
    let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
    let is_sound = printed.project == project
        && names.len() <= BRANCHES_MAX
        && unique.len() == names.len()
        && branch_name(&printed.default).is_ok()
        && names.iter().all(|name| branch_name(name).is_ok());
    let branches = Branches {
        default: printed.default,
        names,
    };
    is_sound.then_some(branches).ok_or_else(invalid)
}

/// `accounts` when it is a list a person reads: bounded, every name one, none twice in its place.
pub fn checked_accounts(accounts: Vec<Account>) -> BridgeResult<Vec<Account>> {
    let named: HashSet<(&str, Place)> = accounts
        .iter()
        .map(|account| (account.name.as_str(), account.place))
        .collect();
    let is_sound = accounts.len() <= ACCOUNTS_MAX
        && named.len() == accounts.len()
        && accounts
            .iter()
            .all(|account| validate::account(&account.name).is_ok());
    if is_sound {
        Ok(accounts)
    } else {
        Err(BridgeError::failed(Said::new("settings.invalid.accounts")))
    }
}

/// `box projects --json`.
pub async fn read_projects(cli: &BoxCli) -> BridgeResult<Vec<Listed>> {
    let bytes = cli.run_bytes(&["projects", "--json"], Limit::Read).await?;
    parse_projects(&bytes)
}

/// `box branches <project> --json`.
pub async fn read_branches(cli: &BoxCli, project: &str) -> BridgeResult<Branches> {
    let project = validate::project(project)?;
    let bytes = cli
        .run_bytes(&["branches", project, "--json"], Limit::Read)
        .await?;
    parse_branches(project, &bytes)
}

/// `box accounts --json`.
pub async fn read_accounts(cli: &BoxCli) -> BridgeResult<Vec<Account>> {
    let accounts = cli.run_json(&["accounts", "--json"], Limit::Read).await?;
    checked_accounts(accounts)
}

/// `box project set <name> <option> <value>`.
pub async fn set_project(cli: &BoxCli, name: &str, change: &Change) -> BridgeResult<()> {
    let (name, (option, value)) = (validate::project(name)?, change.option()?);
    let args = ["project", "set", name, option, &value];
    cli.run_text(&args, Limit::Action).await.map(drop)
}

/// `box project rm <name>`. `box` refuses while a task uses the project; its line is the details.
pub async fn remove_project(cli: &BoxCli, name: &str) -> BridgeResult<()> {
    let name = validate::project(name)?;
    cli.run_text(&["project", "rm", name], Limit::Action)
        .await
        .map(drop)
}

/// Whether `value` is text `box` keeps in a project: no control character, at most
/// [`VALUE_MAX_CHARS`] characters.
pub(crate) fn is_plain(value: &str) -> bool {
    value.chars().count() <= VALUE_MAX_CHARS && !value.chars().any(char::is_control)
}

fn not_a_value() -> BridgeError {
    BridgeError::invalid(Said::new("settings.invalid.value"))
}

fn plain(value: &str) -> BridgeResult<&str> {
    is_plain(value).then_some(value).ok_or_else(not_a_value)
}

/// A branch name as an argument: plain, not empty, never read as an option.
pub(crate) fn branch_name(value: &str) -> BridgeResult<&str> {
    let is_name =
        !value.is_empty() && !value.starts_with('-') && !value.contains(char::is_whitespace);
    plain(value)
        .ok()
        .filter(|_| is_name)
        .ok_or_else(not_a_value)
}

/// The start of a branch name as `box` takes it: letters, digits, dot, hyphen, underscore, slash;
/// empty puts `box`'s own back.
fn branch_prefix(value: &str) -> BridgeResult<&str> {
    let is_letter = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/');
    let is_prefix = value.chars().all(is_letter) && !value.starts_with('-');
    plain(value)
        .ok()
        .filter(|_| is_prefix)
        .ok_or_else(not_a_value)
}

#[cfg(test)]
mod tests;

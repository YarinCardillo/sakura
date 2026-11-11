//! Adding a project: `box project detect <folder> --json` proposes one from a folder the person
//! chose and writes nothing; what it recognised is shown once to be confirmed, then
//! `box project add` writes it. The proposal is checked where it is parsed and the project to add is
//! checked again before `box` runs, because the person may have changed its name.

use super::{branch_name, is_plain, Check};
use crate::box_cli::{BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult};
use crate::said::Said;
use crate::validate;
use serde::Deserialize;

/// The most local branches a proposal lists.
pub const DETECTED_BRANCHES_MAX: usize = 30;
/// The most checks, patterns or notes a proposal holds.
pub const DETECTED_LIST_MAX: usize = 50;
/// The longest path of a folder, in bytes: the system's own limit.
pub const PATH_MAX_BYTES: usize = 4096;

/// A command `box` proposes and why.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Proposal {
    pub command: String,
    pub reason: String,
}

/// A check `box` proposes and why.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProposedCheck {
    pub verb: String,
    pub command: String,
    pub reason: String,
}

/// A pattern of files `box` proposes (the protected ones, the tests) and why.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pattern {
    pub pattern: String,
    pub reason: String,
}

/// What `box project detect` recognised in a folder.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    /// The root of the repository, and the folder inside it (empty at the root).
    pub repo: String,
    pub subdir: String,
    /// A name no project has yet, made from the folder's.
    pub name: String,
    pub current_branch: Option<String>,
    pub default_branch: String,
    pub branches: Vec<String>,
    pub install: Option<Proposal>,
    pub checks: Vec<ProposedCheck>,
    pub protected: Vec<Pattern>,
    pub test_files: Vec<Pattern>,
    /// Where a proposal would not work as it is, in `box`'s words.
    pub notes: Vec<String>,
}

/// The project `box project add` is asked to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addition {
    pub name: String,
    pub repo: String,
    pub subdir: String,
    pub base: String,
    pub install: Option<String>,
    pub checks: Vec<Check>,
    pub protected: Vec<String>,
    pub test_files: Vec<String>,
}

impl Detected {
    /// The project to add when the person confirms what was recognised as it is.
    pub fn addition(&self) -> Addition {
        let patterns = |list: &[Pattern]| list.iter().map(|each| each.pattern.clone()).collect();
        let check = |check: &ProposedCheck| Check {
            verb: check.verb.clone(),
            command: check.command.clone(),
        };
        Addition {
            name: self.name.clone(),
            repo: self.repo.clone(),
            subdir: self.subdir.clone(),
            base: self.default_branch.clone(),
            install: self.install.as_ref().map(|install| install.command.clone()),
            checks: self.checks.iter().map(check).collect(),
            protected: patterns(&self.protected),
            test_files: patterns(&self.test_files),
        }
    }

    /// Whether this is a proposal `box` documents, within the bounds of one screen.
    fn is_sound(&self) -> bool {
        let reasons = self.checks.iter().map(|check| &check.reason);
        let reasons = reasons.chain(self.install.iter().map(|install| &install.reason));
        let patterns = self.protected.iter().chain(&self.test_files);
        let reasons = reasons.chain(patterns.map(|pattern| &pattern.reason));
        let lists = [
            self.checks.len(),
            self.protected.len(),
            self.test_files.len(),
            self.notes.len(),
        ];
        self.addition().args().is_ok()
            && self.branches.len() <= DETECTED_BRANCHES_MAX
            && lists.iter().all(|length| *length <= DETECTED_LIST_MAX)
            && self
                .branches
                .iter()
                .all(|branch| branch_name(branch).is_ok())
            && self
                .current_branch
                .as_deref()
                .map_or(true, |branch| branch_name(branch).is_ok())
            && reasons.chain(&self.notes).all(|text| is_plain(text))
    }
}

impl Addition {
    /// The arguments of `box project add`, checked.
    pub fn args(&self) -> BridgeResult<Vec<String>> {
        let name = validate::project(&self.name)?;
        let mut args: Vec<String> = ["project", "add", name, "--repo", folder(&self.repo)?]
            .map(str::to_string)
            .to_vec();
        let mut option = |flag: &str, value: String| args.extend([flag.to_string(), value]);
        if !self.subdir.is_empty() {
            option("--subdir", subfolder(&self.subdir)?.to_string());
        }
        option("--base", branch_name(&self.base)?.to_string());
        if let Some(install) = &self.install {
            option("--install", plain(install)?.to_string());
        }
        for check in &self.checks {
            let (verb, command) = (validate::verb(&check.verb)?, plain(&check.command)?);
            option("--check", format!("{verb}={command}"));
        }
        for (flag, patterns) in [
            ("--protected", &self.protected),
            ("--test-files", &self.test_files),
        ] {
            if !patterns.is_empty() {
                option(flag, globs(patterns)?);
            }
        }
        Ok(args)
    }
}

/// The proposal in a reply of `box project detect <folder> --json`.
pub fn parse_detected(bytes: &[u8]) -> BridgeResult<Detected> {
    let invalid = || BridgeError::failed(Said::new("settings.invalid.detected"));
    let detected: Detected = serde_json::from_slice(bytes)
        .map_err(|error| invalid().with_details(Some(error.to_string())))?;
    detected.is_sound().then_some(detected).ok_or_else(invalid)
}

/// `box project detect <folder> --json`. A folder `box` refuses (not a folder, not in a repository,
/// not the person's) is a failure with `box`'s line as its details.
pub async fn detect(cli: &BoxCli, path: &str) -> BridgeResult<Detected> {
    let args = ["project", "detect", folder(path)?, "--json"];
    let bytes = cli.run_bytes(&args, Limit::Read).await?;
    parse_detected(&bytes)
}

/// `box project add <name> --repo <path> [options]`.
pub async fn add_project(cli: &BoxCli, addition: &Addition) -> BridgeResult<()> {
    let args = addition.args()?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    cli.run_text(&args, Limit::Action).await.map(drop)
}

fn not_a_value() -> BridgeError {
    BridgeError::invalid(Said::new("settings.invalid.value"))
}

fn plain(value: &str) -> BridgeResult<&str> {
    is_plain(value).then_some(value).ok_or_else(not_a_value)
}

/// An absolute path of a folder on this Mac, as an argument.
fn folder(path: &str) -> BridgeResult<&str> {
    let is_folder = path.starts_with('/') && path.len() <= PATH_MAX_BYTES;
    plain(path)
        .ok()
        .filter(|_| is_folder)
        .ok_or_else(not_a_value)
}

/// A folder inside a repository: relative, never climbing out.
fn subfolder(path: &str) -> BridgeResult<&str> {
    let climbs = path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..");
    let is_inside = !path.starts_with('-') && !climbs && path.len() <= PATH_MAX_BYTES;
    plain(path)
        .ok()
        .filter(|_| is_inside)
        .ok_or_else(not_a_value)
}

/// Patterns as `box` keeps them: one text, separated by spaces, so none holds one.
fn globs(patterns: &[String]) -> BridgeResult<String> {
    let is_pattern =
        |pattern: &String| !pattern.is_empty() && !pattern.contains(char::is_whitespace);
    let joined = patterns.join(" ");
    let is_sound = patterns.iter().all(is_pattern) && is_plain(&joined);
    is_sound.then_some(joined).ok_or_else(not_a_value)
}

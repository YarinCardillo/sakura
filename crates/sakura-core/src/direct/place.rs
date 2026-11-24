//! Where a task's files are, from the records the service keeps beside its data, which no container
//! can write (`$AGENTBOX_HOME/tasks/<task>.env`, `$AGENTBOX_HOME/projects/<project>.env`). A task's
//! files are `$AGENTBOX_HOME/work/<task>` in both places: a worktree of the person's repository on the
//! Mac, the folder bind mounted into the box otherwise. The records are read, never run, and only the
//! keys below are used.

use super::git::Oid;
use super::root::{not_found, Root};
use super::stat::Identity;
use crate::error::{BridgeError, BridgeResult};
use crate::models::Place;
use crate::said::Said;
use crate::validate;
use std::path::{Path, PathBuf};

/// The largest record read.
const RECORD_MAX_BYTES: u64 = 64 * 1024;

/// What the service recorded of one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub name: String,
    pub project: String,
    pub place: Place,
    /// The folder of its files.
    pub folder: PathBuf,
    /// The commit it left its base branch at.
    pub base: Oid,
    pub base_branch: Option<String>,
    /// On the Mac: the git directory of its worktree and the folder's identity when it was made.
    pub git_dir: Option<PathBuf>,
    pub identity: Option<Identity>,
}

/// What the service recorded of one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub name: String,
    /// The person's repository.
    pub repository: PathBuf,
    pub base_branch: String,
}

impl TaskRecord {
    /// The record of `task` under the service's data folder `home`.
    pub fn read(home: &Path, task: &str) -> BridgeResult<Self> {
        let task = validate::task(task)?;
        let fields = record(&home.join("tasks"), &format!("{task}.env"))?;
        let get = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        let place = match get("WHERE").as_deref() {
            Some("mac") => Place::Mac,
            _ => Place::Box,
        };
        let base = get("BASE_SHA").as_deref().and_then(Oid::from_hex);
        Ok(Self {
            name: task.to_string(),
            project: get("PROFILE").unwrap_or_default(),
            place,
            folder: home.join("work").join(task),
            base: base.ok_or_else(|| unreadable("the task's record has no base commit"))?,
            base_branch: get("BASE").filter(|branch| !branch.is_empty()),
            git_dir: get("WORKTREE_GIT_DIR")
                .filter(|dir| dir.starts_with('/'))
                .map(PathBuf::from),
            identity: get("WORKTREE_IDENTITY")
                .as_deref()
                .and_then(Identity::parse),
        })
    }

    /// The records of every task of `project`.
    pub fn of_project(home: &Path, project: &str) -> BridgeResult<Vec<Self>> {
        let folder = Root::metadata(&home.join("tasks"))?;
        let mut names: Vec<String> = folder
            .dir("")?
            .list()?
            .entries
            .into_iter()
            .filter_map(|(name, _)| name.strip_suffix(".env").map(str::to_string))
            .filter(|name| validate::task(name).is_ok())
            .collect();
        names.sort();
        Ok(names
            .iter()
            .filter_map(|name| Self::read(home, name).ok())
            .filter(|record| record.project == project)
            .collect())
    }
}

impl ProjectRecord {
    /// The record of `project` under the service's data folder `home`.
    pub fn read(home: &Path, project: &str) -> BridgeResult<Self> {
        let project = crate::graph::name(project)?;
        let fields = record(&home.join("projects"), &format!("{project}.env"))?;
        let get = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        let repository = get("REPO").filter(|repo| repo.starts_with('/'));
        Ok(Self {
            name: project.to_string(),
            repository: PathBuf::from(
                repository.ok_or_else(|| unreadable("the project has no repository"))?,
            ),
            base_branch: get("BASE").ok_or_else(|| unreadable("the project has no base branch"))?,
        })
    }
}

/// The `KEY=value` lines of a record; comments and blank lines skipped.
fn record(folder: &Path, name: &str) -> BridgeResult<Vec<(String, String)>> {
    let root = Root::metadata(folder).map_err(|_| not_found())?;
    let bytes = root.read(name, RECORD_MAX_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| unreadable("a record is not text"))?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect())
}

fn unreadable(why: &str) -> BridgeError {
    BridgeError::failed(Said::new("direct.unreadable")).with_details(Some(why.to_string()))
}

//! The readers of the tasks and projects the window shows, kept open between reads: opening one reads
//! its pack indexes, a read reuses them. A task's record is read again on every call, and a reader whose
//! task was replaced (another record, another folder) is opened again.

use super::git::{self, Oid, Repo};
use super::place::{ProjectRecord, TaskRecord};
use super::task::TaskReader;
use super::time::iso;
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::graph::{Commit as GraphCommit, Graph, Lane};
use crate::models::Place;
use crate::said::Said;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// The commits of the base branch the graph shows unless requested another number, as `box graph`.
const GRAPH_DEFAULT_LIMIT: u32 = 100;
const GRAPH_MAX_LIMIT: u32 = 1000;

/// Readers by task and repositories by project, for one data folder of the service.
pub struct Readers {
    home: PathBuf,
    tasks: Mutex<HashMap<String, Arc<TaskReader>>>,
    projects: Mutex<HashMap<String, (ProjectRecord, Arc<Repo>)>>,
}

impl Readers {
    /// The readers of the tasks under the service's data folder `home`.
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            tasks: Mutex::default(),
            projects: Mutex::default(),
        }
    }

    /// Runs `read` on the reader of `task`. A reader that fails because its folder was replaced is
    /// opened again once.
    pub fn with_task<T>(
        &self,
        task: &str,
        read: impl Fn(&TaskReader) -> BridgeResult<T>,
    ) -> BridgeResult<T> {
        let reader = self.task(task)?;
        match read(&reader) {
            Err(error) if is_stale(&error) => {
                self.forget(task);
                read(&*self.task(task)?)
            }
            answer => answer,
        }
    }

    /// The graph of a project (every task of it as a lane) or of one task, as `box graph` answered.
    pub fn graph(&self, of: &str, limit: Option<u32>) -> BridgeResult<Graph> {
        let limit = limit
            .unwrap_or(GRAPH_DEFAULT_LIMIT)
            .clamp(1, GRAPH_MAX_LIMIT) as usize;
        let (project, tasks, base) = match ProjectRecord::read(&self.home, of) {
            Ok(project) => {
                let tasks = TaskRecord::of_project(&self.home, of)?;
                let base = project.base_branch.clone();
                (project, tasks, base)
            }
            Err(_) => {
                let task = TaskRecord::read(&self.home, of)?;
                let project = ProjectRecord::read(&self.home, &task.project)?;
                let base = task
                    .base_branch
                    .clone()
                    .unwrap_or_else(|| project.base_branch.clone());
                (project, vec![task], base)
            }
        };
        let repo = self.project(&project)?;
        let tip = repo
            .resolve(&format!("refs/heads/{base}"))?
            .ok_or_else(|| missing_branch(&base))?;
        let mut lanes = vec![Lane {
            lavoro: None,
            branch: base.clone(),
            place: None,
            head: Some(tip.hex()),
            base: None,
            readable: true,
        }];
        let mut groups = vec![(None, git::latest(&repo, tip, limit)?)];
        for task in tasks {
            let (lane, commits) = self.lane(&task);
            lanes.push(lane);
            groups.push((Some(task.name), commits));
        }
        Ok(Graph {
            project: project.name,
            base,
            commits: assemble(&lanes, groups),
            lanes,
        })
    }

    fn task(&self, task: &str) -> BridgeResult<Arc<TaskReader>> {
        let record = TaskRecord::read(&self.home, task)?;
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(reader) = tasks.get(task).filter(|reader| *reader.record() == record) {
            return Ok(reader.clone());
        }
        let reader = Arc::new(TaskReader::open(record)?);
        tasks.insert(task.to_string(), reader.clone());
        Ok(reader)
    }

    fn forget(&self, task: &str) {
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(task);
    }

    fn project(&self, project: &ProjectRecord) -> BridgeResult<Arc<Repo>> {
        let mut projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((record, repo)) = projects.get(&project.name) {
            if record == project {
                return Ok(repo.clone());
            }
        }
        let repo = Arc::new(Repo::of_repository(&project.repository)?);
        projects.insert(project.name.clone(), (project.clone(), repo.clone()));
        Ok(repo)
    }

    /// The lane of one task and its commits ahead of its base; a task that cannot be read is a lane
    /// without commits.
    fn lane(&self, task: &TaskRecord) -> (Lane, Vec<(Oid, git::Commit)>) {
        let read = self.with_task(&task.name, |reader| {
            let head = reader.repo().head()?;
            let branch = match task.place {
                Place::Mac => reader
                    .repo()
                    .head_branch()?
                    .unwrap_or_else(|| "HEAD".into()),
                Place::Box => format!("box/{}", task.name),
            };
            Ok((head, branch, reader.ahead()?))
        });
        let (head, branch, commits, readable) = match read {
            Ok((head, branch, commits)) => (head, branch, commits, head.is_some()),
            Err(_) => (None, format!("box/{}", task.name), Vec::new(), false),
        };
        let lane = Lane {
            lavoro: Some(task.name.clone()),
            branch,
            place: Some(place_word(task.place).into()),
            head: head.map(|head| head.hex()),
            base: Some(task.base.hex()),
            readable,
        };
        (lane, commits)
    }
}

/// The commits of one lane, with the task they belong to (none for the base).
type Group = (Option<String>, Vec<(Oid, git::Commit)>);

/// Every commit once, newest first: one of the base belongs to no task, each named by the branches
/// whose lane ends at it.
fn assemble(lanes: &[Lane], groups: Vec<Group>) -> Vec<GraphCommit> {
    let mut by_sha: HashMap<String, GraphCommit> = HashMap::new();
    for (lavoro, commits) in groups {
        for (oid, commit) in commits {
            let sha = oid.hex();
            let entry = by_sha.entry(sha.clone()).or_insert_with(|| GraphCommit {
                sha: sha.clone(),
                parents: commit.parents.iter().map(Oid::hex).collect(),
                subject: commit.subject.clone(),
                time: iso(commit.time),
                lavoro: lavoro.clone(),
                branches: Vec::new(),
            });
            if lavoro.is_none() {
                entry.lavoro = None;
            }
        }
    }
    for lane in lanes {
        let at = lane.head.as_ref().and_then(|head| by_sha.get_mut(head));
        if let Some(commit) = at {
            commit.branches.push(lane.branch.clone());
        }
    }
    let mut commits: Vec<GraphCommit> = by_sha.into_values().collect();
    commits.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.sha.cmp(&b.sha)));
    commits
}

fn place_word(place: Place) -> &'static str {
    match place {
        Place::Mac => "mac",
        Place::Box => "box",
    }
}

/// Whether a read failed because the task's folder or repository is not the one the reader opened.
fn is_stale(error: &BridgeError) -> bool {
    error.code == ErrorCode::NotFound || error.said.key.0 == "direct.repository"
}

fn missing_branch(branch: &str) -> BridgeError {
    BridgeError::new(ErrorCode::NotFound, Said::new("direct.not_found"))
        .with_details(Some(branch.to_string()))
}

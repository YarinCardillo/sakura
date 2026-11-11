//! The projects a lavoro can be made in, and the branches each can start from: read when they are
//! needed (when the window has no lavoro to show, when "New task" opens), never at rest. A `box` that
//! has no projects is told apart from one whose answer failed.

use super::source::Source;
use gpui::{Context, Task};
use sakura_core::error::BridgeError;
use sakura_core::projects::Project;
use std::collections::HashMap;

/// What is known of the projects.
#[derive(Debug, Clone, PartialEq)]
pub enum Listed {
    /// Not asked yet, or the first answer has not come.
    Unknown,
    /// This `box` is older than projects: it has none of their verbs.
    Unsupported,
    Loaded(Vec<Project>),
    /// The read failed; what `box` said.
    Failed(BridgeError),
}

/// The projects and their branches.
pub struct Projects {
    source: Source,
    listed: Listed,
    /// The branches of each project requested, the project's own first.
    branches: HashMap<String, Vec<String>>,
    _reading: Option<Task<()>>,
}

impl Projects {
    /// Nothing read yet.
    pub fn new(source: Source) -> Self {
        Self {
            source,
            listed: Listed::Unknown,
            branches: HashMap::new(),
            _reading: None,
        }
    }

    /// What is known of the projects.
    pub fn listed(&self) -> &Listed {
        &self.listed
    }

    /// The project `name`, when it is listed.
    pub fn project(&self, name: &str) -> Option<&Project> {
        match &self.listed {
            Listed::Loaded(projects) => projects.iter().find(|project| project.name == name),
            _ => None,
        }
    }

    /// The projects a lavoro can be made in: those `box` can use.
    pub fn usable(&self) -> Vec<&Project> {
        match &self.listed {
            Listed::Loaded(projects) => projects.iter().filter(|p| p.error.is_none()).collect(),
            _ => Vec::new(),
        }
    }

    /// The branches of `project` a lavoro can start from, once read.
    pub fn branches(&self, project: &str) -> Option<&[String]> {
        self.branches.get(project).map(Vec::as_slice)
    }

    /// Reads the projects (again). The last list stays until `box` answers.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let pending = self.source.projects();
        self._reading = Some(cx.spawn(async move |this, cx| {
            let listed = match pending.await {
                Ok(Some(projects)) => Listed::Loaded(projects),
                Ok(None) => Listed::Unsupported,
                Err(error) => Listed::Failed(error),
            };
            drop(this.update(cx, |projects, cx| {
                projects.listed = listed;
                cx.notify();
            }));
        }));
    }

    /// Reads the projects when they were never requested.
    pub fn load_once(&mut self, cx: &mut Context<Self>) {
        if self.listed == Listed::Unknown && self._reading.is_none() {
            self.load(cx);
        }
    }

    /// Reads the branches of `project` (again); a failure leaves what was known and is said on stderr:
    /// the project's own base can always be chosen.
    pub fn load_branches(&mut self, project: String, cx: &mut Context<Self>) {
        let pending = self.source.branches(project.clone());
        cx.spawn(async move |this, cx| {
            let read = pending.await;
            drop(this.update(cx, |projects, cx| {
                match read {
                    Ok(branches) => {
                        let names = branches.branches.into_iter().map(|branch| branch.name);
                        projects.branches.insert(project, names.collect());
                    }
                    Err(error) => eprintln!("sakura: branches of {project}: {:?}", error.details),
                }
                cx.notify();
            }));
        })
        .detach();
    }
}

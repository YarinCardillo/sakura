//! Adding a project: the Mac's folder panel, then the engine's `add-project` for the folder chosen. The
//! engine decides what a project is; when it refuses, its words are kept here for the views that offer the
//! control (the Waiting page's empty state) to say under it, until the next try.

use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{say, tr, tr_with, Key};
#[cfg(feature = "snapshot")]
use gpui::{App, Global};
use gpui::{Context, Entity, PathPromptOptions, SharedString, Task as GpuiTask, Window};
use sakura_core::engine::cli::ActResult;
use std::path::Path;
#[cfg(feature = "snapshot")]
use std::path::PathBuf;

/// The panel's button, and what a person is asked to choose.
const CHOOSE: Key = Key("projects.choose_folder");
/// What is said when the engine answered without adding the project and without a reason of its own.
const NOT_ADDED: Key = Key("project_add.not_added");

/// The folder the next folder panel returns instead of being shown (a scripted run of a snapshot build).
#[cfg(feature = "snapshot")]
struct NextFolder(PathBuf);

#[cfg(feature = "snapshot")]
impl Global for NextFolder {}

/// Makes the next folder panel answer `folder` without being shown, for the scripted steps.
#[cfg(feature = "snapshot")]
pub fn pick_next_folder(folder: PathBuf, cx: &mut App) {
    cx.set_global(NextFolder(folder));
}

/// The folder a scripted run chose for the panel, once.
#[cfg(feature = "snapshot")]
fn take_next_folder(cx: &mut App) -> Option<PathBuf> {
    cx.try_global::<NextFolder>()?;
    Some(cx.remove_global::<NextFolder>().0)
}

/// Where on screen the person asked to add a project: the refusal is said there and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The empty page that waits for a first project.
    Waiting,
    /// The sidebar's heading and its empty line.
    Sidebar,
    /// The sheet of "New lane" that asks for a project first.
    NewLane,
    /// The Projects page of Settings.
    Settings,
}

/// The state of adding a project, shared by the views that offer it.
pub struct ProjectAdds {
    source: Source,
    facts: Entity<EngineFacts>,
    refusal: Option<SharedString>,
    /// Where the person asked, and so where the refusal is said.
    place: Option<Place>,
    /// The folder last tried, so that a refusal can offer to make it a repository.
    folder: Option<String>,
    can_init: bool,
    is_adding: bool,
    _work: Option<GpuiTask<()>>,
}

impl ProjectAdds {
    /// Adds through `source`; `facts` are read again when a project was added.
    pub fn new(source: Source, facts: Entity<EngineFacts>) -> Self {
        Self {
            source,
            facts,
            refusal: None,
            place: None,
            folder: None,
            can_init: false,
            is_adding: false,
            _work: None,
        }
    }

    /// What the engine said of the last folder it did not add, when the person asked from `place`; none after
    /// one it added, and none anywhere else.
    pub fn refusal_in(&self, place: Place) -> Option<&SharedString> {
        self.refusal.as_ref().filter(|_| self.place == Some(place))
    }

    /// The folder the last refusal was about, when it is a folder with no Git repository in it: what the
    /// person may ask to make one of. Nothing is made unasked.
    pub fn could_be_made_a_repository(&self) -> Option<&str> {
        self.folder.as_deref().filter(|_| self.can_init)
    }

    /// The person asked: `git init` in the folder last refused, then it is added again.
    pub fn make_a_repository_and_add(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.could_be_made_a_repository().map(str::to_string) else {
            return;
        };
        self.can_init = false;
        self.is_adding = true;
        let made = folder.clone();
        let init = cx.background_executor().spawn(async move {
            if !can_initialize(Path::new(&made)) {
                return (Ok(None), false);
            }
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&made)
                .arg("init")
                .stdin(std::process::Stdio::null())
                .output()
                .map(Some);
            (result, can_initialize(Path::new(&made)))
        });
        self._work = Some(cx.spawn(async move |adds, cx| {
            let (made, can_init) = init.await;
            drop(adds.update(cx, |adds, cx| {
                adds.is_adding = false;
                adds.can_init = can_init;
                match made {
                    Ok(Some(output)) if output.status.success() => return adds.add(folder, cx),
                    Ok(None) => {}
                    _ => {
                        adds.refusal = Some(tr(Key("project_add.init_failed")).into());
                    }
                }
                cx.notify();
            }));
        }));
    }

    /// Opens the Mac's folder panel; the folder chosen is added.
    pub fn choose(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_adding {
            return;
        }
        self.place = Some(place);
        #[cfg(feature = "snapshot")]
        if let Some(folder) = take_next_folder(cx) {
            return self.add(folder.to_string_lossy().into_owned(), cx);
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(tr(CHOOSE).into()),
        });
        self._work = Some(cx.spawn_in(window, async move |adds, cx| {
            let folder = match chosen.await {
                Ok(Ok(Some(folders))) => folders.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    eprintln!("sakura: the folder picker could not open: {error}");
                    None
                }
            };
            let Some(folder) = folder else {
                return;
            };
            let folder = folder.to_string_lossy().into_owned();
            drop(adds.update(cx, |adds, cx| adds.add(folder, cx)));
        }));
    }

    /// Adds `folder` as if the person had chosen it from `place`, for the tests.
    #[cfg(test)]
    pub fn add_from(&mut self, place: Place, folder: String, cx: &mut Context<Self>) {
        self.place = Some(place);
        self.add(folder, cx);
    }

    /// Asks the engine to register the repository of `folder`.
    pub fn add(&mut self, folder: String, cx: &mut Context<Self>) {
        self.is_adding = true;
        self.refusal = None;
        self.can_init = false;
        self.folder = Some(folder.clone());
        let pending = self.source.add_project(folder.clone());
        self._work = Some(cx.spawn(async move |adds, cx| {
            let result = pending.await;
            let can_init = if matches!(result, Ok(ActResult::Done { exit_code: 0, .. })) {
                false
            } else {
                cx.background_executor()
                    .spawn(async move { can_initialize(Path::new(&folder)) })
                    .await
            };
            drop(adds.update(cx, |adds, cx| {
                adds.can_init = can_init;
                adds.answered(result, cx);
            }));
        }));
        cx.notify();
    }

    fn answered(
        &mut self,
        result: sakura_core::error::BridgeResult<ActResult>,
        cx: &mut Context<Self>,
    ) {
        self.is_adding = false;
        self.refusal = match result {
            Ok(ActResult::Done { exit_code: 0, .. }) => None,
            Ok(ActResult::Done { exit_code, .. }) => Some(failed(exit_code)),
            Ok(ActResult::Refused { reason, .. }) => Some(sentence(&reason).into()),
            Ok(ActResult::Confirm { .. }) => Some(tr(NOT_ADDED).into()),
            Err(error) => Some(say(&error.said).to_string().into()),
        };
        self.facts.update(cx, |facts, cx| facts.read(cx));
        cx.notify();
    }
}

fn can_initialize(path: &Path) -> bool {
    path.is_dir() && !path.join(".git").exists()
}

/// `words` as a sentence: the engine's reasons start in lower case, and a sentence on screen starts with a capital.
pub(super) fn sentence(words: &str) -> String {
    let mut letters = words.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

fn failed(code: i64) -> SharedString {
    tr_with(Key("waiting.failed"), &[("code", &code.to_string())])
}

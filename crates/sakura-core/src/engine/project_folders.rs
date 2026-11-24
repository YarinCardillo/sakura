use super::snapshot::Project;
use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use tokio::sync::watch::{self, Receiver, Sender};

/// Watches containing directories so a missing project's name can be seen when it returns.
pub struct ProjectFolders {
    folders: HashMap<PathBuf, bool>,
    directories: HashMap<PathBuf, Directory>,
    changed: Sender<()>,
}

struct Directory {
    identity: (u64, u64),
    _watch: native::Watch,
}

impl ProjectFolders {
    /// Creates a monitor and its event signal. Directory watches start when projects are supplied.
    pub fn new() -> (Self, Receiver<()>) {
        let (changed, events) = watch::channel(());
        (
            Self {
                folders: HashMap::new(),
                directories: HashMap::new(),
                changed,
            },
            events,
        )
    }

    /// Tracks the snapshot's projects. Reports if their existence changed during the read.
    pub fn replace(&mut self, projects: &[Project]) -> bool {
        self.folders
            .retain(|folder, _| projects.iter().any(|p| folder == Path::new(&p.folder)));
        for project in projects {
            let folder = Path::new(&project.folder);
            if folder.is_absolute() && folder.parent().is_some() {
                self.folders
                    .entry(folder.into())
                    .or_insert(!project.folder_missing);
            }
        }
        self.refresh()
    }

    /// Checks existence on directory events and reattaches when a containing directory moves.
    pub fn refresh(&mut self) -> bool {
        let mut has_changed = false;
        let mut parents = HashSet::new();
        for (folder, was_present) in &mut self.folders {
            if let Some((path, identity)) = watch_parent(folder) {
                if parents.insert(path) {
                    attach(&mut self.directories, path, identity, &self.changed);
                }
            }
            let is_present = folder.is_dir();
            has_changed |= std::mem::replace(was_present, is_present) != is_present;
        }
        self.directories
            .retain(|parent, _| parents.contains(parent.as_path()));
        has_changed
    }
}

fn watch_parent(folder: &Path) -> Option<(&Path, (u64, u64))> {
    folder.ancestors().skip(1).find_map(|path| {
        let metadata = path.metadata().ok()?;
        metadata
            .is_dir()
            .then(|| (path, directory_identity(&metadata)))
    })
}

fn attach(
    directories: &mut HashMap<PathBuf, Directory>,
    path: &Path,
    identity: (u64, u64),
    changed: &Sender<()>,
) {
    if directories
        .get(path)
        .is_some_and(|directory| directory.identity == identity)
    {
        return;
    }
    directories.remove(path);
    match native::Watch::start(path, changed.clone()) {
        Ok(watch) => {
            directories.insert(
                path.to_path_buf(),
                Directory {
                    identity,
                    _watch: watch,
                },
            );
        }
        Err(error) => eprintln!(
            "sakura: cannot watch project parent {}: {error}",
            path.display()
        ),
    }
}

#[cfg(unix)]
fn directory_identity(metadata: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn directory_identity(_: &Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::RcBlock;
    use dispatch2::{
        _dispatch_source_type_vnode, dispatch_source_vnode_flags_t as Flags, DispatchObject,
        DispatchRetained, DispatchSource,
    };
    use std::cell::Cell;
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    pub(super) struct Watch(DispatchRetained<DispatchSource>);

    impl Watch {
        pub(super) fn start(path: &Path, changed: Sender<()>) -> std::io::Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_EVTONLY)
                .open(path)?;
            let flags = Flags::DISPATCH_VNODE_WRITE.0
                | Flags::DISPATCH_VNODE_DELETE.0
                | Flags::DISPATCH_VNODE_RENAME.0
                | Flags::DISPATCH_VNODE_REVOKE.0;
            // The cancellation handler owns the descriptor until dispatch finishes using it.
            let source = unsafe {
                DispatchSource::new(
                    std::ptr::addr_of!(_dispatch_source_type_vnode).cast_mut(),
                    file.as_raw_fd() as usize,
                    flags as usize,
                    None,
                )
            };
            let changed = RcBlock::new(move || {
                let _ = changed.send(());
            });
            let file = Cell::new(Some(file));
            let cancel = RcBlock::new(move || drop(file.take()));
            // Dispatch copies the blocks and runs the handlers serially for this source.
            unsafe {
                source.set_event_handler_with_block(RcBlock::as_ptr(&changed));
                source.set_registration_handler_with_block(RcBlock::as_ptr(&changed));
                source.set_cancel_handler_with_block(RcBlock::as_ptr(&cancel));
            }
            source.activate();
            Ok(Self(source))
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

    pub(super) struct Watch {
        _watcher: RecommendedWatcher,
    }

    impl Watch {
        pub(super) fn start(path: &Path, changed: Sender<()>) -> notify::Result<Self> {
            let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
                if !matches!(event, Ok(event) if matches!(event.kind, EventKind::Access(_))) {
                    let _ = changed.send(());
                }
            })?;
            watcher.watch(path, RecursiveMode::NonRecursive)?;
            Ok(Self { _watcher: watcher })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_sibling_projects_share_a_parent_watch() {
        let root = tempfile::tempdir().unwrap();
        let projects = ["project-a", "project-b"].map(|name| {
            let folder = root.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            Project {
                track_choices: Vec::new(),
                id: name.into(),
                name: name.into(),
                folder: folder.display().to_string(),
                branch: None,
                folder_missing: false,
                open_lanes: 0,
                actions: Vec::new(),
            }
        });
        let (mut monitor, _changes) = ProjectFolders::new();
        monitor.replace(&projects);
        assert_eq!(watch_count(&monitor), 1);
    }

    fn watch_count(monitor: &ProjectFolders) -> usize {
        monitor.directories.len()
    }
}

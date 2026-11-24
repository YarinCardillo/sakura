//! The confined reader against what an agent in a box can put in its folder.

use super::*;
use crate::error::ErrorCode;
use crate::files::{EntryKind, FileBody};
use std::fs;
use std::os::unix::fs::symlink;

fn names(root: &Root, dir: &str) -> Vec<(String, EntryKind)> {
    let mut entries = root.dir(dir).unwrap().list().unwrap().entries;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

#[test]
fn regular_text_binary_and_size_limit_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("text"), "hello\n").unwrap();
    fs::write(temp.path().join("binary"), [0, 1, 2]).unwrap();
    fs::File::create(temp.path().join("large"))
        .unwrap()
        .set_len(2_097_153)
        .unwrap();
    let root = Root::open(temp.path()).unwrap();
    assert_eq!(root.file("text").unwrap(), FileBody::Text("hello\n".into()));
    assert_eq!(root.file("binary").unwrap(), FileBody::Binary);
    assert_eq!(root.file("large").unwrap(), FileBody::TooLarge);
}

#[test]
fn links_and_traversal_out_of_the_task_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("task")).unwrap();
    fs::create_dir(temp.path().join("outside")).unwrap();
    fs::write(temp.path().join("outside/secret"), "private").unwrap();
    symlink("../outside", temp.path().join("task/dir")).unwrap();
    symlink("../outside/secret", temp.path().join("task/link")).unwrap();
    symlink(
        temp.path().join("outside/secret"),
        temp.path().join("task/absolute"),
    )
    .unwrap();
    symlink("task", temp.path().join("alias")).unwrap();
    let root = Root::open(&temp.path().join("task")).unwrap();
    for path in [
        "link",
        "dir/secret",
        "absolute",
        "../outside/secret",
        "/etc/passwd",
        "a/../../x",
        "a\0b",
    ] {
        assert!(root.file(path).is_err(), "{path:?}");
    }
    assert!(root.dir("dir").is_err());
    assert!(Root::open(&temp.path().join("alias")).is_err());
    assert!(Root::open(&temp.path().join("alias/dir")).is_err());
    let entries = names(&root, "");
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|(_, kind)| *kind == EntryKind::Link));
}

#[test]
fn a_link_inside_the_task_is_walked_inside_it_only() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/file"), "hello").unwrap();
    fs::create_dir(temp.path().join(".git")).unwrap();
    fs::write(temp.path().join(".git/config"), "[core]").unwrap();
    symlink("src/file", temp.path().join("link")).unwrap();
    symlink("../src", temp.path().join("src/up")).unwrap();
    symlink(".git/config", temp.path().join("meta")).unwrap();
    symlink("loop-b", temp.path().join("loop-a")).unwrap();
    symlink("loop-a", temp.path().join("loop-b")).unwrap();
    let root = Root::open(temp.path()).unwrap();
    assert_eq!(root.file("link").unwrap(), FileBody::Text("hello".into()));
    assert_eq!(
        root.file("src/up/file").unwrap(),
        FileBody::Text("hello".into())
    );
    assert!(root.file("meta").is_err(), "a link into the git metadata");
    assert!(root.file("loop-a").is_err(), "a loop of links ends");
}

#[test]
fn git_metadata_roots_refuse_every_link() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("refs")).unwrap();
    fs::write(temp.path().join("HEAD"), "ref: refs/heads/main\n").unwrap();
    symlink("HEAD", temp.path().join("refs/inside")).unwrap();
    let root = Root::metadata(temp.path()).unwrap();
    assert_eq!(root.read("HEAD", 100).unwrap(), b"ref: refs/heads/main\n");
    assert!(root.read("refs/inside", 100).is_err());
    assert!(root.read("HEAD", 4).is_err(), "over its limit");
}

#[test]
fn special_files_are_refused_without_blocking() {
    let temp = tempfile::tempdir().unwrap();
    nix::unistd::mkfifo(
        &temp.path().join("fifo"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    fs::create_dir(temp.path().join("folder")).unwrap();
    let root = Root::open(temp.path()).unwrap();
    assert!(root.file("fifo").is_err());
    assert!(root.file("folder").is_err());
    assert!(root.file("").is_err());
    assert!(names(&root, "").contains(&("fifo".into(), EntryKind::Other)));
}

#[test]
fn an_entry_name_never_climbs_out_of_its_folder() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("task")).unwrap();
    fs::write(temp.path().join("secret"), "private").unwrap();
    let root = Root::open(&temp.path().join("task")).unwrap();
    let top = root.dir("").unwrap();
    for name in ["..", ".", "", "a/b"] {
        assert!(top.stat_child(name).is_none(), "{name:?}");
        assert!(
            top.child(name).map_or(true, |dir| dir.is_none()),
            "{name:?}"
        );
        assert!(top.read(name, 100).is_err(), "{name:?}");
    }
}

#[test]
fn replacing_a_task_root_invalidates_its_reader() {
    let temp = tempfile::tempdir().unwrap();
    let task = temp.path().join("task");
    fs::create_dir(&task).unwrap();
    fs::write(task.join("file"), "old").unwrap();
    let root = Root::open(&task).unwrap();
    fs::rename(&task, temp.path().join("old")).unwrap();
    fs::create_dir(&task).unwrap();
    fs::write(task.join("file"), "new").unwrap();
    assert!(root.file("file").is_err());
    assert!(root.dir("").is_err());
    let other = Root::open(&task).unwrap();
    assert!(Root::open_expecting(&task, root.identity()).is_err());
    assert!(Root::open_expecting(&task, other.identity()).is_ok());
    fs::remove_dir_all(&task).unwrap();
    symlink(temp.path().join("old"), &task).unwrap();
    let error = other.file("file").unwrap_err();
    assert_ne!(error.code, ErrorCode::Internal);
}

#[test]
fn listing_keeps_raw_names_and_hides_the_top_git_only() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..2005 {
        fs::write(temp.path().join(format!("file-{i}")), "").unwrap();
    }
    fs::write(temp.path().join("line\nbreak"), "").unwrap();
    fs::create_dir_all(temp.path().join("sub/.git")).unwrap();
    fs::create_dir(temp.path().join(".git")).unwrap();
    let root = Root::open(temp.path()).unwrap();
    let entries = names(&root, "");
    assert_eq!(entries.len(), 2007);
    assert!(entries.iter().any(|(name, _)| name == "line\nbreak"));
    assert!(!entries.iter().any(|(name, _)| name == ".git"));
    assert_eq!(names(&root, "sub"), vec![(".git".into(), EntryKind::Dir)]);
}

#[test]
fn paths_and_depth_are_bounded_and_metadata_is_hidden() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".git")).unwrap();
    fs::write(temp.path().join(".git/config"), "private metadata").unwrap();
    let root = Root::open(temp.path()).unwrap();
    assert!(root.file(".git/config").is_err());
    assert!(root.dir(".git").is_err());
    assert!(root.file(&"x/".repeat(65)).is_err());
    assert!(root.file(&"x".repeat(4097)).is_err());
    assert!(names(&root, "").is_empty());
}

//! The in-process git reader against git itself (the same patches, commits and marks `git` gives) and
//! against what an agent in a box can do to its repository. git is run here only to build fixtures.

use super::*;
use crate::files::{EntryKind, FileBody};
use crate::patch::{Change as PatchChange, Patch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs git for a fixture, with no configuration of this Mac.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A service data folder with one project whose repository has a few commits on `main`.
struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    source: PathBuf,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        Self::under("")
    }

    /// The same, in a folder of the given name inside the temporary one (a name may hold a space).
    fn under(folder: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap().join(folder);
        let (home, source) = (root.join("home"), root.join("source"));
        for folder in ["work", "tasks", "projects"] {
            fs::create_dir_all(home.join(folder)).unwrap();
        }
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q"]);
        fs::create_dir(source.join("src")).unwrap();
        fs::write(
            source.join("src/lib.rs"),
            "fn one() {}\nfn two() {}\nfn three() {}\n",
        )
        .unwrap();
        fs::write(source.join("README.md"), "# readme\n").unwrap();
        fs::write(source.join("gone.txt"), "to be removed\n").unwrap();
        fs::write(source.join(".gitignore"), "target/\n*.log\n").unwrap();
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-q", "-m", "first"]);
        fs::write(source.join("README.md"), "# readme\nmore\n").unwrap();
        git(&source, &["commit", "-qam", "second"]);
        let base = git(&source, &["rev-parse", "HEAD"]).trim().to_string();
        let project = format!("REPO={}\nBASE=main\n", source.display());
        fs::write(home.join("projects/p.env"), project).unwrap();
        Self {
            _temp: temp,
            home,
            source,
            base,
        }
    }

    /// A task in a box: a clone of the repository in the work folder, on its own branch.
    fn box_task(&self, name: &str) -> PathBuf {
        let folder = self.home.join("work").join(name);
        git(
            &self.home.join("work"),
            &[
                "clone",
                "-q",
                "--no-hardlinks",
                self.source.to_str().unwrap(),
                name,
            ],
        );
        git(&folder, &["checkout", "-q", "-b", &format!("box/{name}")]);
        let record = format!(
            "PROFILE=p\nBASE=main\nBASE_SHA={}\nGENERATION=1\nWHERE=box\n",
            self.base
        );
        fs::write(self.home.join(format!("tasks/{name}.env")), record).unwrap();
        folder
    }

    /// A task on the Mac: a worktree of the repository, recorded as the service records it.
    fn mac_task(&self, name: &str) -> PathBuf {
        let folder = self.home.join("work").join(name);
        let branch = format!("sakura/{name}");
        git(
            &self.source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                folder.to_str().unwrap(),
                &self.base,
            ],
        );
        let pointer = fs::read_to_string(folder.join(".git")).unwrap();
        let git_dir = pointer.trim().strip_prefix("gitdir: ").unwrap().to_string();
        let record = format!(
            "PROFILE=p\nBASE=main\nBASE_SHA={}\nWHERE=mac\nWORKTREE_GIT_DIR={git_dir}\n",
            self.base
        );
        fs::write(self.home.join(format!("tasks/{name}.env")), record).unwrap();
        folder
    }

    fn readers(&self) -> Readers {
        Readers::new(self.home.clone())
    }
}

/// Some work by an agent: a commit, then changes not committed and a file git does not know.
fn work(folder: &Path) {
    let name = folder.file_name().unwrap().to_str().unwrap();
    let lib =
        format!("fn one() {{}}\nfn two_changed() {{}}\nfn three() {{}}\nfn four_{name}() {{}}\n");
    fs::write(folder.join("src/lib.rs"), lib).unwrap();
    git(folder, &["commit", "-qam", "agent: change lib"]);
    fs::write(folder.join("README.md"), "# readme\nmore\nand more\n").unwrap();
    fs::remove_file(folder.join("gone.txt")).unwrap();
    fs::write(folder.join("new.txt"), "new\nfile\n").unwrap();
    fs::write(folder.join("debug.log"), "ignored\n").unwrap();
    fs::create_dir(folder.join("target")).unwrap();
    fs::write(folder.join("target/out"), "ignored\n").unwrap();
}

/// (path, change, added, removed) of each file of a patch.
fn summary(patch: &str) -> Vec<(String, PatchChange, u32, u32)> {
    let parsed = Patch::parse(patch);
    assert!(parsed.is_complete, "{patch}");
    let mut files: Vec<_> = parsed
        .files
        .iter()
        .map(|f| (f.path.clone(), f.change, f.added, f.removed))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// git's own patch of the work tree against the base, untracked files included.
fn git_patch(folder: &Path, base: &str) -> String {
    git(folder, &["add", "-N", "new.txt"]);
    let patch = git(
        folder,
        &["diff", "--no-renames", "--no-ext-diff", "--no-color", base],
    );
    git(folder, &["reset", "-q", "new.txt"]);
    patch
}

fn assert_reads_like_git(fixture: &Fixture, task: &str, folder: &Path) {
    let readers = fixture.readers();
    let patch = readers.with_task(task, TaskReader::working_patch).unwrap();
    assert_eq!(summary(&patch), summary(&git_patch(folder, &fixture.base)));
    let level = readers.with_task(task, |reader| reader.level("")).unwrap();
    let entry = |name: &str| level.entries.iter().find(|e| e.name == name).cloned();
    assert_eq!(
        entry("README.md").map(|e| (e.changed, e.added, e.removed)),
        Some((true, Some(1), Some(0)))
    );
    assert_eq!(entry("gone.txt").map(|e| e.kind), Some(EntryKind::Deleted));
    assert_eq!(
        entry("new.txt").map(|e| (e.changed, e.added)),
        Some((true, Some(2)))
    );
    assert!(entry("src").unwrap().changed);
    assert!(
        entry("debug.log").is_none() && entry("target").is_none(),
        "ignored files are left out"
    );
    let log = readers.with_task(task, TaskReader::log).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(
        (
            log[0].subject.as_str(),
            log[0].files_changed,
            log[0].insertions,
            log[0].deletions
        ),
        ("agent: change lib", 1, 2, 1)
    );
    let commit = readers
        .with_task(task, |reader| reader.commit_patch(&log[0].sha))
        .unwrap();
    let git_commit = git(folder, &["show", "--format=", "--no-ext-diff", &log[0].sha]);
    assert_eq!(summary(&commit), summary(&git_commit));
    let found = readers
        .with_task(task, |reader| reader.search("LIB"))
        .unwrap();
    assert_eq!(found.matches, vec!["src/lib.rs".to_string()]);
    let found = readers
        .with_task(task, |reader| reader.search("."))
        .unwrap();
    assert!(
        found.matches.contains(&"new.txt".to_string())
            && !found.matches.contains(&"debug.log".to_string())
    );
    let file = readers
        .with_task(task, |reader| reader.file("new.txt"))
        .unwrap();
    assert_eq!(file, FileBody::Text("new\nfile\n".into()));
}

#[test]
fn a_box_task_reads_like_git_from_loose_objects() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    work(&folder);
    assert_reads_like_git(&fixture, "t1", &folder);
}

#[test]
fn a_box_task_reads_like_git_from_packs_with_deltas_and_an_index_of_version_4() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    work(&folder);
    git(&folder, &["repack", "-adfq", "--depth=50", "--window=50"]);
    git(&folder, &["pack-refs", "--all"]);
    git(&folder, &["update-index", "--index-version", "4"]);
    assert_reads_like_git(&fixture, "t1", &folder);
}

#[test]
fn a_lavoro_on_the_mac_reads_like_git_through_its_recorded_git_directory() {
    let fixture = Fixture::new();
    let folder = fixture.mac_task("m1");
    work(&folder);
    assert_reads_like_git(&fixture, "m1", &folder);
}

#[test]
fn a_lavoro_whose_git_file_names_another_directory_is_refused() {
    let fixture = Fixture::new();
    let folder = fixture.mac_task("m1");
    let other = fixture.home.join("other.git");
    fs::create_dir(&other).unwrap();
    fs::write(
        folder.join(".git"),
        format!("gitdir: {}\n", other.display()),
    )
    .unwrap();
    assert!(fixture
        .readers()
        .with_task("m1", |reader| reader.level(""))
        .is_err());
}

#[test]
fn the_graph_draws_the_base_and_every_task_of_the_project() {
    let fixture = Fixture::new();
    work(&fixture.box_task("t1"));
    work(&fixture.mac_task("m1"));
    let graph = fixture
        .readers()
        .graph("p", None)
        .unwrap()
        .checked()
        .unwrap();
    let branches: Vec<&str> = graph
        .lanes
        .iter()
        .map(|lane| lane.branch.as_str())
        .collect();
    assert_eq!(branches, ["main", "sakura/m1", "box/t1"]);
    assert_eq!(graph.commits.len(), 4);
    assert!(graph.lanes.iter().all(|lane| lane.readable));
    let of_task = fixture.readers().graph("t1", Some(1)).unwrap();
    assert_eq!((of_task.lanes.len(), of_task.commits.len()), (2, 2));
}

#[test]
fn the_graph_of_a_mac_task_with_no_commit_ahead_is_drawn_in_a_folder_with_a_space() {
    let fixture = Fixture::under("a folder");
    let folder = fixture.mac_task("m1");
    fs::write(folder.join("new.txt"), "new\nfile\n").unwrap();
    let graph = fixture
        .readers()
        .graph("p", None)
        .unwrap()
        .checked()
        .unwrap();
    let branches: Vec<&str> = graph
        .lanes
        .iter()
        .map(|lane| lane.branch.as_str())
        .collect();
    assert_eq!(branches, ["main", "sakura/m1"]);
    assert!(
        graph.lanes.iter().all(|lane| lane.readable),
        "{:?}",
        graph.lanes
    );
    assert_eq!(graph.commits.len(), 2);
}

#[test]
fn the_graph_of_a_repository_with_one_commit_and_a_task_still_on_it_is_drawn() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap().join("a folder");
    let (home, source) = (root.join("home"), root.join("repo"));
    for folder in ["work", "tasks", "projects"] {
        fs::create_dir_all(home.join(folder)).unwrap();
    }
    fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "-q"]);
    fs::write(source.join("README.md"), "# readme\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-q", "-m", "First commit"]);
    let base = git(&source, &["rev-parse", "HEAD"]).trim().to_string();
    let folder = home.join("work/m1");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "sakura/m1",
            folder.to_str().unwrap(),
            &base,
        ],
    );
    fs::write(folder.join("notes.txt"), "new\n").unwrap();
    let git_dir = format!("{}/.git/worktrees/m1", source.display());
    fs::write(
        home.join("tasks/m1.env"),
        format!(
            "PROFILE=repo\nBASE=main\nBASE_SHA={base}\nWHERE=mac\nWORKTREE_GIT_DIR={git_dir}\n"
        ),
    )
    .unwrap();
    fs::write(
        home.join("projects/repo.env"),
        format!("REPO={}\nBASE=main\n", source.display()),
    )
    .unwrap();
    let graph = Readers::new(home)
        .graph("repo", None)
        .unwrap()
        .checked()
        .unwrap();
    assert_eq!(graph.commits.len(), 1);
    assert_eq!(graph.commits[0].branches, ["main", "sakura/m1"]);
    let layout = crate::graph::Layout::of(&graph).unwrap();
    assert_eq!(layout.rows.len(), 1);
}

// What a rogue agent in the box can do.

/// A file a configured program would leave behind if git ran it.
fn ran(fixture: &Fixture) -> PathBuf {
    fixture.home.join("ran")
}

#[test]
fn nothing_the_workspace_configures_is_ever_run() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    work(&folder);
    let marker = ran(&fixture);
    let run = format!("touch {}", marker.display());
    let config = format!(
        "[core]\n\tfsmonitor = {run}\n\thooksPath = hooks\n\tpager = {run}\n[filter \"x\"]\n\tclean = {run}\n\tsmudge = {run}\n\tprocess = {run}\n[diff \"x\"]\n\ttextconv = {run}\n\tcommand = {run}\n[diff]\n\texternal = {run}\n"
    );
    let mut existing = fs::read_to_string(folder.join(".git/config")).unwrap();
    existing.push_str(&config);
    fs::write(folder.join(".git/config"), existing).unwrap();
    fs::write(folder.join(".gitattributes"), "* filter=x diff=x\n").unwrap();
    fs::create_dir_all(folder.join(".git/hooks")).unwrap();
    for hook in [
        "post-checkout",
        "pre-commit",
        "fsmonitor-watchman",
        "post-index-change",
    ] {
        fs::write(
            folder.join(".git/hooks").join(hook),
            format!("#!/bin/sh\n{run}\n"),
        )
        .unwrap();
    }
    let readers = fixture.readers();
    readers.with_task("t1", TaskReader::working_patch).unwrap();
    readers
        .with_task("t1", |reader| reader.level("src"))
        .unwrap();
    readers.with_task("t1", TaskReader::log).unwrap();
    readers
        .with_task("t1", |reader| reader.search("lib"))
        .unwrap();
    assert!(!marker.exists(), "a configured program ran");
}

#[test]
fn a_git_file_pointing_elsewhere_and_linked_metadata_are_refused() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    fs::rename(folder.join(".git"), fixture.home.join("moved.git")).unwrap();
    fs::write(
        folder.join(".git"),
        format!("gitdir: {}\n", fixture.home.join("moved.git").display()),
    )
    .unwrap();
    assert!(fixture
        .readers()
        .with_task("t1", |reader| reader.level(""))
        .is_err());
    fs::remove_file(folder.join(".git")).unwrap();
    std::os::unix::fs::symlink(fixture.home.join("moved.git"), folder.join(".git")).unwrap();
    assert!(fixture
        .readers()
        .with_task("t1", |reader| reader.level(""))
        .is_err());
    fs::remove_file(folder.join(".git")).unwrap();
    fs::rename(fixture.home.join("moved.git"), folder.join(".git")).unwrap();
    let objects = folder.join(".git/objects");
    fs::rename(&objects, fixture.home.join("objects")).unwrap();
    std::os::unix::fs::symlink(fixture.home.join("objects"), &objects).unwrap();
    assert!(fixture
        .readers()
        .with_task("t1", |reader| reader.level(""))
        .is_err());
}

#[test]
fn alternates_and_linked_packs_lead_nowhere() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    git(&folder, &["repack", "-adq"]);
    let pack_dir = folder.join(".git/objects/pack");
    let outside = fixture.home.join("outside-pack");
    fs::rename(&pack_dir, &outside).unwrap();
    fs::create_dir(&pack_dir).unwrap();
    for entry in fs::read_dir(&outside).unwrap() {
        let entry = entry.unwrap();
        std::os::unix::fs::symlink(entry.path(), pack_dir.join(entry.file_name())).unwrap();
    }
    fs::write(
        folder.join(".git/objects/info/alternates"),
        format!("{}\n", fixture.source.join(".git/objects").display()),
    )
    .unwrap();
    let error = fixture
        .readers()
        .with_task("t1", TaskReader::working_patch)
        .unwrap_err();
    assert_eq!(
        error.said.key.0, "direct.repository",
        "the objects behind links and alternates were read"
    );
}

#[test]
fn links_planted_in_the_work_tree_show_their_target_never_what_is_outside() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    let secret = fixture.home.join("secret");
    fs::write(&secret, "PRIVATE KEY\n").unwrap();
    fs::remove_file(folder.join("README.md")).unwrap();
    std::os::unix::fs::symlink(&secret, folder.join("README.md")).unwrap();
    fs::rename(folder.join("src"), fixture.home.join("src-outside")).unwrap();
    std::os::unix::fs::symlink(fixture.home.join("src-outside"), folder.join("src")).unwrap();
    std::os::unix::fs::symlink(&secret, folder.join("planted")).unwrap();
    let readers = fixture.readers();
    let patch = readers.with_task("t1", TaskReader::working_patch).unwrap();
    assert!(!patch.contains("PRIVATE KEY"), "{patch}");
    assert!(
        patch.contains(&format!("+{}", secret.display())),
        "a link is shown as its target"
    );
    let files = summary(&patch);
    assert!(files
        .iter()
        .any(|(path, change, ..)| path == "src/lib.rs" && *change == PatchChange::Deleted));
    assert!(readers
        .with_task("t1", |reader| reader.file("planted"))
        .is_err());
    assert!(readers
        .with_task("t1", |reader| reader.level("src"))
        .is_err());
}

#[test]
fn oversized_files_and_objects_are_said_not_read() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    let big = "x".repeat(3 * 1024 * 1024);
    fs::write(folder.join("big.txt"), &big).unwrap();
    fs::write(folder.join("README.md"), &big).unwrap();
    let patch = fixture
        .readers()
        .with_task("t1", TaskReader::working_patch)
        .unwrap();
    let parsed = Patch::parse(&patch);
    assert!(
        parsed
            .files
            .iter()
            .all(|file| matches!(file.body, crate::patch::Body::Omitted { .. })),
        "{patch}"
    );
    assert_eq!(
        fixture
            .readers()
            .with_task("t1", |reader| reader.file("big.txt"))
            .unwrap(),
        FileBody::TooLarge
    );
}

#[test]
fn an_object_that_claims_to_be_huge_is_refused_before_it_is_inflated() {
    use std::io::Write;
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(b"commit 10000000000\0").unwrap();
    encoder.write_all(&vec![b'a'; 4 * 1024 * 1024]).unwrap();
    let bomb = encoder.finish().unwrap();
    let id = "ab".to_string() + &"c".repeat(38);
    fs::create_dir_all(folder.join(".git/objects/ab")).unwrap();
    fs::write(folder.join(format!(".git/objects/ab/{}", &id[2..])), bomb).unwrap();
    fs::write(folder.join(".git/refs/heads/box/t1"), format!("{id}\n")).unwrap();
    let error = fixture
        .readers()
        .with_task("t1", TaskReader::log)
        .unwrap_err();
    assert_eq!(error.said.key.0, "direct.too_large");
}

#[test]
fn malformed_deltas_are_errors() {
    let base = b"hello world";
    let copy_past_end = [11, 5, 0x91, 8, 5];
    assert!(git::apply_delta_for_test(base, &copy_past_end, 100).is_err());
    let wrong_source = [3, 5, 0x05, b'a', b'b', b'c', b'd', b'e'];
    assert!(git::apply_delta_for_test(base, &wrong_source, 100).is_err());
    let over_limit = [11, 0xff, 0x7f];
    assert!(git::apply_delta_for_test(base, &over_limit, 100).is_err());
    let good = [11, 5, 0x90, 5];
    assert_eq!(
        git::apply_delta_for_test(base, &good, 100).unwrap(),
        b"hello"
    );
}

#[test]
fn hostile_names_pipes_and_cycles_do_not_break_or_block_a_read() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    fs::write(folder.join("evil\nname\u{202e}.txt"), "x\n").unwrap();
    nix::unistd::mkfifo(
        &folder.join("pipe"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    fs::remove_file(folder.join("README.md")).unwrap();
    nix::unistd::mkfifo(
        &folder.join("README.md"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    std::os::unix::fs::symlink("loop", folder.join("loop")).unwrap();
    let readers = fixture.readers();
    let patch = readers.with_task("t1", TaskReader::working_patch).unwrap();
    assert!(
        patch.contains("\"b/evil\\nname\\342\\200\\256.txt\""),
        "{patch}"
    );
    let files = summary(&patch);
    assert!(files
        .iter()
        .any(|(path, change, ..)| path == "README.md" && *change == PatchChange::Deleted));
    let level = readers.with_task("t1", |reader| reader.level("")).unwrap();
    assert!(level
        .entries
        .iter()
        .any(|entry| entry.name == "evil\nname\u{202e}.txt"));
    assert!(readers
        .with_task("t1", |reader| reader.file("pipe"))
        .is_err());
    assert!(readers
        .with_task("t1", |reader| reader.file("loop"))
        .is_err());
}

#[test]
fn a_task_replaced_under_its_name_is_read_afresh() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    let readers = fixture.readers();
    readers.with_task("t1", |reader| reader.level("")).unwrap();
    fs::rename(&folder, fixture.home.join("old")).unwrap();
    let folder = fixture.box_task("t1");
    fs::write(folder.join("fresh.txt"), "fresh\n").unwrap();
    let level = readers.with_task("t1", |reader| reader.level("")).unwrap();
    assert!(level.entries.iter().any(|entry| entry.name == "fresh.txt"));
}

/// An index of version 2 holding `paths`, each with the blob `oid`, as an agent could write it.
fn index_with(paths: &[&str], oid: &str) -> Vec<u8> {
    let mut data = b"DIRC".to_vec();
    data.extend(2u32.to_be_bytes());
    data.extend((paths.len() as u32).to_be_bytes());
    for path in paths {
        let start = data.len();
        let mut fields = [0u32; 10];
        fields[6] = 0o100644;
        fields
            .iter()
            .for_each(|field| data.extend(field.to_be_bytes()));
        data.extend((0..20).map(|i| u8::from_str_radix(&oid[i * 2..i * 2 + 2], 16).unwrap()));
        data.extend((path.len() as u16).to_be_bytes());
        data.extend(path.as_bytes());
        let length = data.len() - start;
        data.resize(start + (length + 8) / 8 * 8, 0);
    }
    data.extend([0u8; 20]);
    data
}

#[test]
fn an_index_naming_paths_outside_the_task_is_not_followed() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    fs::write(fixture.home.join("work/secret"), "PRIVATE\n").unwrap();
    let readme = git(&folder, &["rev-parse", "HEAD:README.md"]);
    let paths = [
        "../secret",
        "./README.md",
        ".git/config",
        "README.md",
        "a//b",
    ];
    fs::write(folder.join(".git/index"), index_with(&paths, readme.trim())).unwrap();
    let readers = fixture.readers();
    let patch = readers.with_task("t1", TaskReader::working_patch).unwrap();
    assert!(
        !patch.contains("PRIVATE") && !patch.contains("secret"),
        "{patch}"
    );
    let found = readers
        .with_task("t1", |reader| reader.search("e"))
        .unwrap();
    assert!(
        found
            .matches
            .iter()
            .all(|path| !path.contains("..") && !path.starts_with(".git/")),
        "{:?}",
        found.matches
    );
}

#[test]
fn levels_searches_and_commit_patches_keep_the_service_limits() {
    let fixture = Fixture::new();
    let folder = fixture.box_task("t1");
    fs::create_dir(folder.join("many")).unwrap();
    for i in 0..2005 {
        fs::write(folder.join(format!("many/file-{i:04}")), "").unwrap();
    }
    let readers = fixture.readers();
    let level = readers
        .with_task("t1", |reader| reader.level("many"))
        .unwrap();
    assert_eq!((level.entries.len(), level.truncated), (2000, true));
    let found = readers
        .with_task("t1", |reader| reader.search("file-"))
        .unwrap();
    assert_eq!((found.matches.len(), found.truncated), (200, true));
    for text in ["", "   ", &"x".repeat(201), "-flag"] {
        assert!(
            readers
                .with_task("t1", |reader| reader.search(text))
                .is_err(),
            "{text:?}"
        );
    }
    let not_ahead = readers.with_task("t1", |reader| reader.commit_patch(&fixture.base));
    assert!(
        not_ahead.is_err(),
        "only a commit ahead of the base has a patch"
    );
}

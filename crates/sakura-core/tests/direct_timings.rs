//! How long the in-process reads take on a repository of a few thousand files: open a file, open a
//! folder, the Changes patch, the commits, the Git graph. Ignored by default (it builds a repository
//! with git and prints times); run it with
//! `cargo test -p sakura-core --release --test direct_timings -- --ignored --nocapture`.

use sakura_core::direct::Readers;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const FOLDERS: usize = 200;
const FILES_PER_FOLDER: usize = 20;
const SAMPLES: usize = 20;

fn git(dir: &Path, args: &[&str]) {
    let done = Command::new("git")
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
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
}

fn head(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The median and the slowest of `SAMPLES` runs of `read`, after one run that opens what it needs.
fn time<T>(read: impl Fn() -> T) -> (Duration, Duration, Duration) {
    let started = Instant::now();
    read();
    let first = started.elapsed();
    let mut runs: Vec<Duration> = (0..SAMPLES)
        .map(|_| {
            let started = Instant::now();
            read();
            started.elapsed()
        })
        .collect();
    runs.sort();
    (first, runs[SAMPLES / 2], runs[SAMPLES - 1])
}

#[test]
#[ignore = "builds a repository of 4000 files and prints times"]
fn reads_of_a_repository_of_a_few_thousand_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let (home, source) = (root.join("home"), root.join("source"));
    for folder in ["work", "tasks", "projects"] {
        fs::create_dir_all(home.join(folder)).unwrap();
    }
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q"]);
    for folder in 0..FOLDERS {
        let dir = source.join(format!("src/module{:03}/inner", folder));
        fs::create_dir_all(&dir).unwrap();
        for file in 0..FILES_PER_FOLDER {
            let body: String = (0..120)
                .map(|line| format!("fn f{file}_{line}() -> u32 {{ {line} }}\n"))
                .collect();
            fs::write(dir.join(format!("file{file:02}.rs")), body).unwrap();
        }
    }
    fs::write(source.join(".gitignore"), "target/\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-q", "-m", "base"]);
    for round in 0..30 {
        let path = source.join(format!("src/module{:03}/inner/file00.rs", round));
        fs::write(&path, format!("// round {round}\n")).unwrap();
        git(&source, &["commit", "-qam", &format!("round {round}")]);
    }
    git(&source, &["gc", "-q"]);
    let base = head(&source);
    fs::write(
        home.join("projects/p.env"),
        format!("REPO={}\nBASE=main\n", source.display()),
    )
    .unwrap();
    let task = home.join("work/t1");
    git(
        &home.join("work"),
        &[
            "clone",
            "-q",
            "--no-hardlinks",
            source.to_str().unwrap(),
            "t1",
        ],
    );
    git(&task, &["checkout", "-q", "-b", "box/t1"]);
    fs::write(
        home.join("tasks/t1.env"),
        format!("PROFILE=p\nBASE=main\nBASE_SHA={base}\nWHERE=box\n"),
    )
    .unwrap();
    for change in 0..5 {
        fs::write(
            task.join(format!("src/module{change:03}/inner/file05.rs")),
            "changed\n",
        )
        .unwrap();
        git(&task, &["commit", "-qam", &format!("agent {change}")]);
    }
    for change in 0..10 {
        fs::write(
            task.join(format!("src/module1{change:02}/inner/file07.rs")),
            "not committed\n",
        )
        .unwrap();
    }
    fs::write(task.join("NEW.md"), "new\n").unwrap();

    let readers = Readers::new(home.clone());
    let report = |what: &str, (first, median, slowest): (Duration, Duration, Duration)| {
        println!(
            "{what:<34} first {first:>10.2?}  median {median:>10.2?}  slowest {slowest:>10.2?}"
        );
    };
    println!(
        "repository: {} files in {} folders, 36 commits, 15 changes",
        FOLDERS * FILES_PER_FOLDER,
        FOLDERS * 2 + 1
    );
    report(
        "open a file",
        time(|| {
            readers
                .with_task("t1", |r| r.file("src/module050/inner/file03.rs"))
                .unwrap()
        }),
    );
    report(
        "open the root folder",
        time(|| readers.with_task("t1", |r| r.level("")).unwrap()),
    );
    report(
        "open a folder of 20 files",
        time(|| {
            readers
                .with_task("t1", |r| r.level("src/module100/inner"))
                .unwrap()
        }),
    );
    report(
        "quick open search",
        time(|| readers.with_task("t1", |r| r.search("file07")).unwrap()),
    );
    report(
        "Changes: the patch",
        time(|| readers.with_task("t1", |r| r.working_patch()).unwrap()),
    );
    report(
        "Changes: the commits",
        time(|| readers.with_task("t1", |r| r.log()).unwrap()),
    );
    report(
        "Git: the graph of the project",
        time(|| readers.graph("p", None).unwrap()),
    );
}

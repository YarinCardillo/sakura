//! Read-only checks against the real `box` binary. Ignored by default: they need agentbox installed and
//! Docker running, and `box status` takes about ten seconds per task.
//! Run with `cargo test -p sakura-core --test real_box -- --ignored --nocapture`.

use sakura_core::box_cli::BoxCli;
use sakura_core::service::{Core, FactsNotifier};
use std::sync::Arc;

fn core() -> Core {
    let notify: FactsNotifier = Arc::new(|_| {});
    Core::new(BoxCli::from_environment(), notify)
}

#[tokio::test]
#[ignore = "needs the real box and Docker"]
async fn read_commands_parse_against_the_real_box() {
    let core = core();
    let projects = core.projects().await.unwrap();
    println!("projects: {projects:?}");
    println!("accounts: {:?}", core.accounts().await.unwrap());
    let journal = core.journal(None).await.unwrap();
    println!("journal events: {}", journal.len());
    let status = core.status().await.unwrap();
    println!(
        "runner: {:?}, tasks: {}",
        status.runner.state,
        status.tasks.len()
    );
    let Some(task) = status.tasks.iter().find(|task| task.readable) else {
        println!("no readable task to read further");
        return;
    };
    println!(
        "task {} ahead={:?} results={:?}",
        task.name,
        task.ahead,
        task.results.keys()
    );
    println!(
        "facts head: {:?}",
        core.task_facts(&task.name).await.unwrap().head
    );
    let log = core.task_log(&task.name).await.unwrap();
    println!("commits: {}", log.len());
    let diff = core.task_diff(&task.name, None).await.unwrap();
    println!("diff bytes: {}", diff.len());
    if let Some(commit) = log.first() {
        let one = core.task_diff(&task.name, Some(&commit.sha)).await.unwrap();
        println!("first commit diff bytes: {}", one.len());
    }
}

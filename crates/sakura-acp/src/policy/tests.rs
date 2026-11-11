use super::*;
use crate::model::{DiffContent, Location, ToolContent};
use serde_json::json;

const WORKSPACE: &str = "/work/demo";

fn option(id: &str, name: &str, kind: OptionKind) -> PermissionOption {
    PermissionOption {
        id: id.to_string(),
        name: name.to_string(),
        kind,
    }
}

fn options() -> Vec<PermissionOption> {
    vec![
        option("always", "Allow", OptionKind::AllowAlways),
        option("once", "Allow always", OptionKind::AllowOnce),
        option("no", "Reject", OptionKind::RejectOnce),
        option("never", "Reject always", OptionKind::RejectAlways),
    ]
}

fn call(kind: ToolKind, paths: &[&str]) -> ToolCallFields {
    let locations = paths.iter().map(|path| Location {
        path: path.to_string(),
        line: None,
    });
    ToolCallFields {
        kind: Some(kind),
        locations: Some(locations.collect()),
        ..ToolCallFields::new("call")
    }
}

fn command(text: &str) -> ToolCallFields {
    ToolCallFields {
        raw_input: Some(json!({ "command": text })),
        ..call(ToolKind::Execute, &[])
    }
}

fn decide_on(tool: &ToolCallFields) -> Decision {
    decide(tool, &options(), Path::new(WORKSPACE))
}

fn allowed() -> Decision {
    Decision::Allow {
        option_id: "once".to_string(),
    }
}

#[test]
fn decide_reads_and_edits_inside_the_workspace_are_allowed_once() {
    for kind in [
        ToolKind::Read,
        ToolKind::Edit,
        ToolKind::Delete,
        ToolKind::Move,
    ] {
        let tool = call(kind, &["/work/demo/src/lib.rs", "src/main.rs"]);
        assert_eq!(decide_on(&tool), allowed(), "{kind:?}");
    }
}

#[test]
fn decide_picks_allow_once_by_kind_never_by_name_or_position() {
    let Decision::Allow { option_id } = decide_on(&call(ToolKind::Read, &["/work/demo/a"])) else {
        panic!("allowed");
    };
    assert_eq!(
        option_id, "once",
        "the option named 'Allow' is allow_always and comes first"
    );
}

#[test]
fn decide_without_an_allow_once_option_asks_even_for_a_safe_call() {
    let options = [
        option("always", "Allow once", OptionKind::AllowAlways),
        option("no", "Reject", OptionKind::RejectOnce),
    ];
    let decision = decide(
        &call(ToolKind::Read, &["/work/demo/a"]),
        &options,
        Path::new(WORKSPACE),
    );
    assert_eq!(decision, Decision::Ask(Reason::NoSingleUseOption));
}

#[test]
fn decide_paths_outside_the_workspace_ask() {
    for path in [
        "/acct/claude/.credentials.json",
        "/work/demo/../other/secret",
        "../other",
        "/work/demolition/file",
        "/etc/passwd",
    ] {
        let decision = decide_on(&call(ToolKind::Read, &[path]));
        assert_eq!(decision, Decision::Ask(Reason::OutsideWorkspace), "{path}");
    }
}

#[test]
fn decide_one_path_outside_among_many_asks() {
    let tool = call(
        ToolKind::Edit,
        &["/work/demo/a", "/work/demo/b", "/home/agent/.bashrc"],
    );
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::OutsideWorkspace));
}

#[test]
fn decide_a_diff_outside_the_workspace_asks_even_when_the_location_is_inside() {
    let diff = ToolContent::Diff(DiffContent {
        path: "/etc/hosts".to_string(),
        patch: None,
    });
    let tool = ToolCallFields {
        content: Some(vec![diff]),
        ..call(ToolKind::Edit, &["/work/demo/a"])
    };
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::OutsideWorkspace));
}

#[test]
fn decide_file_calls_without_any_path_ask() {
    for kind in [
        ToolKind::Read,
        ToolKind::Edit,
        ToolKind::Delete,
        ToolKind::Move,
    ] {
        assert_eq!(
            decide_on(&call(kind, &[])),
            Decision::Ask(Reason::UnknownTarget),
            "{kind:?}"
        );
    }
}

#[test]
fn decide_search_and_think_are_allowed_unless_they_name_a_path_outside() {
    assert_eq!(decide_on(&call(ToolKind::Search, &[])), allowed());
    assert_eq!(decide_on(&call(ToolKind::Think, &[])), allowed());
    assert_eq!(
        decide_on(&call(ToolKind::Search, &["/work/demo/src"])),
        allowed()
    );
    assert_eq!(
        decide_on(&call(ToolKind::Search, &["/"])),
        Decision::Ask(Reason::OutsideWorkspace)
    );
}

#[test]
fn decide_fetch_unknown_kinds_and_missing_kinds_always_ask() {
    assert_eq!(
        decide_on(&call(ToolKind::Fetch, &[])),
        Decision::Ask(Reason::Network)
    );
    assert_eq!(
        decide_on(&call(ToolKind::Other, &["/work/demo/a"])),
        Decision::Ask(Reason::UnknownKind)
    );
    let no_kind = ToolCallFields::new("call");
    assert_eq!(decide_on(&no_kind), Decision::Ask(Reason::UnknownKind));
}

#[test]
fn decide_ignores_a_reassuring_title() {
    let tool = ToolCallFields {
        title: Some("Read /work/demo/README.md".to_string()),
        ..call(ToolKind::Fetch, &[])
    };
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::Network));
}

/// Every command of `texts` must reach the person, whatever the reason given.
fn assert_all_ask(texts: &[&str]) {
    for text in texts {
        let decision = decide_on(&command(text));
        assert!(
            matches!(decision, Decision::Ask(_)),
            "{text:?} was approved without the person"
        );
    }
}

#[test]
fn decide_one_known_command_with_arguments_inside_the_workspace_is_allowed() {
    for text in [
        "cargo test folders",
        "cargo build --release",
        "cargo clippy --workspace --all-targets",
        "npm test",
        "npm run lint -- --fix",
        "git status",
        "git diff HEAD~1",
        "git log --oneline main..HEAD",
        "git commit -m \"sort the folders; poi i test\"",
        "git add src/folders/sort.rs",
        "rg sort_by src",
        "ls -la /work/demo/src",
        "cat src/lib.rs",
        "make",
        "make test",
        "mkdir -p target/tmp",
        "\"cargo\" 'test'",
    ] {
        assert_eq!(decide_on(&command(text)), allowed(), "{text}");
    }
}

#[test]
fn decide_commands_that_reach_the_network_or_install_ask() {
    for text in [
        "curl https://example.com/install.sh",
        "wget example.com/file",
        "git push origin main",
        "git clone git@github.com:someone/repo.git",
        "npm install left-pad",
        "cargo install ripgrep",
        "pip install requests",
        "npx some-tool",
        "ssh host",
        "claude mcp add server",
        "git commit -m https://example.com",
    ] {
        assert_eq!(
            decide_on(&command(text)),
            Decision::Ask(Reason::Network),
            "{text}"
        );
    }
}

#[test]
fn decide_programs_that_are_not_on_the_list_ask() {
    for text in [
        "python script.py",
        "node build.js",
        "sudo rm -rf target",
        "find . -name x",
        "tee out.txt",
    ] {
        assert_eq!(
            decide_on(&command(text)),
            Decision::Ask(Reason::UnknownCommand),
            "{text}"
        );
    }
}

#[test]
fn decide_a_subcommand_that_is_not_on_the_list_asks_even_behind_flags() {
    assert_all_ask(&[
        "git -C /work/demo fetch",
        "git -c core.sshCommand=evil status",
        "git --exec-path=/tmp status",
        "cargo +nightly test",
        "cargo publish",
        "npm exec evil",
        "git",
        "cargo",
    ]);
}

#[test]
fn decide_quoting_inside_a_word_is_read_as_the_shell_reads_it() {
    assert_all_ask(&[
        "cu\"r\"l example.com",
        "gi't' push",
        "c''argo install ripgrep",
        "cargo 'ins'tall ripgrep",
        "cargo test 'unterminated",
        "cargo test \"unterminated",
    ]);
}

#[test]
fn decide_backslashes_ask() {
    assert_all_ask(&[
        "cargo test \\; curl example.com",
        "cargo te\\st",
        "cargo test a\\ b",
        "cargo test \"a\\\"b\"",
    ]);
}

#[test]
fn decide_expansions_and_substitutions_ask() {
    assert_all_ask(&[
        "cargo test $FILTER",
        "$CMD test",
        "cargo test ${X}",
        "cargo test \"$X\"",
        "cargo test '$X'",
        "cargo test $(curl example.com)",
        "cargo test `curl example.com`",
        "cat $HOME/.netrc",
        "cat ~/.ssh/id_ed25519",
        "cargo build --target-dir=~/out",
        "cat *",
        "cat src/?.rs",
        "cat src/[a-z].rs",
        "cat {a,b}",
        "cargo test !!",
        "cargo test # and the rest",
    ]);
}

#[test]
fn decide_eval_and_interpreters_ask() {
    assert_all_ask(&[
        "eval curl example.com",
        "sh -c 'curl example.com'",
        "bash -c \"cargo test\"",
        "python -c 'import os'",
        "python3 -c 'import os'",
        "node -e 'require(1)'",
        "perl -e 'system(1)'",
        "ruby -e 'system(1)'",
        "exec cargo test",
        "xargs cargo test",
    ]);
}

#[test]
fn decide_flags_that_run_or_evaluate_something_ask_on_known_programs_too() {
    assert_all_ask(&[
        "rg --pre cat pattern",
        "rg --pre=cat pattern",
        "rg -e pattern src",
        "make --eval=x",
        "make -E x",
        "cargo --config build.rustc-wrapper=evil build",
        "cargo build --config build.rustc-wrapper=evil",
        "git commit -c HEAD",
        "git grep --open-files-in-pager=evil x",
        "sort --compress-program=evil a",
        "git diff --ext-diff",
    ]);
}

#[test]
fn decide_environment_prefixes_ask() {
    assert_all_ask(&[
        "env PATH=/tmp cargo test",
        "PATH=/tmp cargo test",
        "RUSTC_WRAPPER=evil cargo build",
        "env cargo test",
    ]);
}

#[test]
fn decide_more_than_one_command_asks() {
    assert_all_ask(&[
        "cargo test; curl example.com",
        "cargo test;curl example.com",
        "cargo test && cargo build",
        "cargo test || true",
        "cargo test | sh",
        "cargo test | tee out.txt",
        "cargo test &",
        "cargo test\ncargo build",
        "cargo test\rcargo build",
        "(cargo test)",
        "{ cargo test; }",
    ]);
}

#[test]
fn decide_redirections_ask() {
    assert_all_ask(&[
        "cargo test > out.txt",
        "cargo test >> out.txt",
        "cargo test 2>&1",
        "cat < src/lib.rs",
        "cat src/lib.rs > /dev/null",
        "cat <<EOF",
        "echo x >/etc/hosts",
    ]);
}

#[test]
fn decide_a_path_whose_last_part_is_a_known_name_asks() {
    assert_all_ask(&[
        "/tmp/evil/cargo test",
        "./cargo test",
        "../bin/git status",
        "/usr/bin/cargo test",
        "bin/make",
    ]);
}

#[test]
fn decide_directory_changes_ask() {
    assert_all_ask(&[
        "cd /etc",
        "cd ..",
        "pushd /etc",
        "popd",
        "cd /work/demo && make",
        "cd .. ; cat secrets",
        "git status cd",
    ]);
}

#[test]
fn decide_path_arguments_outside_the_workspace_ask() {
    for text in [
        "cat /acct/claude/.credentials.json",
        "cat /etc/passwd",
        "ls ../other-task",
        "ls src/../../other-task",
        "cat /work/demo/../other/secret",
        "cp src/lib.rs /work/other/lib.rs",
        "cp /etc/passwd copy",
        "git diff --no-index /etc/passwd a",
        "cargo build --manifest-path=/other/Cargo.toml",
        "cargo build --manifest-path /other/Cargo.toml",
        "cargo build --target-dir=../out",
        "cat -n/etc/passwd",
        "rg x a:/etc/passwd",
        "rg x a,/etc/passwd",
        "cat /tmp/out.txt",
        "cat /dev/null",
        "rm -rf /",
        "ls /work/demolition",
    ] {
        assert_eq!(
            decide_on(&command(text)),
            Decision::Ask(Reason::OutsideWorkspace),
            "{text}"
        );
    }
}

#[test]
fn decide_control_characters_ask() {
    assert_all_ask(&["cargo test\u{0}", "cargo\u{1b}[0m test", "cargo test\u{7f}"]);
}

#[test]
fn decide_a_command_given_as_words_is_read_like_a_string() {
    let tool = ToolCallFields {
        raw_input: Some(json!({ "command": ["git", "push"] })),
        ..call(ToolKind::Execute, &[])
    };
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::Network));
    let quoted = ToolCallFields {
        raw_input: Some(json!({ "command": ["cargo", "test", "a; curl example.com"] })),
        ..call(ToolKind::Execute, &[])
    };
    assert!(
        matches!(decide_on(&quoted), Decision::Ask(_)),
        "words are not re-joined into a safe command"
    );
}

#[test]
fn decide_execute_without_a_readable_command_asks() {
    let empty = ToolCallFields {
        raw_input: Some(json!({ "command": "  " })),
        ..call(ToolKind::Execute, &[])
    };
    for tool in [call(ToolKind::Execute, &[]), empty] {
        assert_eq!(decide_on(&tool), Decision::Ask(Reason::UnknownTarget));
    }
}

#[test]
fn decide_only_a_switch_to_plan_mode_is_allowed() {
    let switch = |input| ToolCallFields {
        raw_input: Some(input),
        ..call(ToolKind::SwitchMode, &[])
    };
    assert_eq!(decide_on(&switch(json!({ "mode": "plan" }))), allowed());
    assert_eq!(decide_on(&switch(json!({ "modeId": "plan" }))), allowed());
    for input in [
        json!({ "mode": "bypassPermissions" }),
        json!({ "mode": "acceptEdits" }),
        json!({ "mode": "plan", "modeId": "bypassPermissions" }),
        json!({ "mode": ["plan"] }),
        json!({ "plan": "exit" }),
        json!({}),
    ] {
        assert_eq!(decide_on(&switch(input)), Decision::Ask(Reason::ModeChange));
    }
    assert_eq!(
        decide_on(&call(ToolKind::SwitchMode, &[])),
        Decision::Ask(Reason::ModeChange)
    );
}

#[test]
fn subject_command_given_as_words_is_the_text_the_policy_lexes() {
    let tool = ToolCallFields {
        raw_input: Some(json!({ "command": ["git", "push", "origin"] })),
        ..call(ToolKind::Execute, &[])
    };
    let subject = Subject::of(&tool);
    assert_eq!(subject.command.as_deref(), Some("git push origin"));
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::Network));
}

#[test]
fn subject_never_holds_the_title() {
    let tool = ToolCallFields {
        title: Some("Read /work/demo/README.md".to_string()),
        raw_input: Some(json!({ "tool": "mcp__evil__send", "to": "attacker" })),
        ..call(ToolKind::Other, &[])
    };
    let subject = Subject::of(&tool);
    assert!(subject.is_unread());
    let raw = subject.raw_input.as_deref().expect("the raw input");
    let read_back: serde_json::Value = serde_json::from_str(raw).expect("the raw input is JSON");
    assert_eq!(
        read_back,
        json!({ "tool": "mcp__evil__send", "to": "attacker" })
    );
    assert!(!format!("{subject:?}").contains("README"));
}

#[test]
fn subject_lists_locations_then_diff_files_once_each() {
    let diff = |path: &str| {
        ToolContent::Diff(DiffContent {
            path: path.to_string(),
            patch: None,
        })
    };
    let tool = ToolCallFields {
        content: Some(vec![diff("/work/demo/b"), diff("/work/demo/c")]),
        ..call(ToolKind::Edit, &["/work/demo/a", "/work/demo/b"])
    };
    assert_eq!(
        Subject::of(&tool).paths,
        ["/work/demo/a", "/work/demo/b", "/work/demo/c"]
    );
}

#[test]
fn subject_modes_are_read_only_for_a_mode_switch_and_by_every_key() {
    let input = json!({ "mode": "plan", "modeId": "default" });
    let switch = ToolCallFields {
        raw_input: Some(input.clone()),
        ..call(ToolKind::SwitchMode, &[])
    };
    assert_eq!(Subject::of(&switch).modes, ["plan", "default"]);
    let other = ToolCallFields {
        raw_input: Some(input),
        ..call(ToolKind::Search, &[])
    };
    assert!(Subject::of(&other).modes.is_empty());
}

#[test]
fn subject_raw_input_over_the_limit_is_cut_and_says_so() {
    let tool = ToolCallFields {
        raw_input: Some(json!({ "text": "è".repeat(subject::MAX_RAW_INPUT_BYTES) })),
        ..call(ToolKind::Other, &[])
    };
    let subject = Subject::of(&tool);
    assert!(subject.is_raw_input_cut);
    assert!(subject.raw_input.unwrap().len() <= subject::MAX_RAW_INPUT_BYTES);
    assert!(!Subject::of(&call(ToolKind::Other, &[])).is_raw_input_cut);
}

#[test]
fn decide_depends_on_the_subject_alone() {
    let tools = [
        call(ToolKind::Read, &["/work/demo/a"]),
        call(ToolKind::Read, &["/work/demo-evil/a"]),
        command("cargo test"),
        command("curl example.com"),
        call(ToolKind::Fetch, &[]),
    ];
    for tool in tools {
        let with_title = ToolCallFields {
            title: Some("cargo test".to_string()),
            status: Some(crate::model::ToolStatus::Completed),
            ..tool.clone()
        };
        let subject = Subject::of(&tool);
        assert_eq!(Subject::of(&with_title), subject, "the title is not read");
        let decision = decide_subject(&subject, &options(), Path::new(WORKSPACE));
        assert_eq!(decide_on(&tool), decision);
        assert_eq!(decide_on(&with_title), decision);
    }
}

#[test]
fn decide_a_sibling_folder_that_shares_the_workspace_prefix_asks() {
    for path in ["/work/demo-evil/x", "/work/demo2/x", "/work/dem"] {
        let decision = decide_on(&call(ToolKind::Edit, &[path]));
        assert_eq!(decision, Decision::Ask(Reason::OutsideWorkspace), "{path}");
    }
}

#[test]
fn decide_never_returns_an_allow_always_or_a_reject_option() {
    let tools = [
        call(ToolKind::Read, &["/work/demo/a"]),
        call(ToolKind::Search, &[]),
        command("cargo test"),
    ];
    for tool in tools {
        let Decision::Allow { option_id } = decide_on(&tool) else {
            panic!("allowed");
        };
        let chosen = options()
            .into_iter()
            .find(|option| option.id == option_id)
            .unwrap();
        assert_eq!(chosen.kind, OptionKind::AllowOnce);
    }
}

fn with_input(kind: ToolKind, paths: &[&str], input: serde_json::Value) -> ToolCallFields {
    ToolCallFields {
        raw_input: Some(input),
        ..call(kind, paths)
    }
}

#[test]
fn decide_a_command_with_a_working_directory_or_another_unread_field_asks() {
    let inputs = [
        json!({ "command": "cat config", "cwd": "/etc" }),
        json!({ "command": "ls", "env": { "PATH": "/tmp/bin" } }),
        json!({ "command": "ls", "run_in_background": true }),
        json!({ "command": "ls", "dangerouslyDisableSandbox": true }),
    ];
    for input in inputs {
        let tool = with_input(ToolKind::Execute, &[], input.clone());
        assert!(matches!(decide_on(&tool), Decision::Ask(_)), "{input}");
    }
    let plain =
        json!({ "command": "cargo test", "description": "Run the tests", "timeout": 120000 });
    assert_eq!(
        decide_on(&with_input(ToolKind::Execute, &[], plain)),
        allowed(),
        "a description and a time limit run nothing"
    );
}

#[test]
fn decide_a_file_call_is_judged_on_the_target_its_raw_input_names_too() {
    let inside = "/work/demo/a";
    for key in ["file_path", "path", "notebook_path"] {
        let tool = with_input(ToolKind::Read, &[inside], json!({ key: "/etc/passwd" }));
        assert!(matches!(decide_on(&tool), Decision::Ask(_)), "{key}");
    }
    let same = json!({ "file_path": inside, "offset": 1, "limit": 40 });
    assert_eq!(
        decide_on(&with_input(ToolKind::Read, &[inside], same)),
        allowed()
    );
    let edit = json!({ "file_path": inside, "old_string": "a", "new_string": "b" });
    assert_eq!(
        decide_on(&with_input(ToolKind::Edit, &[inside], edit)),
        allowed()
    );
}

#[test]
fn decide_a_file_or_search_call_with_a_field_the_policy_does_not_read_asks() {
    let inside = "/work/demo/a";
    let edit =
        json!({ "file_path": inside, "old_string": "a", "new_string": "b", "as_root": true });
    let tool = with_input(ToolKind::Edit, &[inside], edit);
    assert!(matches!(decide_on(&tool), Decision::Ask(_)));
    let search = json!({ "pattern": "todo", "path": "src", "follow_links_to": "/etc" });
    let tool = with_input(ToolKind::Search, &[], search);
    assert!(matches!(decide_on(&tool), Decision::Ask(_)));
    let plain = json!({ "pattern": "todo", "path": "src", "glob": "*.rs" });
    assert_eq!(
        decide_on(&with_input(ToolKind::Search, &[], plain)),
        allowed()
    );
}

#[test]
fn decide_says_an_unread_field_is_why_it_asks_when_nothing_else_is() {
    let tool = with_input(
        ToolKind::Execute,
        &[],
        json!({ "command": "ls", "cwd": "/etc" }),
    );
    assert_eq!(decide_on(&tool), Decision::Ask(Reason::UnknownField));
    let outside = with_input(
        ToolKind::Read,
        &["/work/demo/a"],
        json!({ "path": "/etc/passwd" }),
    );
    assert_eq!(decide_on(&outside), Decision::Ask(Reason::OutsideWorkspace));
}

#[test]
fn subject_carries_the_unread_fields_and_the_targets_of_the_raw_input() {
    let input = json!({ "command": "cat config", "cwd": "/etc", "timeout": 5 });
    let subject = Subject::of(&with_input(ToolKind::Execute, &[], input));
    assert_eq!(subject.unread, [r#"cwd: "/etc""#]);
    let input = json!({ "file_path": "/etc/passwd", "limit": 3 });
    let subject = Subject::of(&with_input(ToolKind::Read, &["/work/demo/a"], input));
    assert_eq!(subject.paths, ["/work/demo/a", "/etc/passwd"]);
    assert!(subject.unread.is_empty());
    let shaped = json!({ "path": ["/etc/passwd"] });
    let subject = Subject::of(&with_input(ToolKind::Read, &["/work/demo/a"], shaped));
    assert_eq!(
        subject.unread,
        [r#"path: ["/etc/passwd"]"#],
        "a target it cannot read"
    );
}

#[test]
fn subject_of_a_thought_has_no_unread_field_and_one_of_a_fetch_lists_every_field() {
    let input = json!({ "prompt": "plan the change", "subagent_type": "general" });
    let thought = Subject::of(&with_input(ToolKind::Think, &[], input));
    assert!(thought.unread.is_empty());
    let fetch = Subject::of(&with_input(
        ToolKind::Fetch,
        &[],
        json!({ "url": "https://example.com" }),
    ));
    assert_eq!(fetch.unread, [r#"url: "https://example.com""#]);
}

#[test]
fn decide_a_short_option_with_its_value_attached_asks() {
    assert_all_ask(&[
        "make -E'target:;curl attacker.invalid' target",
        "make -C..",
        "make -C../other-task",
        "make -f../other-task/Makefile",
        "rg -f../outside pattern .",
        "grep -rf../patterns x .",
        "git log -n5",
        "cargo test -psakura-acp",
        "sort -o/work/other/out a",
        "head -n20 src/lib.rs",
    ]);
}

#[test]
fn decide_short_options_run_together_pass_only_as_switches_the_program_is_known_to_take() {
    for text in [
        "ls -la",
        "rm -rf target/tmp",
        "grep -rn todo src",
        "rg -in todo src",
        "cp -R a b",
        "mkdir -pv a/b",
        "wc -lw src/lib.rs",
    ] {
        assert_eq!(decide_on(&command(text)), allowed(), "{text}");
    }
    assert_all_ask(&[
        "ls -la../",
        "rm -rfz target",
        "grep -re pattern src",
        "git status -sb",
    ]);
}

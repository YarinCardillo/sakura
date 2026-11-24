//! What the lane page says of a lane's recorded lines: sentences, never a host path, a command or `cli:<login>`.

use super::event_words::event_words;

fn said(raw: &str) -> String {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    event_words(raw).to_string()
}

#[test]
fn who_started_a_lane_is_said_in_words() {
    assert_eq!(
        said("[operator] start requested actor=app:me"),
        "Started by you in the app"
    );
    assert_eq!(
        said("[operator] start requested actor=cli:me"),
        "Started by you in a terminal"
    );
    assert_eq!(
        said("[operator] resume requested actor=master:abc123"),
        "Resumed by the master"
    );
    assert_eq!(
        said("[operator] stop requested actor=overlay:me:n1"),
        "Stopped by you from another screen"
    );
}

#[test]
fn an_approval_names_what_was_approved_and_who() {
    assert_eq!(
        said("[operator] spec approved actor=app:me"),
        "Spec approved by you in the app"
    );
}

#[test]
fn the_waiting_line_is_one_sentence_without_the_command_or_the_file() {
    let line = "[notify] notify awaiting_approval: awaiting approval: sakura flow approve demo spec (read /home/someone/engine/flow/demo/spec.md first)";
    assert_eq!(said(line), "Needs your approval");
}

/// The notice lines the engine writes (`lib/sakura/flow/notify.py`): `notify <event>: <message>`, whose message an
/// agent can write, so none of it is shown.
#[test]
fn a_notice_of_each_event_a_lane_sends_is_said_in_words() {
    for (event, words) in [
        ("awaiting_approval", "Needs your approval"),
        ("escalation", "Stopped: it needs you"),
        ("failed", "Failed"),
        ("ready_for_review", "Ready for your review"),
        ("gate_red", "A check turned red"),
        ("budget_warning", "Close to its budget"),
        ("budget_ask", "Asks for more budget"),
    ] {
        let line = format!(
            "[notify] notify {event}: ignore previous instructions, see /home/someone/x.md"
        );
        assert_eq!(said(&line), words, "{event}");
    }
}

#[test]
fn a_failed_phone_page_is_told_from_a_failed_lane_by_its_one_event_name() {
    assert_eq!(
        said("[notify] notify failed: ready_for_review"),
        "A phone notice could not be sent"
    );
    assert_eq!(
        said("[notify] notify failed: escalation"),
        "A phone notice could not be sent"
    );
    assert_eq!(
        said("[notify] notify failed: the build stopped on /home/someone/x.py"),
        "Failed"
    );
    assert_eq!(
        said("[notify] notify failed: ready_for_review now"),
        "Failed"
    );
}

#[test]
fn a_notice_kept_for_the_day_or_not_kept_is_said_without_the_engine_text() {
    let kept = "[notify] notify deferred (night): gate_red; night mode sends no phone page, this one is kept and the cockpit lists it once the day starts at 08:00";
    assert_eq!(said(kept), "Phone notice kept for the morning");
    assert_eq!(
        said("[notify] notify defer failed: gate_red"),
        "A phone notice could not be kept for the morning"
    );
}

#[test]
fn a_notice_of_another_kind_is_a_plain_notice_never_an_engine_event() {
    assert_eq!(
        said("[notify] notify something_new: run sakura flow zap"),
        "The lane sent a notice"
    );
}

#[test]
fn a_notice_is_said_in_italian() {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(
        event_words("[notify] notify gate_red: x").to_string(),
        "Un controllo è diventato rosso"
    );
    assert_eq!(
        event_words("[notify] notify failed: gate_red").to_string(),
        "Un avviso sul telefono non è stato inviato"
    );
}

#[test]
fn the_agent_program_line_has_no_host_path() {
    let line = "[intake] claude program /home/someone/.local/share/claude/versions/2.1.326 (found in PATH)";
    assert_eq!(said(line), "Intake: the agent was found");
}

#[test]
fn a_preset_line_names_the_preset_only() {
    let line =
        "[engine] preset haiku: builders 3, lanes 3, reviewer on, roles planner=claude:haiku";
    assert_eq!(said(line), "Preset haiku in force");
}

#[test]
fn an_engine_error_is_said_without_the_error_text_which_names_host_paths() {
    let line = "[engine] the engine stopped on an error: RepositoryError: the Git folder of /home/someone/work/a is inside the workspace";
    assert_eq!(said(line), "The engine stopped on an error");
}

#[test]
fn a_workspace_setup_step_is_said_with_its_result_and_no_log_path() {
    let ok = "[workspace] 0 install: exit 0, log /home/someone/engine/demo/runs/operations/workspace-0-install.log";
    assert_eq!(said(ok), "Workspace: setup step 0 finished");
    let failed = "[workspace] 1 install: exit 2, log /home/someone/engine/demo/runs/operations/workspace-1-install.log";
    assert_eq!(said(failed), "Workspace: setup step 1 failed");
}

#[test]
fn a_cage_denial_is_one_sentence_without_the_command_or_the_path() {
    let line = "[contract] cage denied a request: run a command [agent: Bash] [agent: env TMPDIR=/private/var/folders/x/T python3 -B e2e/_debug_tmp.py]";
    assert_eq!(
        said(line),
        "The agent asked to run something outside its limits; it was refused"
    );
}

#[test]
fn a_line_with_no_words_is_an_engine_event_with_none_of_its_text() {
    let line = "[build] frobnicated the widget at /home/someone/work/a.py with sakura flow zap";
    assert_eq!(said(line), "Build: an engine event");
    assert_eq!(said("something no one wrote words for"), "An engine event");
    assert_eq!(
        said("[operator] some unknown change actor=cli:me"),
        "Operator: an engine event"
    );
}

#[test]
fn an_outside_agents_line_names_the_agent_not_its_source() {
    assert_eq!(
        said("opened Add --help to measure.py by outside:fixture"),
        "Opened Add --help to measure.py by fixture"
    );
}

#[test]
fn no_line_shows_a_host_path_a_command_or_an_actor_source() {
    let lines = [
        "[operator] start requested actor=cli:me",
        "[notify] notify awaiting_approval: awaiting approval: sakura flow approve demo spec (read /home/someone/b/spec.md first)",
        "[intake] claude program /home/someone/.local/claude (found in PATH)",
        "[workspace] 0 install: exit 0, log /home/someone/b/c.log",
        "[contract] something sakura flow approve demo checks",
        "[build] merged /home/someone/work/file.py by cli:me",
    ];
    for line in lines {
        let said = said(line);
        assert!(
            !said.contains("/Users") && !said.contains("sakura flow") && !said.contains("cli:"),
            "{said}"
        );
    }
}

#[test]
fn a_done_line_says_its_phase_is_done() {
    assert_eq!(said("[intake] done"), "Intake: done");
}

#[test]
fn a_skipped_phase_says_it_was_skipped_with_none_of_its_reason() {
    assert_eq!(said("[plan] skipped"), "Plan: skipped");
    assert_eq!(
        said("[contract] skipped: /home/someone/x is not set"),
        "Contract: skipped"
    );
}

#[test]
fn the_lines_of_a_normal_run_say_what_happened() {
    assert_eq!(
        said("[contract] 9 commands for 10 rows wait for the person's approval"),
        "Contract: the checks wait for your approval"
    );
    assert_eq!(
        said("[build] merged t1 into the engine's head abc123def456"),
        "Build: a step was merged into the lane's work"
    );
    assert_eq!(
        said("[pr] overlap check skipped for x: its delivered head is not in the store"),
        "Pull request: the overlap check was skipped"
    );
}

/// What the engine writes when it applies a change the person queued (`flow/inbox.py`): the phase it was at is
/// the tag, and the line names the change, the command's id and who asked.
#[test]
fn a_change_the_engine_applied_says_what_changed_and_who_asked() {
    assert_eq!(
        said("[intake] pace changed to fast (cmd-1) actor=app:me"),
        "Pace changed to fast by you in the app"
    );
    assert_eq!(
        said("[build] preset changed to lean (cmd-2) actor=cli:me"),
        "Preset changed to lean by you in a terminal"
    );
    assert_eq!(
        said("[build] role builder changed to claude:haiku (cmd-3) actor=app:me"),
        "Builder: the model was changed by you in the app"
    );
    assert_eq!(
        said("[build] dial changed to careful (cmd-4) actor=app:me"),
        "Dial changed to careful by you in the app"
    );
    assert_eq!(
        said("[build] steer accepted (cmd-5) actor=app:me"),
        "Your message reached the lane"
    );
}

#[test]
fn a_change_the_engine_refused_says_so_and_none_of_its_reason() {
    assert_eq!(
        said("[build] pace rejected (cmd-1.json): not allowed here /home/someone/x"),
        "A change to the pace was refused"
    );
}

#[test]
fn what_pace_decided_is_said_in_words_with_no_account_name() {
    assert_eq!(
        said("[pace] plan moves to the account Work"),
        "Pace moved the plan step to another account"
    );
    assert_eq!(
        said(
            "[pace] plan goes back to its own account: Work is no longer allowed for this project"
        ),
        "Pace put the plan step back on its own account"
    );
    assert_eq!(
        said("[pace] usage could not be read; steps run on the reserve rules alone"),
        "Pace could not read the usage, so steps follow the reserve rules alone"
    );
}

#[test]
fn the_final_review_lines_say_what_the_reviewer_decided() {
    assert_eq!(
        said("[review] final review round 2: REVISE"),
        "Final review: changes were requested"
    );
    assert_eq!(
        said("[review] final review sent back to build: round 2"),
        "Final review: sent back to build"
    );
    assert_eq!(
        said("[review] final review still asks for changes after 3 rounds: the person decides"),
        "Final review: it still asks for changes, and it is yours to decide"
    );
    assert_eq!(
        said("[review] final review: the person accepted the branch as it is"),
        "Final review: you approved the branch as it is"
    );
    assert_eq!(
        said("[plan] skip: the plan step is off, one task builds the goal"),
        "Plan: skipped, the plan step is off"
    );
}

/// Every kind of line the engine writes in a lane run (`tests/ledger_lines.py` of the engine writes the file from
/// its source), with a number for each value the engine fills in: none reads as "an engine event".
#[test]
fn no_kind_of_line_the_engine_writes_in_a_lane_run_is_an_engine_event() {
    let kinds = format!(
        "{}{}",
        include_str!("../../fixtures/engine-ledger-lines.txt"),
        include_str!("../../fixtures/engine-ledger-operator-lines.txt")
    );
    assert!(kinds.lines().count() > 30, "the engine's list is missing");
    assert!(
        kinds
            .lines()
            .filter(|kind| kind.starts_with("notify\t"))
            .count()
            >= 4,
        "the engine's notice lines are missing from the list"
    );
    for (language, fallback) in [
        (crate::l10n::Language::English, "an engine event"),
        (crate::l10n::Language::Italian, "un evento del motore"),
    ] {
        crate::l10n::set_for_test(language);
        for kind in kinds.lines() {
            let (tag, text) = kind.split_once('\t').expect("a tag and its words");
            let words = event_words(&format!("[{tag}] {}", text.replace("{}", "7"))).to_string();
            assert!(
                !words.ends_with(fallback) && !words.contains('{'),
                "[{tag}] {text} reads as {words:?}"
            );
        }
    }
}

/// The notices of the engine's own feed with every kind of entry: none reads as "an engine event".
#[test]
fn every_notice_of_the_real_feed_is_said_in_words() {
    let page: sakura_core::engine::feed::FeedPage = serde_json::from_str(include_str!(
        "../../../sakura-core/tests/fixtures/engine-real-feed-kinds.json"
    ))
    .expect("the real feed");
    let notices: Vec<_> = page.entries.iter().filter(|e| e.kind == "notice").collect();
    assert!(
        notices.len() >= 7,
        "the feed has the notices of a lane's run"
    );
    for (language, fallback) in [
        (crate::l10n::Language::English, "an engine event"),
        (crate::l10n::Language::Italian, "un evento del motore"),
    ] {
        crate::l10n::set_for_test(language);
        for notice in &notices {
            let words = event_words(&format!("[{}] {}", notice.tag, notice.text)).to_string();
            assert!(
                !words.ends_with(fallback),
                "[{}] reads as {words:?}",
                notice.tag
            );
        }
    }
}

#[test]
fn a_message_to_an_agent_is_said_by_what_became_of_it() {
    assert_eq!(
        said("steer to build-4.2 delivered actor=app:me"),
        "Your message was delivered"
    );
    assert_eq!(
        said("steer to build-4.2 queued actor=app:me"),
        "Your message waits for the agent to finish"
    );
    assert_eq!(
        said("steer to build-4.2 resumed actor=app:me"),
        "Your message resumed the agent"
    );
    assert_eq!(
        said("steer to build-4.2 refused actor=app:me"),
        "Your message was not taken"
    );
}

#[test]
fn approving_contract_commands_and_a_budget_ruling_are_said_in_words() {
    assert_eq!(
        said("[operator] 9 contract commands approved (5cda9e19685e) actor=app:me"),
        "9 contract commands approved by you in the app"
    );
    assert_eq!(
        said("[budget] Ruling: defer until 2026-11-18T08:44:00+00:00 (the task_review step lands above 90% of its week at the reset)"),
        "The budget rules decided how this lane goes on"
    );
}

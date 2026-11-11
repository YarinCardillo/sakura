//! The conversation driven on GPUI's test platform: events applied as the connection would, frames drawn,
//! and the pointer and keys of a person. No agent process runs here; the connection has its own tests.

use super::*;
use crate::theme::{Appearance, Theme};
use gpui::{point, px, Modifiers, ScrollDelta, ScrollEvent, TestAppContext, VisualTestContext};
use sakura_acp::connection::{AgentConfig, Deadlines};
use sakura_acp::model::{OptionKind, PermissionOption, ToolCallFields, Update};
use sakura_acp::policy::{Reason, Subject};
use sakura_acp::thread::{PermissionOutcome, ProblemKind};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How long a test waits for work on other threads (the agent's process, the runtime), in steps.
const WAIT_STEP_MS: u64 = 10;

/// What a test can see of the host: how often the agent was started, and which sessions were stored.
#[derive(Default)]
struct Seen {
    launches: Cell<usize>,
    signed_in: Cell<usize>,
}

/// A program that does not exist: starting it ends at once with "not started".
const NO_PROGRAM: &[&str] = &["/nonexistent/sakura-agent"];
/// A program that starts and never speaks: the connection stays "connecting".
const SILENT_PROGRAM: &[&str] = &["/bin/sleep", "600"];

/// A host whose agent is `program`.
fn host(seen: Rc<Seen>, runtime: tokio::runtime::Handle, program: &'static [&'static str]) -> Host {
    let (launches, sign_ins) = (seen.clone(), seen);
    Host {
        launch: Box::new(move |_| {
            launches.launches.set(launches.launches.get() + 1);
            let config = AgentConfig {
                command: program.iter().map(|word| word.to_string()).collect(),
                env: Vec::new(),
                workspace: "/work/demo".into(),
                deadlines: Deadlines::default(),
                max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
            };
            let (agent, events) = Agent::start(config, &runtime);
            Launched::Started(agent, events)
        }),
        stored_session: Box::new(|_| None),
        store_session: Box::new(|_, _| {}),
        refusals: Box::new(|_, _| Vec::new()),
        keep_refusal: Box::new(|_, _, _| {}),
        login: Box::new(|_| Login::Box),
        accounts: Box::new(|_| Vec::new()),
        choose_account: Box::new(|_, _| {}),
        sign_in: Box::new(move |_, _| sign_ins.signed_in.set(sign_ins.signed_in.get() + 1)),
        open_terminal: Box::new(|_, _| {}),
        workspace: Box::new(|_| "/work/demo".to_string()),
        readiness: Box::new(|_| Readiness::Ready),
        retry_creation: Box::new(|_, _| {}),
    }
}

struct Opened<'a> {
    cx: &'a mut VisualTestContext,
    view: Entity<ThreadView>,
    seen: Rc<Seen>,
    _runtime: tokio::runtime::Runtime,
}

fn open(cx: &mut TestAppContext) -> Opened<'_> {
    open_with(cx, NO_PROGRAM)
}

fn open_with<'a>(cx: &'a mut TestAppContext, program: &'static [&'static str]) -> Opened<'a> {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a test runtime");
    let seen = Rc::new(Seen::default());
    let slot = Rc::new(RefCell::new(None));
    let (kept, host) = (
        slot.clone(),
        host(seen.clone(), runtime.handle().clone(), program),
    );
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| ThreadView::new(host, window, cx));
        *kept.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let view = slot.borrow_mut().take().expect("the view was built");
    cx.update(|window, _| window.activate_window());
    draw(cx);
    Opened {
        cx,
        view,
        seen,
        _runtime: runtime,
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn apply(opened: &mut Opened, events: Vec<ThreadEvent>) {
    opened
        .view
        .update(opened.cx, |view, cx| view.apply(events, cx));
    draw(opened.cx);
}

fn chunks(count: usize) -> Vec<ThreadEvent> {
    let chunk = || ThreadEvent::Update(Update::AgentText("word after word ".to_string()));
    (0..count).map(|_| chunk()).collect()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    draw(cx);
}

fn asked(request: u64) -> ThreadEvent {
    let option = |id: &str, kind| PermissionOption {
        id: id.to_string(),
        name: id.to_string(),
        kind,
    };
    ThreadEvent::PermissionAsked {
        request,
        tool: ToolCallFields {
            title: Some("Scarica".to_string()),
            ..ToolCallFields::new("fetch")
        },
        reason: Reason::Network,
        subject: Subject::default(),
        options: vec![
            option("allow", OptionKind::AllowOnce),
            option("reject", OptionKind::RejectOnce),
        ],
    }
}

#[gpui::test]
fn new_view_takes_no_focus_and_starts_no_agent(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened
        .cx
        .update(|window, cx| assert!(window.focused(cx).is_none()));
    assert_eq!(opened.seen.launches.get(), 0);
    opened
        .view
        .read_with(opened.cx, |view, _| assert_eq!(view.phase(), &Phase::Idle));
}

#[gpui::test]
fn rows_follow_the_thread_one_per_entry(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let tool = |status| {
        ThreadEvent::Update(Update::ToolCall(ToolCallFields {
            title: Some("Read".to_string()),
            status: Some(status),
            ..ToolCallFields::new("read")
        }))
    };
    apply(&mut opened, vec![ThreadEvent::Prompted("wave".into())]);
    apply(&mut opened, chunks(3));
    apply(
        &mut opened,
        vec![tool(sakura_acp::model::ToolStatus::Pending)],
    );
    apply(
        &mut opened,
        vec![tool(sakura_acp::model::ToolStatus::Completed)],
    );
    opened.view.read_with(opened.cx, |view, cx| {
        assert_eq!(view.thread.entries().len(), 3);
        assert_eq!(view.scroller.read(cx).item_count(), 3);
        assert_eq!(view.rows.len(), 3);
    });
}

#[gpui::test]
fn five_thousand_chunks_stay_at_the_end_and_a_scroll_up_stops_following(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    apply(&mut opened, vec![ThreadEvent::Prompted("explain".into())]);
    for _ in 0..50 {
        apply(&mut opened, chunks(100));
    }
    let scroller = opened
        .view
        .read_with(opened.cx, |view, _| view.scroller.clone());
    scroller.read_with(opened.cx, |scroller, _| {
        assert!(
            scroller.is_following_tail(),
            "the list follows its end while the answer streams"
        );
        assert!(!scroller.is_scrolled_up());
    });
    assert!(opened.cx.debug_bounds("thread-jump").is_none());

    let wheel = ScrollEvent {
        position: point(px(300.), px(300.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(400.))),
        ..ScrollEvent::default()
    };
    opened.cx.simulate_event(wheel);
    draw(opened.cx);
    let top = scroller.read_with(opened.cx, |scroller, _| {
        assert!(scroller.is_scrolled_up(), "a scroll up leaves the end");
        assert!(!scroller.is_following_tail());
        scroller.logical_scroll_top()
    });

    apply(&mut opened, chunks(200));
    scroller.read_with(opened.cx, |scroller, _| {
        let now = scroller.logical_scroll_top();
        assert_eq!(
            (now.item_ix, now.offset_in_item),
            (top.item_ix, top.offset_in_item)
        );
        assert!(
            scroller.is_scrolled_up(),
            "new words do not pull the person back to the end"
        );
    });

    // The jump button fades in over the base duration.
    opened
        .cx
        .executor()
        .advance_clock(crate::theme::DURATION_BASE * 2);
    draw(opened.cx);
    click(opened.cx, "thread-jump");
    scroller.read_with(opened.cx, |scroller, _| {
        assert!(
            scroller.is_following_tail(),
            "\"Vai in fondo\" follows the end again"
        );
        assert!(!scroller.is_scrolled_up());
    });
}

#[gpui::test]
fn thought_is_open_while_it_streams_closed_after_and_a_click_reopens_it(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let thought = ThreadEvent::Update(Update::Thought("thinking".into()));
    apply(
        &mut opened,
        vec![ThreadEvent::Prompted("x".into()), thought],
    );
    let id = opened
        .view
        .read_with(opened.cx, |view, _| view.thread.entries()[1].id);
    let is_open = |opened: &mut Opened| {
        opened.view.read_with(opened.cx, |view, _| {
            let EntryKind::Thought(message) = &view.thread.entries()[1].kind else {
                panic!("a thought");
            };
            view.is_open(id, message.is_streaming)
        })
    };
    assert!(is_open(&mut opened));
    apply(&mut opened, chunks(1));
    assert!(
        !is_open(&mut opened),
        "the answer started: the thought closes"
    );
    let selector: &'static str = format!("thread-fold-{id}").leak();
    click(opened.cx, selector);
    assert!(is_open(&mut opened));
    click(opened.cx, selector);
    assert!(!is_open(&mut opened));
}

#[gpui::test]
fn permission_request_tells_the_header_and_shows_a_button_per_choice(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let told = Rc::new(RefCell::new(Vec::new()));
    let heard = told.clone();
    let _subscription = opened.cx.update(|_, cx| {
        cx.subscribe(&opened.view, move |_, event: &ThreadViewEvent, _| {
            heard.borrow_mut().push(*event);
        })
    });
    apply(
        &mut opened,
        vec![ThreadEvent::Prompted("scarica".into()), asked(7)],
    );
    assert!(opened
        .view
        .read_with(opened.cx, |view, _| view.waits_for_person()));
    assert!(opened.cx.debug_bounds("thread-permission-7-0").is_some());
    assert!(opened.cx.debug_bounds("thread-permission-7-1").is_some());
    assert!(opened.cx.debug_bounds("thread-permission-7-2").is_none());

    let answered = ThreadEvent::PermissionAnswered {
        request: 7,
        outcome: PermissionOutcome::Selected(OptionKind::AllowOnce),
    };
    apply(&mut opened, vec![answered]);
    assert!(!opened
        .view
        .read_with(opened.cx, |view, _| view.waits_for_person()));
    assert!(
        opened.cx.debug_bounds("thread-permission-7-0").is_none(),
        "an answered card is one line"
    );
    let events = [
        ThreadViewEvent::WaitsForPerson(true),
        ThreadViewEvent::WaitsForPerson(false),
    ];
    assert_eq!(*told.borrow(), events);
}

#[gpui::test]
fn replayed_session_is_drawn_whole_with_a_row_per_entry(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    apply(&mut opened, vec![ThreadEvent::Prompted("old".into())]);
    let replay = vec![
        ThreadEvent::ReplayStarted,
        ThreadEvent::Update(Update::UserText("Fix the sorting".into())),
        ThreadEvent::Update(Update::Thought("thinking".into())),
        ThreadEvent::Update(Update::AgentText("Done".into())),
        ThreadEvent::ReplayFinished,
    ];
    apply(&mut opened, replay);
    opened.view.read_with(opened.cx, |view, cx| {
        assert_eq!(view.thread.entries().len(), 3);
        assert_eq!(view.scroller.read(cx).item_count(), 3);
        assert_eq!(
            view.rows.len(),
            3,
            "rows of the thread before the replay are gone"
        );
        assert!(view.thread.entries().iter().all(|entry| entry.is_replayed));
    });
}

#[gpui::test]
fn enter_sends_the_prompt_at_once_and_shift_enter_does_not(cx: &mut TestAppContext) {
    let opened = open_with(cx, SILENT_PROGRAM);
    click(opened.cx, "thread-composer");
    opened.cx.simulate_input("line one");
    opened.cx.simulate_keystrokes("shift-enter");
    opened.cx.simulate_input("line two");
    draw(opened.cx);
    opened.view.read_with(opened.cx, |view, _| {
        assert!(
            view.thread.entries().is_empty(),
            "shift-enter breaks the line"
        );
    });
    assert_eq!(opened.seen.launches.get(), 0);

    opened.cx.simulate_keystrokes("enter");
    opened.view.read_with(opened.cx, |view, cx| {
        let entries = view.thread.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].kind,
            EntryKind::User("line one\nline two".into())
        );
        assert_eq!(
            view.thread.turn(),
            Turn::Running,
            "the turn shows before the agent answers"
        );
        assert_eq!(view.phase(), &Phase::Connecting);
        assert!(
            composed(&view.composer, cx).is_empty(),
            "the composer is emptied"
        );
    });
    assert_eq!(
        opened.seen.launches.get(),
        1,
        "the first prompt starts the agent"
    );
}

#[gpui::test]
fn agent_that_does_not_start_ends_the_turn_and_offers_a_restart(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened.view.update_in(opened.cx, |view, window, cx| {
        view.write("wave", window, cx);
        view.send(window, cx);
    });
    let mut is_ended = false;
    // Waits on the view's phase; the deadline only bounds a hang on a busy machine.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        draw(opened.cx);
        opened
            .cx
            .executor()
            .advance_clock(crate::theme::thread::EVENT_FRAME);
        is_ended = opened
            .view
            .read_with(opened.cx, |view, _| matches!(view.phase(), Phase::Ended(_)));
        if is_ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(WAIT_STEP_MS));
    }
    assert!(is_ended, "the agent's end reached the view");
    draw(opened.cx);
    opened.view.read_with(opened.cx, |view, _| {
        let Phase::Ended(problem) = view.phase() else {
            panic!("ended");
        };
        assert_eq!(problem.kind, ProblemKind::NotStarted);
        assert_eq!(view.thread.turn(), Turn::Idle);
        assert!(matches!(
            view.thread.entries().last().unwrap().kind,
            EntryKind::Problem(_)
        ));
    });
    click(opened.cx, "thread-restart");
    assert_eq!(
        opened.seen.launches.get(),
        2,
        "\"Riavvia\" starts the agent again"
    );
}

#[gpui::test]
fn sign_in_needed_shows_the_way_to_sign_in_and_to_try_again(cx: &mut TestAppContext) {
    let opened = open(cx);
    opened.view.update(opened.cx, |view, cx| {
        view.receive(vec![sakura_acp::connection::AgentEvent::AuthRequired], cx);
    });
    draw(opened.cx);
    opened.view.read_with(opened.cx, |view, _| {
        assert_eq!(view.phase(), &Phase::SignIn)
    });
    click(opened.cx, "thread-sign-in");
    assert_eq!(opened.seen.signed_in.get(), 1);
    assert!(opened.cx.debug_bounds("thread-retry").is_some());
}

#[gpui::test]
fn permission_card_keeps_showing_what_was_asked_when_the_call_changes_under_it(
    cx: &mut TestAppContext,
) {
    use super::permission::{subject_lines, SubjectLine};
    let mut opened = open(cx);
    let asked_about = ToolCallFields {
        title: Some("Run the tests".to_string()),
        kind: Some(sakura_acp::model::ToolKind::Execute),
        raw_input: Some(serde_json::json!({ "command": ["curl", "example.com"] })),
        ..ToolCallFields::new("run")
    };
    let asked = ThreadEvent::PermissionAsked {
        request: 3,
        subject: Subject::of(&asked_about),
        tool: asked_about,
        reason: Reason::Network,
        options: Vec::new(),
    };
    let lines = |opened: &mut Opened| {
        opened.view.read_with(opened.cx, |view, cx| {
            let EntryKind::Permission(permission) = &view.thread.entries()[2].kind else {
                panic!("a permission");
            };
            subject_lines(
                &permission.subject,
                std::path::Path::new(&(view.host.workspace)(cx)),
            )
        })
    };
    apply(&mut opened, vec![ThreadEvent::Prompted("x".into()), asked]);
    let before = lines(&mut opened);
    assert_eq!(
        before,
        [SubjectLine::Command("curl example.com".to_string())]
    );

    let changed = ThreadEvent::Update(Update::ToolCallUpdate(ToolCallFields {
        title: Some("cargo test".to_string()),
        raw_input: Some(serde_json::json!({ "command": "cargo test" })),
        ..ToolCallFields::new("run")
    }));
    apply(&mut opened, vec![changed]);
    assert_eq!(
        lines(&mut opened),
        before,
        "the card still shows the command that was requested"
    );
}

/// An answer in the chunks a model would send it in.
fn answer_in_chunks(text: &str) -> Vec<ThreadEvent> {
    text.split_inclusive(' ')
        .map(|chunk| ThreadEvent::Update(Update::AgentText(chunk.to_string())))
        .collect()
}

const LONG_ANSWER: &str = "## How it works\n\nFirst paragraph with **bold** and `code`.\n\n\
    - one\n- two\n\n```rust\nfn sort() {}\n\nfn other() {}\n```\n\nSecond paragraph.\n\n\
    **Third** paragraph, starting in bold.\n\n### Section\n\nAfter the section.\n\nLast.";

#[gpui::test]
fn answer_is_drawn_in_parts_as_rows_of_their_own_and_other_entries_keep_their_place(
    cx: &mut TestAppContext,
) {
    let mut opened = open(cx);
    apply(&mut opened, vec![ThreadEvent::Prompted("explain".into())]);
    for chunk in answer_in_chunks(LONG_ANSWER) {
        apply(&mut opened, vec![chunk]);
    }
    let tool = ThreadEvent::Update(Update::ToolCall(ToolCallFields {
        title: Some("Read".to_string()),
        ..ToolCallFields::new("read")
    }));
    apply(
        &mut opened,
        vec![
            tool,
            ThreadEvent::TurnEnded(sakura_acp::model::StopReason::EndTurn),
        ],
    );
    opened.view.read_with(opened.cx, |view, cx| {
        // The heading stays with its paragraph and the list; the fence, the two paragraphs after it
        // (the second with the section and what follows it) and the last one are parts of their own.
        assert_eq!(
            view.spans,
            [1, 5, 1],
            "prompt, the answer's parts, the tool call"
        );
        assert_eq!(view.starts, [0, 1, 6]);
        assert_eq!(view.scroller.read(cx).item_count(), 7);
        assert_eq!(view.locate(0), Some((0, 0)));
        assert_eq!(view.locate(1), Some((1, 0)));
        assert_eq!(view.locate(5), Some((1, 4)));
        assert_eq!(view.locate(6), Some((2, 0)));
        assert_eq!(view.locate(7), None);
    });
}

#[gpui::test]
fn answer_streamed_and_answer_replayed_whole_are_cut_into_the_same_rows(cx: &mut TestAppContext) {
    let mut streamed = open(cx);
    apply(&mut streamed, vec![ThreadEvent::Prompted("explain".into())]);
    apply(&mut streamed, answer_in_chunks(LONG_ANSWER));
    apply(
        &mut streamed,
        vec![ThreadEvent::TurnEnded(
            sakura_acp::model::StopReason::EndTurn,
        )],
    );
    let live = streamed
        .view
        .read_with(streamed.cx, |view, _| view.spans.clone());

    let replay = vec![
        ThreadEvent::ReplayStarted,
        ThreadEvent::Update(Update::UserText("explain".into())),
        ThreadEvent::Update(Update::AgentText(LONG_ANSWER.to_string())),
        ThreadEvent::ReplayFinished,
    ];
    apply(&mut streamed, replay);
    let replayed = streamed
        .view
        .read_with(streamed.cx, |view, _| view.spans.clone());
    assert_eq!(live, replayed);
}

#[gpui::test]
fn answer_that_defines_a_reference_becomes_one_row_again(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    apply(&mut opened, vec![ThreadEvent::Prompted("x".into())]);
    apply(
        &mut opened,
        answer_in_chunks("See [here][1].\n\nSecond.\n\nThird.\n\n"),
    );
    assert_eq!(
        opened
            .view
            .read_with(opened.cx, |view, _| view.spans.clone()),
        [1, 3]
    );
    apply(
        &mut opened,
        answer_in_chunks("[1]: https://example.com\n\nFourth.\n"),
    );
    opened.view.read_with(opened.cx, |view, cx| {
        assert_eq!(
            view.spans,
            [1, 1],
            "the parts would not share the reference"
        );
        assert_eq!(view.scroller.read(cx).item_count(), 2);
    });
}

/// The same markdown drawn whole and drawn in parts, side by side, to compare what they take.
struct Compared {
    whole: Entity<gpui_component::text::TextViewState>,
    parts: Vec<Entity<gpui_component::text::TextViewState>>,
}

impl Render for Compared {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui::InteractiveElement as _;
        use gpui_component::text::TextView;
        let column = || div().w(px(360.)).flex_none();
        let parts = self.parts.iter().enumerate().map(|(at, part)| {
            let gap = if at == 0 {
                gpui::rems(0.)
            } else {
                crate::theme::thread::ANSWER_PART_GAP
            };
            div().pt(gap).child(TextView::new(part))
        });
        div()
            .flex()
            .items_start()
            .child(
                column()
                    .debug_selector(|| "whole".to_string())
                    .child(TextView::new(&self.whole)),
            )
            .child(
                column()
                    .debug_selector(|| "parts".to_string())
                    .children(parts),
            )
    }
}

#[gpui::test]
fn answer_drawn_in_parts_is_as_tall_as_the_same_answer_drawn_whole(cx: &mut TestAppContext) {
    use super::segments::{Segmenter, Step};
    use gpui_component::text::TextViewState;
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let mut texts = vec![String::new()];
    let mut segmenter = Segmenter::default();
    let mut steps = segmenter.feed(LONG_ANSWER);
    steps.extend(segmenter.finish(LONG_ANSWER));
    for step in steps {
        match step {
            Step::Text(range) => texts.last_mut().unwrap().push_str(&LONG_ANSWER[range]),
            Step::Cut => texts.push(String::new()),
            Step::Literal(more) => texts.last_mut().unwrap().push_str(&more),
        }
    }
    assert_eq!(texts.len(), 5);
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let state = |text: &str, cx: &mut gpui::App| cx.new(|cx| TextViewState::markdown(text, cx));
        let compared = Compared {
            whole: state(LONG_ANSWER, cx),
            parts: texts.iter().map(|text| state(text, cx)).collect(),
        };
        let compared = cx.new(|_| compared);
        gpui_component::Root::new(compared, window, cx)
    });
    for _ in 0..3 {
        draw(cx);
    }
    let height = |cx: &mut VisualTestContext, name: &'static str| {
        let bounds = cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not drawn"));
        f32::from(bounds.size.height)
    };
    let (whole, parts) = (height(cx, "whole"), height(cx, "parts"));
    assert!(whole > 200., "the answer was laid out: {whole}");
    assert!(
        (whole - parts).abs() < 0.5,
        "whole {whole} px, in parts {parts} px"
    );
}

#[test]
fn answer_part_gap_is_the_paragraph_gap_of_the_markdown_view() {
    let gap = gpui_base::text::TextViewStyle::default().paragraph_gap();
    assert_eq!(gap, crate::theme::thread::ANSWER_PART_GAP);
}

#[gpui::test]
fn a_message_that_waits_for_the_agent_to_start_reads_as_work_not_as_preparation(
    cx: &mut TestAppContext,
) {
    let opened = open_with(cx, SILENT_PROGRAM);
    opened.view.update_in(opened.cx, |view, window, cx| {
        view.write("new", window, cx);
        view.send(window, cx);
    });
    draw(opened.cx);
    assert_eq!(
        opened
            .view
            .read_with(opened.cx, |view, _| view.phase().clone()),
        Phase::Connecting
    );
    assert_eq!(
        opened.view.read_with(opened.cx, |view, _| view.told_live),
        sakura_core::model::Live::Working,
        "the header says one state while the line over the composer says the agent is starting"
    );
}

#[gpui::test]
fn prompt_sent_while_a_stored_session_opens_is_shown_after_the_replay_and_its_turn_runs(
    cx: &mut TestAppContext,
) {
    use sakura_acp::connection::AgentEvent;
    let opened = open_with(cx, SILENT_PROGRAM);
    opened.view.update_in(opened.cx, |view, window, cx| {
        view.write("new", window, cx);
        view.send(window, cx);
    });
    let replay = [
        ThreadEvent::ReplayStarted,
        ThreadEvent::Update(Update::UserText("old".into())),
        ThreadEvent::Update(Update::AgentText("Done".into())),
        ThreadEvent::ReplayFinished,
    ];
    let opening = AgentEvent::SessionOpened {
        session_id: "stored".into(),
        is_loaded: true,
    };
    let events = replay.into_iter().map(AgentEvent::Thread).chain([opening]);
    opened.view.update(opened.cx, |view, cx| {
        view.receive(events.collect(), cx);
    });
    draw(opened.cx);
    opened.view.read_with(opened.cx, |view, cx| {
        let entries = view.thread.entries();
        assert_eq!(
            entries.len(),
            3,
            "the replayed turn, then the prompt that waited"
        );
        assert_eq!(entries[2].kind, EntryKind::User("new".into()));
        assert_eq!(view.thread.turn(), Turn::Running, "the agent works on it");
        assert!(!view.can_send(cx), "no second prompt while it does");
    });
}

/// Clicks where the text of the answer part `selector` starts.
fn click_start_of(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not on screen"));
    let inside = bounds.origin + point(px(6.), bounds.size.height / 2.);
    cx.simulate_click(inside, Modifiers::none());
    draw(cx);
}

#[gpui::test]
fn link_in_an_answer_opens_only_a_web_address(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let said = |text: &str| {
        vec![
            ThreadEvent::Prompted("vai".into()),
            ThreadEvent::Update(Update::AgentText(text.to_string())),
            ThreadEvent::TurnEnded(sakura_acp::model::StopReason::EndTurn),
        ]
    };
    apply(&mut opened, said("[the password file](file:///etc/passwd)"));
    click_start_of(opened.cx, "thread-answer-1-0");
    assert_eq!(
        opened.cx.opened_url(),
        None,
        "a file address is never opened"
    );
    apply(
        &mut opened,
        said("[un programma](x-apple-systempreferences:com.apple.x)"),
    );
    click_start_of(opened.cx, "thread-answer-3-0");
    assert_eq!(
        opened.cx.opened_url(),
        None,
        "nor another program's address"
    );
    apply(
        &mut opened,
        said("[la documentazione](https://example.com/guida)"),
    );
    click_start_of(opened.cx, "thread-answer-5-0");
    assert_eq!(
        opened.cx.opened_url().as_deref(),
        Some("https://example.com/guida")
    );
}

/// The problem a test shows: a refusal with the provider's own words.
fn refused(detail: &str) -> ThreadEvent {
    ThreadEvent::Failed(Problem {
        kind: ProblemKind::Refusal,
        detail: detail.to_string(),
    })
}

#[gpui::test]
fn refusal_is_one_plain_line_and_the_providers_words_open_under_details(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    let words = "API Error: unable to respond (request req_011, see https://example.com/aup)";
    apply(
        &mut opened,
        vec![ThreadEvent::Prompted("x".into()), refused(words)],
    );
    let id = opened
        .view
        .read_with(opened.cx, |view, _| view.thread.entries()[1].id);
    let sentence = opened
        .cx
        .debug_bounds("thread-problem-sentence")
        .expect("the refusal is said");
    assert!(
        opened.cx.debug_bounds("thread-problem-detail").is_none(),
        "the provider's words are not shown until requested"
    );
    let selector: &'static str = format!("thread-fold-{id}").leak();
    let fold = opened.cx.debug_bounds(selector).expect("\"Details\"");
    assert!(
        fold.top() >= sentence.bottom(),
        "\"Details\" sits under the sentence"
    );
    click(opened.cx, selector);
    assert!(opened.cx.debug_bounds("thread-problem-detail").is_some());
    click(opened.cx, selector);
    assert!(opened.cx.debug_bounds("thread-problem-detail").is_none());
}

#[gpui::test]
fn problem_with_nothing_more_to_say_offers_no_details(cx: &mut TestAppContext) {
    let mut opened = open(cx);
    apply(
        &mut opened,
        vec![ThreadEvent::Prompted("x".into()), refused("")],
    );
    let id = opened
        .view
        .read_with(opened.cx, |view, _| view.thread.entries()[1].id);
    assert!(opened.cx.debug_bounds("thread-problem-sentence").is_some());
    let selector: &'static str = format!("thread-fold-{id}").leak();
    assert!(opened.cx.debug_bounds(selector).is_none());
}

#[gpui::test]
fn answer_keeps_the_line_breaks_the_agent_wrote(cx: &mut TestAppContext) {
    use super::answer::Answer;
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
    });
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let one_line = Answer::new("one two three", false, true, cx);
        let three_lines = Answer::new("one\ntwo\nthree", false, true, cx);
        let compared = Compared {
            whole: one_line.parts()[0].clone(),
            parts: three_lines.parts().to_vec(),
        };
        let compared = cx.new(|_| compared);
        gpui_component::Root::new(compared, window, cx)
    });
    for _ in 0..3 {
        draw(cx);
    }
    let height = |cx: &mut VisualTestContext, name: &'static str| {
        f32::from(cx.debug_bounds(name).expect("drawn").size.height)
    };
    let (one, three) = (height(cx, "whole"), height(cx, "parts"));
    assert!(one > 0., "the answer was laid out");
    assert!(
        (three - 3. * one).abs() < 1.,
        "three lines the agent wrote are three lines: one line {one} px, the three {three} px"
    );
}

#[gpui::test]
fn kept_conversation_is_asked_for_once_by_being_shown(cx: &mut TestAppContext) {
    let opened = open_with(cx, SILENT_PROGRAM);
    opened.view.update(opened.cx, |view, cx| view.load_kept(cx));
    assert_eq!(
        opened.seen.launches.get(),
        0,
        "nothing kept: nothing starts"
    );
    opened.view.update(opened.cx, |view, cx| {
        view.host.stored_session = Box::new(|_| Some("kept".to_string()));
        view.load_kept(cx);
        view.load_kept(cx);
    });
    assert_eq!(opened.seen.launches.get(), 1);
    assert_eq!(
        opened
            .view
            .read_with(opened.cx, |view, _| view.phase().clone()),
        Phase::Connecting
    );
    assert_eq!(
        opened.view.read_with(opened.cx, |view, _| view.told_live),
        sakura_core::model::Live::Unknown,
        "showing a conversation again is no work of the agent: the lavoro's state stays what it was"
    );
    // An agent that ended without a word leaves the conversation idle and still empty.
    opened.view.update(opened.cx, |view, cx| {
        view.phase = Phase::Idle;
        view.load_kept(cx);
    });
    assert_eq!(
        opened.seen.launches.get(),
        1,
        "looking again starts nothing"
    );
}

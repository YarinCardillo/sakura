//! The Engine page's layout with the words of the real text system, in both languages and at both ends of
//! the window's width: no text box of the pace card touches another or leaves the card.

use super::tests::open_with_rules;
use crate::theme;
use crate::ui::engine_fixture_tests::owner_accounts;
use crate::ui::lane_tests::bounds;
use crate::ui::real_text;
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext};

/// The accounts of the fixture, as `owner_accounts` names them.
const ACCOUNTS: [&str; 3] = ["claude-a", "codex-b", "claude-c"];
/// How far two boxes may overlap before the layout counts them as touching (anti-aliasing, not layout).
const TOLERANCE: f32 = 0.5;

/// The boxes of the pace card that hold text or a drawing of their own, by selector.
fn pace_parts() -> Vec<String> {
    let mut parts: Vec<String> = [
        "engine-pace-word",
        "engine-pace-sentence",
        "engine-pace-slider",
        "engine-pace-ends",
        "engine-usage-updated",
        "engine-lanes-title",
        "engine-lanes-controls",
    ]
    .map(str::to_string)
    .into();
    for account in ACCOUNTS {
        for part in ["name", "plan", "bar", "ends", "resets"] {
            parts.push(format!("engine-account-{account}-{part}"));
        }
    }
    parts
}

fn overlap(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    let tolerance = Pixels::from(TOLERANCE);
    a.left() < b.right() - tolerance
        && b.left() < a.right() - tolerance
        && a.top() < b.bottom() - tolerance
        && b.top() < a.bottom() - tolerance
}

fn measured(cx: &mut VisualTestContext, parts: &[String]) -> Vec<(String, Bounds<Pixels>)> {
    parts
        .iter()
        .map(|part| {
            let found = bounds(cx, part).unwrap_or_else(|| panic!("{part} is not on the page"));
            (part.clone(), found)
        })
        .collect()
}

fn assert_pace_card_holds(cx: &mut VisualTestContext, width: f32) {
    cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(2400.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = bounds(cx, "engine-card-pace").expect("the pace card");
    let found = measured(cx, &pace_parts());
    for (name, part) in &found {
        assert!(
            part.left() >= card.left() && part.right() <= card.right(),
            "{name} {part:?} leaves the card {card:?} at {width} wide"
        );
    }
    for (nth, (first, a)) in found.iter().enumerate() {
        for (second, b) in &found[nth + 1..] {
            assert!(
                !overlap(a, b),
                "{first} {a:?} and {second} {b:?} overlap at {width} wide"
            );
        }
    }
}

fn open_owner_page(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_rig, _desk, cx) = open_with_rules(cx, &owner_accounts());
    cx
}

#[test]
fn no_text_of_the_pace_card_touches_another_with_the_owner_s_three_accounts_at_1280() {
    real_text::run(|cx| assert_pace_card_holds(open_owner_page(cx), 1280.));
}

#[test]
fn no_text_of_the_pace_card_touches_another_at_the_window_s_least_width() {
    real_text::run(|cx| {
        let least = f32::from(theme::WINDOW_MIN_WIDTH);
        assert_pace_card_holds(open_owner_page(cx), least);
    });
}

#[test]
fn the_lanes_at_once_row_stands_below_the_accounts_and_their_caption() {
    real_text::run(|cx| {
        let cx = open_owner_page(cx);
        let updated = bounds(cx, "engine-usage-updated").expect("the caption");
        let lanes = bounds(cx, "engine-lanes-title").expect("the lanes row");
        assert!(
            lanes.top() >= updated.bottom(),
            "{lanes:?} must start below {updated:?}"
        );
    });
}

/// The nodes of the drawn pipeline, by the id the window knows them as.
const NODES: [&str; 9] = [
    "start",
    "plan",
    "plan_review",
    "build",
    "task_review",
    "final_review",
    "checks",
    "deliver",
    "guard",
];
/// The steps, which are drawn as one kind, and the reviews, which are another.
const STEPS: [&str; 5] = ["start", "plan", "build", "checks", "deliver"];
const REVIEWS: [&str; 3] = ["plan_review", "task_review", "final_review"];

#[test]
fn the_text_of_every_node_keeps_the_padding_token_from_each_side_of_its_card() {
    real_text::run(|cx| {
        let (_rig, _desk, cx) = open_with_rules(cx, &crate::ui::engine_fixture_tests::full());
        let inset = f32::from(crate::theme::engine_page::NODE_PADDING);
        for node in NODES {
            let card = bounds(cx, &format!("engine-node-{node}")).expect(node);
            let text = bounds(cx, &format!("engine-node-{node}-text")).expect(node);
            let sides = [
                ("left", text.left() - card.left()),
                ("right", card.right() - text.right()),
                ("top", text.top() - card.top()),
                ("bottom", card.bottom() - text.bottom()),
            ];
            for (side, gap) in sides {
                assert!(
                    f32::from(gap) >= inset - TOLERANCE,
                    "{node}: the text is {gap:?} from the {side} edge, under {inset} points; card {card:?}, text {text:?}"
                );
            }
        }
    });
}

#[test]
fn nodes_of_the_same_kind_are_the_same_width() {
    real_text::run(|cx| {
        let (_rig, _desk, cx) = open_with_rules(cx, &crate::ui::engine_fixture_tests::full());
        for kind in [&STEPS[..], &REVIEWS[..]] {
            let widths: Vec<_> = kind
                .iter()
                .map(|node| {
                    bounds(cx, &format!("engine-node-{node}"))
                        .expect(node)
                        .size
                        .width
                })
                .collect();
            assert!(
                widths.iter().all(|width| *width == widths[0]),
                "{kind:?} are {widths:?} wide"
            );
        }
    });
}

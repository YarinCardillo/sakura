//! Every dialog of the engine's window stands in one place: top-anchored at the offset the command palette
//! has, whatever its width, so a sheet, a modal and a confirmation open where the eye already is.

use super::engine_rig_tests::{wait_until, SNAPSHOT};
use super::lane_tests::{bounds, click, open_answering};
use crate::theme::{DIALOG_TOP, DIALOG_WIDTH_LARGE, DIALOG_WIDTH_MEDIUM, DIALOG_WIDTH_SMALL};
use gpui::{TestAppContext, VisualTestContext};
use sakura_core::tabs::Page;

const ANSWER: &str = r#"{"result": "done", "exit_code": 0, "output": "ok\n"}"#;
const NOTHING: &str = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [],
    "projects": []}"#;

/// Whether the dialog drawn as `selector` has its top edge at the shared offset, to the pixel the screen snaps to.
fn stands_at_the_offset(cx: &mut VisualTestContext, selector: &str) -> bool {
    let top = f32::from(bounds(cx, selector).expect(selector).top());
    (top - f32::from(crate::theme::DIALOG_SHEET_TOP)).abs() <= 1.
}

#[gpui::test]
fn the_new_lane_sheet_stands_at_the_shared_offset(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    assert!(stands_at_the_offset(cx, "new-lane-sheet"));
    assert_eq!(
        bounds(cx, "new-lane-sheet").unwrap().size.width,
        DIALOG_WIDTH_MEDIUM
    );
}

#[gpui::test]
fn the_add_a_project_first_dialog_stands_at_the_shared_offset(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, NOTHING, ANSWER, ANSWER);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("new-lane-no-project").is_some()
    });
    assert!(stands_at_the_offset(cx, "new-lane-no-project"));
    assert_eq!(
        bounds(cx, "new-lane-no-project").unwrap().size.width,
        DIALOG_WIDTH_SMALL
    );
}

#[gpui::test]
fn settings_stands_at_the_shared_offset(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
    wait_until(cx, "the modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    assert!(stands_at_the_offset(cx, "settings-modal"));
    assert_eq!(
        bounds(cx, "settings-modal").unwrap().size.width,
        DIALOG_WIDTH_LARGE
    );
}

#[gpui::test]
fn the_digest_dialog_stands_at_the_shared_offset(cx: &mut TestAppContext) {
    let (_rig, desk, cx) =
        open_answering(cx, &crate::ui::engine_fixture_tests::full(), ANSWER, ANSWER);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(2400.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "the row", |cx| {
        cx.debug_bounds("engine-digest-row").is_some()
    });
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    assert!(stands_at_the_offset(cx, "engine-digest-dialog"));
}

#[gpui::test]
fn the_digest_scrolls_long_sources_between_its_header_and_footer_at_1280_by_800(
    cx: &mut TestAppContext,
) {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&crate::ui::engine_fixture_tests::full()).unwrap();
    let sources = snapshot["digest"]["sources"].as_array_mut().unwrap();
    let sample = sources[0].clone();
    sources.extend((0..24).map(|index| {
        let mut source = sample.clone();
        source["id"] = format!("extra-{index}").into();
        source
    }));
    let (_rig, desk, cx) = open_answering(cx, &snapshot.to_string(), ANSWER, ANSWER);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(2400.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "digest row", |cx| {
        bounds(cx, "engine-digest-row").is_some()
    });
    click(cx, "engine-digest-row");
    wait_until(cx, "digest dialog", |cx| {
        bounds(cx, "engine-digest-dialog").is_some()
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    wait_until(cx, "digest dialog", |cx| {
        bounds(cx, "engine-digest-run").is_some()
    });
    for language in [
        crate::l10n::Language::English,
        crate::l10n::Language::Italian,
    ] {
        crate::l10n::set_for_test(language);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let dialog = bounds(cx, "engine-digest-dialog").unwrap();
        let footer = bounds(cx, "engine-digest-run").unwrap();
        assert!(footer.bottom() <= dialog.bottom() && dialog.bottom() <= gpui::px(800.));
        let row = bounds(cx, "engine-digest-source-github").unwrap();
        cx.simulate_event(gpui::ScrollEvent {
            position: row.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-100.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(bounds(cx, "engine-digest-source-github").unwrap().top() < row.top());
        assert_eq!(bounds(cx, "engine-digest-run"), Some(footer));
        assert!(stands_at_the_offset(cx, "engine-digest-dialog"));
    }
}

#[test]
fn the_command_palette_keeps_its_launcher_offset() {
    assert_eq!(crate::theme::palette::COMMAND_TOP, f32::from(DIALOG_TOP));
}

#[gpui::test]
fn modal_from_either_pane_stays_top_anchored_and_blocks_tab_commands(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    let (left, right) =
        super::engine_desk_tests::make_split(&desk, Page::Inbox, Page::Engine, 5000, cx);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    for id in [left, right] {
        bar.update(cx, |bar, cx| {
            bar.activate(id, cx);
        });
        cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
        wait_until(cx, "settings", |cx| bounds(cx, "settings-modal").is_some());
        assert!(stands_at_the_offset(cx, "settings-modal"));
        assert!(
            !cx.update(|window, cx| desk.update(cx, |desk, cx| desk.focus_other_pane(window, cx)))
        );
        cx.simulate_keystrokes("alt-cmd-j cmd-w");
        bar.read_with(cx, |bar, _| {
            assert_eq!(bar.tabs().active().id(), id);
            assert_eq!(bar.tabs().visible(), (left, Some(right)));
        });
        cx.simulate_keystrokes("escape");
        wait_until(cx, "settings closed", |cx| {
            bounds(cx, "settings-modal").is_none()
        });
    }
}

#[gpui::test]
fn new_lane_sheet_blocks_tab_commands_when_split(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    let (left, right) =
        super::engine_desk_tests::make_split(&desk, Page::Inbox, Page::Engine, 5000, cx);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "new lane", |cx| bounds(cx, "new-lane-sheet").is_some());
    assert!(!cx.update(|window, cx| desk.update(cx, |desk, cx| desk.focus_other_pane(window, cx))));
    cx.simulate_keystrokes("alt-cmd-j cmd-w");
    desk.read_with(cx, |desk, cx| {
        assert_eq!(desk.bar.read(cx).tabs().visible(), (left, Some(right)))
    });
}

#[gpui::test]
fn the_dark_dialog_surface_has_a_visible_face_and_edge_over_the_scrim(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    cx.update(|_, cx| crate::theme::Theme::set(crate::theme::Appearance::Dark, cx));
    let sheet = bounds(cx, "new-lane-sheet").unwrap();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let painted = window.painted_quads();
        let palette = crate::theme::Palette::dark();
        let layer = sheet.scale(window.scale_factor());
        let face = painted
            .iter()
            .find_map(|quad| {
                (quad.bounds == layer
                    && quad.background.as_solid() == Some(palette.overlay_surface))
                .then_some(palette.overlay_surface)
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing overlay fill: {:?}",
                    painted
                        .iter()
                        .filter(|quad| quad.bounds == layer)
                        .collect::<Vec<_>>()
                )
            });
        let edge = painted
            .iter()
            .find(|quad| {
                quad.bounds == layer
                    && quad.border_color == palette.overlay_edge
                    && quad.border_widths.top.0 > 0.
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing overlay edge: {:?}",
                    painted
                        .iter()
                        .filter(|quad| quad.bounds == layer)
                        .collect::<Vec<_>>()
                )
            });
        let canvas: gpui::Rgba = palette.canvas.into();
        let scrim: gpui::Rgba = palette.scrim.into();
        let behind: gpui::Hsla = gpui::Rgba {
            r: scrim.r * scrim.a + canvas.r * (1. - scrim.a),
            g: scrim.g * scrim.a + canvas.g * (1. - scrim.a),
            b: scrim.b * scrim.a + canvas.b * (1. - scrim.a),
            a: 1.,
        }
        .into();
        assert!(
            crate::theme_contrast::contrast(face, behind) >= 1.6,
            "{face:?}"
        );
        assert!(
            crate::theme_contrast::contrast(edge.border_color, face) >= 1.25,
            "{edge:?}"
        );
    });
}

#[gpui::test]
fn audit_sheets_clear_the_page_header_and_use_the_available_height(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, ANSWER, ANSWER);
    for (width, height) in [
        (
            crate::theme::WINDOW_START_WIDTH,
            crate::theme::WINDOW_START_HEIGHT,
        ),
        (
            crate::theme::WINDOW_MIN_WIDTH,
            crate::theme::WINDOW_MIN_HEIGHT,
        ),
    ] {
        cx.simulate_resize(gpui::size(width, height));
        cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
        wait_until(cx, "Settings", |cx| {
            cx.debug_bounds("settings-modal").is_some()
        });
        let panel = bounds(cx, "settings-modal").unwrap();
        let bar = bounds(cx, "page-panel").unwrap();
        assert!(panel.top() - (bar.top() + crate::theme::HEADER_HEIGHT) >= gpui::px(24.));
        let bottom = if height == crate::theme::WINDOW_MIN_HEIGHT {
            crate::theme::STATUS_BAR_HEIGHT + crate::theme::CONTENT_PADDING
        } else {
            panel.top()
        };
        assert!((bottom - (height - panel.bottom())).abs() <= gpui::px(1.));
        cx.simulate_keystrokes("escape");
    }
}

use super::engine_desk_tests;
use super::engine_fixture_tests;
use super::engine_rig_tests::{self, wait_until, SNAPSHOT};
use super::lane_tests::bounds;
use super::{account_add, kit, master_agent::MasterAgent};
use crate::theme::{self, Appearance, Theme};
use gpui::{ParentElement as _, Styled as _, TestAppContext, VisualTestContext};
use sakura_core::tabs::Page;

fn resize(window: &mut VisualTestContext, width: gpui::Pixels, height: gpui::Pixels) {
    window.simulate_resize(gpui::size(width, height));
    window.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn audit_account_sheet_fits_all_actions_and_centers_close_on_the_title(cx: &mut TestAppContext) {
    let (rig, _desk, window) = engine_desk_tests::open(cx, SNAPSHOT);
    window.update(|window, cx| {
        account_add::open_for(
            rig.source,
            rig.facts,
            MasterAgent::Claude,
            Some("Studio"),
            None,
            window,
            cx,
        );
    });
    for (width, height) in [
        (theme::WINDOW_START_WIDTH, theme::WINDOW_START_HEIGHT),
        (theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT),
    ] {
        resize(window, width, height);
        let title = bounds(window, "account-add-title").unwrap();
        let action = bounds(window, "account-add-log-in").unwrap();
        let label = bounds(window, "account-add-log-in-label").unwrap();
        let footer = bounds(window, "dialog-footer").unwrap();
        assert!(footer.left() <= action.left() && action.right() <= footer.right());
        assert!(label.size.width >= gpui::px(130.));
        let close = bounds(window, "dialog-sheet-close").expect("a dismiss in the title row");
        assert!((title.center().y - close.center().y).abs() <= gpui::px(0.5));
        assert_eq!(footer.right(), close.right());
    }
}

struct Primary;

impl gpui::Render for Primary {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div().p(gpui::px(30.)).child(kit::button(
            "audit-primary",
            "Save",
            kit::ButtonKind::Primary,
            &Theme::palette(cx),
        ))
    }
}

#[gpui::test]
fn audit_primary_paints_a_flat_fill_in_both_themes(cx: &mut TestAppContext) {
    super::engine_rig_tests::prepare(cx);
    let (_, window) = cx.add_window_view(|_, _| Primary);
    for appearance in [Appearance::Dark, Appearance::Light] {
        window.update(|window, cx| {
            Theme::set(appearance, cx);
            window.draw(cx).clear(cx);
        });
        let button = bounds(window, "audit-primary").unwrap();
        window.update(|window, cx| {
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.bounds == button.scale(window.scale_factor())
                        && quad.background.as_solid() == Some(Theme::palette(cx).primary)
                }),
                "{appearance:?} primary fill"
            );
        });
    }
}

#[gpui::test]
fn audit_engine_inspector_controls_share_a_width_and_right_edge(cx: &mut TestAppContext) {
    let fixture = engine_fixture_tests::full();
    let (_rig, desk, window) = engine_desk_tests::open(cx, &fixture);
    desk.update(window, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(window, "Build rules", |cx| {
        bounds(cx, "engine-rule-steps.build.model").is_some()
    });
    let ids = [
        "engine-rule-steps.build.model",
        "engine-rule-steps.build.account",
        "engine-rule-subagents",
    ];
    for (width, height) in [
        (theme::WINDOW_START_WIDTH, theme::WINDOW_START_HEIGHT),
        (theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT),
    ] {
        resize(window, width, height);
        let [model, account, subagents] = ids.map(|id| bounds(window, id).expect(id));
        assert_eq!(model.size.width, account.size.width);
        assert_eq!(model.size.width, subagents.size.width);
        assert_eq!(model.right(), account.right());
        assert_eq!(model.right(), subagents.right());
    }
}

#[gpui::test]
fn audit_inbox_status_and_age_columns_hold_without_a_timestamp(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    resize(
        window,
        theme::WINDOW_START_WIDTH,
        theme::WINDOW_START_HEIGHT,
    );
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(SNAPSHOT.as_bytes()).unwrap();
    let checks = snapshot
        .waits
        .iter()
        .find(|wait| wait.title == "Check row R1")
        .unwrap();
    let approval = snapshot
        .waits
        .iter()
        .find(|wait| wait.since.is_some())
        .unwrap();
    for prefix in ["state-ask", "row-age-ask"] {
        let first = bounds(window, &format!("{prefix}-{}", checks.id)).unwrap();
        let second = bounds(window, &format!("{prefix}-{}", approval.id)).unwrap();
        assert_eq!(first.right(), second.right(), "{prefix}");
        assert_eq!(first.size.width, second.size.width, "{prefix}");
    }
}

#[test]
fn audit_uses_the_owner_s_names_for_actions_and_examples() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    use crate::l10n::{tr, Key};
    assert_eq!(tr(Key("inbox.mark.done")), "Mark done");
    assert_eq!(tr(Key("inbox.view.done")), "Done");
    for key in ["sidebar.add_project", "new_lane.add_project"] {
        assert_eq!(tr(Key(key)), "Add project");
    }
    assert_eq!(
        tr(Key("scheduled.name_hint")),
        "e.g. Weekly dependency check"
    );
    assert_eq!(tr(Key("track.new")), "New track");
    assert_eq!(
        tr(Key("settings_modal.projects.add_title")),
        "Repository folder"
    );
}

#[gpui::test]
fn reaudit_retino_skips_the_status_glyphs(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    for (width, height) in [
        (theme::WINDOW_START_WIDTH, theme::WINDOW_START_HEIGHT),
        (theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT),
    ] {
        resize(window, width, height);
        let glyphs = [
            "toggle-sidebar-icon",
            "status-settings-icon",
            "status-tour-icon",
        ]
        .map(|id| bounds(window, id).expect(id).dilate(gpui::px(2.)));
        window.update(|window, cx| {
            let scale = window.scale_factor();
            let ink = Theme::palette(cx).retino_dot;
            for dot in window
                .painted_quads()
                .iter()
                .filter(|quad| quad.background.as_solid() == Some(ink))
            {
                assert!(
                    glyphs
                        .iter()
                        .all(|glyph| !dot.bounds.intersects(&glyph.scale(scale))),
                    "retino dot touches a foreground glyph at {width:?}: {:?}",
                    dot.bounds
                );
            }
        });
    }
}

#[gpui::test]
fn reaudit_retino_has_no_lone_edge_column(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    resize(
        window,
        theme::WINDOW_START_WIDTH,
        theme::WINDOW_START_HEIGHT,
    );
    window.update(|window, cx| {
        let scale = window.scale_factor();
        let ink = Theme::palette(cx).retino_dot;
        assert!(window
            .painted_quads()
            .iter()
            .filter(|quad| { quad.background.as_solid() == Some(ink) })
            .all(|dot| dot.bounds.left() >= gpui::px(4.).scale(scale)));
    });
}

#[test]
fn reaudit_dark_cyan_retains_its_accent_brightness() {
    use crate::theme_brand::Accent;
    let palette = theme::Palette::of(Appearance::Dark, Accent::InstrumentCyan);
    let rgba = gpui::Rgba::from(palette.retino_dot);
    assert!(rgba.b >= 0.55 && rgba.g >= 0.47, "{rgba:?}");
}

#[gpui::test]
fn reaudit_inbox_toast_stays_outside_the_detail_pane(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    for (width, height) in [
        (theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT),
        (theme::WINDOW_START_WIDTH, theme::WINDOW_START_HEIGHT),
    ] {
        resize(window, width, height);
        window.update(|window, cx| {
            kit::Toast::show(
                window,
                cx,
                "Marked done: Check row R1",
                Some(Box::new(|_, _| {})),
            );
            window.draw(cx).clear(cx);
        });
        let toast = bounds(window, "toast").unwrap();
        let list = bounds(window, "inbox-list").unwrap();
        let pane = bounds(window, "inbox-detail").unwrap();
        assert!(
            !pane.intersects(&toast),
            "toast {toast:?}, list {list:?}, detail {pane:?}"
        );
        assert!(!toast.intersects(&list));
    }
}

#[test]
fn reaudit_copy_names_the_actual_controls() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    use crate::l10n::{tr, Key};
    let cases = [
        ("first_launch.look_again", "Scan again"),
        ("scheduled.name", "Title"),
        ("scheduled.prompt", "Description"),
    ];
    let mismatches = cases
        .into_iter()
        .filter_map(|(key, expected)| {
            let actual = tr(Key(key));
            (actual != expected).then_some((key, actual, expected))
        })
        .collect::<Vec<_>>();
    assert!(mismatches.is_empty(), "{mismatches:?}");
    assert!(tr(Key("settings_modal.connectors.intro")).contains("the Master"));
}

#[gpui::test]
fn reaudit_missing_ages_keep_a_visible_placeholder(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    let id = r#"["wait","/work/repo","demo","owner_row","R1"]"#;
    let selector = format!("missing-age-{id}");
    assert!(bounds(window, &selector).is_some());
}

#[gpui::test]
fn reaudit3_retino_has_no_dots_along_the_sidebar_content_gutter(cx: &mut TestAppContext) {
    let tracks: serde_json::Value = serde_json::from_str(include_str!(
        "../../../sakura-core/tests/fixtures/engine-real-tracks.json"
    ))
    .unwrap();
    let fixture = engine_fixture_tests::edited(|snapshot| {
        snapshot["tracks"] = tracks["tracks"].clone();
    });
    let (_, _, window) = engine_desk_tests::open(cx, &fixture);
    resize(window, theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT);
    let projects = bounds(window, "sidebar-projects-content").unwrap();
    window.update(|window, cx| {
        let ink = Theme::palette(cx).retino_dot;
        let scale = window.scale_factor();
        let gutter = gpui::Bounds::from_corners(
            gpui::point(gpui::Pixels::ZERO, projects.top()),
            gpui::point(projects.left(), projects.bottom()),
        )
        .scale(scale);
        assert!(
            window.painted_quads().iter().all(|quad| {
                quad.background.as_solid() != Some(ink) || !quad.bounds.intersects(&gutter)
            }),
            "retino extends beside the sidebar content at {gutter:?}"
        );
    });
}

#[gpui::test]
fn reaudit3_toggle_uses_a_light_thumb_in_dark(cx: &mut TestAppContext) {
    engine_rig_tests::prepare(cx);
    cx.update(|cx| {
        use gpui_component::ActiveTheme as _;
        let thumb = gpui::Rgba::from(gpui::Hsla::from(cx.theme().tokens.switch_thumb));
        assert!(
            thumb.r >= 0.9 && thumb.g >= 0.9 && thumb.b >= 0.9,
            "{thumb:?}"
        );
    });
}

#[gpui::test]
fn reaudit3_a_non_inbox_toast_reserves_space_outside_page_rows(cx: &mut TestAppContext) {
    let fixture = engine_fixture_tests::full();
    let (_, desk, window) = engine_desk_tests::open(cx, &fixture);
    resize(window, theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT);
    desk.update(window, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(window, "engine page", |cx| {
        bounds(cx, "engine-page").is_some()
    });
    window.update(|window, cx| {
        kit::Toast::show_with_detail(
            window,
            cx,
            "Task saved",
            Some("Changes apply to new runs.".into()),
            None,
        );
        window.draw(cx).clear(cx);
    });
    let toast = bounds(window, "toast").unwrap();
    let pane = bounds(window, "engine-page").unwrap();
    assert!(
        !pane.intersects(&toast),
        "toast {toast:?} overlays page content {pane:?}"
    );
}

#[test]
fn reaudit3_owner_copy_names_master_consistently() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    use crate::l10n::{tr, Key};
    assert_eq!(tr(Key("master.placeholder")), "Ask the Master");
    assert!(tr(Key("thread.ask_master")).starts_with("Ask the Master"));
}

#[test]
fn reaudit3_project_prompt_names_the_folder_action() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    use crate::l10n::{tr, Key};
    assert_eq!(
        tr(Key("new_lane.no_project_detail")),
        "A lane starts in a Git project. Choose its folder to add it."
    );
}

#[gpui::test]
fn reaudit3_tooltip_changes_invalidate_the_desks_layer(cx: &mut TestAppContext) {
    let (_, _, window) = engine_desk_tests::open(cx, SNAPSHOT);
    window.update(|window, cx| {
        let size = window.viewport_size();
        let target = gpui::Bounds::new(
            gpui::point(gpui::px(300.), gpui::px(300.)),
            gpui::size(gpui::px(120.), gpui::px(30.)),
        );
        let tips = cx.global::<super::tip::TipsHandle>().0.clone();
        tips.update(cx, |tips, cx| {
            tips.enter(
                "fixture-hover".into(),
                super::tip::Words::plain("A tooltip"),
                (target, size),
                cx,
            );
        });
    });
    window.run_until_parked();
    window.executor().advance_clock(super::tip::COLD_DELAY);
    window.run_until_parked();
    assert!(window.update(|_, cx| cx
        .global::<super::tip::TipsHandle>()
        .0
        .read(cx)
        .shown()
        .is_some()));
    window.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        bounds(window, "tip-card").is_some(),
        "the desk did not redraw the tooltip layer"
    );
}

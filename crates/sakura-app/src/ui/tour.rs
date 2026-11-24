use super::engine_desk::Desk;
use super::first_launch::{self, Finished};
use super::master_accounts::{self, MasterAccount};
use super::settings_modal::Section;
use crate::l10n::{tr, tr_with, Key};
use crate::prefs::Preferences;
use crate::theme::{self, Palette};
use crate::ui::kit::{self, ButtonKind};
use gpui::{
    actions, deferred, div, Action as _, AnyElement, App, Context, FocusHandle, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};
use gpui_component::WindowExt as _;
use sakura_core::engine::state::EngineState;
use sakura_core::tabs::Page;
use std::{cell::Cell, rc::Rc};

const LAYER: usize = 180;

actions!(tour, [Advance, Skip]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Master,
    Lanes,
    Inbox,
    Engine,
    Connectors,
}

struct Active(Guide);

#[derive(Clone)]
struct Guide {
    step: Step,
    target: Rc<Cell<Option<overlay::Target>>>,
    focus: FocusHandle,
    has_focused: Rc<Cell<bool>>,
}

impl Global for Active {}

impl Step {
    fn next(self) -> Option<Self> {
        match self {
            Self::Master => Some(Self::Lanes),
            Self::Lanes => Some(Self::Inbox),
            Self::Inbox => Some(Self::Engine),
            Self::Engine => Some(Self::Connectors),
            Self::Connectors => None,
        }
    }

    fn page(self) -> Option<Page> {
        match self {
            Self::Master => Some(Page::Master),
            Self::Lanes => Some(Page::Lanes),
            Self::Inbox => Some(Page::Inbox),
            Self::Engine => Some(Page::Engine),
            Self::Connectors => None,
        }
    }

    fn title(self) -> Key {
        Key(match self {
            Self::Master => "page.master",
            Self::Lanes => "page.lanes",
            Self::Inbox => "page.inbox",
            Self::Engine => "page.engine",
            Self::Connectors => "settings_modal.section.connectors",
        })
    }

    fn id(self) -> &'static str {
        match self {
            Self::Master => "tour-master",
            Self::Lanes => "tour-lanes",
            Self::Inbox => "tour-inbox",
            Self::Engine => "tour-engine",
            Self::Connectors => "tour-connectors",
        }
    }
}

pub(super) fn install(cx: &mut Context<Desk>) -> Subscription {
    let desk = cx.weak_entity();
    cx.intercept_keystrokes(move |event, window, cx| {
        let Some(step) = active_step(cx) else {
            return;
        };
        if event.keystroke.key != "escape"
            || (window.has_active_dialog(cx) && step != Step::Connectors)
        {
            return;
        }
        if desk.update(cx, |desk, cx| desk.skip_tour(cx)).is_ok() {
            cx.stop_propagation();
        }
    })
}

impl Desk {
    pub fn offer_first_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = first_launch::offer(self.source.clone(), self.facts.clone(), window, cx)
        else {
            return;
        };
        cx.subscribe_in(&setup, window, |desk, _, event, window, cx| {
            if *event == Finished::Applied {
                desk.show_tour(window, cx);
            }
        })
        .detach();
    }

    pub(super) fn show_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.has_active_dialog(cx) {
            self.show_tour_step(Step::Master, window, cx);
        }
    }

    pub(super) fn advance_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match active_step(cx).and_then(Step::next) {
            Some(step) => self.show_tour_step(step, window, cx),
            None => self.skip_tour(cx),
        }
    }

    fn show_tour_step(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(page) = step.page() {
            let is_engine = page == Page::Engine;
            self.navigate(page, false, cx);
            if is_engine {
                self.reveal_engine_rules(window, cx);
            }
        } else {
            self.open_settings_at(Section::Connectors, window, cx);
        }
        let focus = cx.focus_handle();
        cx.set_global(Active(Guide {
            step,
            target: Rc::new(Cell::new(None)),
            focus,
            has_focused: Rc::new(Cell::new(false)),
        }));
        if step == Step::Master {
            let conversation = self.shown_master(cx).read(cx).conversation();
            conversation.update(cx, |_, cx| cx.notify());
        }
        cx.refresh_windows();
    }

    pub(super) fn skip_tour(&mut self, cx: &mut Context<Self>) {
        reset(cx);
    }

    pub(super) fn tour_layer(&self, palette: &Palette, cx: &App) -> Option<AnyElement> {
        let guide = active(cx)?.clone();
        let words = match guide.step {
            Step::Master => master_words(&self.shown_master(cx).read(cx).account_entries(cx), cx),
            Step::Engine => engine_words(
                self.facts
                    .read(cx)
                    .snapshot()
                    .and_then(|snapshot| snapshot.engine.as_ref()),
            ),
            Step::Lanes => tr_with(
                Key("tour.lanes"),
                &[
                    ("page", tr(Key("page.lanes"))),
                    ("new_lane", tr(Key("lanes_page.new_lane"))),
                ],
            ),
            Step::Inbox => tr_with(Key("tour.inbox"), &[("page", tr(Key("page.inbox")))]),
            Step::Connectors => tr(Key("tour.connectors")).into(),
        };
        Some(
            deferred(overlay::Overlay {
                guide,
                palette: *palette,
                words,
            })
            .with_priority(LAYER)
            .into_any_element(),
        )
    }
}

fn active(cx: &App) -> Option<&Guide> {
    cx.try_global::<Active>().map(|state| &state.0)
}

pub(super) fn active_step(cx: &App) -> Option<Step> {
    active(cx).map(|guide| guide.step)
}

pub(super) fn reset(cx: &mut App) {
    if active(cx).is_some() {
        cx.remove_global::<Active>();
        cx.refresh_windows();
    }
}

pub(super) fn connectors_closed(cx: &mut App) {
    if active_step(cx) == Some(Step::Connectors) {
        reset(cx);
    }
}

pub(super) fn track_master_chip(bounds: gpui::Bounds<Pixels>, cx: &App) {
    if let Some(guide) = active(cx).filter(|guide| guide.step == Step::Master) {
        guide.target.set(Some(overlay::Target {
            bounds,
            radius: theme::RADIUS_CONTROL,
            composer_top: None,
        }));
    }
}

pub(super) fn track_master_composer(top: Pixels, cx: &App) {
    if let Some(guide) = active(cx).filter(|guide| guide.step == Step::Master) {
        if let Some(mut target) = guide.target.get() {
            target.composer_top = Some(top);
            guide.target.set(Some(target));
        }
    }
}

pub(super) fn track(step: Step, radius: Pixels, child: impl IntoElement) -> AnyElement {
    overlay::Tracked {
        step,
        radius,
        child: child.into_any_element(),
    }
    .into_any_element()
}

impl Guide {
    fn skip_missing(&self, window: &Window, cx: &mut App) {
        let target = self.target.clone();
        window.defer(cx, move |window, cx| {
            if active(cx).is_some_and(|guide| Rc::ptr_eq(&guide.target, &target)) {
                window.dispatch_action(Advance.boxed_clone(), cx);
            }
        });
    }

    fn focus(&self, window: &Window, cx: &mut App) {
        if !self.has_focused.replace(true) {
            let target = self.target.clone();
            let focus = self.focus.clone();
            window.defer(cx, move |window, cx| {
                if active(cx).is_some_and(|guide| Rc::ptr_eq(&guide.target, &target)) {
                    focus.focus(window, cx);
                }
            });
        }
    }
}

fn master_words(accounts: &[MasterAccount], cx: &App) -> SharedString {
    let kept = Preferences::try_current(cx).and_then(|prefs| prefs.new_master_account);
    let default = master_accounts::default_account(accounts, kept.as_deref());
    let account = accounts
        .iter()
        .find(|account| Some(account.id.as_str()) == default.as_deref());
    match account {
        Some(account) => tr_with(Key("tour.master_default"), &[("account", &account.label)]),
        None => tr(Key("tour.master")).into(),
    }
}

fn engine_words(state: Option<&EngineState>) -> SharedString {
    let choices = state.and_then(|state| {
        let preset = state
            .preset_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())?;
        let pace = super::lane_paces::pace_words(state.pace.as_deref()?);
        Some((preset, pace))
    });
    match choices {
        Some((preset, pace)) => tr_with(
            Key("tour.engine_defaults"),
            &[("preset", preset), ("pace", pace.as_ref())],
        ),
        None => tr_with(Key("tour.engine"), &[("page", tr(Key("page.engine")))]),
    }
}

fn card(guide: &Guide, words: SharedString, palette: &Palette) -> AnyElement {
    let skip = kit::button(
        "tour-skip",
        tr(Key("tour.skip")),
        ButtonKind::Quiet,
        palette,
    )
    .on_click(|_, window, cx| window.dispatch_action(Skip.boxed_clone(), cx));
    let next_key = if guide.step.next().is_some() {
        "tour.next"
    } else {
        "tour.done"
    };
    let next = kit::button("tour-next", tr(Key(next_key)), ButtonKind::Primary, palette)
        .on_click(|_, window, cx| window.dispatch_action(Advance.boxed_clone(), cx));
    let id = guide.step.id();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .track_focus(&guide.focus)
        .tab_group()
        .w_full()
        .p(theme::GAP_WIDE)
        .flex()
        .flex_col()
        .gap(theme::GAP_SMALL)
        .bg(palette.overlay_surface)
        .border_1()
        .border_color(palette.overlay_edge)
        .rounded(theme::RADIUS_CONTROL)
        .shadow_md()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(tr(guide.step.title())),
        )
        .child(
            div()
                .text_size(theme::TEXT_SMALL)
                .text_color(palette.text_secondary)
                .child(words),
        )
        .child(
            div()
                .pt(theme::GAP_TIGHT)
                .flex()
                .justify_end()
                .gap(theme::GAP_SMALL)
                .child(skip)
                .child(next),
        )
        .into_any_element()
}

mod overlay {
    use super::{active, Guide, Step};
    use crate::theme::{self, Palette};
    use gpui::{
        div, fill, point, px, size, AnyElement, App, AvailableSpace, Bounds, Element, ElementId,
        GlobalElementId, Hsla, InspectorElementId, InteractiveElement as _, IntoElement, LayoutId,
        ParentElement as _, PathBuilder, Pixels, Point, Size, Style, Styled as _, Window,
    };

    use theme::tour::{
        ARROW_SIZE as ARROW, CARD_WIDTH as WIDTH, TARGET_GAP as GAP, WINDOW_MARGIN as MARGIN,
    };

    #[derive(Clone, Copy)]
    pub(super) struct Target {
        pub bounds: Bounds<Pixels>,
        pub radius: Pixels,
        pub composer_top: Option<Pixels>,
    }

    pub(super) struct Tracked {
        pub step: Step,
        pub radius: Pixels,
        pub child: AnyElement,
    }

    impl IntoElement for Tracked {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    impl Element for Tracked {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }
        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, ()) {
            (self.child.request_layout(window, cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            if let Some(guide) = active(cx).filter(|guide| guide.step == self.step) {
                let mask = window.content_mask().bounds;
                if bounds.size.width > Pixels::ZERO
                    && bounds.size.height > Pixels::ZERO
                    && mask.contains(&bounds.origin)
                    && mask.contains(&point(bounds.right(), bounds.bottom()))
                {
                    guide.target.set(Some(Target {
                        bounds,
                        radius: self.radius,
                        composer_top: None,
                    }));
                }
            }
            self.child.prepaint(window, cx);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut (),
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            self.child.paint(window, cx);
        }
    }

    pub(super) struct Overlay {
        pub guide: Guide,
        pub palette: Palette,
        pub words: gpui::SharedString,
    }

    pub(super) struct Painted {
        shield: AnyElement,
        card: AnyElement,
        target: Target,
        placement: Placement,
    }

    impl IntoElement for Overlay {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    impl Element for Overlay {
        type RequestLayoutState = ();
        type PrepaintState = Option<Painted>;

        fn id(&self) -> Option<ElementId> {
            None
        }
        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, ()) {
            (window.request_layout(Style::default(), [], cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) -> Option<Painted> {
            let viewport = window.viewport_size();
            let target = if self.guide.step == Step::Master {
                self.guide.target.get()
            } else {
                self.guide.target.take()
            };
            let Some(target) = target else {
                self.guide.skip_missing(window, cx);
                return None;
            };
            let mut card = super::card(&self.guide, self.words.clone(), &self.palette);
            let card_size = card.layout_as_root(
                size(AvailableSpace::Definite(WIDTH), AvailableSpace::MaxContent),
                window,
                cx,
            );
            let Some(placement) = place(
                self.guide.step,
                target.bounds,
                card_size,
                viewport,
                target.composer_top,
            ) else {
                self.guide.skip_missing(window, cx);
                return None;
            };
            let marker = div()
                .absolute()
                .left(target.bounds.left())
                .top(target.bounds.top())
                .w(target.bounds.size.width)
                .h(target.bounds.size.height)
                .debug_selector(|| "tour-spotlight".into());
            let mut shield = div().size_full().occlude().child(marker).into_any_element();
            shield.prepaint_as_root(
                point(px(0.), px(0.)),
                size(
                    AvailableSpace::Definite(viewport.width),
                    AvailableSpace::Definite(viewport.height),
                ),
                window,
                cx,
            );
            card.prepaint_at(placement.card.origin, window, cx);
            self.guide.focus(window, cx);
            Some(Painted {
                shield,
                card,
                target,
                placement,
            })
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut (),
            painted: &mut Option<Painted>,
            window: &mut Window,
            cx: &mut App,
        ) {
            if let Some(painted) = painted {
                painted.shield.paint(window, cx);
                paint_spotlight(painted.target, self.palette.scrim, window);
                painted.card.paint(window, cx);
                paint_arrow(painted.placement, &self.palette, window);
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Side {
        Below,
        Above,
        Right,
        Left,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Placement {
        card: Bounds<Pixels>,
        side: Side,
        arrow: Point<Pixels>,
    }

    fn place(
        step: Step,
        target: Bounds<Pixels>,
        card: Size<Pixels>,
        viewport: Size<Pixels>,
        composer_top: Option<Pixels>,
    ) -> Option<Placement> {
        let sides = match step {
            Step::Inbox => [Side::Left, Side::Below, Side::Above, Side::Right],
            Step::Master => [Side::Above, Side::Right, Side::Left, Side::Below],
            _ => [Side::Below, Side::Above, Side::Right, Side::Left],
        };
        let margin = MARGIN + theme::PANEL_INSET;
        let max = point(
            viewport.width - margin - card.width,
            viewport.height - margin - card.height,
        );
        if max.x < margin || max.y < margin {
            return None;
        }
        let middle = target.center();
        sides.into_iter().find_map(|side| {
            let x = (middle.x - card.width / 2.).clamp(margin, max.x);
            let y = (middle.y - card.height / 2.).clamp(margin, max.y);
            let origin = match side {
                Side::Below if step == Step::Lanes => {
                    point(x, (target.bottom() + GAP).max(theme::DIALOG_SHEET_TOP))
                }
                Side::Below => point(x, target.bottom() + GAP),
                Side::Above => point(
                    x,
                    composer_top.unwrap_or_else(|| target.top()) - GAP - card.height,
                ),
                Side::Right => point(target.right() + GAP, y),
                Side::Left => point(target.left() - GAP - card.width, y),
            };
            let inside =
                origin.x >= margin && origin.x <= max.x && origin.y >= margin && origin.y <= max.y;
            inside.then(|| Placement {
                card: Bounds::new(origin, card),
                side,
                arrow: arrow_origin(side, Bounds::new(origin, card), middle),
            })
        })
    }

    fn arrow_origin(side: Side, card: Bounds<Pixels>, target: Point<Pixels>) -> Point<Pixels> {
        let corner = theme::RADIUS_CONTROL + ARROW;
        let x = target.x.clamp(card.left() + corner, card.right() - corner);
        let y = target.y.clamp(card.top() + corner, card.bottom() - corner);
        match side {
            Side::Below => point(x, card.top()),
            Side::Above => point(x, card.bottom()),
            Side::Right => point(card.left(), y),
            Side::Left => point(card.right(), y),
        }
    }

    fn paint_spotlight(target: Target, dim: Hsla, window: &mut Window) {
        let hole = target.bounds;
        let viewport = window.viewport_size();
        let (left, top, right, bottom) = (hole.left(), hole.top(), hole.right(), hole.bottom());
        let zero = Pixels::ZERO;
        for (x1, y1, x2, y2) in [
            (zero, zero, viewport.width, top),
            (zero, bottom, viewport.width, viewport.height),
            (zero, top, left, bottom),
            (right, top, viewport.width, bottom),
        ] {
            if x1 < x2 && y1 < y2 {
                window.paint_quad(fill(
                    Bounds::new(point(x1, y1), size(x2 - x1, y2 - y1)),
                    dim,
                ));
            }
        }
        let radius = target
            .radius
            .min(hole.size.width / 2.)
            .min(hole.size.height / 2.);
        for (x, y, dx, dy) in [
            (left, top, 1., 1.),
            (right, top, -1., 1.),
            (left, bottom, 1., -1.),
            (right, bottom, -1., -1.),
        ] {
            paint_corner(point(x, y), point(radius * dx, radius * dy), dim, window);
        }
    }

    fn paint_corner(at: Point<Pixels>, radius: Point<Pixels>, color: Hsla, window: &mut Window) {
        let mut path = PathBuilder::fill();
        let handle = 0.552_284_8;
        path.move_to(at);
        path.line_to(point(at.x + radius.x, at.y));
        path.cubic_bezier_to(
            point(at.x, at.y + radius.y),
            point(at.x + radius.x * (1. - handle), at.y),
            point(at.x, at.y + radius.y * (1. - handle)),
        );
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }

    fn paint_arrow(placement: Placement, palette: &Palette, window: &mut Window) {
        let at = placement.arrow;
        let across = match placement.side {
            Side::Below | Side::Above => point(ARROW, px(0.)),
            Side::Right | Side::Left => point(px(0.), ARROW),
        };
        let outward = match placement.side {
            Side::Below => point(px(0.), -ARROW),
            Side::Above => point(px(0.), ARROW),
            Side::Right => point(-ARROW, px(0.)),
            Side::Left => point(ARROW, px(0.)),
        };
        triangle(
            [at - across, at + outward, at + across],
            palette.overlay_edge,
            window,
        );
        triangle(
            [
                at - across * 0.8 - outward * 0.2,
                at + outward * 0.8,
                at + across * 0.8 - outward * 0.2,
            ],
            palette.overlay_surface,
            window,
        );
    }

    fn triangle(points: [Point<Pixels>; 3], color: Hsla, window: &mut Window) {
        let mut path = PathBuilder::fill();
        path.move_to(points[0]);
        path.line_to(points[1]);
        path.line_to(points[2]);
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn audit_the_lanes_coach_has_a_full_margin_from_the_panel_edge() {
            let viewport = size(px(1100.), px(700.));
            let target = Bounds::new(point(px(974.), px(54.)), size(px(100.), px(28.)));
            let placement =
                place(Step::Lanes, target, size(WIDTH, px(140.)), viewport, None).unwrap();
            assert!(placement.card.right() <= viewport.width - MARGIN - theme::PANEL_INSET);
        }

        #[test]
        fn card_flips_above_a_target_near_the_bottom() {
            let viewport = size(px(800.), px(600.));
            let target = Bounds::new(point(px(690.), px(530.)), size(px(75.), px(32.)));
            let placement =
                place(Step::Lanes, target, size(WIDTH, px(140.)), viewport, None).unwrap();
            assert_eq!(placement.side, Side::Above);
            assert!(placement.card.right() <= viewport.width - MARGIN);
            assert!(placement.card.bottom() < target.top());
            assert!(placement.arrow.x >= target.left() && placement.arrow.x <= target.right());
        }
    }
}

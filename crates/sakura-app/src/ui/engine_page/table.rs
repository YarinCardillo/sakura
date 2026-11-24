//! The table of running lanes, under the pace: each lane's mark, name, project, preset and pace.

use super::EnginePage;
use crate::l10n::{tr, Key};
use crate::theme::engine_page as sizes;
use crate::theme::{self, Palette};
use crate::ui::engine_acts::SET_LANE_PACE;
use crate::ui::kit;
use crate::ui::lane_paces::{pace_words, pending_note, LANE_PACES};
use crate::ui::lanes;
use gpui::{
    div, AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, Styled as _,
};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use sakura_core::engine::snapshot::Item;

impl EnginePage {
    /// The table of the lanes that are open.
    pub(super) fn lanes_table(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let running: Vec<Item> = self
            .snapshot(cx)
            .map(|snapshot| lanes::open_lanes(&snapshot.items).cloned().collect())
            .unwrap_or_default();
        if running.is_empty() {
            return self.unreported(Key("engine_page.no_lanes"), "engine-no-lanes", palette);
        }
        let rows = running.iter().map(|item| self.lane_row(item, palette, cx));
        div()
            .debug_selector(|| "engine-lanes-table".into())
            .flex()
            .flex_col()
            .child(header_row(palette))
            .children(rows)
            .into_any_element()
    }

    fn lane_row(&self, item: &Item, palette: &Palette, cx: &gpui::App) -> AnyElement {
        let id = format!("engine-lane-{}", item.task());
        let acts = self.acts.clone();
        let (target, press_key) = (item.id.clone(), format!("engine-lane-pace-{}", item.task()));
        let told = crate::ui::engine_acts::line(&self.acts, &press_key, palette, cx);
        let current = item.pace.clone().unwrap_or_default();
        let label = item.pace.as_deref().map_or_else(
            || tr(Key("engine_page.table.follows_pace")).into(),
            pace_words,
        );
        let selector = press_key.clone();
        let pace = kit::menu_chip_at_width(press_key.clone(), label, sizes::TABLE_PACE_WIDTH, cx)
            .debug_selector(move || selector.clone())
            .dropdown_menu(move |menu, _, _| {
                LANE_PACES.iter().fold(menu, |menu, (level, word)| {
                    let (acts, target, press_key) =
                        (acts.clone(), target.clone(), press_key.clone());
                    let level = *level;
                    let item = PopupMenuItem::new(tr(*word))
                        .checked(level == current)
                        .on_click(move |_: &ClickEvent, _, cx| {
                            let act = SET_LANE_PACE.act_on(&target, &[("level", level)]);
                            acts.update(cx, |acts, cx| acts.press(&press_key, act, "", cx));
                        });
                    menu.item(item)
                })
            });
        let preset = item.preset_name().unwrap_or_default();
        let row = div()
            .debug_selector(move || id.clone())
            .h(sizes::TABLE_ROW_HEIGHT)
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .text_size(sizes::ROW_TEXT)
            .child(
                div()
                    .w(sizes::TABLE_GLYPH_WIDTH)
                    .flex_none()
                    .child(lanes::glyph_of(item).paint(theme::HAND_MARK_SIZE, palette)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(lanes::lane_words(item, false)),
            )
            .child(
                div()
                    .w(sizes::TABLE_PROJECT_WIDTH)
                    .flex_none()
                    .flex()
                    .child(project_label(item.project(), palette)),
            )
            .child(
                div()
                    .w(sizes::TABLE_PRESET_WIDTH)
                    .flex_none()
                    .text_color(palette.text_secondary)
                    .child(preset),
            )
            .child(div().w(sizes::TABLE_PACE_WIDTH).flex_none().child(pace));
        let pending = item.pace_pending.then(|| {
            let selector = format!("engine-lane-pace-pending-{}", item.task());
            div()
                .pb(theme::GAP_SMALL)
                .pl(sizes::TABLE_GLYPH_WIDTH + theme::GAP_WIDE)
                .child(pending_note(selector, palette))
        });
        div()
            .border_t_1()
            .border_color(palette.border)
            .child(row)
            .children(pending)
            .children(told)
            .into_any_element()
    }
}

fn header_row(palette: &Palette) -> AnyElement {
    let cell = |width, key| div().w(width).flex_none().child(tr(Key(key)));
    div()
        .h(sizes::TABLE_HEAD_HEIGHT)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .text_size(sizes::SMALL_TEXT)
        .text_color(palette.text_tertiary)
        .child(div().w(sizes::TABLE_GLYPH_WIDTH).flex_none())
        .child(div().flex_1().child(tr(Key("engine_page.table.lanes"))))
        .child(cell(
            sizes::TABLE_PROJECT_WIDTH,
            "engine_page.table.project",
        ))
        .child(cell(sizes::TABLE_PRESET_WIDTH, "engine_page.table.preset"))
        .child(cell(sizes::TABLE_PACE_WIDTH, "engine_page.table.pace"))
        .into_any_element()
}

fn project_label(project: &str, palette: &Palette) -> AnyElement {
    crate::ui::chrome::text(sizes::SMALL_TEXT, palette.text_secondary)
        .debug_selector(|| "engine-table-tag".into())
        .truncate()
        .child(project.to_string())
        .into_any_element()
}

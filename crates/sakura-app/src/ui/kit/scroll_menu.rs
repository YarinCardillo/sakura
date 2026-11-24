use crate::theme;
use crate::theme::kit::dropdown::{
    CHECK_SIZE, ITEM_HEIGHT, MAX_HEIGHT, MAX_WIDTH, MIN_WIDTH, PADDING, ROW_GAP,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, size, Anchor, AnyElement, App, AvailableSpace, ClickEvent, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
};
use gpui_base::actions::{Confirm, SelectDown, SelectUp};
use gpui_base::PopoverState;
use gpui_component::button::Button;
use gpui_component::popover::Popover;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ElementExt as _, Icon, Sizable as _, ThemeStyled as _};
use gpui_kit_assets::IconName;
use std::rc::Rc;

type Press = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Content = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type Build = Box<dyn Fn(ScrollMenu, &mut Window, &mut App) -> ScrollMenu>;

enum Face {
    Text(SharedString),
    Custom(Content),
    Label(SharedString),
    Separator,
}

pub struct ScrollMenuItem {
    face: Face,
    press: Option<Press>,
    is_disabled: bool,
    is_checked: Option<bool>,
}

impl ScrollMenuItem {
    fn face(face: Face) -> Self {
        Self {
            face,
            press: None,
            is_disabled: false,
            is_checked: None,
        }
    }

    pub fn new(label: impl Into<SharedString>) -> Self {
        Self::face(Face::Text(label.into()))
    }

    pub fn element(render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self::face(Face::Custom(Box::new(render)))
    }

    pub fn disabled(mut self, is_disabled: bool) -> Self {
        self.is_disabled = is_disabled;
        self
    }

    pub fn checked(mut self, is_checked: bool) -> Self {
        self.is_checked = Some(is_checked);
        self
    }

    pub fn on_click(
        mut self,
        press: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.press = Some(Rc::new(press));
        self
    }
}

pub struct ScrollMenu {
    items: Vec<ScrollMenuItem>,
    head_len: usize,
    footer_start: Option<usize>,
    min_width: Pixels,
    max_width: Pixels,
    max_height: Pixels,
}

impl ScrollMenu {
    fn new(window: &Window) -> Self {
        let viewport = window.viewport_size();
        Self {
            items: Vec::new(),
            head_len: 0,
            footer_start: None,
            min_width: MIN_WIDTH,
            max_width: (viewport.width - theme::PANEL_INSET * 2.).min(MAX_WIDTH),
            max_height: (viewport.height * 0.5).min(MAX_HEIGHT),
        }
    }

    pub fn min_w(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    pub fn max_w(mut self, width: Pixels) -> Self {
        self.max_width = self.max_width.min(width);
        self
    }

    pub fn max_h(mut self, height: Pixels) -> Self {
        self.max_height = height;
        self
    }

    pub fn item(mut self, item: ScrollMenuItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn separator(self) -> Self {
        self.item(ScrollMenuItem::face(Face::Separator))
    }

    pub fn label(self, label: impl Into<SharedString>) -> Self {
        self.item(ScrollMenuItem::face(Face::Label(label.into())))
    }

    /// Keeps the rows built so far above the scrolling list.
    pub fn pin_header(mut self) -> Self {
        self.head_len = self.items.len();
        self
    }

    /// Adds a divider and keeps the following actions below the scrolling list.
    pub fn footer(mut self) -> Self {
        self.footer_start = Some(self.items.len());
        self.separator()
    }

    fn bands(&self) -> [std::ops::Range<usize>; 3] {
        let end = self.footer_start.unwrap_or(self.items.len());
        [0..self.head_len, self.head_len..end, end..self.items.len()]
    }

    fn render(
        self,
        state: Entity<MenuState>,
        popover: WeakEntity<PopoverState>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let draw = MenuDraw::new(&self, state, popover, cx);
        let [head, list, footer] = self.bands();
        let width = self.width(&draw, window, cx);
        let layout = |range, window: &mut Window, cx: &mut App| {
            let rows = draw.rows(range, window, cx);
            measure(rows, AvailableSpace::Definite(width), window, cx).height
        };
        let head_height = layout(head.clone(), window, cx);
        let footer_height = layout(footer.clone(), window, cx);
        let list_height = layout(list.clone(), window, cx);
        let room = (self.max_height - head_height - footer_height - PADDING * 2.).max(ITEM_HEIGHT);
        let is_overflowing = list_height > room;
        let list_rows = draw.rows(list, window, cx);
        let surface = div()
            .id("dropdown-surface")
            .debug_selector(|| "dropdown-surface".into())
            .role(gpui::accesskit::Role::Menu)
            .track_focus(&draw.focus)
            .key_context("PopupMenu")
            .popover_style(cx)
            .flex()
            .flex_col()
            .w(width + PADDING * 2.)
            .p(PADDING)
            .on_action(draw.move_selection::<SelectUp>(false))
            .on_action(draw.move_selection::<SelectDown>(true))
            .on_action(draw.confirm())
            .child(draw.rows(head, window, cx))
            .child(draw.list(list_rows, is_overflowing.then_some(room)))
            .child(
                div()
                    .when(!footer.is_empty(), |row| {
                        row.debug_selector(|| "dropdown-footer".into())
                    })
                    .child(draw.rows(footer, window, cx)),
            );
        surface.into_any_element()
    }

    fn width(&self, draw: &MenuDraw, window: &mut Window, cx: &mut App) -> Pixels {
        let rows = draw.rows(0..self.items.len(), window, cx);
        let width = measure(rows, AvailableSpace::MaxContent, window, cx).width + PADDING * 2.;
        width.clamp(self.min_width.min(self.max_width), self.max_width) - PADDING * 2.
    }
}

fn measure(
    rows: gpui::Div,
    width: AvailableSpace,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Size<Pixels> {
    rows.into_any_element()
        .layout_as_root(size(width, AvailableSpace::MaxContent), window, cx)
}

struct MenuState {
    focus: FocusHandle,
    scroll: ScrollHandle,
    selected: Option<usize>,
    has_revealed: bool,
}

#[derive(IntoElement)]
pub struct ScrollDropdown {
    button: Button,
    anchor: Anchor,
    build: Build,
}

pub fn scroll_dropdown(
    button: Button,
    anchor: Anchor,
    build: impl Fn(ScrollMenu, &mut Window, &mut App) -> ScrollMenu + 'static,
) -> ScrollDropdown {
    ScrollDropdown {
        button,
        anchor,
        build: Box::new(build),
    }
}

impl RenderOnce for ScrollDropdown {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self
            .button
            .interactivity()
            .element_id
            .clone()
            .expect("dropdown button ID");
        let menu_state = window.use_keyed_state(
            SharedString::from(format!("scroll-menu-state:{id:?}")),
            cx,
            |_, cx| MenuState {
                focus: cx.focus_handle(),
                scroll: ScrollHandle::new(),
                selected: None,
                has_revealed: false,
            },
        );
        let focus = menu_state.read(cx).focus.clone();
        let state = menu_state.clone();
        Popover::new(SharedString::from(format!("scroll-dropdown:{id:?}")))
            .anchor(self.anchor)
            .appearance(false)
            .track_focus(&focus)
            .trigger(self.button)
            .on_open_change(move |_, _, cx| {
                state.update(cx, |state, _| {
                    state.scroll = ScrollHandle::new();
                    state.selected = None;
                    state.has_revealed = false;
                });
            })
            .content(move |_, window, cx| {
                let menu = (self.build)(ScrollMenu::new(window), window, cx);
                menu.render(menu_state.clone(), cx.weak_entity(), window, cx)
            })
    }
}

struct MenuDraw<'a> {
    menu: &'a ScrollMenu,
    state: Entity<MenuState>,
    popover: WeakEntity<PopoverState>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    selected: Option<usize>,
    has_checks: bool,
    palette: crate::theme::Palette,
}

impl<'a> MenuDraw<'a> {
    fn new(
        menu: &'a ScrollMenu,
        state: Entity<MenuState>,
        popover: WeakEntity<PopoverState>,
        cx: &App,
    ) -> Self {
        let current = state.read(cx);
        Self {
            menu,
            focus: current.focus.clone(),
            scroll: current.scroll.clone(),
            selected: current.selected,
            state,
            popover,
            has_checks: menu.items.iter().any(|item| item.is_checked.is_some()),
            palette: theme::Theme::palette(cx),
        }
    }

    fn rows(
        &self,
        indices: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(ROW_GAP)
            .text_size(theme::TEXT_SMALL)
            .children(indices.map(|index| self.row(index, window, cx)))
    }

    fn row(&self, index: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let item = &self.menu.items[index];
        let check = self.has_checks.then(|| {
            div().flex_none().size(CHECK_SIZE).children(
                item.is_checked
                    .unwrap_or(false)
                    .then(|| Icon::new(IconName::Check).with_size(CHECK_SIZE)),
            )
        });
        let mut row = div()
            .id(("dropdown-item", index))
            .debug_selector(move || format!("dropdown-item-{index}"))
            .flex_none()
            .flex()
            .items_center()
            .min_h(ITEM_HEIGHT)
            .px(theme::GAP_SMALL)
            .gap(theme::GAP_SMALL)
            .rounded(theme::RADIUS_CONTROL)
            .children(check);
        match &item.face {
            Face::Separator => {
                return div()
                    .flex_none()
                    .h(theme::GAP_SMALL)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .h(theme::PANEL_RING_WIDTH)
                            .bg(self.palette.border),
                    )
                    .into_any_element()
            }
            Face::Label(label) => {
                return row
                    .text_color(self.palette.text_tertiary)
                    .child(label.clone())
                    .into_any_element()
            }
            Face::Text(label) => {
                row = row
                    .aria_label(label.clone())
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()));
            }
            Face::Custom(render) => {
                row = row.child(div().flex_1().min_w_0().child(render(window, cx)));
            }
        }
        let hover = self.palette.hover;
        row.role(gpui::accesskit::Role::MenuItem)
            .aria_selected(self.selected == Some(index))
            .when_some(item.is_checked, |row, is_checked| {
                row.aria_toggled(if is_checked {
                    gpui::accesskit::Toggled::True
                } else {
                    gpui::accesskit::Toggled::False
                })
            })
            .when(item.is_disabled, |row| {
                row.text_color(self.palette.text_tertiary)
            })
            .when_some(
                item.press.clone().filter(|_| !item.is_disabled),
                |row, press| {
                    let popover = self.popover.clone();
                    row.when(self.selected == Some(index), |row| row.bg(hover))
                        .hover(move |row| row.bg(hover))
                        .on_click(move |event, window, cx| run(&popover, &press, event, window, cx))
                },
            )
            .into_any_element()
    }

    fn list(&self, rows: gpui::Div, height: Option<Pixels>) -> AnyElement {
        let Some(height) = height else {
            return rows.into_any_element();
        };
        let start = self.menu.head_len;
        let checked = self.menu.bands()[1]
            .clone()
            .find(|index| self.menu.items[*index].is_checked == Some(true));
        let state = self.state.clone();
        div()
            .relative()
            .flex_none()
            .child(
                rows.id("dropdown-scroll")
                    .debug_selector(|| "dropdown-scroll".into())
                    .h(height)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_prepaint(move |_, window, cx| {
                        state.update(cx, |state, _| {
                            if !state.has_revealed {
                                state.has_revealed = true;
                                if let Some(index) = checked {
                                    state.scroll.scroll_to_item(index - start);
                                    window.refresh();
                                }
                            }
                        });
                    }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .debug_selector(|| "dropdown-scrollbar".into())
                    .vertical_scrollbar(&self.scroll),
            )
            .into_any_element()
    }

    fn move_selection<A: gpui::Action>(
        &self,
        forward: bool,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let state = self.state.clone();
        let choices: Vec<_> = self
            .menu
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.is_disabled && item.press.is_some())
            .map(|(index, _)| index)
            .collect();
        let list = self.menu.bands()[1].clone();
        move |_, window, cx| {
            if choices.is_empty() {
                return;
            }
            state.update(cx, |state, _| {
                let current = state
                    .selected
                    .and_then(|selected| choices.iter().position(|index| *index == selected));
                let next = match (current, forward) {
                    (Some(index), true) => (index + 1) % choices.len(),
                    (Some(index), false) => (index + choices.len() - 1) % choices.len(),
                    (None, true) => 0,
                    (None, false) => choices.len() - 1,
                };
                let index = choices[next];
                state.selected = Some(index);
                if list.contains(&index) {
                    state.scroll.scroll_to_item(index - list.start);
                }
            });
            window.refresh();
        }
    }

    fn confirm(&self) -> impl Fn(&Confirm, &mut Window, &mut App) + 'static {
        let press = self
            .selected
            .and_then(|index| self.menu.items[index].press.clone());
        let popover = self.popover.clone();
        move |_, window, cx| {
            if let Some(press) = &press {
                run(&popover, press, &ClickEvent::default(), window, cx);
            }
        }
    }
}

fn run(
    popover: &WeakEntity<PopoverState>,
    press: &Press,
    event: &ClickEvent,
    window: &mut Window,
    cx: &mut App,
) {
    drop(popover.update(cx, |popover, cx| popover.dismiss(window, cx)));
    press(event, window, cx);
    window.refresh();
}

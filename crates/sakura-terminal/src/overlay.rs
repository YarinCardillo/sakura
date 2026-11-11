//! What the view draws above the cached grid, every frame: the cursor, the text the input method is composing,
//! and the mouse and text input hooks. Painting it does not lay out the grid again, so the cursor blinks
//! without repainting the rows.
//!
//! Cursor shapes and the marked-text overlay follow Zed's `crates/terminal_view/src/terminal_element.rs`
//! (zed@d2a4fb6, GPL-3.0-or-later, Copyright Zed Industries).
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use crate::grid_view::color;
use crate::view::TerminalView;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, ModifiersChangedEvent, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Position, ScrollEvent, ShapedLine, Style,
    TextAlign, TextRun, UnderlineStyle, Window,
};

/// Width of a bar cursor and height of an underline cursor.
const CURSOR_THICKNESS: f32 = 2.0;
/// Width of the outline of an unfocused cursor.
const HOLLOW_THICKNESS: f32 = 1.0;
/// Height of the line under an address while Cmd is held over it.
const LINK_UNDERLINE: Pixels = px(1.0);

enum CursorPaint {
    Block {
        bounds: Bounds<Pixels>,
        color: Hsla,
        glyph: Option<Box<ShapedLine>>,
    },
    Rect(Bounds<Pixels>, Hsla),
    Hollow(Bounds<Pixels>, Hsla),
}

struct MarkedPaint {
    origin: Point<Pixels>,
    line: ShapedLine,
    background: Hsla,
}

pub(crate) struct OverlayPaint {
    hitbox: Hitbox,
    cursor: Option<CursorPaint>,
    marked: Option<MarkedPaint>,
    line_height: Pixels,
}

/// The element drawn over the grid by [`TerminalView`].
pub(crate) struct OverlayElement {
    view: Entity<TerminalView>,
}

impl OverlayElement {
    pub fn new(view: Entity<TerminalView>) -> Self {
        Self { view }
    }
}

impl IntoElement for OverlayElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OverlayElement {
    type RequestLayoutState = ();
    type PrepaintState = OverlayPaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        style.inset.top = px(0.).into();
        style.inset.left = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let view = self.view.read(cx);
        let line_height = view
            .info
            .get()
            .geometry
            .map_or(px(0.), |geometry| geometry.line_height);
        let marked = marked_paint(view, window);
        let cursor = if marked.is_some() {
            None
        } else {
            cursor_paint(view, window)
        };
        OverlayPaint {
            hitbox,
            cursor,
            marked,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        paint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_cursor_style(self.view.read(cx).pointer_style(), &paint.hitbox);
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        register_mouse_listeners(&self.view, &paint.hitbox, window);
        paint_hovered_link(self.view.read(cx), window);
        if let Some(cursor) = paint.cursor.take() {
            paint_cursor(cursor, paint.line_height, window, cx);
        }
        if let Some(marked) = paint.marked.take() {
            let background = Bounds::new(marked.origin, size(marked.line.width, paint.line_height));
            window.paint_quad(fill(background, marked.background));
            paint_line(&marked.line, marked.origin, paint.line_height, window, cx);
        }
        self.view.read(cx).stats.view_painted();
    }
}

fn cursor_paint(view: &TerminalView, window: &mut Window) -> Option<CursorPaint> {
    let info = view.info.get();
    let (geometry, cursor) = (info.geometry?, info.cursor?);
    let cells = if cursor.is_wide { 2 } else { 1 };
    let bounds = geometry.cell_bounds(cursor.point.line, cursor.point.column, cells);
    let cursor_color = color(view.style.palette.cursor, 1.0);
    let is_focused = view.focus.is_focused(window) && window.is_window_active();
    if !is_focused || cursor.shape == CursorShape::HollowBlock {
        return Some(CursorPaint::Hollow(bounds, cursor_color));
    }
    if !view.is_cursor_shown {
        return None;
    }
    let thickness = px(CURSOR_THICKNESS);
    match cursor.shape {
        CursorShape::Block => Some(CursorPaint::Block {
            bounds,
            color: cursor_color,
            glyph: block_glyph(cursor.character, view, window),
        }),
        CursorShape::Beam => Some(CursorPaint::Rect(
            Bounds::new(bounds.origin, size(thickness, bounds.size.height)),
            cursor_color,
        )),
        CursorShape::Underline => Some(CursorPaint::Rect(
            Bounds::new(
                point(bounds.origin.x, bounds.bottom() - thickness),
                size(bounds.size.width, thickness),
            ),
            cursor_color,
        )),
        CursorShape::HollowBlock | CursorShape::Hidden => None,
    }
}

/// The character under a block cursor, drawn in the background colour so it reads on the block.
fn block_glyph(
    character: char,
    view: &TerminalView,
    window: &mut Window,
) -> Option<Box<ShapedLine>> {
    if character.is_whitespace() {
        return None;
    }
    let text = character.to_string();
    let run = TextRun {
        len: text.len(),
        font: view.style.font(),
        color: color(view.style.palette.background, 1.0),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line =
        window
            .text_system()
            .shape_line(text.into(), view.style.font_size_px(), &[run], None);
    Some(Box::new(line))
}

fn marked_paint(view: &TerminalView, window: &mut Window) -> Option<MarkedPaint> {
    let text = view.marked_text.as_ref().filter(|text| !text.is_empty())?;
    let (origin, _, _) = view.cursor_origin()?;
    let palette = &view.style.palette;
    let foreground = color(palette.foreground, 1.0);
    let run = TextRun {
        len: text.len(),
        font: view.style.font(),
        color: foreground,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(foreground),
            wavy: false,
        }),
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        text.clone().into(),
        view.style.font_size_px(),
        &[run],
        None,
    );
    Some(MarkedPaint {
        origin,
        line,
        background: color(palette.background, 1.0),
    })
}

fn paint_cursor(cursor: CursorPaint, line_height: Pixels, window: &mut Window, cx: &mut App) {
    match cursor {
        CursorPaint::Block {
            bounds,
            color,
            glyph,
        } => {
            window.paint_quad(fill(bounds, color));
            if let Some(glyph) = glyph {
                paint_line(&glyph, bounds.origin, line_height, window, cx);
            }
        }
        CursorPaint::Rect(bounds, color) => window.paint_quad(fill(bounds, color)),
        CursorPaint::Hollow(bounds, color) => {
            let mut quad = outline(bounds, color, BorderStyle::Solid);
            quad.border_widths = px(HOLLOW_THICKNESS).into();
            window.paint_quad(quad);
        }
    }
}

fn paint_line(
    line: &ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    if let Err(error) = line.paint(origin, line_height, TextAlign::Left, None, window, cx) {
        log::warn!("terminal overlay text did not paint: {error}");
    }
}

/// A line under the address the pointer is on while Cmd is held, in the text's colour.
fn paint_hovered_link(view: &TerminalView, window: &mut Window) {
    let (Some(link), Some(geometry)) = (&view.pointer.hovered_link, view.info.get().geometry)
    else {
        return;
    };
    let colour = color(view.style().palette.foreground, 1.0);
    for (line, columns) in &link.spans {
        let cells = geometry.cell_bounds(*line, columns.start, columns.len());
        let origin = point(cells.left(), cells.bottom() - LINK_UNDERLINE);
        let underline = Bounds::new(origin, size(cells.size.width, LINK_UNDERLINE));
        window.paint_quad(fill(underline, colour));
    }
}

fn register_mouse_listeners(view: &Entity<TerminalView>, hitbox: &Hitbox, window: &mut Window) {
    // Cmd pressed or released without moving the pointer shows or hides the underline at once.
    let (target, area) = (view.clone(), hitbox.clone());
    window.on_modifiers_changed(move |event: &ModifiersChangedEvent, window, cx| {
        let over =
            (event.modifiers.platform && area.is_hovered(window)).then(|| window.mouse_position());
        target.update(cx, |view, cx| view.hover_link(over, cx));
    });
    let (target, area) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && area.is_hovered(window) {
            target.update(cx, |view, cx| view.mouse_down(event, window, cx));
        }
    });
    let (target, area) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            let is_hovered = area.is_hovered(window);
            target.update(cx, |view, cx| view.mouse_move(event, is_hovered, cx));
        }
    });
    let target = view.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
        if phase == DispatchPhase::Bubble {
            target.update(cx, |view, cx| view.mouse_up(event, cx));
        }
    });
    let (target, area) = (view.clone(), hitbox.clone());
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && area.should_handle_scroll(window) {
            target.update(cx, |view, cx| view.scroll_wheel(event, cx));
        }
    });
}

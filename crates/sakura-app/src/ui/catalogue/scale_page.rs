//! The scale itself, drawn: each type level as a sample line, each icon size, spacing step, control height and
//! radius as a box, every one with its token name.

use super::{group, specimen};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::kit::catalogue as room;
use crate::theme::kit::scale::*;
use crate::theme::Palette;
use gpui::{div, AnyElement, IntoElement as _, ParentElement as _, Pixels, Styled as _};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// The six type levels and the one control label.
pub(super) fn type_scale(palette: &Palette) -> AnyElement {
    let mut samples: Vec<AnyElement> = TYPE_SCALE
        .iter()
        .map(|(name, style)| sample(name, *style, palette))
        .collect();
    samples.push(sample("CONTROL_LABEL", CONTROL_LABEL, palette));
    let column = div().flex().flex_col().gap(SPACE_L).children(samples);
    group(
        "catalogue-type",
        tr(Key("catalogue.type_scale")),
        vec![column.into_any_element()],
        palette,
    )
}

fn sample(name: &str, style: TextStyle, palette: &Palette) -> AnyElement {
    let words = div()
        .text_style(style)
        .text_color(palette.text)
        .child(tr(Key("catalogue.type_sample")));
    let detail = tr_with(
        Key("catalogue.type_token"),
        &[
            ("name", name),
            ("size", &f32::from(style.size).to_string()),
            ("line", &f32::from(style.line_height).to_string()),
            ("weight", &style.weight.0.to_string()),
        ],
    );
    specimen(&detail, words, palette)
}

/// The three icon sizes and the spacing steps.
pub(super) fn icons_and_spacing(palette: &Palette) -> AnyElement {
    let icons = [
        ("ICON_SMALL", ICON_SMALL),
        ("ICON_REGULAR", ICON_REGULAR),
        ("ICON_LARGE", ICON_LARGE),
    ]
    .map(|(name, size)| {
        let icon = Icon::new(IconName::FolderPlus)
            .with_size(size)
            .text_color(palette.text);
        specimen(&format!("{name} {}", f32::from(size)), icon, palette)
    });
    let steps = [
        ("SPACE_XS", SPACE_XS),
        ("SPACE_S", SPACE_S),
        ("SPACE_M", SPACE_M),
        ("SPACE_L", SPACE_L),
        ("SPACE_XL", SPACE_XL),
        ("SPACE_XXL", SPACE_XXL),
        ("SPACE_XXXL", SPACE_XXXL),
    ]
    .map(|(name, step)| {
        specimen(
            &format!("{name} {}", f32::from(step)),
            bar(step, palette),
            palette,
        )
    });
    let mut all = Vec::from(icons);
    all.extend(steps);
    group(
        "catalogue-icons",
        tr(Key("catalogue.icon_spacing")),
        all,
        palette,
    )
}

fn bar(width: Pixels, palette: &Palette) -> gpui::Div {
    div()
        .w(width)
        .h(room::STEP_BAR_HEIGHT)
        .rounded(RADIUS_SMALL)
        .bg(palette.text_tertiary)
}

/// The three control heights and the four radii.
pub(super) fn heights_and_radii(palette: &Palette) -> AnyElement {
    let heights = [
        ("CONTROL_SMALL", CONTROL_SMALL),
        ("CONTROL_REGULAR", CONTROL_REGULAR),
        ("CONTROL_LARGE", CONTROL_LARGE),
    ]
    .map(|(name, height)| {
        let block = div()
            .w(room::HEIGHT_BLOCK_WIDTH)
            .h(height)
            .rounded(RADIUS_CONTROL)
            .bg(palette.fill_hover);
        specimen(&format!("{name} {}", f32::from(height)), block, palette)
    });
    let radii = [
        ("RADIUS_SMALL", RADIUS_SMALL),
        ("RADIUS_CONTROL", RADIUS_CONTROL),
        ("RADIUS_PILL", RADIUS_PILL),
        ("RADIUS_SURFACE", RADIUS_SURFACE),
    ]
    .map(|(name, radius)| {
        let block = div()
            .size(room::RADIUS_BLOCK)
            .rounded(radius)
            .bg(palette.fill_hover);
        specimen(&format!("{name} {}", f32::from(radius)), block, palette)
    });
    let mut all = Vec::from(heights);
    all.extend(radii);
    group(
        "catalogue-heights",
        tr(Key("catalogue.control_dimensions")),
        all,
        palette,
    )
}

/// The accent: the one colour that means "this needs you", as the dot of an unread ask and the count of the Inbox.
pub(super) fn accent(palette: &Palette) -> AnyElement {
    let dot = div().size(DOT_SIZE).rounded_full().bg(palette.attention);
    let count = div()
        .h(CHIP_HEIGHT)
        .px(CHIP_PADDING_START)
        .flex()
        .items_center()
        .rounded(CHIP_RADIUS)
        .bg(palette.attention)
        .text_style(CONTROL_LABEL)
        .text_color(palette.attention_ink)
        .child("3");
    let all = vec![
        specimen(tr(Key("catalogue.unread_dot")), dot, palette),
        specimen(tr(Key("catalogue.inbox_count")), count, palette),
    ];
    group(
        "catalogue-accent",
        tr(Key("catalogue.accent")),
        all,
        palette,
    )
}

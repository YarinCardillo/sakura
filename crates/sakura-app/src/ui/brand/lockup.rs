//! The lockup: the mark and the word, as the brand's own files (`assets/brand/lockup-on-dark.svg` and
//! `lockup-on-light.svg`, copied as they are). The file is rasterised at the exact device pixels it
//! is shown at, one cell a whole number of pixels, and painted pixel for pixel: nothing is scaled
//! afterwards, so nothing is smoothed. The word is never set in live text.

use super::mark::{device_cell, device_corner_of, DeviceRect};
use crate::theme::Appearance;
use crate::theme_brand::{CLEAR_CELLS, GRID, LOCKUP_CELLS_WIDE};
use gpui::{
    canvas, px, size, App, Bounds, Corners, DevicePixels, Global, IntoElement, Pixels, RenderImage,
    Styled, SvgRenderer, SvgSize, Window,
};
use std::sync::Arc;

const ON_DARK: &[u8] = include_bytes!("../../../assets/brand/lockup-on-dark.svg");
const ON_LIGHT: &[u8] = include_bytes!("../../../assets/brand/lockup-on-light.svg");

/// The lockup's file for a ground of `appearance`.
fn source(appearance: Appearance) -> &'static [u8] {
    match appearance {
        Appearance::Dark => ON_DARK,
        Appearance::Light => ON_LIGHT,
    }
}

/// The lockups rasterised so far, by appearance and by the device pixels of a cell: a handful at
/// most (one per appearance and screen scale met), each painted from the same image on every frame.
#[derive(Default)]
struct Rasterised(Vec<(Appearance, u32, Arc<RenderImage>)>);

impl Global for Rasterised {}

/// The lockup for a ground of `appearance`, one cell `cell_points` points a side, with the brand's
/// clear space around it in its layout box.
pub fn lockup(cell_points: u32, appearance: Appearance) -> impl IntoElement {
    let paint = move |bounds: Bounds<Pixels>, (), window: &mut Window, cx: &mut App| {
        let scale = window.scale_factor();
        let cell = device_cell(cell_points, scale);
        let Some(image) = rasterised(appearance, cell, cx) else {
            return;
        };
        let (width, height) = (LOCKUP_CELLS_WIDE * cell, GRID * cell);
        let (left, top) = device_corner_of(bounds, (width, height), scale);
        let at = DeviceRect {
            left,
            top,
            width,
            height,
        }
        .in_points(scale);
        if let Err(error) = window.paint_image(at, at, Corners::default(), image, 0, false) {
            eprintln!("sakura: the lockup cannot be painted: {error}");
        }
    };
    let cells = |along: u32| px(((along + CLEAR_CELLS * 2) * cell_points) as f32);
    canvas(|_, _, _| (), paint)
        .w(cells(LOCKUP_CELLS_WIDE))
        .h(cells(GRID))
        .flex_none()
}

/// The lockup of `appearance` at `cell` device pixels a cell: the one made before, or a new one.
fn rasterised(appearance: Appearance, cell: u32, cx: &mut App) -> Option<Arc<RenderImage>> {
    let made = &cx.default_global::<Rasterised>().0;
    let found = made
        .iter()
        .find(|(of, at, _)| (*of, *at) == (appearance, cell));
    if let Some((_, _, image)) = found {
        return Some(image.clone());
    }
    match rasterise(&cx.svg_renderer(), source(appearance), cell) {
        Ok(image) => {
            let made = &mut cx.default_global::<Rasterised>().0;
            made.push((appearance, cell, image.clone()));
            Some(image)
        }
        Err(error) => {
            eprintln!("sakura: the lockup cannot be rasterised: {error}");
            None
        }
    }
}

/// The lockup file `svg` as an image `cell` device pixels a cell.
fn rasterise(renderer: &SvgRenderer, svg: &[u8], cell: u32) -> Result<Arc<RenderImage>, String> {
    let device = |cells: u32| DevicePixels((cells * cell) as i32);
    let exact = SvgSize::ExactSize(size(device(LOCKUP_CELLS_WIDE), device(GRID)));
    let parsed = renderer.parse_svg(svg).map_err(|error| error.to_string())?;
    renderer
        .render_parsed(&parsed, exact)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::cells::Phase;
    use super::*;
    use crate::theme_brand::FUCHSIA;

    /// The word's colour in each file, as the brand sets it.
    const WORD_ON_DARK: u32 = 0xededee;
    const WORD_ON_LIGHT: u32 = 0x000000;

    /// A colour as the image holds an opaque pixel of it: blue, green, red, alpha.
    fn opaque(rgb: u32) -> [u8; 4] {
        let [_, red, green, blue] = rgb.to_be_bytes();
        [blue, green, red, u8::MAX]
    }

    #[test]
    fn the_lockup_file_is_rasterised_with_no_pixel_between_its_colours() {
        let renderer = SvgRenderer::new(Arc::new(()));
        let files = [(ON_DARK, WORD_ON_DARK), (ON_LIGHT, WORD_ON_LIGHT)];
        for (svg, word) in files {
            for cell in [1, 2, 3, 4] {
                let image = rasterise(&renderer, svg, cell).expect("the brand's file is an svg");
                let wide = (LOCKUP_CELLS_WIDE * cell) as usize;
                let expected = size(
                    DevicePixels(wide as i32),
                    DevicePixels((GRID * cell) as i32),
                );
                assert_eq!(image.size(0), expected);
                let pixels = image.as_bytes(0).expect("one frame");
                let allowed = [[0; 4], opaque(FUCHSIA), opaque(word)];
                for pixel in pixels.as_chunks::<4>().0 {
                    assert!(allowed.iter().any(|colour| colour == pixel), "{pixel:?}");
                }
                assert_mark_is_the_drawing(pixels, wide, cell as usize);
            }
        }
    }

    /// The first 32 cells of the lockup are the mark: every pixel of a lit cell fuchsia, every pixel
    /// of a dark one clear.
    fn assert_mark_is_the_drawing(pixels: &[u8], wide: usize, cell: usize) {
        let rows = Phase::Whole.rows();
        let grid = GRID as usize;
        for y in 0..grid * cell {
            for x in 0..grid * cell {
                let is_lit = (rows[y / cell] >> (x / cell)) & 1 == 1;
                let at = (y * wide + x) * 4;
                let expected = if is_lit { opaque(FUCHSIA) } else { [0; 4] };
                assert_eq!(pixels[at..at + 4], expected, "pixel {x}, {y}");
            }
        }
    }
}

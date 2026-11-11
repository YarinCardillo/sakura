//! Block elements (U+2580 to U+259F) as rectangles within their cell, the way Ghostty and Zed draw them: a
//! font's block glyphs rarely fill a cell taller than the font, which leaves gaps in drawings made of blocks
//! (Claude Code's banner is one).

/// A rectangle in cell fractions (0.0 is the left or top edge, 1.0 the right or bottom) with its opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BlockRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub alpha: f32,
}

const UPPER_LEFT: u8 = 1;
const UPPER_RIGHT: u8 = 2;
const LOWER_LEFT: u8 = 4;
const LOWER_RIGHT: u8 = 8;

/// Whether `character` is drawn as rectangles.
pub(crate) fn is_block_element(character: char) -> bool {
    ('\u{2580}'..='\u{259F}').contains(&character)
}

/// The rectangles that draw `character`, empty for characters that are not block elements.
pub(crate) fn block_rects(character: char) -> Vec<BlockRect> {
    let code = character as u32;
    let eighth = |count: u32| count as f32 / 8.0;
    match code {
        0x2580 => vec![rect(0.0, 0.0, 1.0, 0.5)],
        0x2581..=0x2588 => vec![rect(0.0, 1.0 - eighth(code - 0x2580), 1.0, 1.0)],
        0x2589..=0x258F => vec![rect(0.0, 0.0, eighth(0x2590 - code), 1.0)],
        0x2590 => vec![rect(0.5, 0.0, 1.0, 1.0)],
        0x2591..=0x2593 => vec![BlockRect {
            alpha: (code - 0x2590) as f32 / 4.0,
            ..rect(0.0, 0.0, 1.0, 1.0)
        }],
        0x2594 => vec![rect(0.0, 0.0, 1.0, eighth(1))],
        0x2595 => vec![rect(eighth(7), 0.0, 1.0, 1.0)],
        0x2596..=0x259F => quadrants(QUADRANTS[(code - 0x2596) as usize]),
        _ => Vec::new(),
    }
}

/// The quadrants of U+2596 to U+259F, in code point order.
const QUADRANTS: [u8; 10] = [
    LOWER_LEFT,
    LOWER_RIGHT,
    UPPER_LEFT,
    UPPER_LEFT | LOWER_LEFT | LOWER_RIGHT,
    UPPER_LEFT | LOWER_RIGHT,
    UPPER_LEFT | UPPER_RIGHT | LOWER_LEFT,
    UPPER_LEFT | UPPER_RIGHT | LOWER_RIGHT,
    UPPER_RIGHT,
    UPPER_RIGHT | LOWER_LEFT,
    UPPER_RIGHT | LOWER_LEFT | LOWER_RIGHT,
];

fn quadrants(mask: u8) -> Vec<BlockRect> {
    [
        (UPPER_LEFT, rect(0.0, 0.0, 0.5, 0.5)),
        (UPPER_RIGHT, rect(0.5, 0.0, 1.0, 0.5)),
        (LOWER_LEFT, rect(0.0, 0.5, 0.5, 1.0)),
        (LOWER_RIGHT, rect(0.5, 0.5, 1.0, 1.0)),
    ]
    .into_iter()
    .filter(|(bit, _)| mask & bit != 0)
    .map(|(_, rect)| rect)
    .collect()
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> BlockRect {
    BlockRect {
        left,
        top,
        right,
        bottom,
        alpha: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_halves_eighths_and_full() {
        assert_eq!(block_rects('▀'), vec![rect(0.0, 0.0, 1.0, 0.5)]);
        assert_eq!(block_rects('▄'), vec![rect(0.0, 0.5, 1.0, 1.0)]);
        assert_eq!(block_rects('█'), vec![rect(0.0, 0.0, 1.0, 1.0)]);
        assert_eq!(block_rects('▁'), vec![rect(0.0, 0.875, 1.0, 1.0)]);
        assert_eq!(block_rects('▌'), vec![rect(0.0, 0.0, 0.5, 1.0)]);
        assert_eq!(block_rects('▏'), vec![rect(0.0, 0.0, 0.125, 1.0)]);
        assert_eq!(block_rects('▐'), vec![rect(0.5, 0.0, 1.0, 1.0)]);
    }

    #[test]
    fn blocks_shades_are_translucent_and_quadrants_combine() {
        assert_eq!(block_rects('▒')[0].alpha, 0.5);
        assert_eq!(block_rects('▛').len(), 3);
        assert_eq!(block_rects('▝'), vec![rect(0.5, 0.0, 1.0, 0.5)]);
        assert_eq!(
            block_rects('▚'),
            vec![rect(0.0, 0.0, 0.5, 0.5), rect(0.5, 0.5, 1.0, 1.0)]
        );
        assert!(block_rects('a').is_empty());
        assert!(!is_block_element('─'));
    }
}

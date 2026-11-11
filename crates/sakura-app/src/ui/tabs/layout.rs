//! Where the tabs stand along the strip. A tab is as wide as its title asks, between the minimum and the
//! maximum; when the strip is too short every tab gives up width, in proportion to what it has to spare,
//! before anything scrolls (Chrome's "shrink"); tabs that still do not fit scroll sideways, the active one
//! always in sight. Which tab is selected never enters the widths: choosing a tab moves nothing.

use super::drag::TabBox;
use super::metrics::{TAB_GAP, TAB_MAX_WIDTH, TAB_MIN_WIDTH};

/// The tabs' boxes from `start`, in a strip with `room` px for them, before any scroll. `naturals` are the
/// widths that show each tab's whole title.
pub fn layout(naturals: &[f32], start: f32, room: f32) -> Vec<TabBox> {
    let mut widths: Vec<f32> = naturals
        .iter()
        .map(|natural| natural.clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH))
        .collect();
    let gaps = TAB_GAP * naturals.len().saturating_sub(1) as f32;
    let missing = widths.iter().sum::<f32>() + gaps - room;
    let spare_total: f32 = widths.iter().map(|width| width - TAB_MIN_WIDTH).sum();
    if missing > 0. && spare_total > 0. {
        let share = (missing / spare_total).min(1.);
        for width in &mut widths {
            // Whole pixels, rounded down: edges stay on the pixel grid and the tabs never overrun the strip.
            *width = (*width - (*width - TAB_MIN_WIDTH) * share).floor();
        }
    }
    let mut left = start;
    widths
        .into_iter()
        .map(|width| {
            let tab = TabBox { left, width };
            left += width + TAB_GAP;
            tab
        })
        .collect()
}

/// How far the strip is scrolled so that `tab` shows whole between `start` and `end`, starting from
/// `scroll` and never past the tabs' own span (`content_end`, before any scroll).
pub fn scroll_to_show(scroll: f32, tab: TabBox, start: f32, end: f32, content_end: f32) -> f32 {
    let furthest = (content_end - end).max(0.);
    let shown = if tab.left - scroll < start {
        tab.left - start
    } else if tab.right() - scroll > end {
        tab.right() - end
    } else {
        scroll
    };
    shown.clamp(0., furthest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(boxes: &[TabBox]) -> Vec<f32> {
        boxes.iter().map(|tab| tab.width).collect()
    }

    #[test]
    fn layout_with_room_gives_each_tab_its_title_between_the_limits() {
        let boxes = layout(&[60., 140., 400.], 0., 1000.);
        assert_eq!(widths(&boxes), [88., 140., 224.]);
    }

    #[test]
    fn layout_puts_the_first_tab_on_the_start_and_a_gap_between_tabs() {
        let boxes = layout(&[120., 120.], 72., 1000.);
        assert_eq!(boxes[0].left, 72.);
        assert_eq!(boxes[1].left, 72. + 120. + TAB_GAP);
    }

    #[test]
    fn layout_in_a_short_strip_narrows_every_tab_alike_to_fit_it() {
        let boxes = layout(&[200., 200., 200.], 0., 500.);
        assert_eq!(boxes[0].width, boxes[1].width);
        assert_eq!(boxes[1].width, boxes[2].width);
        let end = boxes[2].right();
        assert!(
            end <= 500. && end > 497.,
            "they fill the strip to the pixel: {end}"
        );
        assert_eq!(boxes[0].width.fract(), 0., "on whole pixels");
    }

    #[test]
    fn layout_takes_more_from_the_tabs_that_have_more_to_spare() {
        let boxes = layout(&[100., 220.], 0., 260.);
        assert!(boxes[0].width > 88. && boxes[0].width < 100.);
        assert!(boxes[1].width < 180.);
    }

    #[test]
    fn layout_never_narrows_a_tab_below_the_minimum() {
        let boxes = layout(&[200., 200., 200., 200.], 0., 300.);
        assert_eq!(widths(&boxes), [88., 88., 88., 88.]);
        assert!(boxes[3].right() > 300., "what does not fit scrolls");
    }

    #[test]
    fn scroll_to_show_leaves_a_tab_in_sight_alone() {
        let tab = TabBox {
            left: 100.,
            width: 120.,
        };
        assert_eq!(scroll_to_show(0., tab, 0., 400., 900.), 0.);
        assert_eq!(scroll_to_show(50., tab, 0., 400., 900.), 50.);
    }

    #[test]
    fn scroll_to_show_brings_a_tab_past_either_end_just_into_sight() {
        let far = TabBox {
            left: 600.,
            width: 120.,
        };
        assert_eq!(scroll_to_show(0., far, 0., 400., 900.), 320.);
        let near = TabBox {
            left: 100.,
            width: 120.,
        };
        assert_eq!(scroll_to_show(300., near, 0., 400., 900.), 100.);
    }

    #[test]
    fn scroll_to_show_never_scrolls_past_the_tabs() {
        let tab = TabBox {
            left: 0.,
            width: 120.,
        };
        assert_eq!(scroll_to_show(900., tab, 0., 400., 500.), 0.);
        assert_eq!(scroll_to_show(40., tab, 0., 400., 300.), 0., "all in sight");
    }
}

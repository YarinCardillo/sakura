//! What the page header gives up when the window is narrow, in order: the changed-lines figures first, then
//! the state word. The switch between the centre's views always stays, and the branch at the right
//! truncates. Nothing ever overlaps.

/// The widths of the header's parts, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    /// What is always there: the paddings, the state glyph, the gaps, the actions at the right.
    pub fixed: f32,
    /// The switch between the centre's views.
    pub views: f32,
    pub state_word: f32,
    pub figures: f32,
}

/// What the header shows at a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub has_figures: bool,
    pub has_state_word: bool,
}

impl Fit {
    /// Everything shown.
    pub const WHOLE: Fit = Fit {
        has_figures: true,
        has_state_word: true,
    };
}

/// What fits in `available` pixels.
pub fn fit(available: f32, widths: &Widths) -> Fit {
    let needed = |fit: &Fit| {
        let part = |is_shown: bool, width: f32| if is_shown { width } else { 0.0 };
        widths.fixed
            + widths.views
            + part(fit.has_state_word, widths.state_word)
            + part(fit.has_figures, widths.figures)
    };
    let without_figures = Fit {
        has_figures: false,
        ..Fit::WHOLE
    };
    let switch_only = Fit {
        has_state_word: false,
        ..without_figures
    };
    let steps = [Fit::WHOLE, without_figures];
    let fitting = steps.into_iter().find(|fit| needed(fit) <= available);
    fitting.unwrap_or(switch_only)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTHS: Widths = Widths {
        fixed: 300.,
        views: 120.,
        state_word: 80.,
        figures: 60.,
    };

    #[test]
    fn a_wide_header_shows_everything() {
        assert_eq!(fit(560., &WIDTHS), Fit::WHOLE);
    }

    #[test]
    fn the_figures_go_first_then_the_state_word() {
        let without_figures = fit(559., &WIDTHS);
        assert!(!without_figures.has_figures && without_figures.has_state_word);
        let switch_only = fit(499., &WIDTHS);
        assert!(!switch_only.has_figures && !switch_only.has_state_word);
    }

    #[test]
    fn a_header_narrower_than_everything_keeps_the_mark_and_the_switch() {
        let fit = fit(100., &WIDTHS);
        assert_eq!(
            fit,
            Fit {
                has_figures: false,
                has_state_word: false
            }
        );
    }
}

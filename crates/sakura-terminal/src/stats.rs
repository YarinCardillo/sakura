//! Counters that make the view's speed measurable: how often the grid is laid out, how many rows are shaped,
//! how long that takes, and how long a key takes to reach the screen.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// Most samples of each kind kept; older ones are dropped.
const MAX_SAMPLES: usize = 100_000;

/// A copy of the counters at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderStats {
    /// Frames in which the view was painted (cursor, input and overlay).
    pub view_paints: u64,
    /// Frames in which the grid was laid out again (not reused from the previous frame).
    pub grid_layouts: u64,
    /// Rows rebuilt from emulator damage.
    pub rows_rebuilt: u64,
    /// Text segments shaped.
    pub segments_shaped: u64,
    /// Time spent laying out the grid in each frame that did.
    pub grid_layout_times: Vec<Duration>,
    /// Moments the view was painted, to measure frame intervals.
    pub paint_instants: Vec<Instant>,
    /// From a key press to the first grid layout that drew output after it.
    pub key_to_paint: Vec<Duration>,
}

/// Live counters, owned by one view (window thread only).
#[derive(Default)]
pub(crate) struct StatsRecorder {
    view_paints: Cell<u64>,
    grid_layouts: Cell<u64>,
    rows_rebuilt: Cell<u64>,
    segments_shaped: Cell<u64>,
    grid_layout_times: RefCell<Vec<Duration>>,
    paint_instants: RefCell<Vec<Instant>>,
    key_to_paint: RefCell<Vec<Duration>>,
    pending_key: Cell<Option<Instant>>,
}

impl StatsRecorder {
    pub fn view_painted(&self) {
        self.view_paints.set(self.view_paints.get() + 1);
        push_capped(&self.paint_instants, Instant::now());
    }

    pub fn grid_laid_out(&self, took: Duration, rows: usize, segments: usize) {
        self.grid_layouts.set(self.grid_layouts.get() + 1);
        self.rows_rebuilt.set(self.rows_rebuilt.get() + rows as u64);
        self.segments_shaped
            .set(self.segments_shaped.get() + segments as u64);
        push_capped(&self.grid_layout_times, took);
        if rows > 0 {
            if let Some(pressed) = self.pending_key.take() {
                push_capped(&self.key_to_paint, pressed.elapsed());
            }
        }
    }

    /// A key that sends input was pressed; the next grid layout with new output closes the measurement.
    pub fn key_pressed(&self) {
        if self.pending_key.get().is_none() {
            self.pending_key.set(Some(Instant::now()));
        }
    }

    pub fn snapshot(&self) -> RenderStats {
        RenderStats {
            view_paints: self.view_paints.get(),
            grid_layouts: self.grid_layouts.get(),
            rows_rebuilt: self.rows_rebuilt.get(),
            segments_shaped: self.segments_shaped.get(),
            grid_layout_times: self.grid_layout_times.borrow().clone(),
            paint_instants: self.paint_instants.borrow().clone(),
            key_to_paint: self.key_to_paint.borrow().clone(),
        }
    }
}

fn push_capped<T>(samples: &RefCell<Vec<T>>, value: T) {
    let mut samples = samples.borrow_mut();
    if samples.len() >= MAX_SAMPLES {
        samples.remove(0);
    }
    samples.push(value);
}

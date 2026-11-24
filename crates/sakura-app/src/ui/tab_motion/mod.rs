//! What moves a tab: the geometry of a tab dragged along a row, the springs tabs ride on, and the
//! trackpad's tap when a dragged tab crosses into another slot. Shared by the page bar and by the strip of
//! the agentbox build, which are two rows of tabs behaving alike.

pub mod drag;
pub mod haptics;
pub mod motion;

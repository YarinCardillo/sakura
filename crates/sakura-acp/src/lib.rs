//! The agent conversation over the Agent Client Protocol (ACP), without any UI toolkit: the connection to
//! one agent process (`connection`), the conversation's own types (`model`), the reducer that turns events
//! into typed entries (`thread`), the confinement policy for permission requests (`policy`), tool call
//! diffs as hunks (`diff`), where a path is relative to the workspace (`paths`), the session id of each task
//! (`store`), and the stdio transport.

pub mod connection;
mod convert;
pub mod diff;
pub mod model;
pub mod paths;
pub mod policy;
pub mod store;
pub mod thread;
pub mod transport;

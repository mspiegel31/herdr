//! Internal app events delivered via channel.
//!
//! Background tasks (PTY child watchers, future hook listeners, etc.) send
//! events to the main loop through this channel. No polling needed.

use crate::detect::{Agent, AgentState};
use crate::layout::PaneId;

/// An event from a background task to the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A pane's child process exited.
    PaneDied { pane_id: PaneId },
    /// Agent state changed in a pane (detected by the PTY reader).
    StateChanged {
        pane_id: PaneId,
        agent: Option<Agent>,
        state: AgentState,
    },
    /// A new version was downloaded and installed. Restart to use it.
    UpdateReady { version: String },
}

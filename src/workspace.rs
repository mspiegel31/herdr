use std::collections::HashMap;

use ratatui::layout::Direction;
use tokio::sync::mpsc;
use tracing::info;

use crate::detect::{self, AgentState};
use crate::events::AppEvent;
use crate::layout::{PaneId, TileLayout};
use crate::pane::{PaneRuntime, PaneState};

/// A named workspace containing tiled terminal panes.
pub struct Workspace {
    pub name: String,
    pub layout: TileLayout,
    /// Pane state — always present, testable without PTYs.
    pub panes: HashMap<PaneId, PaneState>,
    /// Pane runtimes — only present in production (empty in tests).
    pub runtimes: HashMap<PaneId, PaneRuntime>,
    pub zoomed: bool,
    pub events: mpsc::Sender<AppEvent>,
}

impl Workspace {
    pub fn new(
        name: String,
        rows: u16,
        cols: u16,
        events: mpsc::Sender<AppEvent>,
    ) -> std::io::Result<Self> {
        let (layout, root_id) = TileLayout::new();

        let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
        let runtime = PaneRuntime::spawn(root_id, rows, cols, cwd, events.clone())?;

        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new());
        let mut runtimes = HashMap::new();
        runtimes.insert(root_id, runtime);

        info!(workspace = %name, root_pane = root_id.raw(), "workspace created");
        Ok(Self {
            name,
            layout,
            panes,
            runtimes,
            zoomed: false,
            events,
        })
    }

    /// Split the focused pane. Returns the new pane id.
    pub fn split_focused(
        &mut self,
        direction: Direction,
        rows: u16,
        cols: u16,
        cwd: Option<std::path::PathBuf>,
    ) -> std::io::Result<PaneId> {
        let new_id = self.layout.split_focused(direction);
        let actual_cwd =
            cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));
        let runtime = PaneRuntime::spawn(new_id, rows, cols, actual_cwd, self.events.clone())?;
        self.panes.insert(new_id, PaneState::new());
        self.runtimes.insert(new_id, runtime);
        self.zoomed = false;
        Ok(new_id)
    }

    /// Close the focused pane. Returns the removed pane id, or None if last pane.
    pub fn close_focused(&mut self) -> Option<PaneId> {
        if self.layout.pane_count() <= 1 {
            return None;
        }
        let closed = self.layout.focused();
        if self.layout.close_focused() {
            self.panes.remove(&closed);
            self.runtimes.remove(&closed);
            self.zoomed = false;
            Some(closed)
        } else {
            None
        }
    }

    /// Get the runtime for the focused pane.
    pub fn focused_runtime(&self) -> Option<&PaneRuntime> {
        self.runtimes.get(&self.layout.focused())
    }

    /// Aggregate state + seen across all panes.
    /// Returns the highest-priority state and the worst-case seen flag.
    pub fn aggregate_state(&self) -> (AgentState, bool) {
        let states: Vec<AgentState> = self.panes.values().map(|p| p.state).collect();
        let state = detect::workspace_state(&states);
        let seen = self.panes.values().all(|p| p.seen);
        (state, seen)
    }

    /// Per-pane (state, seen) in BSP tree order (left-to-right, top-to-bottom).
    pub fn pane_states(&self) -> Vec<(AgentState, bool)> {
        self.layout
            .pane_ids()
            .iter()
            .map(|id| {
                self.panes
                    .get(id)
                    .map(|p| (p.state, p.seen))
                    .unwrap_or((AgentState::Unknown, true))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Test helpers — construct workspaces without PTYs
// ---------------------------------------------------------------------------

#[cfg(test)]
impl Workspace {
    /// Create a test workspace with one pane, no PTY runtime.
    pub fn test_new(name: &str) -> Self {
        let (events, _) = mpsc::channel(64);
        let (layout, root_id) = TileLayout::new();
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new());
        Self {
            name: name.to_string(),
            layout,
            panes,
            runtimes: HashMap::new(),
            zoomed: false,
            events,
        }
    }

    /// Add a test pane (splits focused, no PTY runtime).
    pub fn test_split(&mut self, direction: Direction) -> PaneId {
        let new_id = self.layout.split_focused(direction);
        self.panes.insert(new_id, PaneState::new());
        new_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AgentState;

    #[test]
    fn aggregate_state_all_unknown() {
        let ws = Workspace::test_new("test");
        let (state, seen) = ws.aggregate_state();
        assert_eq!(state, AgentState::Unknown);
        assert!(seen);
    }

    #[test]
    fn aggregate_state_priority() {
        let mut ws = Workspace::test_new("test");
        let id2 = ws.test_split(Direction::Horizontal);

        // Set one pane to Busy, one to Idle
        let root_id = *ws.panes.keys().find(|id| **id != id2).unwrap();
        ws.panes.get_mut(&root_id).unwrap().state = AgentState::Idle;
        ws.panes.get_mut(&id2).unwrap().state = AgentState::Busy;

        let (state, _) = ws.aggregate_state();
        // Busy should be higher priority than Idle
        assert_eq!(state, AgentState::Busy);
    }

    #[test]
    fn aggregate_seen_any_unseen_means_unseen() {
        let mut ws = Workspace::test_new("test");
        let id2 = ws.test_split(Direction::Horizontal);

        ws.panes.get_mut(&id2).unwrap().seen = false;

        let (_, seen) = ws.aggregate_state();
        assert!(!seen);
    }

    #[test]
    fn close_focused_removes_pane() {
        let mut ws = Workspace::test_new("test");
        let _id2 = ws.test_split(Direction::Horizontal);
        assert_eq!(ws.panes.len(), 2);

        let closed = ws.close_focused();
        assert!(closed.is_some());
        assert_eq!(ws.panes.len(), 1);
    }

    #[test]
    fn close_focused_last_pane_returns_none() {
        let mut ws = Workspace::test_new("test");
        assert_eq!(ws.panes.len(), 1);

        let closed = ws.close_focused();
        assert!(closed.is_none());
        assert_eq!(ws.panes.len(), 1);
    }

    #[test]
    fn pane_states_matches_layout_order() {
        let mut ws = Workspace::test_new("test");
        let id2 = ws.test_split(Direction::Horizontal);

        // Set second pane to Waiting
        ws.panes.get_mut(&id2).unwrap().state = AgentState::Waiting;

        let states = ws.pane_states();
        assert_eq!(states.len(), 2);
        // One should be Unknown, one Waiting
        assert!(states.iter().any(|(s, _)| *s == AgentState::Waiting));
        assert!(states.iter().any(|(s, _)| *s == AgentState::Unknown));
    }
}

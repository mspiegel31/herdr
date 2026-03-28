//! Input handling — translates crossterm key/mouse events into state mutations.

use bytes::Bytes;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Direction;
use tracing::warn;

use crate::layout::{NavDirection, PaneInfo, SplitBorder};
use crate::selection::Selection;

use super::state::{key_matches, AppState, ContextMenuState, DragState, Mode, CONTEXT_MENU_ITEMS};
use super::App;

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

impl App {
    pub(super) async fn handle_key(&mut self, key: KeyEvent) {
        match self.state.mode {
            Mode::Navigate => handle_navigate_key(&mut self.state, key),
            Mode::Terminal => self.handle_terminal_key(key).await,
            Mode::CreateSession => handle_create_key(self, key),
            Mode::RenameSession => handle_rename_key(&mut self.state, key),
            Mode::Resize => handle_resize_key(&mut self.state, key),
            Mode::ConfirmClose => handle_confirm_close_key(&mut self.state, key),
            Mode::ContextMenu => handle_context_menu_key(&mut self.state, key),
        }
    }

    pub(super) async fn handle_paste(&mut self, text: String) {
        if self.state.mode != Mode::Terminal {
            return;
        }
        if let Some(ws) = self.state.active.and_then(|i| self.state.workspaces.get(i)) {
            if let Some(rt) = ws.focused_runtime() {
                let bracketed = rt
                    .parser
                    .read()
                    .map(|p| p.screen().bracketed_paste())
                    .unwrap_or(false);

                let payload = if bracketed {
                    format!("\x1b[200~{text}\x1b[201~")
                } else {
                    text
                };
                let _ = rt.sender.send(Bytes::from(payload)).await;
            }
        }
    }

    async fn handle_terminal_key(&mut self, key: KeyEvent) {
        self.state.clear_selection();
        self.state.update_dismissed = true;

        if self.state.is_prefix(&key) {
            self.state.mode = Mode::Navigate;
            return;
        }

        if let Some(ws) = self.state.active.and_then(|i| self.state.workspaces.get(i)) {
            if let Some(rt) = ws.focused_runtime() {
                rt.scroll_reset();
                let kitty = rt.kitty_keyboard.load(std::sync::atomic::Ordering::Relaxed);
                let bytes = crate::input::encode_key(key, kitty);
                if bytes.is_empty() {
                    warn!(code = ?key.code, mods = ?key.modifiers, state = ?key.state, "key produced empty encoding");
                } else {
                    let _ = rt.sender.send(Bytes::from(bytes)).await;
                }
            }
        }
    }
}

fn handle_navigate_key(state: &mut AppState, key: KeyEvent) {
    state.update_dismissed = true;

    if state.is_prefix(&key) || key.code == KeyCode::Esc {
        if state.active.is_some() {
            state.mode = Mode::Terminal;
        }
        return;
    }

    // Configurable keybinds (checked before fixed binds)
    let kb = &state.keybinds;
    if key_matches(&key, kb.split_vertical.0, kb.split_vertical.1) {
        state.split_pane(Direction::Horizontal);
        return;
    }
    if key_matches(&key, kb.split_horizontal.0, kb.split_horizontal.1) {
        state.split_pane(Direction::Vertical);
        return;
    }
    if key_matches(&key, kb.close_pane.0, kb.close_pane.1) {
        state.close_pane();
        return;
    }
    if key_matches(&key, kb.fullscreen.0, kb.fullscreen.1) {
        state.toggle_fullscreen();
        return;
    }

    match key.code {
        KeyCode::Char('q') => state.should_quit = true,
        KeyCode::Char('n') => {
            state.mode = Mode::CreateSession;
            state.name_input.clear();
        }
        KeyCode::Char('N') => {
            if !state.workspaces.is_empty() {
                state.name_input = state.workspaces[state.selected].name.clone();
                state.mode = Mode::RenameSession;
            }
        }
        KeyCode::Char('d') => {
            if !state.workspaces.is_empty() {
                if state.confirm_close {
                    state.mode = Mode::ConfirmClose;
                } else {
                    state.close_selected_workspace();
                    if state.workspaces.is_empty() {
                        state.mode = Mode::Navigate;
                    } else {
                        state.mode = Mode::Terminal;
                    }
                }
            }
        }
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
        }
        KeyCode::Down => {
            if !state.workspaces.is_empty() && state.selected < state.workspaces.len() - 1 {
                state.selected += 1;
            }
        }
        KeyCode::Enter => {
            if !state.workspaces.is_empty() {
                state.switch_workspace(state.selected);
                state.mode = Mode::Terminal;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => state.navigate_pane(NavDirection::Left),
        KeyCode::Char('j') => state.navigate_pane(NavDirection::Down),
        KeyCode::Char('k') => state.navigate_pane(NavDirection::Up),
        KeyCode::Char('l') | KeyCode::Right => state.navigate_pane(NavDirection::Right),
        KeyCode::Tab => state.cycle_pane(false),
        KeyCode::BackTab => state.cycle_pane(true),
        KeyCode::Char('r') => state.mode = Mode::Resize,
        KeyCode::Char('b') => state.sidebar_collapsed = !state.sidebar_collapsed,
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as usize) - ('1' as usize);
            if idx < state.workspaces.len() {
                state.switch_workspace(idx);
                state.mode = Mode::Terminal;
            }
        }
        _ => {}
    }
}

fn handle_create_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let name = if app.state.name_input.trim().is_empty() {
                format!("workspace-{}", app.state.workspaces.len() + 1)
            } else {
                app.state.name_input.trim().to_string()
            };
            app.state.name_input.clear();
            app.create_workspace(name);
        }
        KeyCode::Esc => {
            app.state.name_input.clear();
            app.state.mode = Mode::Navigate;
        }
        KeyCode::Backspace => {
            app.state.name_input.pop();
        }
        KeyCode::Char(c) => {
            app.state.name_input.push(c);
        }
        _ => {}
    }
}

fn handle_rename_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let new_name = if state.name_input.trim().is_empty() {
                state.name_input.clone()
            } else {
                state.name_input.trim().to_string()
            };
            if !new_name.is_empty() && !state.workspaces.is_empty() {
                state.workspaces[state.selected].name = new_name;
            }
            state.name_input.clear();
            state.mode = Mode::Navigate;
        }
        KeyCode::Esc => {
            state.name_input.clear();
            state.mode = Mode::Navigate;
        }
        KeyCode::Backspace => {
            state.name_input.pop();
        }
        KeyCode::Char(c) => {
            state.name_input.push(c);
        }
        _ => {}
    }
}

fn handle_resize_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('r') => {
            if state.active.is_some() {
                state.mode = Mode::Terminal;
            } else {
                state.mode = Mode::Navigate;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => state.resize_pane(NavDirection::Left),
        KeyCode::Char('l') | KeyCode::Right => state.resize_pane(NavDirection::Right),
        KeyCode::Char('j') | KeyCode::Down => state.resize_pane(NavDirection::Down),
        KeyCode::Char('k') | KeyCode::Up => state.resize_pane(NavDirection::Up),
        _ => {}
    }
}

fn handle_confirm_close_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => {
            state.close_selected_workspace();
            if state.workspaces.is_empty() {
                state.mode = Mode::Navigate;
            } else {
                state.mode = Mode::Terminal;
            }
        }
        _ => {
            state.mode = Mode::Navigate;
        }
    }
}

fn handle_context_menu_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            state.context_menu = None;
            state.mode = Mode::Navigate;
        }
        KeyCode::Up => {
            if let Some(menu) = &mut state.context_menu {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
            }
        }
        KeyCode::Down => {
            if let Some(menu) = &mut state.context_menu {
                if menu.selected < CONTEXT_MENU_ITEMS.len() - 1 {
                    menu.selected += 1;
                }
            }
        }
        KeyCode::Enter => {
            if let Some(menu) = state.context_menu.take() {
                match CONTEXT_MENU_ITEMS[menu.selected] {
                    "Rename" => {
                        state.selected = menu.ws_idx;
                        state.name_input = state.workspaces[menu.ws_idx].name.clone();
                        state.mode = Mode::RenameSession;
                    }
                    "Close" => {
                        state.selected = menu.ws_idx;
                        if state.confirm_close {
                            state.mode = Mode::ConfirmClose;
                        } else {
                            state.close_selected_workspace();
                            state.mode = Mode::Navigate;
                        }
                    }
                    _ => state.mode = Mode::Navigate,
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Mouse handling
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        let sidebar = self.view.sidebar_rect;
        let in_sidebar = mouse.column >= sidebar.x
            && mouse.column < sidebar.x + sidebar.width
            && mouse.row >= sidebar.y
            && mouse.row < sidebar.y + sidebar.height;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = None;

                if self.mode == Mode::ContextMenu {
                    if let Some(menu) = &self.context_menu {
                        let item_idx = self.context_menu_item_at(mouse.column, mouse.row);
                        if let Some(idx) = item_idx {
                            let ws_idx = menu.ws_idx;
                            self.context_menu = None;
                            match CONTEXT_MENU_ITEMS[idx] {
                                "Rename" => {
                                    self.selected = ws_idx;
                                    self.name_input = self.workspaces[ws_idx].name.clone();
                                    self.mode = Mode::RenameSession;
                                }
                                "Close" => {
                                    self.selected = ws_idx;
                                    if self.confirm_close {
                                        self.mode = Mode::ConfirmClose;
                                    } else {
                                        self.close_selected_workspace();
                                        self.mode = Mode::Navigate;
                                    }
                                }
                                _ => self.mode = Mode::Navigate,
                            }
                        } else {
                            self.context_menu = None;
                            self.mode = Mode::Navigate;
                        }
                    }
                    return;
                }

                if !in_sidebar {
                    if let Some(border) = self.find_border_at(mouse.column, mouse.row) {
                        self.drag = Some(DragState {
                            path: border.path.clone(),
                            direction: border.direction,
                            area: border.area,
                        });
                        return;
                    }
                }

                if in_sidebar {
                    let bottom_y = sidebar.y + sidebar.height.saturating_sub(1);
                    if mouse.row == bottom_y {
                        self.sidebar_collapsed = !self.sidebar_collapsed;
                        return;
                    }

                    let inner_y = sidebar.y + 1;
                    if mouse.row >= inner_y {
                        let idx = (mouse.row - inner_y) as usize;
                        if idx < self.workspaces.len() {
                            self.switch_workspace(idx);
                            self.mode = Mode::Terminal;
                        }
                    }
                } else if let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() {
                    let (row, col) = (
                        mouse.row - info.inner_rect.y,
                        mouse.column - info.inner_rect.x,
                    );
                    self.selection = Some(Selection::anchor(info.id, row, col, info.inner_rect));

                    if let Some(ws) = self.active.and_then(|i| self.workspaces.get_mut(i)) {
                        if ws.layout.focused() != info.id {
                            ws.layout.focus_pane(info.id);
                        }
                    }
                    if self.mode != Mode::Terminal {
                        self.mode = Mode::Terminal;
                    }
                } else {
                    if let Some(info) = self.view.pane_infos.iter().find(|p| {
                        mouse.column >= p.rect.x
                            && mouse.column < p.rect.x + p.rect.width
                            && mouse.row >= p.rect.y
                            && mouse.row < p.rect.y + p.rect.height
                    }) {
                        let id = info.id;
                        if let Some(ws) = self.active.and_then(|i| self.workspaces.get_mut(i)) {
                            if ws.layout.focused() != id {
                                ws.layout.focus_pane(id);
                            }
                        }
                        if self.mode != Mode::Terminal {
                            self.mode = Mode::Terminal;
                        }
                    }
                }
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = &self.drag {
                    let ratio = match drag.direction {
                        Direction::Horizontal => {
                            (mouse.column.saturating_sub(drag.area.x)) as f32
                                / drag.area.width.max(1) as f32
                        }
                        Direction::Vertical => {
                            (mouse.row.saturating_sub(drag.area.y)) as f32
                                / drag.area.height.max(1) as f32
                        }
                    };
                    let ratio = ratio.clamp(0.1, 0.9);
                    let path = drag.path.clone();
                    if let Some(ws) = self.active.and_then(|i| self.workspaces.get_mut(i)) {
                        ws.layout.set_ratio_at(&path, ratio);
                    }
                } else if let Some(sel) = &mut self.selection {
                    sel.drag(mouse.column, mouse.row);
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take().is_some() {
                    // Drag ended
                } else {
                    let was_click = self.selection.as_ref().is_some_and(|s| s.was_just_click());
                    if was_click {
                        self.selection = None;
                    } else {
                        self.copy_selection();
                    }
                }
            }

            MouseEventKind::ScrollUp if !in_sidebar => {
                self.selection = None;
                if let Some(ws) = self.active.and_then(|i| self.workspaces.get(i)) {
                    if let Some(rt) = ws.focused_runtime() {
                        rt.scroll_up(3);
                    }
                }
            }
            MouseEventKind::ScrollDown if !in_sidebar => {
                self.selection = None;
                if let Some(ws) = self.active.and_then(|i| self.workspaces.get(i)) {
                    if let Some(rt) = ws.focused_runtime() {
                        rt.scroll_down(3);
                    }
                }
            }

            MouseEventKind::ScrollUp if in_sidebar => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            MouseEventKind::ScrollDown if in_sidebar => {
                if !self.workspaces.is_empty() && self.selected < self.workspaces.len() - 1 {
                    self.selected += 1;
                }
            }

            MouseEventKind::Down(MouseButton::Right) if in_sidebar => {
                let inner_y = sidebar.y + 1;
                if mouse.row >= inner_y {
                    let idx = (mouse.row - inner_y) as usize;
                    if idx < self.workspaces.len() {
                        self.selected = idx;
                        self.context_menu = Some(ContextMenuState {
                            ws_idx: idx,
                            x: mouse.column,
                            y: mouse.row,
                            selected: 0,
                        });
                        self.mode = Mode::ContextMenu;
                    }
                }
            }

            _ => {}
        }
    }

    fn context_menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.context_menu.as_ref()?;
        let menu_w = 14u16;
        let inner_x = menu.x;
        let inner_y = menu.y + 1;
        if col >= inner_x
            && col < inner_x + menu_w
            && row >= inner_y
            && row < inner_y + CONTEXT_MENU_ITEMS.len() as u16
        {
            Some((row - inner_y) as usize)
        } else {
            None
        }
    }

    fn find_border_at(&self, col: u16, row: u16) -> Option<&SplitBorder> {
        self.view.split_borders.iter().find(|b| match b.direction {
            Direction::Horizontal => {
                (col as i32 - b.pos as i32).unsigned_abs() <= 1
                    && row >= b.area.y
                    && row < b.area.y + b.area.height
            }
            Direction::Vertical => {
                (row as i32 - b.pos as i32).unsigned_abs() <= 1
                    && col >= b.area.x
                    && col < b.area.x + b.area.width
            }
        })
    }

    fn pane_at(&self, col: u16, row: u16) -> Option<&PaneInfo> {
        self.view.pane_infos.iter().find(|p| {
            col >= p.inner_rect.x
                && col < p.inner_rect.x + p.inner_rect.width
                && row >= p.inner_rect.y
                && row < p.inner_rect.y + p.inner_rect.height
        })
    }
}

// Note: split_pane needs runtime (event_tx for PTY spawn), so it lives on App
impl AppState {
    pub(crate) fn split_pane(&mut self, direction: Direction) {
        // Actual PTY spawning happens in Workspace::split_focused
        // which needs events channel — this is called from navigate_key
        // where we don't have async context, so the workspace handles it
        let (rows, cols) = self.estimate_pane_size();
        let new_rows = (rows / 2).max(4);
        let new_cols = (cols / 2).max(10);

        let cwd = self
            .active
            .and_then(|i| self.workspaces.get(i))
            .and_then(|ws| ws.focused_runtime())
            .and_then(|rt| rt.cwd());

        if let Some(ws) = self.active.and_then(|i| self.workspaces.get_mut(i)) {
            if let Ok(new_id) = ws.split_focused(direction, new_rows, new_cols, cwd) {
                ws.layout.focus_pane(new_id);
                self.mode = Mode::Terminal;
            }
        }
    }
}

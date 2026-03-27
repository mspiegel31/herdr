use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use tui_term::widget::PseudoTerminal;

use crate::app::{AppState, Mode};
use crate::detect::AgentState;
use crate::layout::PaneInfo;

const COLLAPSED_WIDTH: u16 = 4; // num + space + dot + separator
const MIN_SIDEBAR_WIDTH: u16 = 16;
const MAX_SIDEBAR_WIDTH: u16 = 36;

/// Compute view geometry and reconcile pane sizes.
/// Called before render to separate mutation from drawing.
pub fn compute_view(app: &mut AppState, area: Rect) {
    let sidebar_w = if app.sidebar_collapsed {
        COLLAPSED_WIDTH
    } else {
        compute_sidebar_width(app)
    };

    let [sidebar_area, main_area] =
        Layout::horizontal([Constraint::Length(sidebar_w), Constraint::Min(1)]).areas(area);

    let terminal_area = main_area;

    // Compute split borders
    let split_borders = app
        .active
        .and_then(|i| app.workspaces.get(i))
        .map(|ws| ws.layout.splits(terminal_area))
        .unwrap_or_default();

    // Compute pane layout + reconcile sizes
    let pane_infos = compute_pane_infos(app, terminal_area);

    app.view = crate::app::ViewState {
        sidebar_rect: sidebar_area,
        terminal_area,
        pane_infos,
        split_borders,
    };
}

/// Render the UI — reads AppState but does not mutate it.
pub fn render(app: &AppState, frame: &mut Frame) {
    let sidebar_area = app.view.sidebar_rect;
    let terminal_area = app.view.terminal_area;

    if app.sidebar_collapsed {
        render_sidebar_collapsed(app, frame, sidebar_area);
    } else {
        render_sidebar(app, frame, sidebar_area);
    }
    render_panes(app, frame, terminal_area);

    match app.mode {
        Mode::Navigate => render_navigate_overlay(app, frame, terminal_area),
        Mode::Resize => render_resize_overlay(app, frame, terminal_area),
        Mode::ConfirmClose => render_confirm_close_overlay(app, frame, terminal_area),
        Mode::ContextMenu => {
            render_navigate_overlay(app, frame, terminal_area);
            render_context_menu(app, frame);
        }
        Mode::CreateSession | Mode::RenameSession => {}
        Mode::Terminal => {}
    }

    // Update notification (rendered on top of everything)
    if let Some(version) = &app.update_available {
        if !app.update_dismissed {
            render_update_notification(frame, terminal_area, version, app.accent);
        }
    }
}

/// Compute pane layout info and resize pane runtimes to match.
fn compute_pane_infos(app: &AppState, area: Rect) -> Vec<PaneInfo> {
    let Some(ws_idx) = app.active else {
        return Vec::new();
    };
    let Some(ws) = app.workspaces.get(ws_idx) else {
        return Vec::new();
    };

    if ws.zoomed {
        let focused_id = ws.layout.focused();
        if let Some(rt) = ws.runtimes.get(&focused_id) {
            rt.resize(area.height, area.width);
        }
        return vec![PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect: area,
            is_focused: true,
        }];
    }

    let multi_pane = ws.layout.pane_count() > 1;
    let terminal_active = app.mode == Mode::Terminal;
    let mut pane_infos = ws.layout.panes(area);

    for info in &mut pane_infos {
        let inner = if multi_pane {
            let border_set = if info.is_focused && terminal_active {
                ratatui::symbols::border::THICK
            } else {
                ratatui::symbols::border::PLAIN
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_set(border_set);
            block.inner(info.rect)
        } else {
            area
        };
        info.inner_rect = inner;

        if let Some(rt) = ws.runtimes.get(&info.id) {
            rt.resize(inner.height, inner.width);
        }
    }

    pane_infos
}

/// Auto-scale sidebar width based on longest workspace name.
fn compute_sidebar_width(app: &AppState) -> u16 {
    if app.workspaces.is_empty() {
        return app.sidebar_width; // config default for empty state
    }
    let max_name = app
        .workspaces
        .iter()
        .map(|ws| {
            let pane_count = ws.layout.pane_count();
            let bracket = if pane_count > 1 { 3 + pane_count } else { 0 }; // " [●●]"
            ws.name.len() + bracket
        })
        .max()
        .unwrap_or(8);
    // marker(2) + dot(2) + name + bracket + padding(2) + borders(2)
    let needed = (max_name as u16) + 8;
    needed.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
}

/// Collapsed sidebar: borderless strip of state dots with selection highlight.
fn render_sidebar_collapsed(app: &AppState, frame: &mut Frame, area: Rect) {
    let is_navigating = matches!(
        app.mode,
        Mode::Navigate
            | Mode::CreateSession
            | Mode::RenameSession
            | Mode::Resize
            | Mode::ConfirmClose
            | Mode::ContextMenu
    );

    // Thin vertical separator line on the right edge
    let sep_style = if is_navigating {
        Style::default().fg(app.accent)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let content_w = area.width.saturating_sub(1); // leave room for separator

    for (i, ws) in app.workspaces.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let (agg_state, agg_seen) = ws.aggregate_state();
        let (icon, icon_style) = state_icon_style(agg_state, agg_seen);
        let is_selected = i == app.selected && is_navigating;

        let num_label = format!("{}", i + 1);

        let row_style = if is_selected {
            Style::default().bg(app.accent).fg(Color::Black)
        } else {
            Style::default()
        };
        let dim_style = if is_selected {
            row_style
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let y = area.y + i as u16;
        let row_area = Rect::new(area.x, y, content_w, 1);

        // Fill background for selected row
        if is_selected {
            let buf = frame.buffer_mut();
            for x in row_area.x..row_area.x + row_area.width {
                buf[(x, y)].set_style(row_style);
            }
        }

        let line = Line::from(vec![
            Span::styled(&num_label, dim_style),
            Span::styled(
                " ",
                if is_selected {
                    row_style
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                icon,
                if is_selected {
                    // Keep icon's fg color, just add the highlight bg
                    icon_style.bg(app.accent)
                } else {
                    icon_style
                },
            ),
        ]);
        frame.render_widget(Paragraph::new(line), row_area);
    }

    // Toggle button at bottom
    render_sidebar_toggle(frame, area, true);
}

fn render_sidebar(app: &AppState, frame: &mut Frame, area: Rect) {
    let is_navigating = matches!(
        app.mode,
        Mode::Navigate
            | Mode::CreateSession
            | Mode::RenameSession
            | Mode::Resize
            | Mode::ConfirmClose
            | Mode::ContextMenu
    );

    let highlight_style = if is_navigating {
        Style::default()
            .fg(Color::Black)
            .bg(app.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(Color::DarkGray)
    };

    // Thin separator on right edge (consistent with collapsed)
    let sep_style = if is_navigating {
        Style::default().fg(app.accent)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    // Content area (left of separator)
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);

    // Title line
    let title = match app.mode {
        Mode::Terminal => format!(" {} ⏎", app.prefix_label),
        Mode::Navigate | Mode::ContextMenu => " NAVIGATE".to_string(),
        Mode::Resize => " RESIZE".to_string(),
        Mode::CreateSession => " NEW".to_string(),
        Mode::RenameSession => " RENAME".to_string(),
        Mode::ConfirmClose => " CLOSE?".to_string(),
    };
    let title_style = if is_navigating {
        Style::default().fg(app.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(&title, title_style)),
        Rect::new(content.x, content.y, content.width, 1),
    );

    // Workspace list (below title)
    let list_area = Rect::new(
        content.x,
        content.y + 1,
        content.width,
        content.height.saturating_sub(1),
    );

    let items: Vec<ListItem> = app
        .workspaces
        .iter()
        .enumerate()
        .map(|(i, ws)| {
            let selected = i == app.selected && is_navigating;
            // When highlighted: text goes black on highlight bg, but icons keep their color
            let text_style = if selected {
                highlight_style
            } else {
                Style::default()
            };
            let dim_style = if selected {
                highlight_style
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let marker = if Some(i) == app.active { "▸" } else { " " };
            let (agg_state, agg_seen) = ws.aggregate_state();
            let (rollup_icon, rollup_style) = state_icon_style(agg_state, agg_seen);
            // Icon keeps its fg color, but picks up the highlight bg when selected
            let icon_style = if selected {
                rollup_style.bg(highlight_style.bg.unwrap_or(app.accent))
            } else {
                rollup_style
            };

            let num_label = format!("{}", i + 1);
            let num_style = if selected {
                highlight_style
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let mut spans = vec![
                Span::styled(num_label, num_style),
                Span::styled(format!("{marker}"), text_style),
                Span::styled(rollup_icon, icon_style),
                Span::styled(format!(" {}", ws.name), text_style),
            ];

            let pane_states = ws.pane_states();
            let has_any_agent = pane_states.iter().any(|(s, _)| *s != AgentState::Unknown);

            if ws.layout.pane_count() > 1 {
                if has_any_agent {
                    spans.push(Span::styled(" [", dim_style));
                    for (pane_state, pane_seen) in &pane_states {
                        let (icon, style) = state_icon_style(*pane_state, *pane_seen);
                        let pane_icon_style = if selected {
                            style.bg(highlight_style.bg.unwrap_or(app.accent))
                        } else {
                            style
                        };
                        spans.push(Span::styled(icon, pane_icon_style));
                    }
                    spans.push(Span::styled("]", dim_style));
                } else {
                    let count = ws.layout.pane_count();
                    spans.push(Span::styled(format!(" [{count}]"), dim_style));
                }
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // No highlight_style on List — we handle it per-span above to preserve icon colors
    let list = List::new(items);
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, list_area, &mut state);

    if app.mode == Mode::CreateSession {
        let input_y = list_area.y + list_area.height.saturating_sub(1);
        let input_area = Rect::new(content.x, input_y, content.width, 1);
        let text = format!(" Name: {}\u{2588}", app.name_input);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Yellow)),
            input_area,
        );
    }

    if app.mode == Mode::RenameSession {
        let input_y = list_area.y + app.selected as u16;
        let input_area = Rect::new(content.x, input_y, content.width, 1);
        let text = format!(" ✎ {}\u{2588}", app.name_input);
        frame.render_widget(Clear, input_area);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Yellow)),
            input_area,
        );
    }

    // Toggle button at bottom (skip when input field occupies that row)
    if app.mode != Mode::CreateSession {
        render_sidebar_toggle(frame, area, false);
    }
}

fn render_sidebar_toggle(frame: &mut Frame, area: Rect, collapsed: bool) {
    let bottom_y = area.y + area.height.saturating_sub(1);
    let content_w = area.width.saturating_sub(1); // exclude separator
    if content_w == 0 || area.height == 0 {
        return;
    }
    let icon = if collapsed { "»" } else { "«" };
    // Center the icon in the content area
    let x = area.x + content_w / 2;
    let toggle_area = Rect::new(x, bottom_y, 1, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(icon, Style::default().fg(Color::DarkGray))),
        toggle_area,
    );
}

fn render_panes(app: &AppState, frame: &mut Frame, area: Rect) {
    let Some(ws_idx) = app.active else {
        render_empty(frame, area, app.accent);
        return;
    };
    let Some(ws) = app.workspaces.get(ws_idx) else {
        render_empty(frame, area, app.accent);
        return;
    };

    let multi_pane = ws.layout.pane_count() > 1;
    let terminal_active = app.mode == Mode::Terminal;

    for info in &app.view.pane_infos {
        if let Some(rt) = ws.runtimes.get(&info.id) {
            // Draw borders for multi-pane layouts
            if multi_pane {
                let (border_style, border_set) = if info.is_focused && terminal_active {
                    (
                        Style::default().fg(app.accent),
                        ratatui::symbols::border::THICK,
                    )
                } else if info.is_focused {
                    (
                        Style::default().fg(app.accent),
                        ratatui::symbols::border::PLAIN,
                    )
                } else {
                    (
                        Style::default().fg(Color::DarkGray),
                        ratatui::symbols::border::PLAIN,
                    )
                };

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .border_set(border_set);
                frame.render_widget(block, info.rect);
            }

            // Draw terminal content
            if let Ok(parser) = rt.parser.read() {
                let pt = PseudoTerminal::new(parser.screen());
                frame.render_widget(pt, info.inner_rect);
            }

            // Dim unfocused panes only in navigate mode
            let should_dim = !info.is_focused && multi_pane && !terminal_active;
            if should_dim {
                let inner = info.inner_rect;
                let buf = frame.buffer_mut();
                for y in inner.y..inner.y + inner.height {
                    for x in inner.x..inner.x + inner.width {
                        let cell = &mut buf[(x, y)];
                        let style = cell.style();
                        let fg = style.fg.unwrap_or(Color::White);
                        let dimmed_fg = dim_color(fg);
                        cell.set_style(style.fg(dimmed_fg));
                    }
                }
            }

            // Selection highlight
            render_selection_highlight(&app.selection, frame, info.id, info.inner_rect);
        }
    }
}

/// Render selection highlight for a pane by inverting fg/bg colors.
/// Reduce a color's brightness by blending it toward black.
fn dim_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(r / 3, g / 3, b / 3),
        Color::White => Color::DarkGray,
        Color::Gray => Color::DarkGray,
        Color::DarkGray => Color::Rgb(30, 30, 30),
        Color::Red => Color::Rgb(60, 0, 0),
        Color::Green => Color::Rgb(0, 60, 0),
        Color::Yellow => Color::Rgb(60, 60, 0),
        Color::Blue => Color::Rgb(0, 0, 60),
        Color::Magenta => Color::Rgb(60, 0, 60),
        Color::Cyan => Color::Rgb(0, 60, 60),
        Color::LightRed => Color::Rgb(80, 30, 30),
        Color::LightGreen => Color::Rgb(30, 80, 30),
        Color::LightYellow => Color::Rgb(80, 80, 30),
        Color::LightBlue => Color::Rgb(30, 30, 80),
        Color::LightMagenta => Color::Rgb(80, 30, 80),
        Color::LightCyan => Color::Rgb(30, 80, 80),
        // Indexed colors and others: just use DIM modifier as fallback
        _ => Color::DarkGray,
    }
}

fn render_selection_highlight(
    selection: &Option<crate::selection::Selection>,
    frame: &mut Frame,
    pane_id: crate::layout::PaneId,
    inner: Rect,
) {
    if let Some(sel) = selection {
        if sel.is_visible() && sel.pane_id == pane_id {
            let buf = frame.buffer_mut();
            for y in 0..inner.height {
                for x in 0..inner.width {
                    if sel.contains(y, x) {
                        let cell = &mut buf[(inner.x + x, inner.y + y)];
                        // Fixed highlight: white text on blue background.
                        // Consistent regardless of the cell's original colors.
                        cell.set_style(
                            Style::default()
                                .fg(Color::White)
                                .bg(Color::Rgb(40, 80, 140)),
                        );
                    }
                }
            }
        }
    }
}

fn render_empty(frame: &mut Frame, area: Rect, accent: Color) {
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  No active workspace",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "n",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to create one", Style::default().fg(Color::DarkGray)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

/// Floating overlay for navigate mode — appears at bottom of terminal area.
fn render_navigate_overlay(app: &AppState, frame: &mut Frame, area: Rect) {
    let key = Style::default().fg(app.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let label = Style::default().fg(Color::White);

    let is_full = app
        .active
        .and_then(|i| app.workspaces.get(i))
        .is_some_and(|ws| ws.zoomed);

    let full_key = if is_full { "unfull" } else { "full" };

    let line1 = Line::from(vec![
        Span::styled(" n", key),
        Span::styled(" new  ", dim),
        Span::styled("N", key),
        Span::styled(" rename  ", dim),
        Span::styled("d", key),
        Span::styled(" close  ", dim),
        Span::styled("v", key),
        Span::styled(" split│  ", dim),
        Span::styled("-", key),
        Span::styled(" split─  ", dim),
        Span::styled("⇥", key),
        Span::styled(" pane  ", dim),
        Span::styled("f", key),
        Span::styled(format!(" {full_key}  "), dim),
        Span::styled("r", key),
        Span::styled(" resize  ", dim),
        Span::styled("b", key),
        Span::styled(" sidebar  ", dim),
        Span::styled("q", key),
        Span::styled(" quit", dim),
    ]);

    let ws_name = app
        .active
        .and_then(|i| app.workspaces.get(i))
        .map(|ws| ws.name.as_str())
        .unwrap_or("—");

    let pane_info = app
        .active
        .and_then(|i| app.workspaces.get(i))
        .filter(|ws| ws.layout.pane_count() > 1)
        .map(|ws| {
            let ids = ws.layout.pane_ids();
            let pos = ids
                .iter()
                .position(|id| *id == ws.layout.focused())
                .unwrap_or(0);
            format!(" [{}/{}]", pos + 1, ids.len())
        })
        .unwrap_or_default();

    let mode_style = Style::default()
        .fg(Color::Black)
        .bg(app.accent)
        .add_modifier(Modifier::BOLD);

    let line2 = Line::from(vec![
        Span::styled(" NAVIGATE ", mode_style),
        Span::raw(" "),
        Span::styled(ws_name, label),
        Span::styled(&pane_info, dim),
        Span::raw("  "),
        Span::styled("esc", key),
        Span::styled(" back  ", dim),
        Span::styled("↑↓", key),
        Span::styled(" select  ", dim),
        Span::styled("⏎", key),
        Span::styled(" open", dim),
    ]);

    let overlay_height = 2;
    let overlay_y = area.y + area.height.saturating_sub(overlay_height);
    let overlay_area = Rect::new(area.x, overlay_y, area.width, overlay_height);

    // Clear the area behind the overlay
    frame.render_widget(Clear, overlay_area);

    let bg = Style::default().bg(Color::Black);
    let buf = frame.buffer_mut();
    for y in overlay_area.y..overlay_area.y + overlay_area.height {
        for x in overlay_area.x..overlay_area.x + overlay_area.width {
            buf[(x, y)].set_style(bg);
        }
    }

    let [row1, row2] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(overlay_area);
    frame.render_widget(Paragraph::new(line1), row1);
    frame.render_widget(Paragraph::new(line2), row2);
}

/// Floating overlay for resize mode.
fn render_resize_overlay(app: &AppState, frame: &mut Frame, area: Rect) {
    let key = Style::default().fg(app.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let mode_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Magenta)
        .add_modifier(Modifier::BOLD);

    let line = Line::from(vec![
        Span::styled(" RESIZE ", mode_style),
        Span::raw("  "),
        Span::styled("h/l", key),
        Span::styled(" width  ", dim),
        Span::styled("j/k", key),
        Span::styled(" height  ", dim),
        Span::styled("esc", key),
        Span::styled(" done", dim),
    ]);

    let overlay_y = area.y + area.height.saturating_sub(1);
    let overlay_area = Rect::new(area.x, overlay_y, area.width, 1);

    frame.render_widget(Clear, overlay_area);
    let bg = Style::default().bg(Color::Black);
    let buf = frame.buffer_mut();
    for x in overlay_area.x..overlay_area.x + overlay_area.width {
        buf[(x, overlay_y)].set_style(bg);
    }
    frame.render_widget(Paragraph::new(line), overlay_area);
}

/// Centered popup confirmation dialog with dimmed background.
fn render_confirm_close_overlay(app: &AppState, frame: &mut Frame, area: Rect) {
    let ws_name = app
        .workspaces
        .get(app.selected)
        .map(|ws| ws.name.as_str())
        .unwrap_or("?");
    let pane_count = app
        .workspaces
        .get(app.selected)
        .map(|ws| ws.layout.pane_count())
        .unwrap_or(0);

    let pane_text = if pane_count == 1 {
        "1 pane".to_string()
    } else {
        format!("{pane_count} panes")
    };

    // Dim the entire background
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let cell = &mut buf[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }

    // Centered popup
    let popup_w = 44u16.min(area.width.saturating_sub(4));
    let popup_h = 5u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(popup_x, popup_y, popup_w, popup_h);

    let key = Style::default().fg(app.accent).add_modifier(Modifier::BOLD);
    let warn = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let title_line = Line::from(vec![Span::styled(" Close workspace?", warn)]);

    let detail_line = Line::from(vec![
        Span::styled(
            format!(" {ws_name}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" — {pane_text}"), dim),
    ]);

    let action_line = Line::from(vec![
        Span::raw(" "),
        Span::styled("y", key),
        Span::styled("/", dim),
        Span::styled("enter", key),
        Span::styled(" confirm    ", dim),
        Span::styled("any key", key),
        Span::styled(" cancel", dim),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    if inner.height >= 3 {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas::<3>(inner);
        frame.render_widget(Paragraph::new(title_line), rows[0]);
        frame.render_widget(Paragraph::new(detail_line), rows[1]);
        frame.render_widget(Paragraph::new(action_line), rows[2]);
    }
}

/// Right-click context menu popup anchored near the click position.
fn render_context_menu(app: &AppState, frame: &mut Frame) {
    use crate::app::CONTEXT_MENU_ITEMS;

    let Some(menu) = &app.context_menu else {
        return;
    };

    let menu_w = 14u16;
    let menu_h = CONTEXT_MENU_ITEMS.len() as u16 + 2; // +2 for border
    let area = frame.area();

    // Position: try to place below-right of click, clamp to screen
    let x = menu.x.min(area.width.saturating_sub(menu_w));
    let y = menu.y.min(area.height.saturating_sub(menu_h));
    let menu_rect = Rect::new(x, y, menu_w, menu_h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.accent))
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(menu_rect);

    frame.render_widget(Clear, menu_rect);
    frame.render_widget(block, menu_rect);

    let highlight = Style::default()
        .fg(Color::Black)
        .bg(app.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);

    for (i, item) in CONTEXT_MENU_ITEMS.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let style = if i == menu.selected {
            highlight
        } else {
            normal
        };
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        frame.render_widget(Paragraph::new(format!(" {item}")).style(style), row);
    }
}

fn render_update_notification(frame: &mut Frame, area: Rect, version: &str, accent: Color) {
    let text = format!(" ✦ herdr v{version} installed — restart to update ");
    let width = text.len() as u16 + 2;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(3);
    let notif_area = Rect::new(x, y, width.min(area.width), 1);

    frame.render_widget(Clear, notif_area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD),
        )),
        notif_area,
    );
}

/// Visual badge for a pane's state + seen flag.
///
/// | State              | Icon | Color  |
/// |--------------------|------|--------|
/// | Busy               | ●    | Yellow |
/// | Done (idle+unseen) | ●    | Blue   |
/// | Idle (seen)        | ○    | Green  |
/// | Unknown            | ·    | Gray   |
///
/// Filled dot = needs attention (working, or finished unseen).
/// Hollow dot = nothing to do here.
fn state_icon_style(state: AgentState, seen: bool) -> (&'static str, Style) {
    match (state, seen) {
        (AgentState::Waiting, _) => ("●", Style::default().fg(Color::Red)),
        (AgentState::Busy, _) => ("●", Style::default().fg(Color::Yellow)),
        (AgentState::Idle, false) => ("●", Style::default().fg(Color::Blue)), // Done
        (AgentState::Idle, true) => ("○", Style::default().fg(Color::Green)),
        (AgentState::Unknown, _) => ("·", Style::default().fg(Color::DarkGray)),
    }
}

fn _build_hints(items: &[(&str, &str)], key_style: Style, dim_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    for (i, (k, desc)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", dim_style));
        }
        spans.push(Span::styled(k.to_string(), key_style));
        spans.push(Span::styled(format!(" {desc}"), dim_style));
    }
    spans
}

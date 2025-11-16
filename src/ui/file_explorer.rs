//! File explorer UI components for dual-pane SFTP file transfer.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::{
    ActivePane, CopyOperation, FileExplorerPane, LeftExplorer, config::manager::Connection,
    filesystem::SftpFileSystem,
};

/// Draw the dual-pane file explorer interface.
#[allow(clippy::too_many_arguments)]
pub fn draw_file_explorer(
    f: &mut Frame,
    area: Rect,
    connection_name: &str,
    left_pane: &FileExplorerPane,
    left_explorer: &mut LeftExplorer,
    remote_explorer: &mut ratatui_explorer::FileExplorer<SftpFileSystem>,
    active_pane: &ActivePane,
    copy_buffer: &[CopyOperation],
    search_mode: bool,
    search_query: &str,
) {
    // Main layout: header, content, footer (and optional search input)
    let constraints = if search_mode {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(1),    // Content (dual panes)
            Constraint::Length(3), // Search input
            Constraint::Length(1), // Footer
        ]
    } else {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(1),    // Content (dual panes)
            Constraint::Length(1), // Footer
        ]
    };

    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Render header
    draw_header(f, main_layout[0], connection_name, copy_buffer);

    // Split content area into left and right panes
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_layout[1]);

    // Determine left pane title based on its type
    let left_title = match left_pane {
        FileExplorerPane::Local => "Local",
        FileExplorerPane::RemoteSsh {
            connection_name, ..
        } => connection_name.as_str(),
    };

    // Render left pane
    match left_explorer {
        LeftExplorer::Local(explorer) => {
            draw_pane(
                f,
                panes[0],
                left_title,
                explorer,
                matches!(active_pane, ActivePane::Left),
                copy_buffer,
            );
        }
        LeftExplorer::Remote(explorer) => {
            draw_pane(
                f,
                panes[0],
                left_title,
                explorer,
                matches!(active_pane, ActivePane::Left),
                copy_buffer,
            );
        }
    }

    // Render right pane (always remote)
    draw_pane(
        f,
        panes[1],
        connection_name,
        remote_explorer,
        matches!(active_pane, ActivePane::Right),
        copy_buffer,
    );

    // Render search input if in search mode
    if search_mode {
        draw_search_input(f, main_layout[2], search_query);
        draw_footer(f, main_layout[3], copy_buffer, search_mode);
    } else {
        draw_footer(f, main_layout[2], copy_buffer, search_mode);
    }
}

/// Draw the header showing connection name and copy status
fn draw_header(f: &mut Frame, area: Rect, connection_name: &str, copy_buffer: &[CopyOperation]) {
    let header_text = if let Some(first) = copy_buffer.first() {
        let direction = match first.direction {
            crate::CopyDirection::LeftToRight => "Left → Right",
            crate::CopyDirection::RightToLeft => "Right → Left",
        };
        let copy_details = if copy_buffer.len() == 1 {
            format!("{} ({direction})", first.source_name)
        } else {
            format!("{} files selected ({direction})", copy_buffer.len())
        };
        format!(
            " SFTP File Transfer - {connection_name} | [COPY MODE] {copy_details} • Tab→switch • v→paste "
        )
    } else {
        format!(" SFTP File Transfer - {connection_name} ")
    };

    let header_style = if copy_buffer.is_empty() {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    };

    let header = Paragraph::new(header_text).style(header_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(header_style),
    );

    f.render_widget(header, area);
}

/// Draw a single pane (local or remote)
fn draw_pane<F: ratatui_explorer::FileSystem>(
    f: &mut Frame,
    area: Rect,
    title: &str,
    explorer: &mut ratatui_explorer::FileExplorer<F>,
    is_active: bool,
    copy_buffer: &[CopyOperation],
) {
    // Build HashSet of selected paths from copy_buffer
    let selected_paths: HashSet<PathBuf> = copy_buffer
        .iter()
        .map(|op| PathBuf::from(&op.source_path))
        .collect();

    // Pass selection state to explorer
    explorer.set_selected_paths(selected_paths);

    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title_text = format!(" {} | {} ", title, explorer.cwd().display());

    // Create a block with border
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title_text)
        .border_style(border_style);

    // Render block
    f.render_widget(block, area);

    // Render the file explorer widget inside the block
    let inner_margin = ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    };
    let inner = area.inner(inner_margin);

    if is_active {
        explorer.set_theme(
            ratatui_explorer::Theme::new()
                .with_item_style(Style::default().fg(Color::White))
                .with_dir_style(Style::default().fg(Color::LightBlue))
                .with_highlight_dir_style(Style::default().fg(Color::LightBlue).bg(Color::Cyan))
                .with_highlight_item_style(Style::default().fg(Color::White).bg(Color::Cyan)),
        );
    } else {
        // Don't highlight the items and directories
        explorer.set_theme(
            ratatui_explorer::Theme::new()
                .with_item_style(Style::default().fg(Color::White))
                .with_dir_style(Style::default().fg(Color::LightBlue))
                .with_highlight_dir_style(Style::default().fg(Color::LightBlue))
                .with_highlight_item_style(Style::default().fg(Color::White)),
        );
    }

    // Render the file explorer with stateful widget to track scroll position
    explorer.widget_stateful().render(inner, f.buffer_mut());

    // Calculate scrollbar state
    // let total_items = explorer.files().len();

    let total_items = explorer.filtered_files().len();

    let selected = explorer.selected_idx();

    let mut scrollbar_state = ScrollbarState::new(total_items).position(selected);

    // Render scrollbar on the right edge
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .style(border_style);

    let scrollbar_area = inner.inner(Margin {
        vertical: 0,
        horizontal: 0,
    });

    f.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);

    // TODO: Show copy marker for files in copy mode
}

/// Draw the search input bar
fn draw_search_input(f: &mut Frame, area: Rect, search_query: &str) {
    let search_widget = Paragraph::new(search_query).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Search")
            .style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(search_widget, area);
}

/// Draw the footer showing available keybindings
fn draw_footer(f: &mut Frame, area: Rect, copy_buffer: &[CopyOperation], search_mode: bool) {
    use ratatui::layout::{Alignment, Constraint, Direction, Layout};

    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let hint_text = if search_mode {
        "Enter/Esc: Exit Search   Backspace: Delete   Arrow Keys: Navigate".to_string()
    } else if !copy_buffer.is_empty() {
        let count_label = if copy_buffer.len() == 1 {
            "1 file selected".to_string()
        } else {
            format!("{} files selected", copy_buffer.len())
        };
        format!("Esc: Clear | Tab: Switch Pane | v: Paste ({count_label}) | q: Quit")
    } else {
        "↑↓/jk: Move | ←→: Dir | Tab: Switch | s: Switch Source | c: Copy | d: Delete | /: Search | h: Hidden | r: Refresh | q: Quit"
            .to_string()
    };

    let left = Paragraph::new(Line::from(Span::styled(
        hint_text,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(Alignment::Left);

    let right = Paragraph::new(Line::from(Span::styled(
        format!("TermiRs v{}", env!("CARGO_PKG_VERSION")),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(Alignment::Right);

    f.render_widget(left, footer_layout[0]);
    f.render_widget(right, footer_layout[1]);
}

/// Draw the file delete confirmation popup
pub fn draw_file_delete_confirmation_popup(f: &mut Frame, area: Rect, file_name: &str) {
    use ratatui::widgets::Clear;

    let popup_w = (area.width as f32 * 0.40) as u16;
    let popup_h = 8u16.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    f.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "Delete File",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));

    let inner = popup.inner(Margin::new(1, 1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // warning message
            Constraint::Length(1), // file name
            Constraint::Length(1), // empty line
            Constraint::Length(1), // confirmation question
            Constraint::Min(1),    // spacer to push buttons to bottom
            Constraint::Length(1), // buttons hint (bottom-aligned)
        ])
        .split(inner);

    // Warning message
    let warning = Paragraph::new(Line::from(Span::styled(
        "⚠️  Are you sure you want to delete this file?",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    f.render_widget(warning, layout[0]);

    // File name
    let file_info = Paragraph::new(Line::from(vec![
        Span::styled("File: ", Style::default().fg(Color::Gray)),
        Span::styled(
            file_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    f.render_widget(file_info, layout[1]);

    // Empty line for spacing
    f.render_widget(Paragraph::new(""), layout[2]);

    // Confirmation question
    let question = Paragraph::new(Line::from(Span::styled(
        "This action cannot be undone.",
        Style::default().fg(Color::Red),
    )));
    f.render_widget(question, layout[3]);

    // Button hints
    let buttons = Paragraph::new(Line::from(vec![
        Span::styled(
            "Y",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Delete   ", Style::default().fg(Color::White)),
        Span::styled(
            "N",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Cancel   ", Style::default().fg(Color::White)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Cancel", Style::default().fg(Color::White)),
    ]))
    .alignment(Alignment::Center);
    f.render_widget(buttons, layout[5]);

    f.render_widget(Paragraph::new("").block(block), popup);
}

/// Draw a generic connection selector popup
#[allow(clippy::too_many_arguments)]
pub fn draw_connection_selector_popup(
    f: &mut Frame,
    area: Rect,
    connections: &[Connection],
    selected: usize,
    exclude_connection_id: Option<&str>,
    include_local_option: bool,
    title: &str,
    search_mode: bool,
    search_query: &str,
) {
    use ratatui::widgets::{Clear, List, ListItem, ListState};

    // Calculate popup size (50% width, fit height to content)
    let popup_w = (area.width as f32 * 0.5) as u16;
    let max_items = connections.len() + 1; // +1 for "Local" option
    let popup_h = (max_items as u16 + 4).min(area.height.saturating_sub(4)); // +4 for border and title

    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    // Clear background
    f.render_widget(Clear, popup);

    // Create block with title
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

    // Build list items: optional local entry + filtered connections
    let filtered_indices =
        filter_connection_indices(connections, exclude_connection_id, search_query);
    let local_offset = if include_local_option { 1 } else { 0 };
    let total_items = filtered_indices.len() + local_offset;
    let clamped_selected = selected.min(total_items.saturating_sub(1));

    let mut items = Vec::with_capacity(total_items.max(1));
    if include_local_option {
        items.push(ListItem::new(Line::from(Span::styled(
            "📁 Local Filesystem",
            Style::default().fg(Color::White),
        ))));
    }

    for idx in filtered_indices {
        if let Some(conn) = connections.get(idx) {
            let display = format!("🌐 {} ({}@{})", conn.display_name, conn.username, conn.host);
            items.push(ListItem::new(Line::from(Span::styled(
                display,
                Style::default().fg(Color::White),
            ))));
        }
    }

    // Draw popup block separately so we can manage footer space ourselves
    f.render_widget(block.clone(), popup);
    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if inner.height > 1 {
            vec![Constraint::Min(1), Constraint::Length(1)]
        } else {
            vec![Constraint::Min(1)]
        })
        .split(inner);
    let list_area = sections[0];
    let footer_area = sections.get(1).copied().unwrap_or(Rect {
        x: inner.x,
        y: inner.y + inner.height,
        width: inner.width,
        height: 0,
    });

    // Create list widget
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    // Create list state
    let mut list_state = ListState::default();
    if total_items == 0 {
        list_state.select(None);
    } else {
        list_state.select(Some(clamped_selected));
    }

    // Render list content
    f.render_stateful_widget(list, list_area, &mut list_state);

    // Render scrollbar aligned with list content
    let mut scrollbar_state = ScrollbarState::new(total_items.max(1))
        .position(clamped_selected.min(total_items.saturating_sub(1)));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .style(Style::default().fg(Color::Cyan));
    f.render_stateful_widget(scrollbar, list_area, &mut scrollbar_state);

    // Render footer content (instructions or search)
    if footer_area.height > 0 {
        if search_mode {
            let mut spans = vec![Span::styled(
                "Search: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )];

            if search_query.is_empty() {
                spans.push(Span::styled(
                    "type to filter connections",
                    Style::default().fg(Color::DarkGray).dim(),
                ));
            } else {
                spans.push(Span::raw(search_query));
            }

            let search_line =
                Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Reset));
            f.render_widget(search_line, footer_area);
        } else {
            let instructions = Paragraph::new(Line::from(vec![
                Span::styled(
                    "↑↓",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(": Navigate  "),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(": Select  "),
                Span::styled(
                    "/",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(": Search  "),
                Span::styled(
                    "Esc",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(": Cancel"),
            ]))
            .style(Style::default().bg(Color::Reset));
            f.render_widget(instructions, footer_area);
        }
    }
}

/// Build the list of connection indices shown in a connection selector after filtering.
pub fn filter_connection_indices(
    connections: &[Connection],
    exclude_connection_id: Option<&str>,
    search_query: &str,
) -> Vec<usize> {
    let query = search_query.trim().to_lowercase();
    connections
        .iter()
        .enumerate()
        .filter(|(_, conn)| {
            exclude_connection_id
                .map(|id| id != conn.id.as_str())
                .unwrap_or(true)
        })
        .filter(|(_, conn)| {
            if query.is_empty() {
                return true;
            }

            let display = conn.display_name.to_lowercase();
            let host = conn.host.to_lowercase();
            let username = conn.username.to_lowercase();
            display.contains(&query) || host.contains(&query) || username.contains(&query)
        })
        .map(|(idx, _)| idx)
        .collect()
}

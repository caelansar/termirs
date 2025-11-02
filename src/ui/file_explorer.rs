//! File explorer UI components for dual-pane SFTP file transfer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::{CopyOperation, FileExplorerPane, filesystem::SftpFileSystem};

/// Draw the dual-pane file explorer interface.
///
/// # Arguments
/// * `f` - The frame to render to
/// * `area` - The area to render in
/// * `connection_name` - Name of the SSH connection
/// * `local_explorer` - The local file explorer
/// * `remote_explorer` - The remote SFTP file explorer
/// * `active_pane` - Which pane is currently active
/// * `copy_buffer` - Collection of selected files pending transfer
/// * `search_mode` - Whether search mode is active
/// * `search_query` - Current search query string
pub fn draw_file_explorer(
    f: &mut Frame,
    area: Rect,
    connection_name: &str,
    local_explorer: &mut ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>,
    remote_explorer: &mut ratatui_explorer::FileExplorer<SftpFileSystem>,
    active_pane: &FileExplorerPane,
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

    // Split content area into left (local) and right (remote) panes
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_layout[1]);

    // Render local pane (left)
    draw_pane(
        f,
        panes[0],
        "Local",
        local_explorer,
        matches!(active_pane, FileExplorerPane::Local),
        copy_buffer,
    );

    // Render remote pane (right)
    draw_pane(
        f,
        panes[1],
        "Remote",
        remote_explorer,
        matches!(active_pane, FileExplorerPane::Remote),
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
            crate::CopyDirection::LocalToRemote => "Local → Remote",
            crate::CopyDirection::RemoteToLocal => "Remote → Local",
        };
        let copy_details = if copy_buffer.len() == 1 {
            format!("{} ({direction})", first.source_name)
        } else {
            format!("{} files selected ({direction})", copy_buffer.len())
        };
        format!(
            " SFTP File Transfer - {} | [COPY MODE] {copy_details} • Tab→switch • v→paste ",
            connection_name
        )
    } else {
        format!(" SFTP File Transfer - {} ", connection_name)
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
        "↑↓/jk: Move | ←→: Dir | Tab: Switch | c: Copy | /: Search | h: Hidden | r: Refresh | q: Quit"
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

//! File explorer UI components for dual-pane SFTP file transfer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

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
/// * `copy_operation` - Optional copy operation in progress
pub fn draw_file_explorer(
    f: &mut Frame,
    area: Rect,
    connection_name: &str,
    local_explorer: &mut ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>,
    remote_explorer: &mut ratatui_explorer::FileExplorer<SftpFileSystem>,
    active_pane: &FileExplorerPane,
    copy_operation: &Option<CopyOperation>,
) {
    // Main layout: header, content, footer
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(1),    // Content (dual panes)
            Constraint::Length(3), // Footer
        ])
        .split(area);

    // Render header
    draw_header(f, main_layout[0], connection_name, copy_operation);

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
        copy_operation,
    );

    // Render remote pane (right)
    draw_pane(
        f,
        panes[1],
        "Remote",
        remote_explorer,
        matches!(active_pane, FileExplorerPane::Remote),
        copy_operation,
    );

    // Render footer
    draw_footer(f, main_layout[2], copy_operation);
}

/// Draw the header showing connection name and copy status
fn draw_header(
    f: &mut Frame,
    area: Rect,
    connection_name: &str,
    copy_operation: &Option<CopyOperation>,
) {
    let header_text = if let Some(copy_op) = copy_operation {
        format!(
            " SFTP File Transfer - {} | [COPY MODE] {} → Press Tab to switch, 'v' to paste ",
            connection_name, copy_op.source_name
        )
    } else {
        format!(" SFTP File Transfer - {} ", connection_name)
    };

    let header_style = if copy_operation.is_some() {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Cyan)
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
    _copy_operation: &Option<CopyOperation>,
) {
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
            ratatui_explorer::Theme::default()
                .with_highlight_dir_style(Style::default().fg(Color::LightBlue).bg(Color::Cyan))
                .with_highlight_item_style(Style::default().fg(Color::White).bg(Color::Cyan)),
        );
    } else {
        // Don't highlight the items and directories
        explorer.set_theme(
            ratatui_explorer::Theme::default()
                .with_highlight_dir_style(Style::default().fg(Color::LightBlue))
                .with_highlight_item_style(Style::default().fg(Color::White)),
        );
    }

    let widget = explorer.widget();
    f.render_widget(&widget, inner);

    // TODO: Show copy marker for files in copy mode
}

/// Draw the footer showing available keybindings
fn draw_footer(f: &mut Frame, area: Rect, copy_operation: &Option<CopyOperation>) {
    let footer_text = if copy_operation.is_some() {
        "Esc: Cancel | Tab: Switch Pane | v: Paste | q: Quit"
    } else {
        "↑↓/jk: Move | ←→: Dir | Tab: Switch | c: Copy | h: Hidden | r: Refresh | q: Quit"
    };

    let footer = Paragraph::new(Line::from(vec![Span::styled(
        footer_text,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(footer, area);
}

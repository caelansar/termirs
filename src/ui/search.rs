use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_textarea::TextArea;

/// Draws a standard search overlay composed of a content area, search input, and footer.
pub fn draw_search_overlay<'a, F>(
    frame: &mut ratatui::Frame<'a>,
    area: Rect,
    search_input: &mut TextArea<'_>,
    footer_hint: &str,
    footer_constraints: [Constraint; 2],
    mut render_content: F,
) where
    F: FnMut(Rect, &mut ratatui::Frame<'a>),
{
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Content/table area
            Constraint::Length(3), // Search input
            Constraint::Length(1), // Footer
        ])
        .split(area);

    render_content(layout[0], frame);

    search_input.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title("Search")
            .style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(&*search_input, layout[1]);

    let footer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(footer_constraints)
        .split(layout[2]);

    let left = Paragraph::new(Line::from(Span::styled(
        footer_hint,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(ratatui::layout::Alignment::Left);

    let right = Paragraph::new(Line::from(Span::styled(
        format!("TermiRs v{}", env!("CARGO_PKG_VERSION")),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(ratatui::layout::Alignment::Right);

    frame.render_widget(left, footer[0]);
    frame.render_widget(right, footer[1]);
}

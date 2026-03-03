pub mod connection;
pub mod file_explorer;
pub mod popup;
pub mod port_forwarding;
pub mod scp;
pub mod table;
pub mod table_renderer;
pub mod terminal;

pub use connection::{ConnectionForm, draw_connection_list};
pub use file_explorer::{draw_connection_selector_popup, draw_file_explorer};
pub use popup::{
    DeleteConfirmationConfig, draw_connecting_popup, draw_connection_form_popup,
    draw_delete_confirmation_popup, draw_error_popup, draw_info_popup,
};
pub use port_forwarding::{
    PortForwardingForm, draw_port_forwarding_form_popup, draw_port_forwarding_list,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
pub use scp::{ScpMode, draw_scp_progress_popup};
pub use terminal::{TerminalSelection, TerminalState, draw_terminal};

/// Helper function to create a rect with only top margin
///
/// # Arguments
/// * `rect` - The original rectangle
/// * `top_margin` - The top margin to subtract
///
/// # Returns
/// A new Rect with the top margin applied, but bottom remains unchanged
pub fn rect_with_top_margin(rect: ratatui::layout::Rect, top_margin: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect {
        x: rect.x,
        y: rect.y + top_margin,
        width: rect.width,
        height: rect.height.saturating_sub(top_margin),
    }
}

/// Render normal mode footer with hints and version.
///
/// Layout: 80% hints (left-aligned) + 20% version (right-aligned)
pub fn render_normal_footer(frame: &mut Frame<'_>, footer_area: Rect, hints: &str) {
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
        .split(footer_area);

    let left = Paragraph::new(Line::from(Span::styled(
        hints,
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

    frame.render_widget(left, footer_layout[0]);
    frame.render_widget(right, footer_layout[1]);
}

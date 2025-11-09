pub mod connection;
pub mod file_explorer;
pub mod popup;
pub mod port_forwarding;
pub mod scp;
pub mod search;
pub mod terminal;

pub use connection::{ConnectionForm, draw_connection_list};
pub use file_explorer::draw_file_explorer;
pub use popup::{
    draw_connecting_popup, draw_connection_form_popup, draw_delete_confirmation_popup,
    draw_error_popup, draw_info_popup,
};
pub use port_forwarding::{
    PortForwardingForm, draw_port_forward_delete_confirmation_popup,
    draw_port_forwarding_form_popup, draw_port_forwarding_list,
};
pub use scp::{ScpMode, draw_scp_progress_popup};
pub use search::draw_search_overlay;
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

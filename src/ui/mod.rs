pub mod components;
pub mod connection;
pub mod file_explorer;
pub mod popup;
pub mod scp;
pub mod terminal;

// Re-export commonly used items for convenience
pub use components::{DropdownState, draw_dropdown_with_rect};
pub use connection::{ConnectionForm, draw_connection_list};
pub use file_explorer::draw_file_explorer;
pub use popup::{
    draw_connection_form_popup, draw_delete_confirmation_popup, draw_error_popup, draw_info_popup,
};
pub use scp::{ScpFocusField, ScpForm, ScpMode, draw_scp_popup, draw_scp_progress_popup};
pub use terminal::{TerminalState, draw_terminal};

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

use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Backend;

use crate::error::AppError;
use crate::terminal::{MouseClickClass, SelectionEndpoint, make_selection_endpoint};
use crate::ui::TerminalState;
use crate::{App, AppMode};

pub mod connected;
pub mod connection_list;
pub mod file_explorer;
pub mod form;
pub mod port_forwarding;
pub mod scp;
pub mod table_handler;

// Re-export commonly used items for convenience
pub use connected::handle_connected_key;
pub use connection_list::handle_connection_list_key;
pub use file_explorer::handle_file_explorer_key;
pub use form::{handle_form_edit_key, handle_form_new_key};
pub use port_forwarding::{
    handle_port_forward_delete_confirmation_key, handle_port_forwarding_form_connection_select_key,
    handle_port_forwarding_form_key, handle_port_forwarding_list_key,
};
pub use scp::{handle_delete_confirmation_key, handle_scp_progress_key};

const TERMINAL_MOUSE_SCROLL_STEP: i32 = 5;

/// Result of handling a key or paste event
pub enum KeyFlow {
    Continue,
    Quit,
}

/// Top-level key event handler, including error popup dismissal and dispatch by AppMode
pub async fn handle_key_event<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    // Only handle actual key presses (ignore repeats/releases)
    if key.kind != KeyEventKind::Press {
        return KeyFlow::Continue;
    }

    // If error popup is visible, handle dismissal only
    if app.error.is_some() {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                if let Some(AppError::ChannelClosedError(_)) = app.error.take() {
                    app.go_to_connection_list_with_selected(app.current_selected());
                }
            }
            _ => {}
        }
        return KeyFlow::Continue;
    }

    // If info popup is visible, handle dismissal only
    if app.info.is_some() {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                app.info = None;
            }
            _ => {}
        }
        return KeyFlow::Continue;
    }

    match &mut app.mode {
        AppMode::ConnectionList { .. } => handle_connection_list_key(app, key).await,
        AppMode::FormNew { .. } => handle_form_new_key(app, key).await,
        AppMode::FormEdit { .. } => handle_form_edit_key(app, key).await,
        AppMode::Connecting { .. } => handle_connecting_key(app, key).await,
        AppMode::Connected { .. } => handle_connected_key(app, key).await,
        AppMode::ScpProgress { .. } => handle_scp_progress_key(app, key).await,
        AppMode::DeleteConfirmation { .. } => handle_delete_confirmation_key(app, key).await,
        AppMode::FileExplorer { .. } => handle_file_explorer_key(app, key).await,
        AppMode::PortForwardingList(_) => handle_port_forwarding_list_key(app, key).await,
        AppMode::PortForwardingFormNew(state) | AppMode::PortForwardingFormEdit(state)
            if state.connection_selector.showing =>
        {
            handle_port_forwarding_form_connection_select_key(app, key).await
        }
        AppMode::PortForwardingFormNew(_) | AppMode::PortForwardingFormEdit(_) => {
            handle_port_forwarding_form_key(app, key).await
        }
        AppMode::PortForwardDeleteConfirmation { .. } => {
            handle_port_forward_delete_confirmation_key(app, key).await
        }
    }
}

/// Paste event handler; dispatches by AppMode
pub async fn handle_paste_event<B: Backend + Write>(app: &mut App<B>, data: &str) {
    match &mut app.mode {
        AppMode::FormNew { form, .. } => {
            let textarea = form.focused_textarea_mut();
            textarea.insert_str(data);
        }
        AppMode::FormEdit { form, .. } => {
            let textarea = form.focused_textarea_mut();
            textarea.insert_str(data);
        }
        AppMode::Connected {
            name: _,
            client,
            state,
            ..
        } => {
            let mut guard = state.lock().await;
            if guard.search.is_inputting() {
                guard.search.push_str(data);
                return;
            }
            if guard.scrollback() > 0 {
                guard.scroll_to_bottom();
            }
            if let Err(e) = client.write_all(data.as_bytes()).await {
                app.error = Some(e);
            }
        }
        AppMode::PortForwardingFormNew(state) => {
            if let Some(textarea) = state.form.focused_textarea_mut() {
                textarea.insert_str(data);
            }
        }
        AppMode::PortForwardingFormEdit(state) => {
            if let Some(textarea) = state.form.focused_textarea_mut() {
                textarea.insert_str(data);
            }
        }
        AppMode::ConnectionList { .. }
        | AppMode::Connecting { .. }
        | AppMode::ScpProgress { .. }
        | AppMode::DeleteConfirmation { .. }
        | AppMode::FileExplorer { .. }
        | AppMode::PortForwardingList { .. }
        | AppMode::PortForwardDeleteConfirmation { .. } => {}
    }
}

/// Handle key events while connecting to SSH
async fn handle_connecting_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    if key.code == KeyCode::Esc
        && let AppMode::Connecting {
            cancel_token,
            return_from,
            return_to,
            ..
        } = &mut app.mode
    {
        // Cancel the connection task
        cancel_token.cancel();

        // Clone the data we need before changing mode
        let return_from = return_from.clone();
        let return_to = *return_to;

        // Return to the appropriate mode
        match return_from {
            crate::ConnectingSource::FormNew { auto_auth, form } => {
                app.mode = AppMode::FormNew {
                    auto_auth,
                    form,
                    current_selected: return_to,
                };
            }
            crate::ConnectingSource::FormEdit { form, original } => {
                app.mode = AppMode::FormEdit {
                    form,
                    original,
                    current_selected: return_to,
                };
            }
            crate::ConnectingSource::ConnectionList { .. } => {
                app.go_to_connection_list_with_selected(return_to);
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_mouse_event<B: Backend + Write>(app: &mut App<B>, event: MouseEvent) {
    let state = match &app.mode {
        AppMode::Connected { state, .. } => state.clone(),
        _ => return,
    };

    match event.kind {
        MouseEventKind::Down(MouseButton::Middle) => {
            let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
            if let Ok(text) = text {
                handle_paste_event(app, &text).await;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((point, _direction)) = app.clamp_point_to_viewport(event.column, event.row)
            {
                match app.register_left_click(point) {
                    MouseClickClass::Triple => {
                        app.stop_selection_auto_scroll();
                        let text = {
                            let guard = state.lock().await;
                            if let Some((anchor, tail)) =
                                compute_triple_click_selection(&guard, point.row)
                            {
                                app.start_selection(anchor);
                                app.update_selection(tail);
                                app.force_selection_nonempty();
                                app.finish_selection();
                                app.selection_text(&guard)
                            } else {
                                None
                            }
                        };
                        if let Some(text) = text {
                            app.copy_text_to_clipboard(text);
                        } else {
                            app.clear_selection();
                        }
                        return;
                    }
                    MouseClickClass::Double => {
                        app.stop_selection_auto_scroll();
                        let text = {
                            let guard = state.lock().await;
                            if let Some((anchor, tail)) =
                                compute_double_click_selection(&guard, point.row, point.col)
                            {
                                app.start_selection(anchor);
                                app.update_selection(tail);
                                if anchor == tail {
                                    app.force_selection_nonempty();
                                }
                                app.finish_selection();
                                app.selection_text(&guard)
                            } else {
                                None
                            }
                        };
                        if let Some(text) = text {
                            app.copy_text_to_clipboard(text);
                        } else {
                            app.clear_selection();
                        }
                        return;
                    }
                    MouseClickClass::Single => {}
                }
                let endpoint = {
                    let guard = state.lock().await;
                    make_selection_endpoint(&guard, point.row, point.col)
                };
                if let Some(endpoint) = endpoint {
                    app.start_selection(endpoint);
                    app.stop_selection_auto_scroll();
                } else {
                    app.clear_selection();
                }
            } else {
                app.clear_selection();
                app.clear_click_tracking();
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            app.clear_click_tracking();
            if !app.is_selecting() {
                return;
            }
            if let Some((point, direction)) = app.clamp_point_to_viewport(event.column, event.row) {
                let endpoint = {
                    let guard = state.lock().await;
                    make_selection_endpoint(&guard, point.row, point.col)
                };
                if let Some(endpoint) = endpoint {
                    app.update_selection(endpoint);
                    if let Some(dir) = direction {
                        app.begin_selection_auto_scroll(dir, point.row, point.col);
                    } else {
                        app.stop_selection_auto_scroll();
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if !app.is_selecting() {
                app.stop_selection_auto_scroll();
                return;
            }
            app.stop_selection_auto_scroll();
            if let Some((point, _)) = app.clamp_point_to_viewport(event.column, event.row) {
                let endpoint = {
                    let guard = state.lock().await;
                    make_selection_endpoint(&guard, point.row, point.col)
                };
                if let Some(endpoint) = endpoint {
                    app.update_selection(endpoint);
                }
            }
            app.finish_selection();
            let text = {
                let guard = state.lock().await;
                app.selection_text(&guard)
            };
            if let Some(text) = text {
                app.copy_text_to_clipboard(text);
            } else {
                app.clear_selection();
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let delta = match event.kind {
                MouseEventKind::ScrollUp => TERMINAL_MOUSE_SCROLL_STEP,
                MouseEventKind::ScrollDown => -TERMINAL_MOUSE_SCROLL_STEP,
                _ => 0,
            };

            if delta == 0 {
                return;
            }

            if app.is_selecting() {
                let mut guard = state.lock().await;
                guard.scroll_by(delta);
                app.mark_redraw();
                return;
            }

            let (in_alt, app_cursor) = {
                let guard = state.lock().await;
                (guard.is_alternate_screen(), guard.application_cursor_keys())
            };

            let interactive = in_alt || app_cursor;

            if interactive {
                let seq = if delta > 0 {
                    if app_cursor { b"\x1bOA" } else { b"\x1b[A" }
                } else if app_cursor {
                    b"\x1bOB"
                } else {
                    b"\x1b[B"
                };

                let repeat = delta.unsigned_abs() as usize;
                for _ in 0..repeat {
                    if let AppMode::Connected { client, .. } = &app.mode
                        && let Err(e) = client.write_all(seq).await
                    {
                        app.error = Some(e);
                        break;
                    }
                }
            } else {
                let mut guard = state.lock().await;
                guard.scroll_by(delta);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharKind {
    Word,
    Whitespace,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct CharInfo {
    kind: CharKind,
    start_col: u16,
    end_col: u16,
}

fn compute_double_click_selection(
    state: &TerminalState,
    view_row: u16,
    view_col: u16,
) -> Option<(SelectionEndpoint, SelectionEndpoint)> {
    let (height, width) = state.screen_size();
    if height == 0 || width == 0 || view_row >= height {
        return None;
    }

    let info = char_info_at_wez(state, view_row, view_col, width);
    let mut start = info.start_col;
    let mut end = info.end_col;
    let kind = info.kind;

    while start > 0 {
        let prev = char_info_at_wez(state, view_row, start - 1, width);
        if prev.kind != kind {
            break;
        }
        start = prev.start_col;
    }

    while end < width {
        let next = char_info_at_wez(state, view_row, end, width);
        if next.kind != kind {
            break;
        }
        end = next.end_col;
    }

    let anchor = make_selection_endpoint(state, view_row, start)?;
    let tail_col = end.saturating_sub(1);
    let tail = make_selection_endpoint(state, view_row, tail_col)?;
    Some((anchor, tail))
}

fn compute_triple_click_selection(
    state: &TerminalState,
    view_row: u16,
) -> Option<(SelectionEndpoint, SelectionEndpoint)> {
    let (height, width) = state.screen_size();
    if height == 0 || width == 0 || view_row >= height {
        return None;
    }

    let anchor = make_selection_endpoint(state, view_row, 0)?;
    // Triple-click behaves like iTerm2: select the full visual row, including trailing whitespace.
    let tail_col = width.saturating_sub(1);
    let tail = make_selection_endpoint(state, view_row, tail_col)?;
    Some((anchor, tail))
}

fn char_info_at_wez(state: &TerminalState, view_row: u16, column: u16, width: u16) -> CharInfo {
    if width == 0 {
        return CharInfo {
            kind: CharKind::Whitespace,
            start_col: 0,
            end_col: 0,
        };
    }

    let max_col = width.saturating_sub(1);
    let clamped = column.min(max_col);

    // Get the line for this view_row
    let screen = state.terminal.screen();
    let total_lines = screen.scrollback_rows();
    let phys_rows = screen.physical_rows;
    let start_row = total_lines
        .saturating_sub(phys_rows)
        .saturating_sub(state.scrollback());
    let abs_row = start_row + view_row as usize;

    let lines = screen.lines_in_phys_range(abs_row..abs_row + 1);
    if lines.is_empty() {
        return CharInfo {
            kind: CharKind::Whitespace,
            start_col: clamped,
            end_col: clamped.saturating_add(1).min(width),
        };
    }

    let line = &lines[0];
    let mut col = 0u16;

    for cell in line.visible_cells() {
        let cell_width = cell.width() as u16;
        let cell_start = col;
        let cell_end = col + cell_width;

        // Check if the target column falls within this cell
        if clamped >= cell_start && clamped < cell_end {
            let cell_text = cell.str();
            if cell_text.is_empty() {
                return CharInfo {
                    kind: CharKind::Whitespace,
                    start_col: cell_start,
                    end_col: cell_end.min(width),
                };
            }
            let ch = cell_text.chars().next().unwrap_or(' ');
            return CharInfo {
                kind: classify_char(ch),
                start_col: cell_start,
                end_col: cell_end.min(width),
            };
        }

        col += cell_width;
        if col > clamped {
            break;
        }
    }

    // Column not found in cells, return whitespace
    CharInfo {
        kind: CharKind::Whitespace,
        start_col: clamped,
        end_col: clamped.saturating_add(1).min(width),
    }
}

fn classify_char(ch: char) -> CharKind {
    // Mirror iTerm2's default double-click grouping: words are made of letters,
    // digits, and underscore/hyphen/period; whitespace groups stay together; everything else
    // (punctuation/symbols) forms its own cluster.
    if ch.is_whitespace() {
        CharKind::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
        CharKind::Word
    } else {
        CharKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_characters_follow_iterm_defaults() {
        assert_eq!(classify_char('a'), CharKind::Word);
        assert_eq!(classify_char('_'), CharKind::Word);
        assert_eq!(classify_char(' '), CharKind::Whitespace);
        assert_eq!(classify_char('-'), CharKind::Word);
    }

    #[test]
    fn double_click_selects_word_bounds() {
        let mut state = TerminalState::new(1, 20);
        state.process_bytes(b"hello world");
        let (start, end) = compute_double_click_selection(&state, 0, 1).unwrap();
        assert_eq!(start.col, 0);
        assert_eq!(end.col, 4);
    }

    #[test]
    fn double_click_selects_whitespace_group() {
        let mut state = TerminalState::new(1, 20);
        state.process_bytes(b"foo  bar");
        let (start, end) = compute_double_click_selection(&state, 0, 4).unwrap();
        assert_eq!(start.col, 3);
        assert_eq!(end.col, 4);
    }

    #[test]
    fn triple_click_selects_full_line() {
        let mut state = TerminalState::new(1, 12);
        state.process_bytes(b"hello world\r\nsecond line");
        let (start, end) = compute_triple_click_selection(&state, 0).unwrap();
        assert_eq!(start.col, 0);
        let (_, width) = state.screen_size();
        assert_eq!(end.col, width.saturating_sub(1));
    }

    #[test]
    fn triple_click_handles_blank_line() {
        let state = TerminalState::new(1, 10);
        let (start, end) = compute_triple_click_selection(&state, 0).unwrap();
        assert_eq!(start.col, 0);
        let (_, width) = state.screen_size();
        assert_eq!(end.col, width.saturating_sub(1));
    }
}

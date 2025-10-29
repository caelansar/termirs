use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Backend;

use crate::ui::TerminalState;
use crate::{App, AppMode, MouseClickClass, SelectionEndpoint, make_selection_endpoint};

pub mod autocomplete;
pub mod connected;
pub mod connection_list;
pub mod file_explorer;
pub mod form;
pub mod port_forwarding;
pub mod scp;

// Re-export commonly used items for convenience
pub use connected::handle_connected_key;
pub use connection_list::handle_connection_list_key;
pub use file_explorer::handle_file_explorer_key;
pub use form::{handle_form_edit_key, handle_form_new_key};
pub use port_forwarding::{
    handle_port_forward_delete_confirmation_key, handle_port_forwarding_form_connection_select_key,
    handle_port_forwarding_form_key, handle_port_forwarding_list_key,
};
pub use scp::{
    handle_delete_confirmation_key, handle_scp_form_dropdown_key, handle_scp_form_key,
    handle_scp_progress_key,
};

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
                app.error = None;
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
        AppMode::Connected { .. } => handle_connected_key(app, key).await,
        AppMode::ScpForm {
            dropdown: Some(_), ..
        } => handle_scp_form_dropdown_key(app, key).await,
        AppMode::ScpForm { .. } => handle_scp_form_key(app, key).await,
        AppMode::ScpProgress { .. } => handle_scp_progress_key(app, key).await,
        AppMode::DeleteConfirmation { .. } => handle_delete_confirmation_key(app, key).await,
        AppMode::FileExplorer { .. } => handle_file_explorer_key(app, key).await,
        AppMode::PortForwardingList { .. } => handle_port_forwarding_list_key(app, key).await,
        AppMode::PortForwardingFormNew {
            select_connection_mode: true,
            ..
        }
        | AppMode::PortForwardingFormEdit {
            select_connection_mode: true,
            ..
        } => handle_port_forwarding_form_connection_select_key(app, key).await,
        AppMode::PortForwardingFormNew { .. } | AppMode::PortForwardingFormEdit { .. } => {
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
            if guard.parser.screen().scrollback() > 0 {
                guard.scroll_to_bottom();
            }
            if let Err(e) = client.write_all(data.as_bytes()).await {
                app.error = Some(e);
            }
        }
        AppMode::ScpForm { form, .. } => {
            let textarea = form.focused_textarea_mut();
            textarea.insert_str(data);
        }
        AppMode::PortForwardingFormNew { form, .. } => {
            if let Some(textarea) = form.focused_textarea_mut() {
                textarea.insert_str(data);
            }
        }
        AppMode::PortForwardingFormEdit { form, .. } => {
            if let Some(textarea) = form.focused_textarea_mut() {
                textarea.insert_str(data);
            }
        }
        AppMode::ConnectionList { .. }
        | AppMode::ScpProgress { .. }
        | AppMode::DeleteConfirmation { .. }
        | AppMode::FileExplorer { .. }
        | AppMode::PortForwardingList { .. }
        | AppMode::PortForwardDeleteConfirmation { .. } => {}
    }
}

pub async fn handle_mouse_event<B: Backend + Write>(app: &mut App<B>, event: MouseEvent) {
    let (client, state) = match &app.mode {
        AppMode::Connected { client, state, .. } => (client.clone(), state.clone()),
        _ => return,
    };

    match event.kind {
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
                let screen = guard.parser.screen();
                (screen.alternate_screen(), screen.application_cursor())
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

                let repeat = delta.abs() as usize;
                for _ in 0..repeat {
                    if let Err(e) = client.write_all(seq).await {
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
    let screen = state.parser.screen();
    let (height, width) = screen.size();
    if height == 0 || width == 0 || view_row >= height {
        return None;
    }

    let info = char_info_at(screen, view_row, view_col, width);
    let mut start = info.start_col;
    let mut end = info.end_col;
    let kind = info.kind;

    while start > 0 {
        let prev = char_info_at(screen, view_row, start - 1, width);
        if prev.kind != kind {
            break;
        }
        start = prev.start_col;
    }

    while end < width {
        let next = char_info_at(screen, view_row, end, width);
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
    let screen = state.parser.screen();
    let (height, width) = screen.size();
    if height == 0 || width == 0 || view_row >= height {
        return None;
    }

    let anchor = make_selection_endpoint(state, view_row, 0)?;
    // Triple-click behaves like iTerm2: select the full visual row, including trailing whitespace.
    let tail_col = width.saturating_sub(1);
    let tail = make_selection_endpoint(state, view_row, tail_col)?;
    Some((anchor, tail))
}

fn char_info_at(screen: &vt100::Screen, row: u16, column: u16, width: u16) -> CharInfo {
    if width == 0 {
        return CharInfo {
            kind: CharKind::Whitespace,
            start_col: 0,
            end_col: 0,
        };
    }
    let max_col = width.saturating_sub(1);
    let clamped = column.min(max_col);
    let base = resolve_base_col(screen, row, clamped);
    let end_col = base.saturating_add(1).min(width);

    if let Some(cell) = screen.cell(row, base) {
        if cell.has_contents() {
            let mut chars = cell.contents().chars();
            let ch = chars.next().unwrap_or(' ');
            let span = if cell.is_wide() { 2 } else { 1 };
            return CharInfo {
                kind: classify_char(ch),
                start_col: base,
                end_col: base.saturating_add(span).min(width),
            };
        }
        if cell.is_wide_continuation() && base > 0 {
            let prev_base = resolve_base_col(screen, row, base - 1);
            let prev_info = char_info_at(screen, row, prev_base, width);
            return CharInfo {
                kind: prev_info.kind,
                start_col: prev_info.start_col,
                end_col: prev_info.end_col,
            };
        }
    }

    CharInfo {
        kind: CharKind::Whitespace,
        start_col: base,
        end_col,
    }
}

fn resolve_base_col(screen: &vt100::Screen, row: u16, mut col: u16) -> u16 {
    while col > 0 {
        if let Some(cell) = screen.cell(row, col) {
            if cell.is_wide_continuation() {
                col -= 1;
                continue;
            }
        }
        break;
    }
    col
}

fn classify_char(ch: char) -> CharKind {
    // Mirror iTerm2's default double-click grouping: words are made of letters,
    // digits, and underscore/hyphen/period; whitespace groups stay together; everything else
    // (punctuation/symbols) forms its own cluster.
    if ch.is_whitespace() {
        CharKind::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' {
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
        assert_eq!(classify_char('-'), CharKind::Other);
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
        let (_, width) = state.parser.screen().size();
        assert_eq!(end.col, width.saturating_sub(1));
    }

    #[test]
    fn triple_click_handles_blank_line() {
        let state = TerminalState::new(1, 10);
        let (start, end) = compute_triple_click_selection(&state, 0).unwrap();
        assert_eq!(start.col, 0);
        let (_, width) = state.parser.screen().size();
        assert_eq!(end.col, width.saturating_sub(1));
    }
}

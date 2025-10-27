use std::io::Write;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::prelude::Backend;

use crate::{App, AppMode};

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
const SELECTION_SUSPEND_MS: u64 = 1500;

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
    match &mut app.mode {
        AppMode::Connected { client, state, .. } => match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = match event.kind {
                    MouseEventKind::ScrollUp => TERMINAL_MOUSE_SCROLL_STEP,
                    MouseEventKind::ScrollDown => -TERMINAL_MOUSE_SCROLL_STEP,
                    _ => 0,
                };

                if delta == 0 {
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
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Err(e) =
                    app.suspend_mouse_capture(Duration::from_millis(SELECTION_SUSPEND_MS))
                {
                    app.error = Some(e);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

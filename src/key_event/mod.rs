use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::Backend;

use crate::{App, AppMode};

pub mod autocomplete;
pub mod connected;
pub mod connection_list;
pub mod form;
pub mod scp;

// Re-export commonly used items for convenience
pub use connected::handle_connected_key;
pub use connection_list::handle_connection_list_key;
pub use form::{handle_form_edit_key, handle_form_new_key};
pub use scp::{
    handle_delete_confirmation_key, handle_scp_form_dropdown_key, handle_scp_form_key,
    handle_scp_progress_key,
};

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
        AppMode::ConnectionList { .. }
        | AppMode::ScpProgress { .. }
        | AppMode::DeleteConfirmation { .. } => {}
    }
}

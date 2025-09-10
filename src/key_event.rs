use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::Backend;

use crate::async_ssh_client::SshSession;
use crate::config::manager::AuthMethod;
use crate::config::manager::Connection;
use crate::error::AppError;
use crate::ui::TerminalState;
use crate::ui::{ScpFocusField, ScpForm};
use crate::{App, AppMode, ScpResult};

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

    // If dropdown is visible, handle dropdown navigation first
    if let Some(dropdown) = &mut app.dropdown {
        match key.code {
            KeyCode::Down => {
                dropdown.next();
                return KeyFlow::Continue;
            }
            KeyCode::Up => {
                dropdown.prev();
                return KeyFlow::Continue;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                dropdown.next();
                return KeyFlow::Continue;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                dropdown.prev();
                return KeyFlow::Continue;
            }
            KeyCode::Enter => {
                // Select the current option and apply it to the focused field
                if let Some(selected_option) = dropdown.get_selected().cloned() {
                    if let Some(form) = &mut app.scp_form {
                        if matches!(form.focus, ScpFocusField::LocalPath) {
                            // Construct the complete path by combining current input with selected option
                            let current = form.local_path.clone();
                            form.local_path = construct_completed_path(&current, &selected_option);
                        }
                    }
                }
                app.dropdown = None;
                return KeyFlow::Continue;
            }
            KeyCode::Esc => {
                app.dropdown = None;
                return KeyFlow::Continue;
            }
            _ => {
                // For other keys, hide dropdown and continue with normal processing
                app.dropdown = None;
            }
        }
    }

    // If SCP progress is visible, handle cancellation
    if app.scp_progress.is_some() {
        match key.code {
            KeyCode::Esc => {
                app.scp_progress = None;
                app.scp_receiver = None; // Clean up the receiver
                app.info = Some("SCP transfer cancelled".to_string());
            }
            _ => {}
        }
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

    // If SCP popup is visible, handle its input
    if let Some(form) = &mut app.scp_form {
        match key.code {
            KeyCode::Esc => {
                app.scp_form = None;
            }
            KeyCode::Tab => {
                // Auto-complete local path when focused on LocalPath field
                if matches!(form.focus, ScpFocusField::LocalPath) {
                    let current = form.local_path.clone();
                    match autocomplete_local_path(&current) {
                        Some(completed) => {
                            if !current.ends_with('/') && completed != current {
                                form.local_path = completed;
                            } else {
                                // Show dropdown with available options when no change
                                if let Some(options) = list_completion_options(&current)
                                    && options.len() > 1
                                {
                                    // We need to calculate the anchor rect for the dropdown
                                    // For now, we'll use a placeholder rect - this will be updated in the UI rendering
                                    let anchor_rect = ratatui::layout::Rect {
                                        x: 0,
                                        y: 0,
                                        width: 40,
                                        height: 3,
                                    };
                                    app.dropdown =
                                        Some(crate::ui::DropdownState::new(options, anchor_rect));
                                }
                            }
                        }
                        None => {
                            // app.info = Some("No matches found".to_string());
                        }
                    }
                } else {
                    form.next();
                }
            }
            KeyCode::Down => {
                form.next();
            }
            KeyCode::BackTab | KeyCode::Up => {
                form.prev();
            }
            KeyCode::Enter => {
                let submitted = form.clone();
                app.scp_form = None;
                let local = submitted.local_path.trim().to_string();
                let mut remote = submitted.remote_path.trim().to_string();
                if local.is_empty() || remote.is_empty() {
                    app.error = Some(AppError::ValidationError(
                        "Local and remote path are required".into(),
                    ));
                    return KeyFlow::Continue;
                }
                if let Some(conn) = app.config.connections().get(app.current_selected()) {
                    // Create channel for communication with background task
                    let (sender, receiver) = tokio::sync::mpsc::channel(1);

                    // Start SCP transfer and show progress
                    let connection_name = conn.display_name.clone();
                    app.scp_progress = Some(crate::ScpProgress::new(
                        local.clone(),
                        remote.clone(),
                        connection_name,
                    ));
                    app.scp_receiver = Some(receiver);
                    app.scp_form = None; // Hide the form

                    let local_path = Path::new(&local);
                    let remote_is_dir = remote.ends_with('/');
                    if remote_is_dir {
                        local_path
                            .file_name()
                            .map(|n| remote.push_str(n.to_string_lossy().as_ref()));
                    }

                    // Start background transfer on tokio
                    let conn_clone = conn.clone();
                    let local_clone = local.clone();
                    let remote_clone = remote.clone();

                    tokio::spawn(async move {
                        let result = match SshSession::sftp_send_file(
                            &conn_clone,
                            &local_clone,
                            &remote_clone,
                        )
                        .await
                        {
                            Ok(_) => ScpResult::Success {
                                local_path: local_clone,
                                remote_path: remote_clone,
                            },
                            Err(e) => ScpResult::Error {
                                error: e.to_string(),
                            },
                        };
                        let _ = sender.send(result).await;
                    });
                }
            }
            KeyCode::Backspace => {
                let s = form.focused_value_mut();
                s.pop();
            }
            KeyCode::Char(ch) => {
                let s = form.focused_value_mut();
                s.push(ch);
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
    }
}

/// Paste event handler; dispatches by AppMode
pub async fn handle_paste_event<B: Backend + Write>(app: &mut App<B>, data: &str) {
    match &mut app.mode {
        AppMode::FormNew { form } => {
            let s = form.focused_value_mut();
            s.push_str(data);
        }
        AppMode::FormEdit { form, .. } => {
            let s = form.focused_value_mut();
            s.push_str(data);
        }
        AppMode::Connected {
            name: _,
            client,
            state,
            ..
        } => {
            if let Ok(mut guard) = state.lock() {
                if guard.parser.screen().scrollback() > 0 {
                    guard.scroll_to_bottom();
                }
            }
            if let Err(e) = client.write_all(data.as_bytes()).await {
                app.error = Some(e);
            }
        }
        AppMode::ConnectionList { .. } => {}
    }
}

async fn handle_connection_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    let len = app.config.connections().len();
    match key.code {
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.mode = AppMode::FormNew {
                form: crate::ui::ConnectionForm::new(),
            };
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.scp_form = Some(ScpForm::new());
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::ConnectionList { selected } = &mut app.mode {
                *selected = if *selected == 0 {
                    len - 1
                } else {
                    *selected - 1
                };
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::ConnectionList { selected } = &mut app.mode {
                *selected = (*selected + 1) % len;
            }
        }
        KeyCode::Enter => {
            let conn = app.config.connections()[app.current_selected()].clone();
            match SshSession::connect(&conn).await {
                Ok(client) => {
                    let state = Arc::new(Mutex::new(TerminalState::new(30, 100)));
                    let app_reader = state.clone();
                    let mut client_clone = client.clone();
                    tokio::spawn(async move {
                        client_clone.read_loop(app_reader).await;
                    });
                    let _ = app.config.touch_last_used(&conn.id);
                    app.go_to_connected(
                        conn.display_name.clone(),
                        client,
                        state,
                        app.current_selected(),
                    );
                }
                Err(e) => {
                    app.error = Some(e);
                }
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            let original = app.config.connections()[app.current_selected()].clone();
            let mut form = crate::ui::ConnectionForm::new();
            form.host = original.host.clone();
            form.port = original.port.to_string();
            form.username = original.username.clone();
            form.display_name = original.display_name.clone();
            form.private_key_path = match &original.auth_method {
                AuthMethod::PublicKey {
                    private_key_path, ..
                } => private_key_path.clone(),
                _ => String::new(),
            };
            form.password = match &original.auth_method {
                AuthMethod::Password(password) => password.clone(),
                _ => String::new(),
            };

            app.mode = AppMode::FormEdit {
                form,
                original,
                current_selected: app.current_selected(),
            };
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(conn) = app.config.connections().get(app.current_selected()) {
                let id = conn.id.clone();
                match app.config.remove_connection(&id) {
                    Ok(_) => {
                        if let Err(e) = app.config.save() {
                            app.error = Some(e);
                        }
                        let new_len = app.config.connections().len();
                        if let AppMode::ConnectionList { selected } = &mut app.mode {
                            if new_len == 0 {
                                *selected = 0;
                            } else if *selected >= new_len {
                                *selected = new_len - 1;
                            }
                        }
                    }
                    Err(e) => app.error = Some(e),
                }
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            return KeyFlow::Quit;
        }
        _ => {}
    }
    KeyFlow::Continue
}

async fn handle_form_new_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            app.go_to_connection_list();
        }
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::FormNew { form } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::FormNew { form } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            if let AppMode::FormNew { form } = &mut app.mode {
                match form.validate() {
                    Ok(_) => {
                        let user = form.username.trim().to_string();

                        let auth_method = if !form.private_key_path.trim().is_empty() {
                            AuthMethod::PublicKey {
                                private_key_path: form.private_key_path.trim().to_string(),
                                passphrase: None,
                            }
                        } else {
                            AuthMethod::Password(form.password.clone())
                        };

                        let mut conn = Connection::new(
                            form.host.trim().to_string(),
                            form.port
                                .parse::<u16>()
                                .unwrap_or(app.config.default_port()),
                            user,
                            auth_method,
                        );
                        if !form.display_name.trim().is_empty() {
                            conn.set_display_name(form.display_name.trim().to_string());
                        }
                        match SshSession::connect(&conn).await {
                            Ok(client) => {
                                if let Err(e) = app.config.add_connection(conn.clone()) {
                                    app.error = Some(e);
                                }

                                let state = Arc::new(Mutex::new(TerminalState::new(30, 100)));
                                let app_reader = state.clone();
                                let mut client_clone = client.clone();
                                tokio::spawn(async move {
                                    client_clone.read_loop(app_reader).await;
                                });
                                form.error = None;
                                let _ = app.config.touch_last_used(&conn.id);
                                app.go_to_connected(conn.display_name.clone(), client, state, 0);
                            }
                            Err(e) => {
                                app.error = Some(e);
                            }
                        }
                    }
                    Err(msg) => {
                        app.error = Some(AppError::ValidationError(msg));
                    }
                }
            }
        }
        KeyCode::Backspace => {
            if let AppMode::FormNew { form } = &mut app.mode {
                let s = form.focused_value_mut();
                s.pop();
            }
        }
        KeyCode::Char(ch) => {
            if let AppMode::FormNew { form } = &mut app.mode {
                let s = form.focused_value_mut();
                s.push(ch);
            }
        }
        _ => {}
    }
    KeyFlow::Continue
}

async fn handle_form_edit_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::FormEdit {
                current_selected, ..
            } = &app.mode
            {
                app.go_to_connection_list_with_selected(*current_selected);
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            if let AppMode::FormEdit { form, original, .. } = &mut app.mode {
                if form.host.trim().is_empty() {
                    app.error = Some(AppError::ValidationError("Host is required".into()));
                    return KeyFlow::Continue;
                }
                if form.port.trim().is_empty() {
                    app.error = Some(AppError::ValidationError("Port is required".into()));
                    return KeyFlow::Continue;
                }
                let parsed_port = match form.port.parse::<u16>() {
                    Ok(p) => p,
                    Err(_) => {
                        app.error = Some(AppError::ValidationError("Port must be a number".into()));
                        return KeyFlow::Continue;
                    }
                };
                if form.username.trim().is_empty() {
                    app.error = Some(AppError::ValidationError("Username is required".into()));
                    return KeyFlow::Continue;
                }

                let new_password = if form.password.is_empty() {
                    // Extract password from original connection's auth_method
                    match &original.auth_method {
                        AuthMethod::Password(password) => password.clone(),
                        _ => String::new(), // Default to empty if not password auth
                    }
                } else {
                    form.password.clone()
                };

                let mut updated = original.clone();
                updated.host = form.host.trim().to_string();
                updated.port = parsed_port;
                updated.username = form.username.trim().to_string();
                updated.auth_method = AuthMethod::Password(new_password);
                updated.display_name = form.display_name.trim().to_string();

                if let Err(e) = updated.validate() {
                    app.error = Some(e);
                    return KeyFlow::Continue;
                }

                match app.config.update_connection(updated) {
                    Ok(_) => {
                        if let Err(e) = app.config.save() {
                            app.error = Some(e);
                        }
                        app.go_to_connection_list();
                    }
                    Err(e) => app.error = Some(e),
                }
            }
        }
        KeyCode::Backspace => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                let s = form.focused_value_mut();
                s.pop();
            }
        }
        KeyCode::Char(ch) => {
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                let s = form.focused_value_mut();
                s.push(ch);
            }
        }
        _ => {}
    }
    KeyFlow::Continue
}

async fn handle_connected_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    if let AppMode::Connected {
        name: _,
        client,
        state,
        current_selected,
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                let (in_alt, app_cursor) = state
                    .lock()
                    .ok()
                    .map(|g| {
                        (
                            g.parser.screen().alternate_screen(),
                            g.parser.screen().application_cursor(),
                        )
                    })
                    .unwrap_or((false, false));
                // If an interactive full-screen/app-cursor mode is active, forward ESC to remote
                if in_alt || app_cursor {
                    if let Ok(mut guard) = state.lock() {
                        if guard.parser.screen().scrollback() > 0 {
                            guard.scroll_to_bottom();
                        }
                    }
                    if let Err(e) = client.write_all(&[0x1b]).await {
                        app.error = Some(e);
                    }
                } else {
                    if let Err(e) = client.close().await {
                        app.error = Some(e);
                    }
                    let current_selected = *current_selected;
                    app.go_to_connection_list_with_selected(current_selected);
                }
            }
            KeyCode::Enter => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\r").await {
                    app.error = Some(e);
                }
            }
            KeyCode::Backspace => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x7f]).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Left => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                let app_cursor = state
                    .lock()
                    .ok()
                    .map(|g| g.parser.screen().application_cursor())
                    .unwrap_or(false);
                let seq = if app_cursor { b"\x1bOD" } else { b"\x1b[D" };
                if let Err(e) = client.write_all(seq).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Right => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                let app_cursor = state
                    .lock()
                    .ok()
                    .map(|g| g.parser.screen().application_cursor())
                    .unwrap_or(false);
                let seq = if app_cursor { b"\x1bOC" } else { b"\x1b[C" };
                if let Err(e) = client.write_all(seq).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Up => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                let app_cursor = state
                    .lock()
                    .ok()
                    .map(|g| g.parser.screen().application_cursor())
                    .unwrap_or(false);
                let seq = if app_cursor { b"\x1bOA" } else { b"\x1b[A" };
                if let Err(e) = client.write_all(seq).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Down => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                let app_cursor = state
                    .lock()
                    .ok()
                    .map(|g| g.parser.screen().application_cursor())
                    .unwrap_or(false);
                let seq = if app_cursor { b"\x1bOB" } else { b"\x1b[B" };
                if let Err(e) = client.write_all(seq).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Tab => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\t").await {
                    app.error = Some(e);
                }
            }
            KeyCode::PageUp => {
                if let Ok(mut guard) = state.lock() {
                    let rows = guard.parser.screen().size().0;
                    let page = (rows.saturating_sub(1)) as i32;
                    guard.scroll_by(page);
                }
            }
            KeyCode::PageDown => {
                if let Ok(mut guard) = state.lock() {
                    let rows = guard.parser.screen().size().0;
                    let page = (rows.saturating_sub(1)) as i32;
                    guard.scroll_by(-page);
                }
            }
            KeyCode::Home => {
                if let Ok(mut guard) = state.lock() {
                    let top = usize::MAX;
                    guard.parser.screen_mut().set_scrollback(top);
                }
            }
            KeyCode::End => {
                if let Ok(mut guard) = state.lock() {
                    guard.scroll_to_bottom();
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x03]).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x04]).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x15]).await {
                    app.error = Some(e);
                }
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let AppMode::Connected { state, .. } = &mut app.mode {
                    if let Ok(mut guard) = state.lock() {
                        guard.scroll_by(-1);
                    }
                }
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let AppMode::Connected { state, .. } = &mut app.mode {
                    if let Ok(mut guard) = state.lock() {
                        guard.scroll_by(1);
                    }
                }
            }
            KeyCode::Char(ch_) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                let mut tmp = [0u8; 4];
                let s = ch_.encode_utf8(&mut tmp);
                if let Err(e) = client.write_all(s.as_bytes()).await {
                    app.error = Some(e);
                }
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

/// Auto-complete local file paths for SCP form
fn autocomplete_local_path(input: &str) -> Option<String> {
    // Handle empty input
    if input.is_empty() {
        return Some("./".to_string());
    }

    // Expand tilde to home directory
    let expanded = if input.starts_with("~") {
        if let Ok(home) = env::var("HOME") {
            let home_path = PathBuf::from(home);
            let tail = &input[1..];
            if tail.is_empty() {
                home_path.to_string_lossy().to_string() + "/"
            } else {
                let tail = tail.strip_prefix('/').unwrap_or(tail);
                home_path.join(tail).to_string_lossy().to_string()
            }
        } else {
            input.to_string()
        }
    } else {
        input.to_string()
    };

    let path = Path::new(&expanded);

    // If path exists and is a directory, add trailing slash if missing
    if path.is_dir() && !expanded.ends_with('/') {
        return Some(expanded + "/");
    }

    // If path exists and is a file, return as-is
    if path.is_file() {
        return Some(expanded);
    }

    // Try to complete based on parent directory
    let (parent_dir, prefix) = if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        (PathBuf::from("."), expanded.clone())
    };

    // Read directory entries
    let entries = match fs::read_dir(&parent_dir) {
        Ok(entries) => entries,
        Err(_) => return None,
    };

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) && !name.starts_with('.') {
            let full_path = parent_dir.join(&name);
            let path_str = if full_path.is_dir() {
                full_path.to_string_lossy().to_string() + "/"
            } else {
                full_path.to_string_lossy().to_string()
            };
            matches.push(path_str);
        }
    }

    match matches.len() {
        0 => None,
        1 => Some(matches.into_iter().next().unwrap()),
        _ => {
            // Find common prefix among matches
            let common = find_common_prefix(&matches);
            if common.len() > expanded.len() {
                Some(common)
            } else {
                // Return the first match if no common prefix extension
                Some(matches.into_iter().next().unwrap())
            }
        }
    }
}

/// Find the longest common prefix among a list of strings
fn find_common_prefix(strings: &[String]) -> String {
    if strings.is_empty() {
        return String::new();
    }

    let first = &strings[0];
    let mut common_len = first.len();

    for s in strings.iter().skip(1) {
        let mut len = 0;
        for (c1, c2) in first.chars().zip(s.chars()) {
            if c1 == c2 {
                len += c1.len_utf8();
            } else {
                break;
            }
        }
        common_len = common_len.min(len);
    }

    first[..common_len].to_string()
}

/// List available completion options for display
fn list_completion_options(input: &str) -> Option<Vec<String>> {
    let expanded = if input.starts_with("~") {
        if let Ok(home) = env::var("HOME") {
            let home_path = PathBuf::from(home);
            let tail = &input[1..];
            if tail.is_empty() {
                home_path.to_string_lossy().to_string() + "/"
            } else {
                let tail = tail.strip_prefix('/').unwrap_or(tail);
                home_path.join(tail).to_string_lossy().to_string()
            }
        } else {
            input.to_string()
        }
    } else {
        input.to_string()
    };

    let path = Path::new(&expanded);
    let (parent_dir, prefix) = if path.is_dir() && expanded.ends_with('/') {
        (path.to_path_buf(), String::new())
    } else if let Some(parent) = path.parent() {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), filename)
    } else {
        (PathBuf::from("."), expanded.clone())
    };

    let entries = fs::read_dir(&parent_dir).ok()?;
    let mut options = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&prefix) && !name.starts_with('.') {
            options.push(name);
        }
    }

    if options.is_empty() {
        None
    } else {
        options.sort();
        Some(options)
    }
}

/// Construct the completed path by combining the current input with the selected option
fn construct_completed_path(current_input: &str, selected_option: &str) -> String {
    // Handle empty input
    if current_input.is_empty() {
        return format!("./{}", selected_option);
    }

    // Expand tilde to home directory
    let expanded = if current_input.starts_with("~") {
        if let Ok(home) = env::var("HOME") {
            let home_path = PathBuf::from(home);
            let tail = &current_input[1..];
            if tail.is_empty() {
                home_path.to_string_lossy().to_string() + "/"
            } else {
                let tail = tail.strip_prefix('/').unwrap_or(tail);
                home_path.join(tail).to_string_lossy().to_string()
            }
        } else {
            current_input.to_string()
        }
    } else {
        current_input.to_string()
    };

    let path = Path::new(&expanded);

    // If the current path is a directory and ends with '/', append the selected option
    if path.is_dir() && expanded.ends_with('/') {
        return format!("{}{}", expanded, selected_option);
    }

    // If the current path has a parent directory, replace the filename with the selected option
    if let Some(parent) = path.parent() {
        let parent_str = parent.to_string_lossy();
        if parent_str.is_empty() || parent_str == "." {
            selected_option.to_string()
        } else {
            format!("{}/{}", parent_str, selected_option)
        }
    } else {
        // No parent directory, just use the selected option
        selected_option.to_string()
    }
}

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::manager::Connection;
use crate::error::AppError;
use crate::ssh_client::SshClient;
use crate::ui::TerminalState;
use crate::ui::{ScpFocusField, ScpForm};
use crate::{App, AppMode};

/// Result of handling a key or paste event
pub enum KeyFlow {
    Continue,
    Quit,
}

/// Top-level key event handler, including error popup dismissal and dispatch by AppMode
pub fn handle_key_event(app: &mut App, key: KeyEvent) -> KeyFlow {
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
                            if completed != current {
                                form.local_path = completed;
                            } else {
                                // Show dropdown with available options when no change
                                if let Some(options) = list_completion_options(&current) {
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
                            app.info = Some("No matches found".to_string());
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
                let remote = submitted.remote_path.trim().to_string();
                if local.is_empty() || remote.is_empty() {
                    app.error = Some(AppError::ValidationError(
                        "Local and remote path are required".into(),
                    ));
                    return KeyFlow::Continue;
                }
                if let Some(conn) = app.config.connections().get(current_selected(app)) {
                    match SshClient::scp_send_file(conn, &local, &remote) {
                        Ok(_) => {
                            app.info =
                                Some(format!("SCP upload completed from {} to {}", local, remote));
                        }
                        Err(e) => {
                            app.error = Some(e);
                        }
                    }
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
        AppMode::MainMenu { .. } => handle_main_menu_key(app, key),
        AppMode::ConnectionList { .. } => handle_connection_list_key(app, key),
        AppMode::FormNew { .. } => handle_form_new_key(app, key),
        AppMode::FormEdit { .. } => handle_form_edit_key(app, key),
        AppMode::Connected { .. } => handle_connected_key(app, key),
    }
}

/// Paste event handler; dispatches by AppMode
pub fn handle_paste_event(app: &mut App, data: &str) {
    match &mut app.mode {
        AppMode::FormNew { form } => {
            let s = form.focused_value_mut();
            s.push_str(data);
        }
        AppMode::FormEdit { form, .. } => {
            let s = form.focused_value_mut();
            s.push_str(data);
        }
        AppMode::Connected { client, state } => {
            if let Ok(mut guard) = state.lock() {
                if guard.parser.screen().scrollback() > 0 {
                    guard.scroll_to_bottom();
                }
            }
            if let Err(e) = client.write_all(data.as_bytes()) {
                app.error = Some(e);
            }
        }
        AppMode::MainMenu { .. } => {}
        AppMode::ConnectionList { .. } => {}
    }
}

fn handle_main_menu_key(app: &mut App, key: KeyEvent) -> KeyFlow {
    const NUM_ITEMS: usize = 3;
    match key.code {
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::MainMenu { selected } = &mut app.mode {
                *selected = if *selected == 0 {
                    NUM_ITEMS - 1
                } else {
                    *selected - 1
                };
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::MainMenu { selected } = &mut app.mode {
                *selected = (*selected + 1) % NUM_ITEMS;
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V') => {
            app.mode = AppMode::ConnectionList { selected: 0 };
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.mode = AppMode::FormNew {
                form: crate::ui::ConnectionForm::new(),
            };
        }
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
            return KeyFlow::Quit;
        }
        KeyCode::Enter => {
            if let AppMode::MainMenu { selected } = &mut app.mode {
                match *selected {
                    0 => {
                        app.mode = AppMode::ConnectionList { selected: 0 };
                    }
                    1 => {
                        app.mode = AppMode::FormNew {
                            form: crate::ui::ConnectionForm::new(),
                        };
                    }
                    2 => {
                        return KeyFlow::Quit;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    KeyFlow::Continue
}

fn handle_connection_list_key(app: &mut App, key: KeyEvent) -> KeyFlow {
    let len = app.config.connections().len();
    if len == 0 {
        match key.code {
            KeyCode::Esc => app.go_to_main_menu(),
            _ => {}
        }
        return KeyFlow::Continue;
    }
    match key.code {
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
            let conn = app.config.connections()[current_selected(app)].clone();
            match SshClient::connect(&conn) {
                Ok(client) => {
                    let state = Arc::new(Mutex::new(TerminalState::new(30, 100)));
                    let app_reader = state.clone();
                    let client_reader = client.channel.clone();
                    thread::spawn(move || {
                        let mut buf = [0u8; 8192];
                        loop {
                            let n = {
                                let mut ch = match client_reader.lock() {
                                    Ok(guard) => guard,
                                    Err(_) => break,
                                };
                                match ch.read(&mut buf) {
                                    Ok(0) => return,
                                    Ok(n) => n,
                                    Err(_) => 0,
                                }
                            };
                            if n > 0 {
                                if let Ok(mut guard) = app_reader.lock() {
                                    guard.process_bytes(&buf[..n]);
                                }
                            } else {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                        }
                    });
                    let _ = app.config.touch_last_used(&conn.id);
                    app.go_to_connected(client, state);
                }
                Err(e) => {
                    app.error = Some(e);
                }
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            let original = app.config.connections()[current_selected(app)].clone();
            let mut form = crate::ui::ConnectionForm::new();
            form.host = original.host.clone();
            form.port = original.port.to_string();
            form.username = original.username.clone();
            form.display_name = original.display_name.clone();
            form.password.clear();
            app.mode = AppMode::FormEdit { form, original };
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            let id = app.config.connections()[current_selected(app)].id.clone();
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
        KeyCode::Esc => {
            app.go_to_main_menu();
        }
        _ => {}
    }
    KeyFlow::Continue
}

fn handle_form_new_key(app: &mut App, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            app.go_to_main_menu();
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
                        let pass = form.password.clone();

                        let mut conn = Connection::new(
                            form.host.trim().to_string(),
                            form.port.parse::<u16>().unwrap_or(22),
                            user,
                            pass,
                        );
                        if !form.display_name.trim().is_empty() {
                            conn.set_display_name(form.display_name.trim().to_string());
                        }
                        match SshClient::connect(&conn) {
                            Ok(client) => {
                                if let Err(e) = app.config.add_connection(conn.clone()) {
                                    app.error = Some(e);
                                }

                                let state = Arc::new(Mutex::new(TerminalState::new(30, 100)));
                                let app_reader = state.clone();
                                let client_reader = client.channel.clone();
                                thread::spawn(move || {
                                    let mut buf = [0u8; 8192];
                                    loop {
                                        let n = {
                                            let mut ch = match client_reader.lock() {
                                                Ok(guard) => guard,
                                                Err(_) => break,
                                            };
                                            match ch.read(&mut buf) {
                                                Ok(0) => return,
                                                Ok(n) => n,
                                                Err(_) => 0,
                                            }
                                        };
                                        if n > 0 {
                                            if let Ok(mut guard) = app_reader.lock() {
                                                guard.process_bytes(&buf[..n]);
                                            }
                                        } else {
                                            std::thread::sleep(Duration::from_millis(10));
                                        }
                                    }
                                });
                                form.error = None;
                                app.go_to_connected(client, state);
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

fn handle_form_edit_key(app: &mut App, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            app.go_to_connection_list();
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
            if let AppMode::FormEdit { form, original } = &mut app.mode {
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
                    original.password.clone()
                } else {
                    form.password.clone()
                };

                let mut updated = original.clone();
                updated.host = form.host.trim().to_string();
                updated.port = parsed_port;
                updated.username = form.username.trim().to_string();
                updated.password = new_password;
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

fn handle_connected_key(app: &mut App, key: KeyEvent) -> KeyFlow {
    if let AppMode::Connected { client, state } = &mut app.mode {
        match key.code {
            KeyCode::Esc => {
                let in_alt = state
                    .lock()
                    .ok()
                    .map(|g| g.parser.screen().alternate_screen())
                    .unwrap_or(false);
                if in_alt {
                    if let Ok(mut guard) = state.lock() {
                        if guard.parser.screen().scrollback() > 0 {
                            guard.scroll_to_bottom();
                        }
                    }
                    if let Err(e) = client.write_all(&[0x1b]) {
                        app.error = Some(e);
                    }
                } else {
                    client.close();
                    app.go_to_connection_list();
                }
            }
            KeyCode::Enter => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\r") {
                    app.error = Some(e);
                }
            }
            KeyCode::Backspace => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x7f]) {
                    app.error = Some(e);
                }
            }
            KeyCode::Left => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\x1b[D") {
                    app.error = Some(e);
                }
            }
            KeyCode::Right => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\x1b[C") {
                    app.error = Some(e);
                }
            }
            KeyCode::Up => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\x1b[A") {
                    app.error = Some(e);
                }
            }
            KeyCode::Down => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\x1b[B") {
                    app.error = Some(e);
                }
            }
            KeyCode::Tab => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(b"\t") {
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
                if let Err(e) = client.write_all(&[0x03]) {
                    app.error = Some(e);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x04]) {
                    app.error = Some(e);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x15]) {
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
                if let Err(e) = client.write_all(s.as_bytes()) {
                    app.error = Some(e);
                }
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

fn current_selected(app: &App) -> usize {
    if let AppMode::ConnectionList { selected } = app.mode {
        let len = app.config.connections().len();
        if len == 0 { 0 } else { selected.min(len - 1) }
    } else {
        0
    }
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

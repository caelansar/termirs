use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::{App, AppMode};

pub async fn handle_connected_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
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
            KeyCode::Delete => {
                if let Ok(mut guard) = state.lock() {
                    if guard.parser.screen().scrollback() > 0 {
                        guard.scroll_to_bottom();
                    }
                }
                if let Err(e) = client.write_all(&[0x1b, 0x5b, 0x33, 0x7e]).await {
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

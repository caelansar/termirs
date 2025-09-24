use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::{App, AppMode};

/// Helper function to scroll to bottom if needed
async fn ensure_scroll_to_bottom(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::ui::TerminalState>>,
) {
    let mut guard = state.lock().await;
    if guard.parser.screen().scrollback() > 0 {
        guard.scroll_to_bottom();
    }
}

/// Encode a key event to ANSI escape sequence
fn encode_key_event_to_ansi(app_cursor: bool, key: &KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Left => Some(if app_cursor {
            b"\x1bOD".to_vec()
        } else {
            b"\x1b[D".to_vec()
        }),
        KeyCode::Right => Some(if app_cursor {
            b"\x1bOC".to_vec()
        } else {
            b"\x1b[C".to_vec()
        }),
        KeyCode::Up => Some(if app_cursor {
            b"\x1bOA".to_vec()
        } else {
            b"\x1b[A".to_vec()
        }),
        KeyCode::Down => Some(if app_cursor {
            b"\x1bOB".to_vec()
        } else {
            b"\x1b[B".to_vec()
        }),
        KeyCode::Home => Some(if app_cursor {
            b"\x1bOH".to_vec()
        } else {
            b"\x1b[H".to_vec()
        }),
        KeyCode::End => Some(if app_cursor {
            b"\x1bOF".to_vec()
        } else {
            b"\x1b[F".to_vec()
        }),
        KeyCode::Delete => Some(vec![0x1b, 0x5b, 0x33, 0x7e]), // CSI 3~
        KeyCode::PageUp => Some(vec![0x1b, 0x5b, 0x35, 0x7e]), // CSI 5~
        KeyCode::PageDown => Some(vec![0x1b, 0x5b, 0x36, 0x7e]), // CSI 6~
        KeyCode::F(n) => {
            // Basic xterm mappings
            let bytes = match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                12 => b"\x1b[24~".to_vec(),
                _ => return None,
            };
            Some(bytes)
        }
        KeyCode::Char(ch) => {
            // CTRL combinations for ASCII letters map to 0x01..0x1A
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = ch.to_ascii_lowercase();
                if lower >= 'a' && lower <= 'z' {
                    let code = (lower as u8) - b'a' + 1;
                    return Some(vec![code]);
                }
            }
            // ALT/META prefixes ESC
            if key.modifiers.contains(KeyModifiers::ALT) {
                let mut tmp = [0u8; 4];
                let s = ch.encode_utf8(&mut tmp);
                let mut out = Vec::with_capacity(1 + s.len());
                out.push(0x1b);
                out.extend_from_slice(s.as_bytes());
                return Some(out);
            }
            // Plain UTF-8 char
            let mut tmp = [0u8; 4];
            let s = ch.encode_utf8(&mut tmp);
            Some(s.as_bytes().to_vec())
        }
        _ => None,
    }
}

pub async fn handle_connected_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    if let AppMode::Connected {
        name: _,
        client,
        state,
        current_selected,
        cancel_token,
    } = &mut app.mode
    {
        // Determine interactive mode (full-screen alt buffer or application cursor)
        let guard = state.lock().await;
        let (in_alt, app_cursor) = (
            guard.parser.screen().alternate_screen(),
            guard.parser.screen().application_cursor(),
        );
        drop(guard); // Release the lock early
        let interactive = in_alt || app_cursor;

        if interactive {
            let mut guard = state.lock().await;
            if guard.parser.screen().scrollback() > 0 {
                guard.scroll_to_bottom();
            }
            if let Some(seq) = encode_key_event_to_ansi(app_cursor, &key) {
                if let Err(e) = client.write_all(&seq).await {
                    app.error = Some(e);
                }
            }
            return KeyFlow::Continue;
        }

        match key.code {
            KeyCode::Esc => {
                let guard = state.lock().await;
                let (in_alt, app_cursor) = (
                    guard.parser.screen().alternate_screen(),
                    guard.parser.screen().application_cursor(),
                );
                drop(guard); // Release the lock early
                // If an interactive full-screen/app-cursor mode is active, forward ESC to remote
                if in_alt || app_cursor {
                    ensure_scroll_to_bottom(state).await;
                    if let Err(e) = client.write_all(&[0x1b]).await {
                        app.error = Some(e);
                    }
                } else {
                    // Cancel the background read task first
                    cancel_token.cancel();

                    // Then close the SSH connection
                    if let Err(e) = client.close().await {
                        app.error = Some(e);
                    }
                    let current_selected = *current_selected;
                    app.go_to_connection_list_with_selected(current_selected);
                }
            }
            // Special scrolling behavior for PageUp/PageDown - these need local scrolling
            KeyCode::PageUp => {
                let mut guard = state.lock().await;
                let rows = guard.parser.screen().size().0;
                let page = (rows.saturating_sub(1)) as i32;
                guard.scroll_by(page);
            }
            KeyCode::PageDown => {
                let mut guard = state.lock().await;
                let rows = guard.parser.screen().size().0;
                let page = (rows.saturating_sub(1)) as i32;
                guard.scroll_by(-page);
            }
            // Special scrolling behavior for Home/End - these need local scrolling
            KeyCode::Home => {
                let mut guard = state.lock().await;
                let top = usize::MAX;
                guard.parser.screen_mut().set_scrollback(top);
            }
            KeyCode::End => {
                let mut guard = state.lock().await;
                guard.scroll_to_bottom();
            }
            // Special local scrolling controls
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let AppMode::Connected { state, .. } = &mut app.mode {
                    let mut guard = state.lock().await;
                    guard.scroll_by(-1);
                }
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let AppMode::Connected { state, .. } = &mut app.mode {
                    let mut guard = state.lock().await;
                    guard.scroll_by(1);
                }
            }
            // All other keys can be handled by the ANSI encoder
            _ => {
                let guard = state.lock().await;
                let app_cursor = guard.parser.screen().application_cursor();
                drop(guard);

                if let Some(seq) = encode_key_event_to_ansi(app_cursor, &key) {
                    ensure_scroll_to_bottom(state).await;
                    if let Err(e) = client.write_all(&seq).await {
                        app.error = Some(e);
                    }
                }
            }
        }
    }
    KeyFlow::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_key_event_to_ansi() {
        let test_cases = vec![
            (KeyCode::Char('f'), KeyModifiers::CONTROL, Some(vec![0x06])),
            (KeyCode::Char('b'), KeyModifiers::CONTROL, Some(vec![0x02])),
            (KeyCode::Char('v'), KeyModifiers::CONTROL, Some(vec![0x16])),
        ];

        for (key, modifiers, expected) in test_cases {
            let key = KeyEvent::new(key, modifiers);
            let seq = encode_key_event_to_ansi(false, &key);
            assert_eq!(seq, expected);
        }
    }
}

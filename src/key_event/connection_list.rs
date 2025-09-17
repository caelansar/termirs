use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::async_ssh_client::SshSession;
use crate::ui::TerminalState;
use crate::{App, AppMode};

pub async fn handle_connection_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Check if we're in search mode
    if let AppMode::ConnectionList {
        search_mode: true,
        search_input,
        ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::ConnectionList {
                    search_mode,
                    search_input,
                    ..
                } = &mut app.mode
                {
                    *search_mode = false;
                    search_input.delete_line_by_head();
                    search_input.delete_line_by_end();
                }
            }
            KeyCode::Enter => {
                if let AppMode::ConnectionList { search_mode, .. } = &mut app.mode {
                    *search_mode = false;
                }
            }
            _ => {
                // Let TextArea handle all other key events (cursor movement, editing, etc.)
                search_input.input(key);
            }
        }
        return KeyFlow::Continue;
    }

    let len = app.config.connections().len();
    match key.code {
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.go_to_form_new();
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.go_to_scp_form(app.current_selected());
        }
        KeyCode::Char('/') => {
            if let AppMode::ConnectionList {
                search_mode,
                search_input,
                ..
            } = &mut app.mode
            {
                *search_mode = true;
                // Clear any existing text and set up the TextArea for search
                search_input.delete_line_by_head();
                search_input.delete_line_by_end();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::ConnectionList { selected, .. } = &mut app.mode {
                *selected = if *selected == 0 {
                    len - 1
                } else {
                    *selected - 1
                };
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::ConnectionList { selected, .. } = &mut app.mode {
                *selected = (*selected + 1) % len;
            }
        }
        KeyCode::Enter => {
            let conn = app.config.connections()[app.current_selected()].clone();
            match SshSession::connect(&conn).await {
                Ok(client) => {
                    // Save the server key if it was received and the connection doesn't have one stored
                    if conn.public_key.is_none() {
                        if let Some(server_key) = client.get_server_key().await {
                            if let Some(stored_conn) =
                                app.config.connections_mut().iter_mut().find(|c| {
                                    c.host == conn.host
                                        && c.port == conn.port
                                        && c.username == conn.username
                                })
                            {
                                stored_conn.public_key = Some(server_key);
                                let _ = app.config.save();
                            }
                        }
                    }

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
            if let Some(original) = app.config.connections().get(app.current_selected()) {
                app.go_to_form_edit(original.into(), original.clone());
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(conn) = app.config.connections().get(app.current_selected()) {
                let connection_name = conn.display_name.clone();
                let connection_id = conn.id.clone();
                let current_selected = app.current_selected();
                app.go_to_delete_confirmation(connection_name, connection_id, current_selected);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            return KeyFlow::Quit;
        }
        _ => {}
    }
    KeyFlow::Continue
}

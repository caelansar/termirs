use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
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
        KeyCode::Char('i') | KeyCode::Char('I') => {
            // Open file explorer for the selected connection
            let selected_idx = app.current_selected();
            if let Some(conn) = app.config.connections().get(selected_idx).cloned() {
                let _ = app.config.touch_last_used(&conn.id);
                let return_from = crate::ConnectingSource::ConnectionList {
                    file_explorer: true,
                };
                let (cancel_token, receiver) =
                    crate::async_ssh_client::SshSession::initiate_connection(conn.clone());
                let connection_name = conn.display_name.clone();
                app.go_to_connecting(
                    conn,
                    connection_name,
                    selected_idx,
                    return_from,
                    cancel_token,
                    receiver,
                );
                // match app.go_to_file_explorer(conn, selected_idx).await {
                //     Ok(_) => {}
                //     Err(e) => {
                //         app.error = Some(e);
                //     }
                // }
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            // Open port forwarding manager
            app.go_to_port_forwarding_list().await;
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
                if len != 0 {
                    *selected = if *selected == 0 {
                        len - 1
                    } else {
                        (*selected - 1).min(len - 1)
                    };
                } else {
                    *selected = 0;
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::ConnectionList { selected, .. } = &mut app.mode
                && len != 0
            {
                *selected = (*selected + 1) % len;
            }
        }
        KeyCode::Enter => {
            if let Some(conn) = app
                .config
                .connections()
                .get(app.current_selected())
                .cloned()
            {
                // Initiate connection
                let (cancel_token, receiver) =
                    crate::async_ssh_client::SshSession::initiate_connection(conn.clone());
                let connection_name = conn.display_name.clone();
                let return_from = crate::ConnectingSource::ConnectionList {
                    file_explorer: false,
                };
                app.go_to_connecting(
                    conn,
                    connection_name,
                    app.current_selected(),
                    return_from,
                    cancel_token,
                    receiver,
                );
            } else if len == 0 {
                app.info = Some("No connections available".to_string());
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
        KeyCode::Char('q') => {
            return KeyFlow::Quit;
        }
        _ => {}
    }
    KeyFlow::Continue
}

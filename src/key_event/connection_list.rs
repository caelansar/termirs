use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::{App, AppMode};

pub async fn handle_connection_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Check if we're in search mode (actively typing)
    if let AppMode::ConnectionList {
        search, selected, ..
    } = &mut app.mode
    {
        if search.is_on() {
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(query) = search.query_mut() {
                        query.push(c);
                    }
                    *selected = 0; // Reset selection when query changes
                    app.mark_redraw();
                }
                KeyCode::Backspace => {
                    if let Some(query) = search.query_mut() {
                        query.pop();
                    }
                    *selected = 0; // Reset selection when query changes
                    app.mark_redraw();
                }
                KeyCode::Esc => {
                    if !search.query().is_empty() {
                        search.clear_query();
                        *selected = 0; // Reset selection when clearing query
                    } else {
                        search.deactivate();
                        *selected = 0; // Reset selection when exiting search
                    }
                    app.mark_redraw();
                }
                KeyCode::Enter => {
                    search.apply();
                    // Keep current selection when applying filter
                    app.mark_redraw();
                }
                _ => {}
            }
            return KeyFlow::Continue;
        }

        // Handle Esc when search filter is applied (but not actively editing)
        if matches!(search, crate::SearchState::Applied { .. }) {
            if key.code == KeyCode::Esc {
                search.deactivate();
                *selected = 0; // Reset selection when clearing filter
                app.mark_redraw();
                return KeyFlow::Continue;
            }
        }
    }

    // Get the effective list length (filtered if search is active)
    let len = if let AppMode::ConnectionList { search, .. } = &app.mode {
        crate::ui::get_filtered_connection_count(app.config.connections(), search.query())
    } else {
        app.config.connections().len()
    };

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
                let (cols, rows) = app.ssh_terminal_size().unwrap_or((80, 24));
                let (cancel_token, receiver) =
                    crate::async_ssh_client::SshSession::initiate_connection(
                        conn.clone(),
                        cols,
                        rows,
                    );
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
                search, selected, ..
            } = &mut app.mode
            {
                search.activate();
                *selected = 0; // Reset selection when starting search
                app.mark_redraw();
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
                // Initiate connection with current terminal size
                let (cols, rows) = app.ssh_terminal_size().unwrap_or((80, 24));
                let (cancel_token, receiver) =
                    crate::async_ssh_client::SshSession::initiate_connection(
                        conn.clone(),
                        cols,
                        rows,
                    );
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

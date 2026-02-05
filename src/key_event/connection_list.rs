use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use super::table_handler::{handle_navigation_keys, handle_search_keys};
use crate::ui::table::TableListState;
use crate::{App, AppMode};

pub async fn handle_connection_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Handle search keys using shared handler
    if let AppMode::ConnectionList(state) = &mut app.mode {
        let mut table_state = TableListState::from_parts(state.selected, state.search.clone());
        if handle_search_keys(&mut table_state, key) {
            state.selected = table_state.selected;
            state.search = table_state.search;
            app.mark_redraw();
            return KeyFlow::Continue;
        }
    }

    // Get the effective list length (filtered if search is active)
    let len = if let AppMode::ConnectionList(state) = &app.mode {
        if state.search.query().is_empty() {
            app.config.connections().len()
        } else {
            // Filter connections using same logic as the component
            let query_lower = state.search.query().to_lowercase();
            app.config
                .connections()
                .iter()
                .filter(|c| {
                    c.host.to_lowercase().contains(&query_lower)
                        || c.username.to_lowercase().contains(&query_lower)
                        || c.display_name.to_lowercase().contains(&query_lower)
                })
                .count()
        }
    } else {
        app.config.connections().len()
    };

    // Handle navigation keys using shared handler
    if let AppMode::ConnectionList(state) = &mut app.mode {
        let mut table_state = TableListState::from_parts(state.selected, state.search.clone());
        if handle_navigation_keys(&mut table_state, key, len) {
            state.selected = table_state.selected;
            state.search = table_state.search;
            app.mark_redraw();
            return KeyFlow::Continue;
        }
    }

    // Handle component-specific actions
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
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            // Open port forwarding manager
            app.go_to_port_forwarding_list().await;
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

use std::io::Write;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;
use tokio_util;
use tui_textarea::Input;

use super::KeyFlow;
use crate::async_ssh_client::SshSession;
use crate::config::manager::AuthMethod;
use crate::config::manager::Connection;
use crate::error::AppError;
use crate::ui::TerminalState;
use crate::{App, AppMode};

pub async fn handle_form_new_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::FormNew {
                current_selected, ..
            } = &mut app.mode
            {
                let current_selected = *current_selected;
                app.go_to_connection_list_with_selected(current_selected);
            }
        }
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                match form.validate() {
                    Ok(_) => {
                        let user = form.get_username_value().trim().to_string();

                        let auth_method = if !form.get_private_key_path_value().trim().is_empty() {
                            AuthMethod::PublicKey {
                                private_key_path: form
                                    .get_private_key_path_value()
                                    .trim()
                                    .to_string(),
                                passphrase: None,
                            }
                        } else {
                            AuthMethod::Password(form.get_password_value().trim().to_string())
                        };

                        let mut conn = Connection::new(
                            form.get_host_value().trim().to_string(),
                            form.get_port_value()
                                .parse::<u16>()
                                .unwrap_or(app.config.default_port()),
                            user,
                            auth_method,
                        );
                        if !form.get_display_name_value().trim().is_empty() {
                            conn.set_display_name(form.get_display_name_value().trim().to_string());
                        }
                        match SshSession::connect(&conn).await {
                            Ok(client) => {
                                // Save the server key if it was received
                                if let Some(server_key) = client.get_server_key().await {
                                    conn.public_key = Some(server_key);
                                }

                                if let Err(e) = app.config.add_connection(conn.clone()) {
                                    app.error = Some(e);
                                }

                                let state = Arc::new(Mutex::new(TerminalState::new(30, 100)));
                                let app_reader = state.clone();
                                let mut client_clone = client.clone();
                                let cancel_token = tokio_util::sync::CancellationToken::new();
                                let cancel_for_task = cancel_token.clone();
                                tokio::spawn(async move {
                                    client_clone.read_loop(app_reader, cancel_for_task).await;
                                });
                                form.error = None;
                                let _ = app.config.touch_last_used(&conn.id);
                                app.go_to_connected(
                                    conn.display_name.clone(),
                                    client,
                                    state,
                                    0,
                                    cancel_token,
                                );
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
        _ => {
            // Use textarea's built-in input handling for all other keys
            if let AppMode::FormNew { form, .. } = &mut app.mode {
                let textarea = form.focused_textarea_mut();
                textarea.input(Input::from(key));
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_form_edit_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
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
            if let AppMode::FormEdit {
                form,
                original,
                current_selected,
            } = &mut app.mode
            {
                if let Err(e) = form.validate() {
                    app.error = Some(AppError::ValidationError(e));
                    return KeyFlow::Continue;
                }

                let new_password = if form.get_password_value().is_empty() {
                    // Extract password from original connection's auth_method
                    match &original.auth_method {
                        AuthMethod::Password(password) => password.clone(),
                        _ => String::new(), // Default to empty if not password auth
                    }
                } else {
                    form.get_password_value().trim().to_string()
                };

                let new_private_key_path = if form.get_private_key_path_value().trim().is_empty() {
                    match &original.auth_method {
                        AuthMethod::PublicKey {
                            private_key_path, ..
                        } => private_key_path.clone(),
                        _ => String::new(), // Default to empty if not public key auth
                    }
                } else {
                    form.get_private_key_path_value().trim().to_string()
                };

                let mut updated = original.clone();
                updated.host = form.get_host_value().trim().to_string();
                let parsed_port = match form.get_port_value().parse::<u16>() {
                    Ok(p) => p,
                    Err(_) => {
                        app.error = Some(AppError::ValidationError("Port must be a number".into()));
                        return KeyFlow::Continue;
                    }
                };
                updated.port = parsed_port;
                updated.username = form.get_username_value().trim().to_string();
                updated.auth_method = if new_private_key_path.is_empty() {
                    AuthMethod::Password(new_password)
                } else {
                    AuthMethod::PublicKey {
                        private_key_path: new_private_key_path,
                        passphrase: None,
                    }
                };
                updated.display_name = form.get_display_name_value().trim().to_string();

                if let Err(e) = updated.validate() {
                    app.error = Some(e);
                    return KeyFlow::Continue;
                }

                match app.config.update_connection(updated) {
                    Ok(_) => {
                        if let Err(e) = app.config.save() {
                            app.error = Some(e);
                        }
                        let current_selected = *current_selected;
                        app.go_to_connection_list_with_selected(current_selected);
                    }
                    Err(e) => app.error = Some(e),
                }
            }
        }
        _ => {
            // Use textarea's built-in input handling for all other keys
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                let textarea = form.focused_textarea_mut();
                textarea.input(Input::from(key));
            }
        }
    }
    KeyFlow::Continue
}

use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;
use tui_textarea::Input;

use super::KeyFlow;
use crate::config::manager::AuthMethod;
use crate::config::manager::Connection;
use crate::error::AppError;
use crate::{App, AppMode};

pub async fn handle_form_new_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::FormNew {
                current_selected, ..
            } = &app.mode
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
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl+L: Load from SSH config
            if let AppMode::FormNew {
                auto_auth, form, ..
            } = &mut app.mode
            {
                let host_pattern = form.get_host_value().trim().to_string();

                if host_pattern.is_empty() {
                    app.error = Some(AppError::ValidationError(
                        "Please enter a hostname to import".to_string(),
                    ));
                    return KeyFlow::Continue;
                }

                match crate::config::ssh_config::query_ssh_config(&host_pattern) {
                    Ok(ssh_host) => {
                        // Populate form fields from SSH config
                        // Clear and set hostname
                        form.host.delete_line_by_head();
                        form.host.delete_line_by_end();
                        form.host.insert_str(&ssh_host.hostname);

                        // Set port if available
                        if let Some(port) = ssh_host.port {
                            form.port.delete_line_by_head();
                            form.port.delete_line_by_end();
                            form.port.insert_str(port.to_string());
                        } else {
                            app.error = Some(AppError::ValidationError(
                                "Host not found in SSH config".to_string(),
                            ));
                            return KeyFlow::Continue;
                        }

                        // Set username if available
                        if let Some(user) = &ssh_host.user {
                            form.username.delete_line_by_head();
                            form.username.delete_line_by_end();
                            form.username.insert_str(user);
                        } else {
                            app.error = Some(AppError::ValidationError(
                                "Host not found in SSH config".to_string(),
                            ));
                            return KeyFlow::Continue;
                        }

                        // Set identity file if available
                        if let Some(identity_file) = &ssh_host.identity_file {
                            if let Some(identity_file) = identity_file.first() {
                                form.private_key_path.delete_line_by_head();
                                form.private_key_path.delete_line_by_end();
                                form.private_key_path
                                    .insert_str(identity_file.to_string_lossy().to_string());
                            }
                        } else {
                            // No identity file found - use auto-auth here so we can skip auth validation
                            *auto_auth = true;
                        }

                        // Set display name to the original host pattern
                        form.display_name.delete_line_by_head();
                        form.display_name.delete_line_by_end();
                        form.display_name.insert_str(&host_pattern);

                        app.info = Some("SSH config loaded successfully".to_string());
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            }
        }
        KeyCode::Enter => {
            if let AppMode::FormNew {
                auto_auth, form, ..
            } = &mut app.mode
            {
                match form.validate(*auto_auth) {
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
                        } else if !form.get_password_value().trim().is_empty() {
                            AuthMethod::Password(form.get_password_value().trim().to_string())
                        } else {
                            // Both password and private_key_path are empty - use AutoLoadKey
                            AuthMethod::AutoLoadKey
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

                        // Initiate connection
                        let (cancel_token, receiver) =
                            crate::async_ssh_client::SshSession::initiate_connection(conn.clone());
                        let connection_name = conn.display_name.clone();
                        let return_from = crate::ConnectingSource::FormNew {
                            auto_auth: *auto_auth,
                            form: form.clone(),
                        };
                        app.go_to_connecting(
                            conn,
                            connection_name,
                            0,
                            return_from,
                            cancel_token,
                            receiver,
                        );
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
                let current_selected = *current_selected;
                app.go_to_connection_list_with_selected(current_selected);
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
                if let Err(e) =
                    form.validate(matches!(original.auth_method, AuthMethod::AutoLoadKey))
                {
                    app.error = Some(AppError::ValidationError(e));
                    return KeyFlow::Continue;
                }

                let new_password = form.get_password_value().trim().to_string();

                let new_private_key_path = form.get_private_key_path_value().trim().to_string();

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

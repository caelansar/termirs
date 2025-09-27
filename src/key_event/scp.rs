use std::io::Write;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;
use tui_textarea::Input;

use super::KeyFlow;
use crate::async_ssh_client::SshSession;
use crate::error::AppError;
use crate::ui::{ScpFocusField, ScpMode};
use crate::{App, AppMode, ScpResult};

use super::autocomplete::{
    autocomplete_local_path, construct_completed_path, list_completion_options,
};

pub async fn handle_scp_form_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::ScpForm {
                current_selected, ..
            } = &app.mode
            {
                let current_selected = *current_selected;
                app.go_to_connection_list_with_selected(current_selected);
            }
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let AppMode::ScpForm { form, .. } = &mut app.mode {
                // Switch between Send and Receive modes
                let new_mode = match form.mode {
                    ScpMode::Send => ScpMode::Receive,
                    ScpMode::Receive => ScpMode::Send,
                };

                // Create new form with switched mode, preserving current values
                let local_value = form.get_local_path_value().to_string();
                let remote_value = form.get_remote_path_value().to_string();

                *form = crate::ui::ScpForm::new_with_mode(new_mode);

                // Restore the values
                if !local_value.is_empty() {
                    form.local_path.delete_line_by_head();
                    form.local_path.insert_str(local_value);
                }
                if !remote_value.is_empty() {
                    form.remote_path.delete_line_by_head();
                    form.remote_path.insert_str(remote_value);
                }
            }
        }
        KeyCode::Tab => {
            if let AppMode::ScpForm { form, dropdown, .. } = &mut app.mode {
                // Auto-complete local path when focused on LocalPath field
                if matches!(form.focus, ScpFocusField::LocalPath) {
                    let current = form.get_local_path_value();
                    match autocomplete_local_path(&current) {
                        Some(completed) => {
                            if !current.ends_with('/') && completed != current {
                                form.local_path.delete_line_by_head();
                                form.local_path.insert_str(completed);
                            } else {
                                // Show dropdown with available options when no change
                                if let Some(options) = list_completion_options(&current)
                                    && options.len() > 1
                                {
                                    *dropdown = Some(crate::ui::DropdownState::new(options));
                                } else {
                                    form.next();
                                }
                            }
                        }
                        None => {
                            if let Some(options) = list_completion_options(&current)
                                && options.len() > 1
                            {
                                *dropdown = Some(crate::ui::DropdownState::new(options));
                            } else {
                                form.next();
                            }
                        }
                    }
                } else {
                    form.next();
                }
            }
        }
        KeyCode::Down => {
            if let AppMode::ScpForm { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::ScpForm { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            // Extract all needed values first to avoid borrow conflicts
            let (local, remote, current_selected, conn_opt) = if let AppMode::ScpForm {
                form,
                current_selected,
                ..
            } = &app.mode
            {
                let local = form.get_local_path_value().trim().to_string();
                let remote = form.get_remote_path_value().trim().to_string();
                let conn_opt = app.config.connections().get(*current_selected).cloned();
                (local, remote, *current_selected, conn_opt)
            } else {
                return KeyFlow::Continue;
            };

            if local.is_empty() || remote.is_empty() {
                app.error = Some(AppError::ValidationError(
                    "Local and remote path are required".into(),
                ));
                return KeyFlow::Continue;
            }

            if let Some(conn) = conn_opt {
                // Get the current mode from the form
                let mode = if let AppMode::ScpForm { form, .. } = &app.mode {
                    form.mode
                } else {
                    ScpMode::Send // Default fallback
                };

                // Create channel for communication with background task
                let (sender, receiver) = tokio::sync::mpsc::channel(1);

                // Start SCP transfer and show progress
                let connection_name = conn.display_name.clone();
                let progress = crate::ScpProgress::new_with_mode(
                    local.clone(),
                    remote.clone(),
                    connection_name,
                    mode,
                );

                app.go_to_scp_progress(progress, receiver, current_selected);

                match mode {
                    ScpMode::Send => {
                        let mut remote_final = remote.clone();
                        let local_path = Path::new(&local);
                        let remote_is_dir = remote.ends_with('/');
                        if remote_is_dir {
                            if let Some(filename) = local_path.file_name() {
                                remote_final.push_str(&filename.to_string_lossy());
                            }
                        }

                        // Start background send transfer
                        let local_clone = local.clone();
                        let remote_clone = remote_final;

                        tokio::spawn(async move {
                            let result = match SshSession::sftp_send_file(
                                &conn,
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
                    ScpMode::Receive => {
                        let mut local_final = local.clone();
                        let remote_path = Path::new(&remote);
                        let local_is_dir = local.ends_with('/');
                        if local_is_dir {
                            if let Some(filename) = remote_path.file_name() {
                                local_final.push_str(&filename.to_string_lossy());
                            }
                        }

                        // Start background receive transfer
                        let remote_clone = remote.clone();
                        let local_clone = local_final;

                        tokio::spawn(async move {
                            let result = match SshSession::sftp_receive_file(
                                &conn,
                                &remote_clone,
                                &local_clone,
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
            }
        }
        _ => {
            // Use textarea's built-in input handling for all other keys
            if let AppMode::ScpForm { form, .. } = &mut app.mode {
                let textarea = form.focused_textarea_mut();
                textarea.input(Input::from(key));
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_scp_form_dropdown_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::ScpForm { form, dropdown, .. } = &mut app.mode {
        if let Some(dropdown_state) = dropdown {
            match key.code {
                KeyCode::Down | KeyCode::Tab => {
                    dropdown_state.next();
                    return KeyFlow::Continue;
                }
                KeyCode::Up => {
                    dropdown_state.prev();
                    return KeyFlow::Continue;
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    dropdown_state.next();
                    return KeyFlow::Continue;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    dropdown_state.prev();
                    return KeyFlow::Continue;
                }
                KeyCode::Enter => {
                    // Select the current option and apply it to the focused field
                    if let Some(selected_option) = dropdown_state.get_selected().cloned() {
                        if matches!(form.focus, ScpFocusField::LocalPath) {
                            // Construct the complete path by combining current input with selected option
                            let current = form.get_local_path_value();
                            let completed_path =
                                construct_completed_path(&current, &selected_option);
                            form.local_path.delete_line_by_head();
                            form.local_path.insert_str(completed_path);
                        }
                    }
                    *dropdown = None;
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    *dropdown = None;
                    return KeyFlow::Continue;
                }
                _ => {
                    // For other keys, hide dropdown and continue with normal processing
                    *dropdown = None;
                }
            }
        }
    }

    // If no dropdown or dropdown was closed, handle normal SCP form keys
    handle_scp_form_key(app, key).await
}

pub async fn handle_scp_progress_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::ScpProgress {
        current_selected, ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                let current_selected = *current_selected;
                app.info = Some("SCP transfer cancelled".to_string());
                app.go_to_connection_list_with_selected(current_selected);
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

pub async fn handle_delete_confirmation_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::DeleteConfirmation {
        connection_id,
        current_selected,
        ..
    } = &app.mode
    {
        let connection_id = connection_id.clone();
        let current_selected = *current_selected;

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Confirm deletion
                match app.config.remove_connection(&connection_id) {
                    Ok(_) => {
                        if let Err(e) = app.config.save() {
                            app.error = Some(e);
                            app.go_to_connection_list_with_selected(current_selected);
                        } else {
                            app.info = Some("Connection deleted successfully".to_string());
                            let new_len = app.config.connections().len();
                            let new_selected = if new_len == 0 {
                                0
                            } else if current_selected >= new_len {
                                new_len - 1
                            } else {
                                current_selected
                            };
                            app.go_to_connection_list_with_selected(new_selected);
                        }
                    }
                    Err(e) => {
                        app.error = Some(e);
                        app.go_to_connection_list_with_selected(current_selected);
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel deletion
                app.go_to_connection_list_with_selected(current_selected);
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

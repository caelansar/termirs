use std::io::Write;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::Backend;
use tui_textarea::Input;

use super::KeyFlow;
use crate::async_ssh_client::SshSession;
use crate::error::AppError;
use crate::ui::{ScpFocusField, ScpMode};
use crate::{App, AppMode};

use super::autocomplete::{
    autocomplete_local_path, construct_completed_path, list_completion_options,
};

pub async fn handle_scp_form_key<B: Backend + Write>(app: &mut App<B>, key: KeyEvent) -> KeyFlow {
    match key.code {
        KeyCode::Esc => {
            if let AppMode::ScpForm { return_mode, .. } = &app.mode {
                // Return to the appropriate mode
                match return_mode {
                    crate::ScpReturnMode::ConnectionList { current_selected } => {
                        app.go_to_connection_list_with_selected(*current_selected);
                    }
                    crate::ScpReturnMode::Connected {
                        name,
                        client,
                        state,
                        current_selected,
                        cancel_token,
                    } => {
                        app.go_to_connected(
                            name.clone(),
                            client.clone(),
                            state.clone(),
                            *current_selected,
                            cancel_token.clone(),
                        );
                    }
                    crate::ScpReturnMode::FileExplorer { .. } => {
                        // Return to file explorer - restore the entire mode
                        // This is handled by setting app.mode directly
                        // For now, do nothing here as we don't support Esc from FileExplorer yet
                    }
                }
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
                    match autocomplete_local_path(current) {
                        Some(completed) => {
                            if !current.ends_with('/') && completed != current {
                                form.local_path.delete_line_by_head();
                                form.local_path.insert_str(completed);
                            } else {
                                // Show dropdown with available options when no change
                                if let Some(options) = list_completion_options(current)
                                    && options.len() > 1
                                {
                                    *dropdown = Some(crate::ui::DropdownState::new(options));
                                } else {
                                    form.next();
                                }
                            }
                        }
                        None => {
                            if let Some(options) = list_completion_options(current)
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
            let (local, remote, conn_opt, return_mode) = if let AppMode::ScpForm {
                form,
                return_mode,
                ..
            } = &app.mode
            {
                let local = form.get_local_path_value().trim().to_string();
                let remote = form.get_remote_path_value().trim().to_string();
                let current_selected = match return_mode {
                    crate::ScpReturnMode::ConnectionList { current_selected } => *current_selected,
                    crate::ScpReturnMode::Connected {
                        current_selected, ..
                    } => *current_selected,
                    crate::ScpReturnMode::FileExplorer { return_to, .. } => *return_to,
                };
                let conn_opt = app.config.connections().get(current_selected).cloned();

                let return_mode_clone = return_mode.clone_without_channel();

                (local, remote, conn_opt, return_mode_clone)
            } else {
                return KeyFlow::Continue;
            };

            // Now take the channel out with mutable access
            let channel = if let AppMode::ScpForm {
                channel: channel_opt,
                ..
            } = &mut app.mode
            {
                channel_opt.take()
            } else {
                None
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

                // Create channels for communication with background task
                let (result_sender, result_receiver) = tokio::sync::mpsc::channel(1);
                let (progress_sender, progress_receiver) = tokio::sync::mpsc::channel(16);

                // Start SCP transfer and show progress
                let connection_name = conn.display_name.clone();

                let transfer_spec = match mode {
                    ScpMode::Send => {
                        let mut remote_final = remote.clone();
                        let local_path = Path::new(&local);
                        if remote.ends_with('/') {
                            if let Some(filename) = local_path.file_name() {
                                remote_final.push_str(&filename.to_string_lossy());
                            }
                        }

                        let destination_filename = Path::new(&remote_final)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();

                        crate::ScpTransferSpec {
                            mode,
                            local_path: local.clone(),
                            remote_path: remote_final,
                            display_name: destination_filename.clone(),
                            destination_filename,
                        }
                    }
                    ScpMode::Receive => {
                        let mut local_final = local.clone();
                        let remote_path = Path::new(&remote);
                        if local.ends_with('/') {
                            if let Some(filename) = remote_path.file_name() {
                                local_final.push_str(&filename.to_string_lossy());
                            }
                        }

                        let destination_filename = Path::new(&local_final)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();

                        crate::ScpTransferSpec {
                            mode,
                            local_path: local_final,
                            remote_path: remote.clone(),
                            display_name: destination_filename.clone(),
                            destination_filename,
                        }
                    }
                };

                let mut progress = crate::ScpProgress::new(
                    connection_name.clone(),
                    vec![crate::ScpFileProgress::from_spec(&transfer_spec)],
                );

                if matches!(transfer_spec.mode, ScpMode::Send) {
                    if let Ok(metadata) =
                        tokio::fs::metadata(crate::expand_tilde(&transfer_spec.local_path)).await
                    {
                        if let Some(file_progress) = progress.files.get_mut(0) {
                            file_progress.total_bytes = Some(metadata.len());
                        }
                    }
                }

                app.go_to_scp_progress(progress, result_receiver, progress_receiver, return_mode);

                let spec = transfer_spec;
                let progress_tx = progress_sender.clone();
                tokio::spawn(async move {
                    let transfer_result = match spec.mode {
                        ScpMode::Send => {
                            SshSession::sftp_send_file(
                                channel,
                                &conn,
                                &spec.local_path,
                                &spec.remote_path,
                                0,
                                Some(progress_tx.clone()),
                            )
                            .await
                        }
                        ScpMode::Receive => {
                            SshSession::sftp_receive_file(
                                channel,
                                &conn,
                                &spec.remote_path,
                                &spec.local_path,
                                0,
                                Some(progress_tx),
                            )
                            .await
                        }
                    };

                    let (success, error) = match transfer_result {
                        Ok(_) => (true, None),
                        Err(e) => (false, Some(e.to_string())),
                    };

                    let summary = crate::ScpFileResult {
                        mode: spec.mode,
                        local_path: spec.local_path,
                        remote_path: spec.remote_path,
                        destination_filename: spec.destination_filename,
                        success,
                        error,
                        completed_at: Some(std::time::Instant::now()),
                    };

                    let _ = result_sender
                        .send(crate::ScpResult::Completed(vec![summary]))
                        .await;
                });
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
                                construct_completed_path(current, &selected_option);
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
        progress,
        return_mode,
        ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Enter if progress.completed => {
                let mode = return_mode.clone_without_channel();
                let results = progress.completion_results.clone();
                let last_success = progress.last_success_destination.clone();
                restore_after_scp_progress(app, mode, results, last_success).await;
                return KeyFlow::Continue;
            }
            KeyCode::Esc => {
                if progress.completed {
                    let mode = return_mode.clone_without_channel();
                    let results = progress.completion_results.clone();
                    let last_success = progress.last_success_destination.clone();
                    restore_after_scp_progress(app, mode, results, last_success).await;
                } else {
                    let mode = return_mode.clone_without_channel();
                    app.info = Some("SCP transfer cancelled".to_string());
                    restore_after_scp_progress(app, mode, None, None).await;
                }
                return KeyFlow::Continue;
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

async fn restore_after_scp_progress<B: Backend + Write>(
    app: &mut App<B>,
    return_mode: crate::ScpReturnMode,
    results: Option<Vec<crate::ScpFileResult>>,
    last_success: Option<String>,
) {
    match return_mode {
        crate::ScpReturnMode::ConnectionList { current_selected } => {
            app.go_to_connection_list_with_selected(current_selected);
        }
        crate::ScpReturnMode::Connected {
            name,
            client,
            state,
            current_selected,
            cancel_token,
        } => {
            app.go_to_connected(name, client, state, current_selected, cancel_token);
        }
        crate::ScpReturnMode::FileExplorer {
            connection_name,
            mut local_explorer,
            mut remote_explorer,
            active_pane,
            copy_buffer,
            return_to,
            sftp_session,
            ssh_connection,
            channel,
            search_mode,
            search_query,
        } => {
            let any_success = results
                .as_ref()
                .map(|items| items.iter().any(|res| res.success))
                .unwrap_or(false);

            if any_success {
                match active_pane {
                    crate::FileExplorerPane::Local => {
                        let local_cwd = local_explorer.cwd().to_path_buf();
                        if let Err(e) = local_explorer.set_cwd(local_cwd).await {
                            app.set_error(AppError::SftpError(format!(
                                "Failed to refresh local pane: {}",
                                e
                            )));
                        } else if let Some(filename) = last_success.clone() {
                            local_explorer.select_file(&filename);
                        }
                    }
                    crate::FileExplorerPane::Remote => {
                        let remote_cwd = remote_explorer.cwd().to_path_buf();
                        if let Err(e) = remote_explorer.set_cwd(remote_cwd).await {
                            app.set_error(AppError::SftpError(format!(
                                "Failed to refresh remote pane: {}",
                                e
                            )));
                        } else if let Some(filename) = last_success.clone() {
                            remote_explorer.select_file(&filename);
                        }
                    }
                }
            }

            app.mode = crate::AppMode::FileExplorer {
                connection_name,
                local_explorer,
                remote_explorer,
                active_pane,
                copy_buffer,
                return_to,
                sftp_session,
                ssh_connection,
                channel,
                search_mode,
                search_query,
            };
        }
    }
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
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
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

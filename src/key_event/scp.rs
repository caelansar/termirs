use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::error::AppError;
use crate::{App, AppMode};

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

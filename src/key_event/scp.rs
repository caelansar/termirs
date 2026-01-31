use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;
use tracing::{debug, info};

use super::KeyFlow;
use crate::error::AppError;
use crate::{App, AppMode, SearchState};

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
        // Allow closing when fully completed OR when all files have reached 100%
        let can_close = progress.completed || progress.all_files_done_at.is_some();

        match key.code {
            KeyCode::Enter if can_close => {
                info!("SCP transfer completed, returning to file explorer");
                if let Some(mode) = return_mode.take() {
                    let results = progress.completion_results.clone();
                    let last_success = progress.last_success_destination.clone();
                    restore_after_scp_progress(app, mode, results, last_success).await;
                }
                return KeyFlow::Continue;
            }
            KeyCode::Esc => {
                if let Some(mode) = return_mode.take() {
                    if can_close {
                        debug!("Closing completed SCP transfer dialog");
                        let results = progress.completion_results.clone();
                        let last_success = progress.last_success_destination.clone();
                        restore_after_scp_progress(app, mode, results, last_success).await;
                    } else {
                        info!("SCP transfer cancelled by user");
                        app.info = Some("SCP transfer cancelled".to_string());
                        restore_after_scp_progress(app, mode, None, None).await;
                    }
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
    app.stop_ticker();

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
            left_pane,
            mut left_explorer,
            left_sftp,
            mut remote_explorer,
            sftp_session,
            ssh_connection,
            channel,
            active_pane,
            copy_buffer,
            return_to,
            search,
        } => {
            let any_success = results
                .as_ref()
                .map(|items| items.iter().any(|res| res.success))
                .unwrap_or(false);

            if any_success {
                match active_pane {
                    crate::ActivePane::Left => {
                        let left_cwd = left_explorer.cwd().to_path_buf();
                        if let Err(e) = left_explorer.set_cwd(left_cwd).await {
                            app.set_error(AppError::SftpError(format!(
                                "Failed to refresh left pane: {e}"
                            )));
                        } else if let Some(filename) = last_success.as_ref() {
                            left_explorer.select_file(filename);
                        }
                    }
                    crate::ActivePane::Right => {
                        let remote_cwd = remote_explorer.cwd().to_path_buf();
                        if let Err(e) = remote_explorer.set_cwd(remote_cwd).await {
                            app.set_error(AppError::SftpError(format!(
                                "Failed to refresh remote pane: {e}"
                            )));
                        } else if let Some(filename) = last_success.as_ref() {
                            remote_explorer.select_file(filename);
                        }
                    }
                }
            }

            app.mode = crate::AppMode::FileExplorer {
                connection_name,
                left_pane,
                left_explorer,
                left_sftp,
                remote_explorer,
                sftp_session,
                ssh_connection,
                channel,
                active_pane,
                copy_buffer,
                return_to,
                search,
                showing_source_selector: false,
                selector_selected: 0,
                selector_search: SearchState::Off,
                showing_delete_confirmation: false,
                delete_file_name: String::new(),
                delete_pane: crate::ActivePane::Left,
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

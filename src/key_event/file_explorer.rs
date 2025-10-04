//! Key event handling for the file explorer mode.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::backend::Backend;
use std::io::Write;

use super::KeyFlow;
use crate::{App, AppMode, CopyDirection, CopyOperation, FileExplorerPane};

/// Handle key events in file explorer mode
pub async fn handle_file_explorer_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::FileExplorer {
        local_explorer,
        remote_explorer,
        active_pane,
        copy_operation,
        return_to,
        ..
    } = &mut app.mode
    {
        // Handle copy mode cancellation first
        if key.code == KeyCode::Esc && copy_operation.is_some() {
            *copy_operation = None;
            app.mark_redraw();
            return KeyFlow::Continue;
        }

        // Handle quit
        if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
            // Return to connection list
            let return_to_idx = *return_to;
            app.go_to_connection_list_with_selected(return_to_idx);
            return KeyFlow::Continue;
        }

        // Handle navigation and actions based on active pane
        // We need to handle each pane separately due to different types
        match key.code {
            // Navigation: Move selection up
            KeyCode::Char('k') | KeyCode::Up => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::Up).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::Up).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Move selection down
            KeyCode::Char('j') | KeyCode::Down => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::Down).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::Down).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Enter directory
            KeyCode::Right | KeyCode::Enter => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::Right).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::Right).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Go to parent directory
            KeyCode::Left | KeyCode::Backspace => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::Left).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::Left).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            // Switch pane with Tab
            KeyCode::Tab => {
                *active_pane = match active_pane {
                    FileExplorerPane::Local => FileExplorerPane::Remote,
                    FileExplorerPane::Remote => FileExplorerPane::Local,
                };
                app.mark_redraw();
            }

            // Copy file: Enter copy mode
            KeyCode::Char('c') => {
                let (current_file, direction) = match active_pane {
                    FileExplorerPane::Local => {
                        (local_explorer.current(), CopyDirection::LocalToRemote)
                    }
                    FileExplorerPane::Remote => {
                        (remote_explorer.current(), CopyDirection::RemoteToLocal)
                    }
                };

                // Only copy files, not directories
                if !current_file.is_dir() {
                    *copy_operation = Some(CopyOperation {
                        source_path: current_file.path().to_string_lossy().to_string(),
                        source_name: current_file.name().to_string(),
                        direction,
                    });

                    // app.info = Some(format!(
                    //     "Copied {} - Press Tab to switch pane, then 'v' to paste",
                    //     current_file.name()
                    // ));
                } else {
                    app.info = Some("Cannot copy directories (not yet supported)".to_string());
                }
                app.mark_redraw();
            }

            // Paste file: Execute transfer
            KeyCode::Char('v') => {
                if let Some(copy_op) = copy_operation.take() {
                    // Get destination directory
                    let dest_dir = match active_pane {
                        FileExplorerPane::Local => {
                            local_explorer.cwd().to_string_lossy().to_string()
                        }
                        FileExplorerPane::Remote => {
                            remote_explorer.cwd().to_string_lossy().to_string()
                        }
                    };

                    // Check if we're pasting in the same pane as we copied from
                    let same_pane = match (&copy_op.direction, active_pane) {
                        (CopyDirection::LocalToRemote, FileExplorerPane::Local) => true,
                        (CopyDirection::RemoteToLocal, FileExplorerPane::Remote) => true,
                        _ => false,
                    };

                    if same_pane {
                        app.info =
                            Some("Cannot paste in the same pane - press Tab to switch".to_string());
                        // Restore the copy operation
                        *copy_operation = Some(copy_op);
                    } else {
                        // Determine local and remote paths based on direction
                        let (local_path, remote_path, mode) = match copy_op.direction {
                            CopyDirection::LocalToRemote => {
                                let remote_dest = format!(
                                    "{}/{}",
                                    dest_dir.trim_end_matches('/'),
                                    copy_op.source_name
                                );
                                (
                                    copy_op.source_path.clone(),
                                    remote_dest,
                                    crate::ui::ScpMode::Send,
                                )
                            }
                            CopyDirection::RemoteToLocal => {
                                let local_dest = format!(
                                    "{}/{}",
                                    dest_dir.trim_end_matches('/'),
                                    copy_op.source_name
                                );
                                (
                                    local_dest,
                                    copy_op.source_path.clone(),
                                    crate::ui::ScpMode::Receive,
                                )
                            }
                        };

                        // We need to extract the entire FileExplorer state to transition to ScpProgress
                        // Take ownership by replacing app.mode temporarily
                        let old_mode = std::mem::replace(
                            &mut app.mode,
                            AppMode::ConnectionList {
                                selected: 0,
                                search_mode: false,
                                search_input: crate::create_search_textarea(),
                            },
                        );

                        if let AppMode::FileExplorer {
                            connection_name,
                            local_explorer,
                            remote_explorer,
                            active_pane,
                            copy_operation: _,
                            return_to,
                            sftp_session,
                            ssh_connection,
                            channel,
                        } = old_mode
                        {
                            // Create channel for communication with background task
                            let (sender, receiver) = tokio::sync::mpsc::channel(1);

                            // Create progress tracker
                            let progress = crate::ScpProgress::new_with_mode(
                                local_path.clone(),
                                remote_path.clone(),
                                connection_name.clone(),
                                mode,
                            );

                            // Create return mode with file explorer state
                            let return_mode = crate::ScpReturnMode::FileExplorer {
                                connection_name: connection_name.clone(),
                                local_explorer,
                                remote_explorer,
                                active_pane,
                                copy_operation: None, // Clear copy operation after paste
                                return_to,
                                sftp_session,
                                ssh_connection: ssh_connection.clone(),
                                channel: None, // Channel will be consumed by transfer
                            };

                            // Transition to progress mode
                            app.go_to_scp_progress(progress, receiver, return_mode);

                            // Spawn background task to perform the transfer
                            tokio::spawn(async move {
                                let result = match mode {
                                    crate::ui::ScpMode::Send => {
                                        crate::async_ssh_client::SshSession::sftp_send_file(
                                            channel,
                                            &ssh_connection,
                                            &local_path,
                                            &remote_path,
                                        )
                                        .await
                                    }
                                    crate::ui::ScpMode::Receive => {
                                        crate::async_ssh_client::SshSession::sftp_receive_file(
                                            channel,
                                            &ssh_connection,
                                            &remote_path,
                                            &local_path,
                                        )
                                        .await
                                    }
                                };

                                let scp_result = match result {
                                    Ok(_) => crate::ScpResult::Success {
                                        mode,
                                        local_path,
                                        remote_path,
                                    },
                                    Err(e) => crate::ScpResult::Error {
                                        error: e.to_string(),
                                    },
                                };

                                let _ = sender.send(scp_result).await;
                            });
                        }
                    }
                } else {
                    app.info = Some("No file in copy mode - press 'c' on a file first".to_string());
                }
                app.mark_redraw();
            }

            // Toggle hidden files
            KeyCode::Char('h') => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        let show_hidden = !local_explorer.show_hidden();
                        local_explorer
                            .set_show_hidden(show_hidden)
                            .await
                            .map(|_| show_hidden)
                    }
                    FileExplorerPane::Remote => {
                        let show_hidden = !remote_explorer.show_hidden();
                        remote_explorer
                            .set_show_hidden(show_hidden)
                            .await
                            .map(|_| show_hidden)
                    }
                };

                match result {
                    Ok(_show_hidden) => {
                        // app.info = Some(format!(
                        //     "Hidden files {}",
                        //     if show_hidden { "shown" } else { "hidden" }
                        // ));
                    }
                    Err(e) => {
                        app.error = Some(crate::error::AppError::SftpError(format!(
                            "Failed to toggle hidden files: {}",
                            e
                        )));
                    }
                }
                app.mark_redraw();
            }

            // Refresh current pane
            KeyCode::Char('r') => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        let current_path = local_explorer.cwd().to_string_lossy().to_string();
                        local_explorer.set_cwd(&current_path).await
                    }
                    FileExplorerPane::Remote => {
                        let current_path = remote_explorer.cwd().to_string_lossy().to_string();
                        remote_explorer.set_cwd(&current_path).await
                    }
                };

                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Failed to refresh: {}",
                        e
                    )));
                } else {
                    app.info = Some("Refreshed".to_string());
                }
                app.mark_redraw();
            }

            // Home/End for quick navigation
            KeyCode::Home => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::Home).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::Home).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            KeyCode::End => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::End).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer.handle(ratatui_explorer::Input::End).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            // Page up/down for faster scrolling
            KeyCode::PageUp => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer.handle(ratatui_explorer::Input::PageUp).await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer
                            .handle(ratatui_explorer::Input::PageUp)
                            .await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            KeyCode::PageDown => {
                let result = match active_pane {
                    FileExplorerPane::Local => {
                        local_explorer
                            .handle(ratatui_explorer::Input::PageDown)
                            .await
                    }
                    FileExplorerPane::Remote => {
                        remote_explorer
                            .handle(ratatui_explorer::Input::PageDown)
                            .await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                app.mark_redraw();
            }

            _ => {}
        }

        KeyFlow::Continue
    } else {
        KeyFlow::Continue
    }
}

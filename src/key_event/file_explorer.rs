//! Key event handling for the file explorer mode.

use crossterm::event::{KeyCode, KeyEvent};
use futures::future::join_all;
use ratatui::backend::Backend;
use std::io::Write;

use super::KeyFlow;
use crate::{
    ActivePane, App, AppMode, CopyDirection, CopyOperation, FileExplorerPane,
    ui::file_explorer::filter_connection_indices,
};

/// Handle key events in file explorer mode
pub async fn handle_file_explorer_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Handle source selector popup first (needs separate scope to avoid borrow issues)
    if let AppMode::FileExplorer {
        showing_source_selector,
        selector_selected,
        selector_search_mode,
        selector_search_query,
        ssh_connection,
        ..
    } = &mut app.mode
    {
        if *showing_source_selector {
            let local_offset = 1;
            let mut filtered_indices = filter_connection_indices(
                app.config.connections(),
                Some(ssh_connection.id.as_str()),
                selector_search_query.as_str(),
            );
            let mut total_items = filtered_indices.len() + local_offset;
            if *selector_selected >= total_items {
                *selector_selected = total_items.saturating_sub(1);
            }

            if *selector_search_mode {
                match key.code {
                    KeyCode::Char(c) => {
                        selector_search_query.push(c);
                        filtered_indices = filter_connection_indices(
                            app.config.connections(),
                            Some(ssh_connection.id.as_str()),
                            selector_search_query.as_str(),
                        );
                        total_items = filtered_indices.len() + local_offset;
                        if selector_search_query.is_empty() {
                            *selector_selected =
                                (*selector_selected).min(total_items.saturating_sub(1));
                        } else if filtered_indices.is_empty() {
                            *selector_selected = 0;
                        } else {
                            *selector_selected = local_offset;
                        }
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }
                    KeyCode::Backspace => {
                        selector_search_query.pop();
                        filtered_indices = filter_connection_indices(
                            app.config.connections(),
                            Some(ssh_connection.id.as_str()),
                            selector_search_query.as_str(),
                        );
                        total_items = filtered_indices.len() + local_offset;
                        if selector_search_query.is_empty() {
                            *selector_selected =
                                (*selector_selected).min(total_items.saturating_sub(1));
                        } else if filtered_indices.is_empty() {
                            *selector_selected = 0;
                        } else {
                            *selector_selected = local_offset;
                        }
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }
                    KeyCode::Esc => {
                        if !selector_search_query.is_empty() {
                            selector_search_query.clear();
                            filtered_indices = filter_connection_indices(
                                app.config.connections(),
                                Some(ssh_connection.id.as_str()),
                                selector_search_query.as_str(),
                            );
                            total_items = filtered_indices.len() + local_offset;
                            *selector_selected =
                                (*selector_selected).min(total_items.saturating_sub(1));
                        } else {
                            *selector_search_mode = false;
                        }
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }
                    _ => {}
                }
            }

            match key.code {
                KeyCode::Char('/') => {
                    *selector_search_mode = true;
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if *selector_selected > 0 {
                        *selector_selected -= 1;
                    } else {
                        *selector_selected = total_items.saturating_sub(1);
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *selector_selected < total_items.saturating_sub(1) {
                        *selector_selected += 1;
                    } else {
                        *selector_selected = 0;
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Enter => {
                    let selected_idx = (*selector_selected).min(total_items.saturating_sub(1));
                    let selection = if selected_idx < local_offset {
                        None
                    } else {
                        filtered_indices
                            .get(selected_idx - local_offset)
                            .and_then(|conn_idx| app.config.connections().get(*conn_idx).cloned())
                    };

                    *showing_source_selector = false;
                    *selector_search_mode = false;
                    selector_search_query.clear();

                    if let Some(conn) = selection {
                        app.switch_left_pane_to_ssh(conn).await;
                    } else {
                        app.switch_left_pane_to_local().await;
                    }

                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    *showing_source_selector = false;
                    *selector_search_mode = false;
                    selector_search_query.clear();
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => return KeyFlow::Continue,
            }
        }
    }

    // Main file explorer handling
    if let AppMode::FileExplorer {
        left_pane,
        left_explorer,
        remote_explorer,
        active_pane,
        copy_buffer,
        return_to,
        search_mode,
        search_query,
        showing_source_selector,
        selector_selected,
        selector_search_mode,
        selector_search_query,
        ssh_connection,
        ..
    } = &mut app.mode
    {
        // Handle search mode first
        if *search_mode {
            match key.code {
                KeyCode::Char(c) => {
                    // Append character to search query
                    search_query.push(c);

                    // Apply filter and select first match
                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(Some(search_query.clone()));
                            // Select first match if available
                            if !left_explorer.files().is_empty() {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(Some(search_query.clone()));
                            // Select first match if available
                            if !remote_explorer.files().is_empty() {
                                remote_explorer.set_selected_idx(0);
                            }
                        }
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Backspace => {
                    // Remove last character from search query
                    search_query.pop();

                    // Apply updated filter
                    let filter = if search_query.is_empty() {
                        None
                    } else {
                        Some(search_query.clone())
                    };

                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(filter);
                            if !search_query.is_empty() && !left_explorer.files().is_empty() {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(filter);
                            if !search_query.is_empty() && !remote_explorer.files().is_empty() {
                                remote_explorer.set_selected_idx(0);
                            }
                        }
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Enter | KeyCode::Esc => {
                    // Exit search mode
                    *search_mode = false;
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => {
                    return KeyFlow::Continue;
                }
            }
        }

        // Handle copy mode cancellation first
        if key.code == KeyCode::Esc && !copy_buffer.is_empty() {
            copy_buffer.clear();
            // app.info = Some("Cleared selected files".to_string());
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Up).await,
                    ActivePane::Right => remote_explorer.handle(ratatui_explorer::Input::Up).await,
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Down).await,
                    ActivePane::Right => {
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Right).await,
                    ActivePane::Right => {
                        remote_explorer.handle(ratatui_explorer::Input::Right).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                // Clear search filter when changing directories
                search_query.clear();
                *search_mode = false;
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            // Navigation: Go to parent directory
            KeyCode::Left | KeyCode::Backspace => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Left).await,
                    ActivePane::Right => {
                        remote_explorer.handle(ratatui_explorer::Input::Left).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {}",
                        e
                    )));
                }
                // Clear search filter when changing directories
                search_query.clear();
                *search_mode = false;
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            // Switch pane with Tab
            KeyCode::Tab => {
                *active_pane = match active_pane {
                    ActivePane::Left => ActivePane::Right,
                    ActivePane::Right => ActivePane::Left,
                };
                app.mark_redraw();
            }

            // Switch left pane source with 's'
            KeyCode::Char('s') => {
                if matches!(active_pane, ActivePane::Left) {
                    *showing_source_selector = true;
                    *selector_search_mode = false;
                    selector_search_query.clear();
                    // Reset selector to current source
                    let base_indices = filter_connection_indices(
                        app.config.connections(),
                        Some(ssh_connection.id.as_str()),
                        "",
                    );
                    *selector_selected = match left_pane {
                        FileExplorerPane::Local => 0,
                        FileExplorerPane::RemoteSsh { connection, .. } => {
                            let connections = app.config.connections();
                            base_indices
                                .iter()
                                .position(|idx| {
                                    connections
                                        .get(*idx)
                                        .map(|conn| conn.id == connection.id)
                                        .unwrap_or(false)
                                })
                                .map(|idx| idx + 1)
                                .unwrap_or(0)
                        }
                    };
                    app.mark_redraw();
                }
            }

            // Copy file: Toggle selection
            KeyCode::Char('c') => {
                let (current_file, direction) = match active_pane {
                    ActivePane::Left => (left_explorer.current(), CopyDirection::LeftToRight),
                    ActivePane::Right => (remote_explorer.current(), CopyDirection::RightToLeft),
                };

                if current_file.is_dir() {
                    app.info = Some("Cannot copy directories (not yet supported)".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                let source_path = current_file.path().to_string_lossy().to_string();
                if let Some(existing_idx) = copy_buffer
                    .iter()
                    .position(|item| item.source_path == source_path)
                {
                    copy_buffer.remove(existing_idx);
                    // if copy_buffer.is_empty() {
                    //     app.info = Some(format!("Removed {} from selection", current_file.name()));
                    // } else {
                    //     app.info = Some(format!(
                    //         "Removed {} ({} remaining)",
                    //         current_file.name(),
                    //         copy_buffer.len()
                    //     ));
                    // }
                } else {
                    if let Some(existing_direction) = copy_buffer.first().map(|item| item.direction)
                    {
                        if existing_direction != direction {
                            app.info = Some(
                                "Selected files must all come from the same source pane"
                                    .to_string(),
                            );
                            app.mark_redraw();
                            return KeyFlow::Continue;
                        }
                    }

                    copy_buffer.push(CopyOperation {
                        source_path,
                        source_name: current_file.name().to_string(),
                        direction,
                    });

                    // let count = copy_buffer.len();
                    // let count_msg = if count == 1 {
                    //     "1 file selected".to_string()
                    // } else {
                    //     format!("{count} files selected")
                    // };
                    // app.info = Some(format!(
                    //     "{} - Press Tab to switch pane, then 'v' to paste",
                    //     count_msg
                    // ));
                }
                app.mark_redraw();
            }

            // Paste file: Execute transfer (batch aware)
            KeyCode::Char('v') => {
                if copy_buffer.is_empty() {
                    app.info =
                        Some("No files selected - press 'c' on files to add them".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                let dest_dir = match active_pane {
                    ActivePane::Left => left_explorer.cwd().to_string_lossy().to_string(),
                    ActivePane::Right => remote_explorer.cwd().to_string_lossy().to_string(),
                };

                let direction = copy_buffer[0].direction;
                let same_pane = match (direction, active_pane) {
                    (CopyDirection::LeftToRight, ActivePane::Left) => true,
                    (CopyDirection::RightToLeft, ActivePane::Right) => true,
                    _ => false,
                };

                if same_pane {
                    app.info =
                        Some("Cannot paste in the same pane - press Tab to switch".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                // Determine transfer mode based on pane types
                let (is_ssh_to_ssh, mode) = match direction {
                    CopyDirection::LeftToRight => {
                        // Source is left pane, dest is right pane (always SSH)
                        let is_ssh_to_ssh = matches!(left_pane, FileExplorerPane::RemoteSsh { .. });
                        (is_ssh_to_ssh, crate::ui::ScpMode::Send)
                    }
                    CopyDirection::RightToLeft => {
                        // Source is right pane (always SSH), dest is left pane
                        let is_ssh_to_ssh = matches!(left_pane, FileExplorerPane::RemoteSsh { .. });
                        (is_ssh_to_ssh, crate::ui::ScpMode::Receive)
                    }
                };

                let mut transfer_specs = Vec::with_capacity(copy_buffer.len());
                for item in copy_buffer.iter() {
                    let (local_path, remote_path, display_name, destination_filename) =
                        match direction {
                            CopyDirection::LeftToRight => {
                                let remote_dest = format!(
                                    "{}/{}",
                                    dest_dir.trim_end_matches('/'),
                                    item.source_name
                                );
                                (
                                    item.source_path.clone(),
                                    remote_dest.clone(),
                                    item.source_name.clone(),
                                    std::path::Path::new(&remote_dest)
                                        .file_name()
                                        .map(|f| f.to_string_lossy().to_string())
                                        .unwrap_or_else(|| item.source_name.clone()),
                                )
                            }
                            CopyDirection::RightToLeft => {
                                let local_dest = format!(
                                    "{}/{}",
                                    dest_dir.trim_end_matches('/'),
                                    item.source_name
                                );
                                (
                                    local_dest.clone(),
                                    item.source_path.clone(),
                                    item.source_name.clone(),
                                    std::path::Path::new(&local_dest)
                                        .file_name()
                                        .map(|f| f.to_string_lossy().to_string())
                                        .unwrap_or_else(|| item.source_name.clone()),
                                )
                            }
                        };

                    transfer_specs.push(crate::ScpTransferSpec {
                        mode,
                        local_path,
                        remote_path,
                        display_name,
                        destination_filename,
                        is_ssh_to_ssh,
                    });
                }

                copy_buffer.clear();

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
                    left_pane,
                    left_explorer,
                    left_sftp,
                    remote_explorer,
                    sftp_session,
                    ssh_connection,
                    channel: _channel,
                    active_pane,
                    copy_buffer: _,
                    return_to,
                    search_mode,
                    search_query,
                    ..
                } = old_mode
                {
                    let (result_sender, result_receiver) = tokio::sync::mpsc::channel(1);
                    let (progress_sender, progress_receiver) = tokio::sync::mpsc::channel(64);

                    let progress_items: Vec<crate::ScpFileProgress> = transfer_specs
                        .iter()
                        .map(crate::ScpFileProgress::from_spec)
                        .collect();
                    let mut progress =
                        crate::ScpProgress::new(connection_name.clone(), progress_items);

                    for (idx, spec) in transfer_specs.iter().enumerate() {
                        if matches!(spec.mode, crate::ui::ScpMode::Send) {
                            if let Ok(metadata) =
                                tokio::fs::metadata(crate::expand_tilde(&spec.local_path)).await
                            {
                                if let Some(file_progress) = progress.files.get_mut(idx) {
                                    file_progress.total_bytes = Some(metadata.len());
                                }
                            }
                        }
                    }

                    // Clone left_sftp if it exists for SSH-to-SSH transfers (before moving into return_mode)
                    let left_sftp_for_transfer = left_sftp
                        .as_ref()
                        .map(|(session, conn)| (session.clone(), conn.clone()));

                    let return_mode = crate::ScpReturnMode::FileExplorer {
                        connection_name: connection_name.clone(),
                        left_pane,
                        left_explorer,
                        left_sftp,
                        remote_explorer,
                        sftp_session,
                        ssh_connection: ssh_connection.clone(),
                        channel: None,
                        active_pane,
                        copy_buffer: Vec::new(),
                        return_to,
                        search_mode,
                        search_query,
                    };

                    app.go_to_scp_progress(
                        progress,
                        result_receiver,
                        progress_receiver,
                        return_mode,
                    );

                    tokio::spawn(async move {
                        let total = transfer_specs.len();
                        let mut tasks = Vec::with_capacity(total);

                        for (index, spec) in transfer_specs.into_iter().enumerate() {
                            let ssh_connection = ssh_connection.clone();
                            let progress_tx = progress_sender.clone();
                            let left_sftp_clone = left_sftp_for_transfer.clone();

                            tasks.push(tokio::spawn(async move {
                                let transfer_result = if spec.is_ssh_to_ssh {
                                    // SSH-to-SSH transfer
                                    match spec.mode {
                                        crate::ui::ScpMode::Send => {
                                            // Left SSH → Right SSH
                                            if let Some((left_sftp_session, _left_conn)) = left_sftp_clone {
                                                match crate::filesystem::sftp_file::open_for_read(left_sftp_session.clone(), &spec.local_path).await {
                                                    Ok(sftp_file) => {
                                                        crate::async_ssh_client::SshSession::sftp_send_file_with_timeout(
                                                            None,
                                                            &ssh_connection,
                                                            sftp_file,
                                                            &spec.remote_path,
                                                            None,
                                                            &tokio_util::sync::CancellationToken::new(),
                                                            index,
                                                            Some(progress_tx.clone()),
                                                        )
                                                        .await
                                                    }
                                                    Err(e) => Err(e),
                                                }
                                            } else {
                                                Err(crate::error::AppError::SftpError(
                                                    "SSH-to-SSH Send failed: left SFTP session not available".to_string()
                                                ))
                                            }
                                        }
                                        crate::ui::ScpMode::Receive => {
                                            // Right SSH → Left SSH
                                            if let Some((left_sftp_session, _left_conn)) = left_sftp_clone {
                                                match crate::filesystem::sftp_file::open_for_write(left_sftp_session.clone(), &spec.local_path).await {
                                                    Ok(sftp_file) => {
                                                        crate::async_ssh_client::SshSession::sftp_receive_file_with_timeout(
                                                            None,
                                                            &ssh_connection,
                                                            &spec.remote_path,
                                                            sftp_file,
                                                            None,
                                                            &tokio_util::sync::CancellationToken::new(),
                                                            index,
                                                            Some(progress_tx.clone()),
                                                        )
                                                        .await
                                                    }
                                                    Err(e) => Err(e),
                                                }
                                            } else {
                                                Err(crate::error::AppError::SftpError(
                                                    "SSH-to-SSH Receive failed: left SFTP session not available".to_string()
                                                ))
                                            }
                                        }
                                    }
                                } else {
                                    // Regular local-to-SSH or SSH-to-local transfer
                                    match spec.mode {
                                        crate::ui::ScpMode::Send => {
                                            crate::async_ssh_client::SshSession::sftp_send_file(
                                                None,
                                                &ssh_connection,
                                                &spec.local_path,
                                                &spec.remote_path,
                                                index,
                                                Some(progress_tx.clone()),
                                            )
                                            .await
                                        }
                                        crate::ui::ScpMode::Receive => {
                                            crate::async_ssh_client::SshSession::sftp_receive_file(
                                                None,
                                                &ssh_connection,
                                                &spec.remote_path,
                                                &spec.local_path,
                                                index,
                                                Some(progress_tx),
                                            )
                                            .await
                                        }
                                    }
                                };

                                let success = transfer_result.is_ok();
                                let error = transfer_result.err().map(|e| e.to_string());

                                crate::ScpFileResult {
                                    mode: spec.mode,
                                    local_path: spec.local_path,
                                    remote_path: spec.remote_path,
                                    destination_filename: spec.destination_filename,
                                    success,
                                    error,
                                    completed_at: Some(std::time::Instant::now()),
                                }
                            }));
                        }

                        drop(progress_sender);

                        let joined = join_all(tasks).await;
                        let mut results = Vec::with_capacity(joined.len());

                        for outcome in joined {
                            match outcome {
                                Ok(file_result) => results.push(file_result),
                                Err(e) => {
                                    let _ = result_sender
                                        .send(crate::ScpResult::Error {
                                            error: format!("Transfer task failed to complete: {e}"),
                                        })
                                        .await;
                                    return;
                                }
                            }
                        }

                        let _ = result_sender
                            .send(crate::ScpResult::Completed(results))
                            .await;
                    });
                }
            }

            // Toggle hidden files
            KeyCode::Char('h') => {
                let result = match active_pane {
                    ActivePane::Left => {
                        let show_hidden = !left_explorer.show_hidden();
                        left_explorer
                            .set_show_hidden(show_hidden)
                            .await
                            .map(|_| show_hidden)
                    }
                    ActivePane::Right => {
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
                    ActivePane::Left => {
                        let current_path = left_explorer.cwd().to_path_buf();
                        left_explorer.set_cwd(current_path).await
                    }
                    ActivePane::Right => {
                        let current_path = remote_explorer.cwd().to_path_buf();
                        remote_explorer.set_cwd(current_path).await
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Home).await,
                    ActivePane::Right => {
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::End).await,
                    ActivePane::Right => remote_explorer.handle(ratatui_explorer::Input::End).await,
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
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::PageUp).await,
                    ActivePane::Right => {
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
                    ActivePane::Left => {
                        left_explorer
                            .handle(ratatui_explorer::Input::PageDown)
                            .await
                    }
                    ActivePane::Right => {
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

            // Enter search mode with '/'
            KeyCode::Char('/') => {
                *search_mode = true;
                search_query.clear();
                // Clear any existing filters to show all items
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            _ => {}
        }

        KeyFlow::Continue
    } else {
        KeyFlow::Continue
    }
}

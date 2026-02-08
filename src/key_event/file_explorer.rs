//! Key event handling for the file explorer mode.

use crossterm::event::{KeyCode, KeyEvent};
use futures::future::join_all;
use ratatui::backend::Backend;
use std::io::Write;
use tracing::{debug, error, info};

use super::KeyFlow;
use crate::AppEvent;
use crate::async_ssh_client::HostFile;
use crate::ui::file_explorer::filter_connection_indices;
use crate::{
    ActivePane, App, AppMode, CopyDirection, CopyOperation, FileExplorerPane, ScpFileProgress,
    ScpFileResult, ScpProgress, ScpResult, ScpTransferSpec,
};

/// Handle key events in file explorer mode
pub async fn handle_file_explorer_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Handle delete confirmation popup first (highest priority)
    if let AppMode::FileExplorer {
        delete_confirmation,
        left_explorer,
        remote_explorer,
        ..
    } = &mut app.mode
        && delete_confirmation.showing
    {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                // Perform the deletion
                debug!("User confirmed file deletion");
                let result = match delete_confirmation.pane {
                    ActivePane::Left => {
                        info!("Deleting file from left pane");
                        left_explorer.handle(ratatui_explorer::Input::Delete).await
                    }
                    ActivePane::Right => {
                        info!("Deleting file from right pane");
                        remote_explorer
                            .handle(ratatui_explorer::Input::Delete)
                            .await
                    }
                };

                if let Err(e) = result {
                    error!("File deletion failed: {}", e);
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Delete error: {e}"
                    )));
                } else {
                    info!("File deletion completed successfully");
                }

                // Close the confirmation dialog
                delete_confirmation.hide();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel deletion
                delete_confirmation.hide();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            _ => {
                // Ignore other keys when dialog is showing
                return KeyFlow::Continue;
            }
        }
    }

    // Handle source selector popup first (needs separate scope to avoid borrow issues)
    if let AppMode::FileExplorer {
        source_selector,
        ssh_connection,
        ..
    } = &mut app.mode
        && source_selector.showing
    {
        let local_offset = 1;
        let mut filtered_indices = filter_connection_indices(
            app.config.connections(),
            Some(ssh_connection.id.as_str()),
            source_selector.search.query(),
        );
        let mut total_items = filtered_indices.len() + local_offset;
        if source_selector.selected >= total_items {
            source_selector.selected = total_items.saturating_sub(1);
        }

        if source_selector.search.is_on() {
            match key.code {
                KeyCode::Char(c) => {
                    if let Some(query) = source_selector.search.query_mut() {
                        query.push(c);
                    }
                    filtered_indices = filter_connection_indices(
                        app.config.connections(),
                        Some(ssh_connection.id.as_str()),
                        source_selector.search.query(),
                    );
                    total_items = filtered_indices.len() + local_offset;
                    if source_selector.search.query().is_empty() {
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else if filtered_indices.is_empty() {
                        source_selector.selected = 0;
                    } else {
                        source_selector.selected = local_offset;
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Backspace => {
                    if let Some(query) = source_selector.search.query_mut() {
                        query.pop();
                    }
                    filtered_indices = filter_connection_indices(
                        app.config.connections(),
                        Some(ssh_connection.id.as_str()),
                        source_selector.search.query(),
                    );
                    total_items = filtered_indices.len() + local_offset;
                    if source_selector.search.query().is_empty() {
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else if filtered_indices.is_empty() {
                        source_selector.selected = 0;
                    } else {
                        source_selector.selected = local_offset;
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    if !source_selector.search.query().is_empty() {
                        source_selector.search.clear_query();
                        filtered_indices = filter_connection_indices(
                            app.config.connections(),
                            Some(ssh_connection.id.as_str()),
                            source_selector.search.query(),
                        );
                        total_items = filtered_indices.len() + local_offset;
                        source_selector.selected =
                            source_selector.selected.min(total_items.saturating_sub(1));
                    } else {
                        source_selector.search.deactivate();
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('/') => {
                source_selector.search.activate();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if source_selector.selected > 0 {
                    source_selector.selected -= 1;
                } else {
                    source_selector.selected = total_items.saturating_sub(1);
                }
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if source_selector.selected < total_items.saturating_sub(1) {
                    source_selector.selected += 1;
                } else {
                    source_selector.selected = 0;
                }
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Enter => {
                let selected_idx = source_selector.selected.min(total_items.saturating_sub(1));
                let selection = if selected_idx < local_offset {
                    None
                } else {
                    filtered_indices
                        .get(selected_idx - local_offset)
                        .and_then(|conn_idx| app.config.connections().get(*conn_idx).cloned())
                };

                source_selector.hide();
                source_selector.search.deactivate();

                if let Some(conn) = selection {
                    app.switch_left_pane_to_ssh(conn).await;
                } else {
                    app.switch_left_pane_to_local().await;
                }

                app.mark_redraw();
                return KeyFlow::Continue;
            }
            KeyCode::Esc => {
                source_selector.hide();
                source_selector.search.deactivate();
                app.mark_redraw();
                return KeyFlow::Continue;
            }
            _ => return KeyFlow::Continue,
        }
    }

    // Main file explorer handling
    if let AppMode::FileExplorer {
        left_pane,
        left_explorer,
        left_sftp,
        remote_explorer,
        sftp_session,
        active_pane,
        copy_buffer,
        return_to,
        search,
        source_selector,
        ssh_connection,
        delete_confirmation,
        ..
    } = &mut app.mode
    {
        // Handle search mode first
        if search.is_on() {
            match key.code {
                KeyCode::Char(c) => {
                    // Append character to search query
                    if let Some(query) = search.query_mut() {
                        query.push(c);
                    }

                    // Apply filter and select first match
                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(Some(search.query().to_string()));
                            // Select first match if available
                            if !left_explorer.files().is_empty() {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(Some(search.query().to_string()));
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
                    if let Some(query) = search.query_mut() {
                        query.pop();
                    }

                    // Apply updated filter
                    let filter = if search.query().is_empty() {
                        None
                    } else {
                        Some(search.query().to_string())
                    };

                    match active_pane {
                        ActivePane::Left => {
                            left_explorer.set_search_filter(filter);
                            if !search.query().is_empty() && !left_explorer.files().is_empty() {
                                left_explorer.set_selected_idx(0);
                            }
                        }
                        ActivePane::Right => {
                            remote_explorer.set_search_filter(filter);
                            if !search.query().is_empty() && !remote_explorer.files().is_empty() {
                                remote_explorer.set_selected_idx(0);
                            }
                        }
                    }
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Enter => {
                    // Apply search filter
                    search.apply();
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                KeyCode::Esc => {
                    // Exit search mode
                    search.deactivate();
                    // Clear the filter when exiting
                    left_explorer.set_search_filter(None);
                    remote_explorer.set_search_filter(None);
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }
                _ => {
                    return KeyFlow::Continue;
                }
            }
        }

        // Handle Esc when search filter is applied (but not actively editing)
        if matches!(search, crate::SearchState::Applied { .. }) && key.code == KeyCode::Esc {
            search.deactivate();
            // Clear the filter
            left_explorer.set_search_filter(None);
            remote_explorer.set_search_filter(None);
            app.mark_redraw();
            return KeyFlow::Continue;
        }

        // Handle copy mode cancellation first
        if key.code == KeyCode::Esc && !copy_buffer.is_empty() {
            copy_buffer.clear();
            // app.info = Some("Cleared selected files".to_string());
            app.mark_redraw();
            return KeyFlow::Continue;
        }

        // Handle quit
        if key.code == KeyCode::Char('q') {
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
                        "Navigation error: {e}"
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
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Navigation: Enter directory
            KeyCode::Right | KeyCode::Enter | KeyCode::Char('l') => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Right).await,
                    ActivePane::Right => {
                        remote_explorer.handle(ratatui_explorer::Input::Right).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                // Clear search filter when changing directories
                search.deactivate();
                left_explorer.set_search_filter(None);
                remote_explorer.set_search_filter(None);
                app.mark_redraw();
            }

            // Navigation: Go to parent directory
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                let result = match active_pane {
                    ActivePane::Left => left_explorer.handle(ratatui_explorer::Input::Left).await,
                    ActivePane::Right => {
                        remote_explorer.handle(ratatui_explorer::Input::Left).await
                    }
                };
                if let Err(e) = result {
                    app.error = Some(crate::error::AppError::SftpError(format!(
                        "Navigation error: {e}"
                    )));
                }
                // Clear search filter when changing directories
                search.deactivate();
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
                    source_selector.show();
                    source_selector.search.deactivate();
                    // Reset selector to current source
                    let base_indices = filter_connection_indices(
                        app.config.connections(),
                        Some(ssh_connection.id.as_str()),
                        "",
                    );
                    source_selector.selected = match left_pane {
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

            // Delete file: Show delete confirmation
            KeyCode::Char('d') => {
                let (current_file, pane) = match active_pane {
                    ActivePane::Left => (left_explorer.current(), ActivePane::Left),
                    ActivePane::Right => (remote_explorer.current(), ActivePane::Right),
                };

                // Skip delete confirmation for directories
                if current_file.is_dir() {
                    app.info = Some("Cannot delete directories".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                // Show delete confirmation dialog
                delete_confirmation.show(current_file.name().to_string(), pane);
                app.mark_redraw();
            }

            // Copy file: Toggle selection
            KeyCode::Char('c') => {
                let (current_file, direction) = match active_pane {
                    ActivePane::Left => (left_explorer.current(), CopyDirection::LeftToRight),
                    ActivePane::Right => (remote_explorer.current(), CopyDirection::RightToLeft),
                };

                let is_dir = current_file.is_dir();
                let source_path = current_file.path().to_string_lossy().into_owned();
                if let Some(existing_idx) = copy_buffer
                    .iter()
                    .position(|item| item.source_path == source_path)
                {
                    copy_buffer.remove(existing_idx);
                } else {
                    if let Some(existing_direction) = copy_buffer.first().map(|item| item.direction)
                        && existing_direction != direction
                    {
                        app.info = Some(
                            "Selected files must all come from the same source pane".to_string(),
                        );
                        app.mark_redraw();
                        return KeyFlow::Continue;
                    }

                    info!(
                        "File added to copy buffer: {} (direction: {:?})",
                        source_path, direction
                    );
                    copy_buffer.push(CopyOperation {
                        source_path,
                        source_name: current_file.name().to_string(),
                        direction,
                        is_dir,
                    });
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

                info!(
                    "Initiating file paste operation with {} files",
                    copy_buffer.len()
                );
                let dest_dir = match active_pane {
                    ActivePane::Left => left_explorer.cwd().to_string_lossy().into_owned(),
                    ActivePane::Right => remote_explorer.cwd().to_string_lossy().into_owned(),
                };

                let direction = copy_buffer[0].direction;
                let same_pane = matches!(
                    (direction, active_pane),
                    (CopyDirection::LeftToRight, ActivePane::Left)
                        | (CopyDirection::RightToLeft, ActivePane::Right)
                );

                if same_pane {
                    debug!("Cannot paste in same pane");
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

                let mut transfer_specs = Vec::new();
                let mut all_dirs_to_create: Vec<String> = Vec::new();

                for item in copy_buffer.iter() {
                    let source_name = item.source_name.trim_end_matches('/');

                    if item.is_dir {
                        // Directory: enumerate recursively to build manifest
                        let dest_dir_path =
                            format!("{}/{}", dest_dir.trim_end_matches('/'), source_name);

                        let manifest_result = match direction {
                            CopyDirection::LeftToRight => {
                                // Source is left pane
                                match left_pane {
                                    FileExplorerPane::Local => {
                                        crate::filesystem::dir_walker::walk_local_dir(
                                            &item.source_path,
                                            &dest_dir_path,
                                        )
                                        .await
                                    }
                                    FileExplorerPane::RemoteSsh { .. } => {
                                        if let Some((left_session, _)) = left_sftp {
                                            let sftp_fs = crate::filesystem::SftpFileSystem::new(
                                                left_session.clone(),
                                            );
                                            crate::filesystem::dir_walker::walk_remote_dir(
                                                &sftp_fs,
                                                &item.source_path,
                                                &dest_dir_path,
                                            )
                                            .await
                                        } else {
                                            Err(std::io::Error::other(
                                                "Left SFTP session not available",
                                            ))
                                        }
                                    }
                                }
                            }
                            CopyDirection::RightToLeft => {
                                // Source is right pane (always SSH)
                                let sftp_fs =
                                    crate::filesystem::SftpFileSystem::new(sftp_session.clone());
                                crate::filesystem::dir_walker::walk_remote_dir(
                                    &sftp_fs,
                                    &item.source_path,
                                    &dest_dir_path,
                                )
                                .await
                            }
                        };

                        match manifest_result {
                            Ok(manifest) => {
                                all_dirs_to_create.extend(manifest.directories);
                                for (src, dst, _size) in manifest.files {
                                    let (local_path, remote_path) = match direction {
                                        CopyDirection::LeftToRight => (src, dst),
                                        CopyDirection::RightToLeft => (dst, src),
                                    };
                                    let display_name = local_path
                                        .rsplit_once('/')
                                        .map(|(_, name)| name.to_string())
                                        .unwrap_or_else(|| local_path.clone());
                                    let destination_filename = display_name.clone();

                                    transfer_specs.push(ScpTransferSpec {
                                        mode,
                                        local_path,
                                        remote_path,
                                        display_name,
                                        destination_filename,
                                        is_ssh_to_ssh,
                                    });
                                }
                            }
                            Err(e) => {
                                app.error = Some(crate::error::AppError::SftpError(format!(
                                    "Failed to enumerate directory '{}': {e}",
                                    source_name
                                )));
                                app.mark_redraw();
                                return KeyFlow::Continue;
                            }
                        }
                    } else {
                        // Regular file: existing logic
                        let (local_path, remote_path, display_name, destination_filename) =
                            match direction {
                                CopyDirection::LeftToRight => {
                                    let remote_dest = format!(
                                        "{}/{}",
                                        dest_dir.trim_end_matches('/'),
                                        source_name
                                    );
                                    (
                                        item.source_path.clone(),
                                        remote_dest.clone(),
                                        source_name.to_string(),
                                        std::path::Path::new(&remote_dest)
                                            .file_name()
                                            .map(|f| f.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| source_name.to_string()),
                                    )
                                }
                                CopyDirection::RightToLeft => {
                                    let local_dest = format!(
                                        "{}/{}",
                                        dest_dir.trim_end_matches('/'),
                                        source_name
                                    );
                                    (
                                        local_dest.clone(),
                                        item.source_path.clone(),
                                        source_name.to_string(),
                                        std::path::Path::new(&local_dest)
                                            .file_name()
                                            .map(|f| f.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| source_name.to_string()),
                                    )
                                }
                            };

                        transfer_specs.push(ScpTransferSpec {
                            mode,
                            local_path,
                            remote_path,
                            display_name,
                            destination_filename,
                            is_ssh_to_ssh,
                        });
                    }
                }

                if transfer_specs.is_empty() && all_dirs_to_create.is_empty() {
                    app.info = Some("No files found in selected directories".to_string());
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                copy_buffer.clear();

                let old_mode = std::mem::replace(
                    &mut app.mode,
                    AppMode::ConnectionList(crate::ListSelectionState::new(0)),
                );

                if let AppMode::FileExplorer {
                    connection_name,
                    left_pane,
                    left_explorer,
                    left_sftp,
                    left_session,
                    remote_explorer,
                    sftp_session,
                    ssh_connection,
                    channel: _channel,
                    ssh_session,
                    active_pane,
                    copy_buffer: _,
                    return_to,
                    search,
                    ..
                } = old_mode
                {
                    let (sftp_sender, mut sftp_receiver) =
                        tokio::sync::mpsc::channel::<ScpResult>(64);

                    let progress_items: Vec<ScpFileProgress> = transfer_specs
                        .iter()
                        .map(ScpFileProgress::from_spec)
                        .collect();
                    let progress = ScpProgress::new(connection_name.clone(), progress_items);

                    // for (idx, spec) in transfer_specs.iter().enumerate() {
                    //     if matches!(spec.mode, crate::ui::ScpMode::Send)
                    //         && let Ok(metadata) =
                    //             tokio::fs::metadata(crate::expand_tilde(&spec.local_path)).await
                    //         && let Some(file_progress) = progress.files.get_mut(idx)
                    //     {
                    //         file_progress.total_bytes = Some(metadata.len());
                    //     }
                    // }

                    // Clone left_sftp if it exists for SSH-to-SSH transfers (before moving into return_mode)
                    let left_sftp_for_transfer = left_sftp
                        .as_ref()
                        .map(|(session, conn)| (session.clone(), conn.clone()));

                    // Clone session handles for SSH-to-SSH transfers
                    let ssh_session_for_transfer = ssh_session.clone();

                    let return_mode = crate::ScpReturnMode::FileExplorer {
                        connection_name: connection_name.clone(),
                        left_pane,
                        left_explorer,
                        left_sftp,
                        left_session,
                        remote_explorer,
                        sftp_session,
                        ssh_connection: ssh_connection.clone(),
                        channel: None,
                        ssh_session,
                        active_pane,
                        copy_buffer: Vec::new(),
                        return_to,
                        search: search.clone(),
                    };

                    app.go_to_scp_progress(progress, return_mode);

                    // Spawn forwarder task to convert ScpResult -> AppEvent::SftpProgress
                    let event_tx = app.get_event_sender().expect("event sender must be set");
                    tokio::spawn(async move {
                        while let Some(result) = sftp_receiver.recv().await {
                            if event_tx.send(AppEvent::SftpProgress(result)).await.is_err() {
                                break;
                            }
                        }
                    });

                    tokio::spawn(async move {
                        // Phase 1: Create destination directories
                        if !all_dirs_to_create.is_empty() {
                            let dir_result = if is_ssh_to_ssh {
                                match mode {
                                    crate::ui::ScpMode::Send => {
                                        // Dest is right SSH
                                        create_remote_dirs(&ssh_connection, &all_dirs_to_create)
                                            .await
                                    }
                                    crate::ui::ScpMode::Receive => {
                                        // Dest is left SSH
                                        if let Some((_, left_conn)) = &left_sftp_for_transfer {
                                            create_remote_dirs(left_conn, &all_dirs_to_create).await
                                        } else {
                                            create_local_dirs(&all_dirs_to_create).await
                                        }
                                    }
                                }
                            } else {
                                match mode {
                                    crate::ui::ScpMode::Send => {
                                        // Dest is remote SSH
                                        create_remote_dirs(&ssh_connection, &all_dirs_to_create)
                                            .await
                                    }
                                    crate::ui::ScpMode::Receive => {
                                        // Dest is local filesystem
                                        create_local_dirs(&all_dirs_to_create).await
                                    }
                                }
                            };

                            if let Err(e) = dir_result {
                                let _ = sftp_sender
                                    .send(ScpResult::Error {
                                        error: format!("Failed to create directories: {e}"),
                                    })
                                    .await;
                                return;
                            }
                        }

                        // Phase 2: Transfer files
                        let total = transfer_specs.len();
                        let mut tasks = Vec::with_capacity(total);

                        for (index, spec) in transfer_specs.into_iter().enumerate() {
                            let ssh_connection = ssh_connection.clone();
                            let unified_tx = sftp_sender.clone();
                            let left_sftp_clone = left_sftp_for_transfer.clone();
                            let ssh_session_clone = ssh_session_for_transfer.clone();

                            // Open a channel from the right SSH session for this transfer
                            let channel = {
                                let session = ssh_session_clone.lock().await;
                                session.channel_open_session().await.ok()
                            };

                            tasks.push(tokio::spawn(async move {
                                let transfer_result = if spec.is_ssh_to_ssh {
                                    // SSH-to-SSH transfer: Open a channel from the existing session handle
                                    // This reuses the TCP connection instead of creating a new one per file
                                    match spec.mode {
                                        crate::ui::ScpMode::Send => {
                                            // Left SSH → Right SSH: Open channel from RIGHT session
                                            if let Some((left_sftp_session, _left_conn)) = left_sftp_clone {
                                                match crate::filesystem::sftp_file::open_for_read(left_sftp_session.clone(), &spec.local_path).await {
                                                    Ok(sftp_file) => {
                                                        let file_size = sftp_file.file_size().await;
                                                        let progress_reporter = crate::async_ssh_client::TxProgressReporter::new(Some(unified_tx.clone()), index, file_size.ok());


                                                        crate::async_ssh_client::SshSession::sftp_send_file_with_timeout(
                                                            channel,
                                                            &ssh_connection,
                                                            sftp_file,
                                                            &spec.remote_path,
                                                            None,
                                                            &tokio_util::sync::CancellationToken::new(),
                                                            progress_reporter,
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
                                            // Right SSH → Left SSH: Open channel from LEFT session (if available)
                                            if let Some((left_sftp_session, _left_conn)) = left_sftp_clone {
                                                match crate::filesystem::sftp_file::open_for_write(left_sftp_session.clone(), &spec.local_path).await {
                                                    Ok(sftp_file) => {
                                                        let progress_reporter = crate::async_ssh_client::TxProgressReporter::new(Some(unified_tx.clone()), index, None);

                                                        crate::async_ssh_client::SshSession::sftp_receive_file_with_timeout(
                                                            channel,
                                                            &ssh_connection,
                                                            &spec.remote_path,
                                                            sftp_file,
                                                            None,
                                                            &tokio_util::sync::CancellationToken::new(),
                                                            progress_reporter,
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
                                                channel,
                                                &ssh_connection,
                                                &spec.local_path,
                                                &spec.remote_path,
                                                index,
                                                Some(unified_tx.clone()),
                                            )
                                            .await
                                        }
                                        crate::ui::ScpMode::Receive => {
                                            crate::async_ssh_client::SshSession::sftp_receive_file(
                                                channel,
                                                &ssh_connection,
                                                &spec.remote_path,
                                                &spec.local_path,
                                                index,
                                                Some(unified_tx),
                                            )
                                            .await
                                        }
                                    }
                                };

                                let success = transfer_result.is_ok();
                                let error = transfer_result.err().map(|e| e.to_string());

                                ScpFileResult {
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

                        let joined = join_all(tasks).await;
                        let mut results = Vec::with_capacity(joined.len());

                        for outcome in joined {
                            match outcome {
                                Ok(file_result) => results.push(file_result),
                                Err(e) => {
                                    let _ = sftp_sender
                                        .send(ScpResult::Error {
                                            error: format!("Transfer task failed to complete: {e}"),
                                        })
                                        .await;
                                    return;
                                }
                            }
                        }

                        let _ = sftp_sender.send(ScpResult::Completed(results)).await;
                    });
                }
            }

            // Toggle hidden files
            KeyCode::Char('H') => {
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
                            "Failed to toggle hidden files: {e}"
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
                        "Failed to refresh: {e}"
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
                        "Navigation error: {e}"
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
                        "Navigation error: {e}"
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
                        "Navigation error: {e}"
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
                        "Navigation error: {e}"
                    )));
                }
                app.mark_redraw();
            }

            // Enter search mode with '/'
            KeyCode::Char('/') => {
                search.activate();
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

/// Create directories on a remote host via a new SFTP session.
async fn create_remote_dirs(
    connection: &crate::config::manager::Connection,
    dirs: &[String],
) -> crate::error::Result<()> {
    let sftp = crate::async_ssh_client::SshSession::setup_sftp_session(
        None,
        connection,
        None,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    for dir in dirs {
        match sftp
            .mkdir(dir.as_str(), russh_sftp::protocol::FileAttributes::empty())
            .await
        {
            Ok(_) => {}
            Err(_) => {
                // Check if it already exists by trying stat
                match sftp.stat(dir.as_str()).await {
                    Ok(attrs) if attrs.attrs.is_dir() => {} // Already exists
                    _ => {
                        return Err(crate::error::AppError::SftpError(format!(
                            "Failed to create remote directory: {dir}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Create directories on the local filesystem.
async fn create_local_dirs(dirs: &[String]) -> crate::error::Result<()> {
    for dir in dirs {
        tokio::fs::create_dir_all(dir).await.map_err(|e| {
            crate::error::AppError::SftpError(format!(
                "Failed to create local directory '{dir}': {e}"
            ))
        })?;
    }
    Ok(())
}

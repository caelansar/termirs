use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;

use super::KeyFlow;
use crate::config::manager::{PortForward, PortForwardStatus};
use crate::error::AppError;
use crate::ui::PortForwardingForm;
use crate::{App, AppMode};

pub async fn handle_port_forwarding_list_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Check if we're in search mode
    if let AppMode::PortForwardingList {
        search_mode: true,
        search_input,
        ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::PortForwardingList {
                    search_mode,
                    search_input,
                    ..
                } = &mut app.mode
                {
                    *search_mode = false;
                    search_input.delete_line_by_head();
                    search_input.delete_line_by_end();
                }
            }
            KeyCode::Enter => {
                if let AppMode::PortForwardingList { search_mode, .. } = &mut app.mode {
                    *search_mode = false;
                }
            }
            _ => {
                // Let TextArea handle all other key events (cursor movement, editing, etc.)
                search_input.input(key);
            }
        }
        return KeyFlow::Continue;
    }

    let len = app.config.port_forwards().len();
    match key.code {
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.go_to_port_forwarding_form_new();
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(original) = app.config.port_forwards().get(app.current_selected()) {
                let form = PortForwardingForm::from(original);
                app.go_to_port_forwarding_form_edit(form, original.clone());
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(pf) = app.config.port_forwards().get(app.current_selected()) {
                let pf_name = pf.get_display_name();
                let pf_id = pf.id.clone();
                let current_selected = app.current_selected();
                app.go_to_port_forward_delete_confirmation(pf_name, pf_id, current_selected);
            }
        }
        KeyCode::Char('/') => {
            if let AppMode::PortForwardingList {
                search_mode,
                search_input,
                ..
            } = &mut app.mode
            {
                *search_mode = true;
                // Clear any existing text and set up the TextArea for search
                search_input.delete_line_by_head();
                search_input.delete_line_by_end();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::PortForwardingList { selected, .. } = &mut app.mode {
                if len != 0 {
                    *selected = if *selected == 0 {
                        len - 1
                    } else {
                        (*selected - 1).min(len - 1)
                    };
                } else {
                    *selected = 0;
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::PortForwardingList { selected, .. } = &mut app.mode {
                if len != 0 {
                    *selected = (*selected + 1) % len;
                }
            }
        }
        KeyCode::Enter => {
            // Toggle start/stop port forward
            if let Some(pf) = app.config.port_forwards().get(app.current_selected()) {
                let pf_id = pf.id.clone();
                let pf_name = pf.get_display_name();

                // Find the connection for this port forward
                if let Some(connection) = app
                    .config
                    .connections()
                    .iter()
                    .find(|c| c.id == pf.connection_id)
                {
                    // Clone the connection to avoid borrowing conflicts
                    let connection = connection.clone();

                    if let Some(pf_mut) = app.config.find_port_forward_mut(&pf_id) {
                        match pf_mut.status {
                            PortForwardStatus::Stopped => {
                                // Start the port forward
                                match app
                                    .port_forwarding_runtime
                                    .start_port_forward(pf_mut, &connection)
                                    .await
                                {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Running;
                                        app.info = Some(format!(
                                            "Port forward '{}' started successfully",
                                            pf_name
                                        ));
                                        app.needs_redraw = true;
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to start port forward '{}': {}",
                                            pf_name, e
                                        )));
                                        app.needs_redraw = true;
                                    }
                                }
                            }
                            PortForwardStatus::Running => {
                                // Stop the port forward
                                match app.port_forwarding_runtime.stop_port_forward(&pf_id).await {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Stopped;
                                        app.info = Some(format!(
                                            "Port forward '{}' stopped successfully",
                                            pf_name
                                        ));
                                        app.needs_redraw = true;
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to stop port forward '{}': {}",
                                            pf_name, e
                                        )));
                                        app.needs_redraw = true;
                                    }
                                }
                            }
                            PortForwardStatus::Failed(_) => {
                                // Try to start again after failure
                                match app
                                    .port_forwarding_runtime
                                    .start_port_forward(pf_mut, &connection)
                                    .await
                                {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Running;
                                        app.info = Some(format!(
                                            "Port forward '{}' restarted successfully",
                                            pf_name
                                        ));
                                        app.needs_redraw = true;
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to restart port forward '{}': {}",
                                            pf_name, e
                                        )));
                                        app.needs_redraw = true;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    app.error = Some(AppError::ConfigError(format!(
                        "Connection not found for port forward '{}'",
                        pf_name
                    )));
                }
            } else if len == 0 {
                app.info = Some("No port forwards available".to_string());
            }
        }
        KeyCode::Esc => {
            // Return to connection list
            app.go_to_connection_list_with_selected(app.current_selected());
        }
        _ => {}
    }
    KeyFlow::Continue
}

pub async fn handle_port_forwarding_form_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    match key.code {
        KeyCode::Tab | KeyCode::Down => {
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                form.next();
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                form.prev();
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Enter => {
            // Save the port forward
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                let form_clone = form.clone();
                match save_port_forward(app, &form_clone, true).await {
                    Ok(_) => {
                        // After creating a new port forward, go to the end of the list
                        let new_index = app.config.port_forwards().len().saturating_sub(1);
                        app.go_to_port_forwarding_list_with_selected(new_index)
                            .await;
                        if let Err(e) = app
                            .port_forwarding_runtime
                            .start_port_forward(
                                &app.config.port_forwards()[new_index],
                                &app.config
                                    .find_connection(&form_clone.connection_id)
                                    .unwrap(),
                            )
                            .await
                        {
                            // TODO: dedicated error for port forward runtime
                            app.error = Some(AppError::ConfigError(format!(
                                "Failed to start port forward: {}",
                                e
                            )));
                        }
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            } else if let AppMode::PortForwardingFormEdit {
                form,
                current_selected,
                ..
            } = &mut app.mode
            {
                let form_clone = form.clone();
                let saved_selected = *current_selected;
                match save_port_forward(app, &form_clone, false).await {
                    Ok(_) => {
                        // After editing, return to the same position
                        app.go_to_port_forwarding_list_with_selected(saved_selected)
                            .await;
                    }
                    Err(e) => {
                        app.error = Some(e);
                    }
                }
            }
        }
        KeyCode::Esc => {
            if let AppMode::PortForwardingFormNew {
                current_selected, ..
            } = &app.mode
            {
                app.go_to_port_forwarding_list_with_selected(*current_selected)
                    .await;
            } else if let AppMode::PortForwardingFormEdit {
                current_selected, ..
            } = &app.mode
            {
                app.go_to_port_forwarding_list_with_selected(*current_selected)
                    .await;
            } else {
                app.go_to_port_forwarding_list().await;
            }
        }
        KeyCode::Char(' ') => {
            if let AppMode::PortForwardingFormNew {
                form,
                select_connection_mode,
                connection_selected,
                ..
            } = &mut app.mode
            {
                if form.focus == crate::ui::port_forwarding::FocusField::Connection {
                    if app.config.connections().is_empty() {
                        app.error = Some(AppError::ConfigError(
                            "No connections available".to_string(),
                        ));
                    } else {
                        // Find the index of the currently selected connection, or default to 0
                        *connection_selected = app
                            .config
                            .connections()
                            .iter()
                            .position(|c| c.id == form.connection_id)
                            .unwrap_or(0);
                        *select_connection_mode = true;
                    }
                } else {
                    // If not focused on Connection field, pass the key to text input
                    if let Some(textarea) = form.focused_textarea_mut() {
                        textarea.input(key);
                    }
                }
            } else if let AppMode::PortForwardingFormEdit {
                form,
                select_connection_mode,
                connection_selected,
                ..
            } = &mut app.mode
            {
                if form.focus == crate::ui::port_forwarding::FocusField::Connection {
                    if app.config.connections().is_empty() {
                        app.error = Some(AppError::ConfigError(
                            "No connections available".to_string(),
                        ));
                    } else {
                        // Find the index of the currently selected connection, or default to 0
                        *connection_selected = app
                            .config
                            .connections()
                            .iter()
                            .position(|c| c.id == form.connection_id)
                            .unwrap_or(0);
                        *select_connection_mode = true;
                    }
                } else {
                    // If not focused on Connection field, pass the key to text input
                    if let Some(textarea) = form.focused_textarea_mut() {
                        textarea.input(key);
                    }
                }
            } else {
                // Debug: We're not in the right mode
                app.info = Some("Not in port forwarding form mode".to_string());
            }
        }
        _ => {
            // Handle text input for focused field
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                if let Some(textarea) = form.focused_textarea_mut() {
                    textarea.input(key);
                }
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                if let Some(textarea) = form.focused_textarea_mut() {
                    textarea.input(key);
                }
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_port_forwarding_form_connection_select_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    // Check if we're in search mode first
    if let AppMode::PortForwardingFormNew {
        connection_search_mode: true,
        connection_search_input,
        ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::PortForwardingFormNew {
                    connection_search_mode,
                    connection_search_input,
                    ..
                } = &mut app.mode
                {
                    *connection_search_mode = false;
                    connection_search_input.delete_line_by_head();
                    connection_search_input.delete_line_by_end();
                }
            }
            KeyCode::Enter => {
                if let AppMode::PortForwardingFormNew {
                    connection_search_mode,
                    ..
                } = &mut app.mode
                {
                    *connection_search_mode = false;
                }
            }
            _ => {
                // Let TextArea handle all other key events
                connection_search_input.input(key);
            }
        }
        return KeyFlow::Continue;
    } else if let AppMode::PortForwardingFormEdit {
        connection_search_mode: true,
        connection_search_input,
        ..
    } = &mut app.mode
    {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::PortForwardingFormEdit {
                    connection_search_mode,
                    connection_search_input,
                    ..
                } = &mut app.mode
                {
                    *connection_search_mode = false;
                    connection_search_input.delete_line_by_head();
                    connection_search_input.delete_line_by_end();
                }
            }
            KeyCode::Enter => {
                if let AppMode::PortForwardingFormEdit {
                    connection_search_mode,
                    ..
                } = &mut app.mode
                {
                    *connection_search_mode = false;
                }
            }
            _ => {
                // Let TextArea handle all other key events
                connection_search_input.input(key);
            }
        }
        return KeyFlow::Continue;
    }

    // Get filtered connection list length for navigation
    let get_filtered_len = |search_query: &str, app: &App<B>| -> usize {
        if search_query.is_empty() {
            app.config.connections().len()
        } else {
            app.config
                .connections()
                .iter()
                .filter(|c| {
                    c.host.to_lowercase().contains(&search_query.to_lowercase())
                        || c.username
                            .to_lowercase()
                            .contains(&search_query.to_lowercase())
                        || c.display_name
                            .to_lowercase()
                            .contains(&search_query.to_lowercase())
                })
                .count()
        }
    };

    match key.code {
        KeyCode::Char('/') => {
            // Enter search mode
            if let AppMode::PortForwardingFormNew {
                connection_search_mode,
                connection_search_input,
                ..
            } = &mut app.mode
            {
                *connection_search_mode = true;
                connection_search_input.delete_line_by_head();
                connection_search_input.delete_line_by_end();
            } else if let AppMode::PortForwardingFormEdit {
                connection_search_mode,
                connection_search_input,
                ..
            } = &mut app.mode
            {
                *connection_search_mode = true;
                connection_search_input.delete_line_by_head();
                connection_search_input.delete_line_by_end();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::PortForwardingFormNew {
                connection_search_input,
                ..
            } = &app.mode
            {
                let search_query = connection_search_input.lines()[0].as_str().to_string();

                let len = get_filtered_len(&search_query, app);

                // Re-borrow to update
                if let AppMode::PortForwardingFormNew {
                    connection_selected,
                    ..
                } = &mut app.mode
                {
                    if len != 0 {
                        *connection_selected = if *connection_selected == 0 {
                            len - 1
                        } else {
                            (*connection_selected - 1).min(len - 1)
                        };
                    }
                }
            } else if let AppMode::PortForwardingFormEdit {
                connection_search_input,
                ..
            } = &app.mode
            {
                let search_query = connection_search_input.lines()[0].as_str().to_string();

                let len = get_filtered_len(&search_query, app);

                if let AppMode::PortForwardingFormEdit {
                    connection_selected,
                    ..
                } = &mut app.mode
                {
                    if len != 0 {
                        *connection_selected = if *connection_selected == 0 {
                            len - 1
                        } else {
                            (*connection_selected - 1).min(len - 1)
                        };
                    }
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::PortForwardingFormNew {
                connection_search_input,
                ..
            } = &app.mode
            {
                let search_query = connection_search_input.lines()[0].as_str().to_string();

                let len = get_filtered_len(&search_query, app);

                if let AppMode::PortForwardingFormNew {
                    connection_selected,
                    ..
                } = &mut app.mode
                {
                    if len != 0 {
                        *connection_selected = (*connection_selected + 1) % len;
                    }
                }
            } else if let AppMode::PortForwardingFormEdit {
                connection_search_input,
                ..
            } = &app.mode
            {
                let search_query = connection_search_input.lines()[0].as_str().to_string();

                let len = get_filtered_len(&search_query, app);

                if let AppMode::PortForwardingFormEdit {
                    connection_selected,
                    ..
                } = &mut app.mode
                {
                    if len != 0 {
                        *connection_selected = (*connection_selected + 1) % len;
                    }
                }
            }
        }
        KeyCode::Enter => {
            // Select the connection from filtered list
            if let AppMode::PortForwardingFormNew {
                form,
                connection_selected,
                select_connection_mode,
                connection_search_input,
                ..
            } = &mut app.mode
            {
                // Get the search query
                let search_query = connection_search_input.lines()[0].as_str();

                // Filter connections based on search query (same logic as in draw_connection_list)
                let mut filtered_connections: Vec<_> = app.config.connections().iter().collect();
                if !search_query.is_empty() {
                    filtered_connections.retain(|c| {
                        c.host.to_lowercase().contains(&search_query.to_lowercase())
                            || c.username
                                .to_lowercase()
                                .contains(&search_query.to_lowercase())
                            || c.display_name
                                .to_lowercase()
                                .contains(&search_query.to_lowercase())
                    });
                }

                // Get connection from filtered list
                if let Some(connection) = filtered_connections.get(*connection_selected) {
                    form.connection_id = connection.id.clone();
                    connection_search_input.delete_line_by_head();
                    connection_search_input.delete_line_by_end();
                    form.next();
                    *select_connection_mode = false;
                }
            } else if let AppMode::PortForwardingFormEdit {
                form,
                connection_selected,
                select_connection_mode,
                connection_search_input,
                ..
            } = &mut app.mode
            {
                // Get the search query
                let search_query = connection_search_input.lines()[0].as_str();

                // Filter connections based on search query (same logic as in draw_connection_list)
                let mut filtered_connections: Vec<_> = app.config.connections().iter().collect();
                if !search_query.is_empty() {
                    filtered_connections.retain(|c| {
                        c.host.to_lowercase().contains(&search_query.to_lowercase())
                            || c.username
                                .to_lowercase()
                                .contains(&search_query.to_lowercase())
                            || c.display_name
                                .to_lowercase()
                                .contains(&search_query.to_lowercase())
                    });
                }

                // Get connection from filtered list
                if let Some(connection) = filtered_connections.get(*connection_selected) {
                    form.connection_id = connection.id.clone();
                    connection_search_input.delete_line_by_head();
                    connection_search_input.delete_line_by_end();
                    form.next();
                    *select_connection_mode = false;
                }
            }
        }
        KeyCode::Esc => {
            // Cancel connection selection
            if let AppMode::PortForwardingFormNew {
                select_connection_mode,
                ..
            } = &mut app.mode
            {
                *select_connection_mode = false;
            } else if let AppMode::PortForwardingFormEdit {
                select_connection_mode,
                ..
            } = &mut app.mode
            {
                *select_connection_mode = false;
            }
        }
        _ => {}
    }
    KeyFlow::Continue
}

async fn save_port_forward<B: Backend + Write>(
    app: &mut App<B>,
    form: &PortForwardingForm,
    is_new: bool,
) -> Result<(), crate::error::AppError> {
    // Validate the form
    form.validate(app.config.connections())
        .map_err(|e| crate::error::AppError::ValidationError(e))?;

    let local_port = form
        .get_local_port_value()
        .parse::<u16>()
        .map_err(|_| crate::error::AppError::ValidationError("Invalid local port".to_string()))?;

    let service_port = form
        .get_service_port_value()
        .parse::<u16>()
        .map_err(|_| crate::error::AppError::ValidationError("Invalid service port".to_string()))?;

    let display_name = if form.get_display_name_value().trim().is_empty() {
        None
    } else {
        Some(form.get_display_name_value().to_string())
    };

    if is_new {
        // Create a new port forward with a new ID
        let mut port_forward = PortForward::new(
            form.connection_id.clone(),
            form.get_local_addr_value().to_string(),
            local_port,
            form.get_service_host_value().to_string(),
            service_port,
            display_name,
        );
        // new port forward will be immediately started, so we set the status to running here
        port_forward.status = PortForwardStatus::Running;
        app.config.add_port_forward(port_forward)?;
        app.info = Some("Port forward created successfully".to_string());
    } else {
        // For updates, we need to use the existing ID
        let id = form.id.clone().ok_or_else(|| {
            crate::error::AppError::ValidationError(
                "Port forward ID is missing for update".to_string(),
            )
        })?;

        // Find the existing port forward to get created_at and status
        let existing = app
            .config
            .port_forwards()
            .iter()
            .find(|pf| pf.id == id)
            .ok_or_else(|| {
                crate::error::AppError::ConfigError("Port forward not found".to_string())
            })?
            .clone();

        // Create the updated port forward preserving ID, created_at, and status
        let port_forward = PortForward {
            id,
            connection_id: form.connection_id.clone(),
            local_addr: form.get_local_addr_value().to_string(),
            local_port,
            service_host: form.get_service_host_value().to_string(),
            service_port,
            display_name,
            created_at: existing.created_at,
            status: existing.status,
        };

        app.config.update_port_forward(port_forward)?;
        app.info = Some("Port forward updated successfully".to_string());
    }

    Ok(())
}

/// Sync port forwarding status with the runtime to ensure consistency
pub async fn sync_port_forwarding_status<B: Backend + Write>(app: &mut App<B>) {
    for pf in app.config.port_forwards_mut() {
        let is_running = app.port_forwarding_runtime.is_running(&pf.id).await;
        match pf.status {
            PortForwardStatus::Running => {
                if !is_running {
                    // Port forward was marked as running but isn't actually running
                    pf.status = PortForwardStatus::Stopped;
                }
            }
            PortForwardStatus::Stopped => {
                if is_running {
                    // Port forward is actually running but marked as stopped
                    pf.status = PortForwardStatus::Running;
                }
            }
            PortForwardStatus::Failed(_) => {
                // Keep failed status as is - user can retry manually
            }
        }
    }
}

pub async fn handle_port_forward_delete_confirmation_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    if let AppMode::PortForwardDeleteConfirmation {
        port_forward_id,
        current_selected,
        ..
    } = &app.mode
    {
        let port_forward_id = port_forward_id.clone();
        let current_selected = *current_selected;

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                // Confirm deletion
                match app.config.remove_port_forward(&port_forward_id) {
                    Ok(_) => {
                        if let Err(e) = app.config.save() {
                            app.error = Some(e);
                            app.go_to_port_forwarding_list_with_selected(current_selected)
                                .await;
                        } else {
                            app.info = Some("Port forward deleted successfully".to_string());
                            let new_len = app.config.port_forwards().len();
                            let new_selected = if new_len == 0 {
                                0
                            } else if current_selected >= new_len {
                                new_len - 1
                            } else {
                                current_selected
                            };
                            app.go_to_port_forwarding_list_with_selected(new_selected)
                                .await;
                        }
                    }
                    Err(e) => {
                        app.error = Some(e);
                        app.go_to_port_forwarding_list_with_selected(current_selected)
                            .await;
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel deletion
                app.go_to_port_forwarding_list_with_selected(current_selected)
                    .await;
            }
            _ => {}
        }
    }
    KeyFlow::Continue
}

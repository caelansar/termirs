use std::io::Write;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::Backend;
use tracing::{error, info};

use super::KeyFlow;
use crate::app::{App, AppMode};
use crate::config::manager::{PortForward, PortForwardStatus};
use crate::error::AppError;
use crate::ui::{PortForwardingForm, file_explorer::filter_connection_indices};

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
                app.go_to_port_forwarding_form_edit(form);
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
            if let AppMode::PortForwardingList { selected, .. } = &mut app.mode
                && len != 0
            {
                *selected = (*selected + 1) % len;
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
                                info!("Starting port forward: {}", pf_name);
                                match app
                                    .port_forwarding_runtime
                                    .start_port_forward(pf_mut, &connection)
                                    .await
                                {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Running;
                                        info!("Port forward '{}' started successfully", pf_name);
                                        app.info = Some(format!(
                                            "Port forward '{pf_name}' started successfully"
                                        ));
                                        app.mark_redraw();
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        error!("Failed to start port forward '{}': {}", pf_name, e);
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to start port forward '{pf_name}': {e}"
                                        )));
                                        app.mark_redraw();
                                    }
                                }
                            }
                            PortForwardStatus::Running => {
                                // Stop the port forward
                                info!("Stopping port forward: {}", pf_name);
                                match app.port_forwarding_runtime.stop_port_forward(&pf_id).await {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Stopped;
                                        info!("Port forward '{}' stopped successfully", pf_name);
                                        app.info = Some(format!(
                                            "Port forward '{pf_name}' stopped successfully"
                                        ));
                                        app.mark_redraw();
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        error!("Failed to stop port forward '{}': {}", pf_name, e);
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to stop port forward '{pf_name}': {e}"
                                        )));
                                        app.mark_redraw();
                                    }
                                }
                            }
                            PortForwardStatus::Failed(_) => {
                                // Try to start again after failure
                                info!("Restarting failed port forward: {}", pf_name);
                                match app
                                    .port_forwarding_runtime
                                    .start_port_forward(pf_mut, &connection)
                                    .await
                                {
                                    Ok(_) => {
                                        pf_mut.status = PortForwardStatus::Running;
                                        info!("Port forward '{}' restarted successfully", pf_name);
                                        app.info = Some(format!(
                                            "Port forward '{pf_name}' restarted successfully"
                                        ));
                                        app.mark_redraw();
                                    }
                                    Err(e) => {
                                        pf_mut.status = PortForwardStatus::Failed(e.to_string());
                                        error!(
                                            "Failed to restart port forward '{}': {}",
                                            pf_name, e
                                        );
                                        app.error = Some(AppError::ConfigError(format!(
                                            "Failed to restart port forward '{pf_name}': {e}"
                                        )));
                                        app.mark_redraw();
                                    }
                                }
                            }
                        }
                    }
                } else {
                    app.error = Some(AppError::ConfigError(format!(
                        "Connection not found for port forward '{pf_name}'"
                    )));
                }
            } else if len == 0 {
                app.info = Some("No port forwards available".to_string());
            }
        }
        KeyCode::Char('q') => {
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
    use crate::config::manager::PortForwardType;
    use crate::ui::port_forwarding::FocusField;

    match key.code {
        KeyCode::Tab => {
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                form.next();
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                form.next();
            }
        }
        KeyCode::BackTab => {
            if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                form.prev();
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                form.prev();
            }
        }
        KeyCode::Down => {
            // Check if ForwardType is focused
            let is_forward_type_focused =
                if let AppMode::PortForwardingFormNew { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else {
                    false
                };

            if is_forward_type_focused {
                // Cycle forward through types
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                }
            } else {
                // Normal navigation
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.next();
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.next();
                }
            }
        }
        KeyCode::Up => {
            // Check if ForwardType is focused
            let is_forward_type_focused =
                if let AppMode::PortForwardingFormNew { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else {
                    false
                };

            if is_forward_type_focused {
                // Cycle backward through types
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                }
            } else {
                // Normal navigation
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.prev();
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.prev();
                }
            }
        }
        KeyCode::Left => {
            // Check if ForwardType is focused - cycle backward
            let is_forward_type_focused =
                if let AppMode::PortForwardingFormNew { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else {
                    false
                };

            if is_forward_type_focused {
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Dynamic,
                        PortForwardType::Remote => PortForwardType::Local,
                        PortForwardType::Dynamic => PortForwardType::Remote,
                    };
                }
            }
        }
        KeyCode::Right => {
            // Check if ForwardType is focused - cycle forward
            let is_forward_type_focused =
                if let AppMode::PortForwardingFormNew { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &app.mode {
                    form.focus == FocusField::ForwardType
                } else {
                    false
                };

            if is_forward_type_focused {
                if let AppMode::PortForwardingFormNew { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode {
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                }
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
                        let connection = app.config.find_connection(&form_clone.connection_id);
                        if let Some(connection) = connection {
                            if let Err(e) = app
                                .port_forwarding_runtime
                                .start_port_forward(
                                    &app.config.port_forwards()[new_index],
                                    connection,
                                )
                                .await
                            {
                                app.error = Some(e);
                            }
                        } else {
                            app.error = Some(AppError::PortForwardingError(format!(
                                "Connection not found for port forward '{}'",
                                form_clone.get_display_name_value()
                            )));
                        }
                        app.go_to_port_forwarding_list_with_selected(new_index)
                            .await;
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
                connection_search_mode,
                connection_search_query,
                ..
            } = &mut app.mode
            {
                if form.focus == FocusField::ForwardType {
                    // Cycle through forward types with Space
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if form.focus == FocusField::Connection {
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
                        *connection_search_mode = false;
                        connection_search_query.clear();
                    }
                } else {
                    // If not focused on Connection or ForwardType field, pass the key to text input
                    if let Some(textarea) = form.focused_textarea_mut() {
                        textarea.input(key);
                    }
                }
            } else if let AppMode::PortForwardingFormEdit {
                form,
                select_connection_mode,
                connection_selected,
                connection_search_mode,
                connection_search_query,
                ..
            } = &mut app.mode
            {
                if form.focus == FocusField::ForwardType {
                    // Cycle through forward types with Space
                    form.forward_type = match form.forward_type {
                        PortForwardType::Local => PortForwardType::Remote,
                        PortForwardType::Remote => PortForwardType::Dynamic,
                        PortForwardType::Dynamic => PortForwardType::Local,
                    };
                } else if form.focus == FocusField::Connection {
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
                        *connection_search_mode = false;
                        connection_search_query.clear();
                    }
                } else {
                    // If not focused on Connection or ForwardType field, pass the key to text input
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
            } else if let AppMode::PortForwardingFormEdit { form, .. } = &mut app.mode
                && let Some(textarea) = form.focused_textarea_mut()
            {
                textarea.input(key);
            }
        }
    }
    KeyFlow::Continue
}

pub async fn handle_port_forwarding_form_connection_select_key<B: Backend + Write>(
    app: &mut App<B>,
    key: KeyEvent,
) -> KeyFlow {
    let Some(selector) = connection_selector_state(&mut app.mode) else {
        return KeyFlow::Continue;
    };

    let connections = app.config.connections();
    let mut filtered_indices =
        filter_connection_indices(connections, None, selector.search_query.as_str());
    let mut total_items = filtered_indices.len();

    if *selector.search_mode {
        match key.code {
            KeyCode::Char(c) => {
                selector.search_query.push(c);
                *selector.connection_selected = 0;
                app.mark_redraw();
            }
            KeyCode::Backspace => {
                selector.search_query.pop();
                filtered_indices =
                    filter_connection_indices(connections, None, selector.search_query.as_str());
                total_items = filtered_indices.len();
                if total_items == 0 {
                    *selector.connection_selected = 0;
                } else if *selector.connection_selected >= total_items {
                    *selector.connection_selected = total_items - 1;
                }
                app.mark_redraw();
            }
            KeyCode::Esc => {
                if !selector.search_query.is_empty() {
                    selector.search_query.clear();
                    filtered_indices = filter_connection_indices(
                        connections,
                        None,
                        selector.search_query.as_str(),
                    );
                    total_items = filtered_indices.len();
                    if total_items == 0 {
                        *selector.connection_selected = 0;
                    } else if *selector.connection_selected >= total_items {
                        *selector.connection_selected = total_items - 1;
                    }
                } else {
                    *selector.search_mode = false;
                }
                app.mark_redraw();
            }
            KeyCode::Enter => {
                *selector.search_mode = false;
                if total_items == 0 {
                    app.mark_redraw();
                    return KeyFlow::Continue;
                }

                let idx = (*selector.connection_selected).min(total_items.saturating_sub(1));
                if let Some(conn_idx) = filtered_indices.get(idx)
                    && let Some(connection) = connections.get(*conn_idx)
                {
                    selector.form.connection_id = connection.id.clone();
                    selector.form.next();
                    *selector.select_connection_mode = false;
                    *selector.search_mode = false;
                    selector.search_query.clear();
                }
                app.mark_redraw();
            }
            _ => {}
        }
        return KeyFlow::Continue;
    }

    match key.code {
        KeyCode::Char('/') => {
            *selector.search_mode = true;
            selector.search_query.clear();
            app.mark_redraw();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if total_items > 0 {
                if *selector.connection_selected == 0 {
                    *selector.connection_selected = total_items - 1;
                } else {
                    *selector.connection_selected -= 1;
                }
            }
            app.mark_redraw();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if total_items > 0 {
                *selector.connection_selected = (*selector.connection_selected + 1) % total_items;
            }
            app.mark_redraw();
        }
        KeyCode::Enter => {
            if total_items == 0 {
                app.mark_redraw();
                return KeyFlow::Continue;
            }

            let idx = (*selector.connection_selected).min(total_items.saturating_sub(1));
            if let Some(conn_idx) = filtered_indices.get(idx)
                && let Some(connection) = connections.get(*conn_idx)
            {
                selector.form.connection_id = connection.id.clone();
                selector.form.next();
                *selector.select_connection_mode = false;
                *selector.search_mode = false;
                selector.search_query.clear();
                app.mark_redraw();
            }
        }
        KeyCode::Esc => {
            *selector.select_connection_mode = false;
            *selector.search_mode = false;
            selector.search_query.clear();
            app.mark_redraw();
        }
        _ => {}
    }
    KeyFlow::Continue
}

struct ConnectionSelectorState<'a> {
    form: &'a mut PortForwardingForm,
    select_connection_mode: &'a mut bool,
    connection_selected: &'a mut usize,
    search_mode: &'a mut bool,
    search_query: &'a mut String,
}

fn connection_selector_state<'a>(mode: &'a mut AppMode) -> Option<ConnectionSelectorState<'a>> {
    match mode {
        AppMode::PortForwardingFormNew {
            form,
            select_connection_mode,
            connection_selected,
            connection_search_mode,
            connection_search_query,
            ..
        }
        | AppMode::PortForwardingFormEdit {
            form,
            select_connection_mode,
            connection_selected,
            connection_search_mode,
            connection_search_query,
            ..
        } => Some(ConnectionSelectorState {
            form,
            select_connection_mode,
            connection_selected,
            search_mode: connection_search_mode,
            search_query: connection_search_query,
        }),
        _ => None,
    }
}

async fn save_port_forward<B: Backend + Write>(
    app: &mut App<B>,
    form: &PortForwardingForm,
    is_new: bool,
) -> Result<(), crate::error::AppError> {
    // Validate the form
    form.validate(app.config.connections())
        .map_err(crate::error::AppError::ValidationError)?;

    let local_port = form
        .get_local_port_value()
        .parse::<u16>()
        .map_err(|_| crate::error::AppError::ValidationError("Invalid local port".to_string()))?;

    // Parse service_port only if needed (not for Dynamic)
    let service_port = if matches!(
        form.forward_type,
        crate::config::manager::PortForwardType::Dynamic
    ) {
        0 // Default value for Dynamic, not used
    } else {
        form.get_service_port_value().parse::<u16>().map_err(|_| {
            crate::error::AppError::ValidationError("Invalid service port".to_string())
        })?
    };

    let display_name = if form.get_display_name_value().trim().is_empty() {
        None
    } else {
        Some(form.get_display_name_value().to_string())
    };

    let remote_bind_addr = if matches!(
        form.forward_type,
        crate::config::manager::PortForwardType::Remote
    ) {
        let addr = form.get_remote_bind_addr_value().trim();
        if addr.is_empty() {
            None
        } else {
            Some(addr.to_string())
        }
    } else {
        None
    };

    if is_new {
        // Create a new port forward with a new ID
        let mut port_forward = PortForward::new(
            form.connection_id.clone(),
            form.forward_type,
            form.get_local_addr_value().to_string(),
            local_port,
            form.get_service_host_value().to_string(),
            service_port,
            remote_bind_addr,
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
            forward_type: form.forward_type,
            local_addr: form.get_local_addr_value().to_string(),
            local_port,
            service_host: form.get_service_host_value().to_string(),
            service_port,
            remote_bind_addr,
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

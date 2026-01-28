use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row};
use tui_textarea::TextArea;

use crate::config::manager::{Connection, PortForward, PortForwardStatus, PortForwardType};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusField {
    Connection,
    ForwardType,
    LocalAddr,
    LocalPort,
    ServiceHost,
    ServicePort,
    RemoteBind,
    DisplayName,
}

#[derive(Clone, Debug)]
pub struct PortForwardingForm {
    pub id: Option<String>, // Store the ID when editing
    pub connection_id: String,
    pub forward_type: PortForwardType,
    pub local_addr: TextArea<'static>,
    pub local_port: TextArea<'static>,
    pub service_host: TextArea<'static>,
    pub service_port: TextArea<'static>,
    pub remote_bind_addr: TextArea<'static>,
    pub display_name: TextArea<'static>,
    pub focus: FocusField,
    pub error: Option<String>,
}

impl Default for PortForwardingForm {
    fn default() -> Self {
        Self::new()
    }
}

impl PortForwardingForm {
    pub fn new() -> Self {
        let mut local_addr = TextArea::default();
        local_addr.set_placeholder_text("127.0.0.1");
        local_addr.set_cursor_line_style(Style::default());

        let mut local_port = TextArea::default();
        local_port.set_placeholder_text("8080");
        local_port.set_cursor_line_style(Style::default());

        let mut service_host = TextArea::default();
        service_host.set_placeholder_text("localhost");
        service_host.set_cursor_line_style(Style::default());

        let mut service_port = TextArea::default();
        service_port.set_placeholder_text("5432");
        service_port.set_cursor_line_style(Style::default());

        let mut remote_bind_addr = TextArea::default();
        remote_bind_addr.set_placeholder_text("0.0.0.0 or 127.0.0.1");
        remote_bind_addr.set_cursor_line_style(Style::default());

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());

        Self {
            id: None, // No ID for new port forwards
            connection_id: String::new(),
            forward_type: PortForwardType::Local,
            local_addr,
            local_port,
            service_host,
            service_port,
            remote_bind_addr,
            display_name,
            focus: FocusField::ForwardType,
            error: None,
        }
    }

    pub fn next(&mut self) {
        use PortForwardType::*;
        self.focus = match (self.focus, self.forward_type) {
            // Start with ForwardType, then Connection
            (FocusField::ForwardType, _) => FocusField::Connection,
            (FocusField::Connection, Local) => FocusField::LocalAddr,
            (FocusField::Connection, Remote) => FocusField::RemoteBind,
            (FocusField::Connection, Dynamic) => FocusField::LocalAddr,

            // Local forwarding: ForwardType → Connection → LocalAddr → LocalPort → ServiceHost → ServicePort → DisplayName
            (FocusField::LocalAddr, Local) => FocusField::LocalPort,
            (FocusField::LocalPort, Local) => FocusField::ServiceHost,
            (FocusField::ServiceHost, Local) => FocusField::ServicePort,
            (FocusField::ServicePort, Local) => FocusField::DisplayName,

            // Remote forwarding: ForwardType → Connection → RemoteBind → LocalPort → ServiceHost → ServicePort → DisplayName
            (FocusField::RemoteBind, Remote) => FocusField::LocalPort,
            (FocusField::LocalPort, Remote) => FocusField::ServiceHost,
            (FocusField::ServiceHost, Remote) => FocusField::ServicePort,
            (FocusField::ServicePort, Remote) => FocusField::DisplayName,

            // Dynamic forwarding: ForwardType → Connection → LocalAddr → LocalPort → DisplayName
            (FocusField::LocalAddr, Dynamic) => FocusField::LocalPort,
            (FocusField::LocalPort, Dynamic) => FocusField::DisplayName,

            // Back to ForwardType from DisplayName
            (FocusField::DisplayName, _) => FocusField::ForwardType,

            // Fallback for unused combinations
            _ => FocusField::ForwardType,
        };
    }

    pub fn prev(&mut self) {
        use PortForwardType::*;
        self.focus = match (self.focus, self.forward_type) {
            // ForwardType is first, Connection is second
            (FocusField::ForwardType, _) => FocusField::DisplayName,
            (FocusField::Connection, _) => FocusField::ForwardType,

            // Local forwarding (reverse order)
            (FocusField::LocalAddr, Local) => FocusField::Connection,
            (FocusField::LocalPort, Local) => FocusField::LocalAddr,
            (FocusField::ServiceHost, Local) => FocusField::LocalPort,
            (FocusField::ServicePort, Local) => FocusField::ServiceHost,
            (FocusField::DisplayName, Local) => FocusField::ServicePort,

            // Remote forwarding (reverse order)
            (FocusField::RemoteBind, Remote) => FocusField::Connection,
            (FocusField::LocalPort, Remote) => FocusField::RemoteBind,
            (FocusField::ServiceHost, Remote) => FocusField::LocalPort,
            (FocusField::ServicePort, Remote) => FocusField::ServiceHost,
            (FocusField::DisplayName, Remote) => FocusField::ServicePort,

            // Dynamic forwarding (reverse order)
            (FocusField::LocalAddr, Dynamic) => FocusField::Connection,
            (FocusField::LocalPort, Dynamic) => FocusField::LocalAddr,
            (FocusField::DisplayName, Dynamic) => FocusField::LocalPort,

            // Fallback
            _ => FocusField::ForwardType,
        };
    }

    pub fn focused_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            FocusField::Connection => None,  // Handled by dropdown
            FocusField::ForwardType => None, // Handled by type selector
            FocusField::LocalAddr => Some(&mut self.local_addr),
            FocusField::LocalPort => Some(&mut self.local_port),
            FocusField::ServiceHost => Some(&mut self.service_host),
            FocusField::ServicePort => Some(&mut self.service_port),
            FocusField::RemoteBind => Some(&mut self.remote_bind_addr),
            FocusField::DisplayName => Some(&mut self.display_name),
        }
    }

    pub fn get_local_addr_value(&self) -> &str {
        &self.local_addr.lines()[0]
    }

    pub fn get_local_port_value(&self) -> &str {
        &self.local_port.lines()[0]
    }

    pub fn get_service_host_value(&self) -> &str {
        &self.service_host.lines()[0]
    }

    pub fn get_service_port_value(&self) -> &str {
        &self.service_port.lines()[0]
    }

    pub fn get_display_name_value(&self) -> &str {
        &self.display_name.lines()[0]
    }

    pub fn get_remote_bind_addr_value(&self) -> &str {
        &self.remote_bind_addr.lines()[0]
    }

    pub fn validate(&self, connections: &[Connection]) -> Result<(), String> {
        if self.connection_id.trim().is_empty() {
            return Err("Connection is required".into());
        }

        if !connections.iter().any(|c| c.id == self.connection_id) {
            return Err("Selected connection not found".into());
        }

        // Validate based on forward type
        match self.forward_type {
            PortForwardType::Local => {
                if self.get_local_addr_value().trim().is_empty() {
                    return Err("Local address is required".into());
                }

                if self.get_local_port_value().trim().is_empty() {
                    return Err("Local port is required".into());
                }

                if self.get_local_port_value().parse::<u16>().is_err() {
                    return Err("Local port must be a number".into());
                }

                if self.get_service_host_value().trim().is_empty() {
                    return Err("Service host is required".into());
                }

                if self.get_service_port_value().trim().is_empty() {
                    return Err("Service port is required".into());
                }

                if self.get_service_port_value().parse::<u16>().is_err() {
                    return Err("Service port must be a number".into());
                }
            }
            PortForwardType::Remote => {
                if self.get_local_port_value().trim().is_empty() {
                    return Err("Remote port is required".into());
                }

                if self.get_local_port_value().parse::<u16>().is_err() {
                    return Err("Remote port must be a number".into());
                }

                if self.get_service_host_value().trim().is_empty() {
                    return Err("Local service host is required".into());
                }

                if self.get_service_port_value().trim().is_empty() {
                    return Err("Local service port is required".into());
                }

                if self.get_service_port_value().parse::<u16>().is_err() {
                    return Err("Local service port must be a number".into());
                }

                // remote_bind_addr is optional, but validate if provided
                let remote_bind = self.get_remote_bind_addr_value().trim();
                if !remote_bind.is_empty() {
                    // Basic validation - could be more sophisticated
                    if remote_bind != "0.0.0.0"
                        && remote_bind != "127.0.0.1"
                        && !remote_bind.contains('.')
                    {
                        return Err(
                            "Remote bind address must be a valid IP (e.g., 0.0.0.0 or 127.0.0.1)"
                                .into(),
                        );
                    }
                }
            }
            PortForwardType::Dynamic => {
                if self.get_local_addr_value().trim().is_empty() {
                    return Err("Local address is required".into());
                }

                if self.get_local_port_value().trim().is_empty() {
                    return Err("Local port is required".into());
                }

                if self.get_local_port_value().parse::<u16>().is_err() {
                    return Err("Local port must be a number".into());
                }
            }
        }

        Ok(())
    }

    fn from_port_forward(pf: &PortForward) -> Self {
        let mut local_addr = TextArea::default();
        local_addr.set_placeholder_text("127.0.0.1");
        local_addr.set_cursor_line_style(Style::default());
        local_addr.insert_str(&pf.local_addr);

        let mut local_port = TextArea::default();
        local_port.set_placeholder_text("8080");
        local_port.set_cursor_line_style(Style::default());
        local_port.insert_str(pf.local_port.to_string());

        let mut service_host = TextArea::default();
        service_host.set_placeholder_text("localhost");
        service_host.set_cursor_line_style(Style::default());
        service_host.insert_str(&pf.service_host);

        let mut service_port = TextArea::default();
        service_port.set_placeholder_text("5432");
        service_port.set_cursor_line_style(Style::default());
        service_port.insert_str(pf.service_port.to_string());

        let mut remote_bind_addr = TextArea::default();
        remote_bind_addr.set_placeholder_text("0.0.0.0 or 127.0.0.1");
        remote_bind_addr.set_cursor_line_style(Style::default());
        if let Some(addr) = &pf.remote_bind_addr {
            remote_bind_addr.insert_str(addr);
        }

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());
        if let Some(name) = &pf.display_name {
            display_name.insert_str(name);
        }

        Self {
            id: Some(pf.id.clone()), // Preserve the ID when editing
            connection_id: pf.connection_id.clone(),
            forward_type: pf.forward_type,
            local_addr,
            local_port,
            service_host,
            service_port,
            remote_bind_addr,
            display_name,
            focus: FocusField::ForwardType,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PortForwardingListItem<'a> {
    pub status_icon: &'a str,
    pub forward_type: &'a str,
    pub display_name: String,
    pub connection_name: &'a str,
    pub local_address: String,
    pub service_address: String,
    pub created_at: String,
}

/// Table component implementation for PortForwardingList
pub struct PortForwardingTableComponent;

impl super::table::TableListComponent<7> for PortForwardingTableComponent {
    type Item<'a> = PortForwardingListItem<'a>;

    const HEADER_LABELS: &'static [&'static str; 7] = &[
        "Status",
        "Type",
        "Name",
        "Connection",
        "Bind",
        "Service",
        "Created",
    ];

    const COLUMN_CONSTRAINTS: &'static [Constraint; 7] = &[
        Constraint::Length(8),  // Status
        Constraint::Length(8),  // Type
        Constraint::Min(12),    // Name
        Constraint::Min(10),    // Connection
        Constraint::Min(12),    // Bind
        Constraint::Min(12),    // Service
        Constraint::Length(16), // Created
    ];

    fn render_row(&self, item: &PortForwardingListItem<'_>) -> Row<'static> {
        use ratatui::text::Span;

        let status_color = match item.status_icon {
            "●" => Color::Green,
            "○" => Color::Gray,
            "✗" => Color::Red,
            _ => Color::White,
        };

        Row::new(vec![
            Cell::from(Span::styled(
                item.status_icon.to_string(),
                Style::default().fg(status_color),
            )),
            Cell::from(item.forward_type.to_string()),
            Cell::from(item.display_name.to_string()),
            Cell::from(item.connection_name.to_string()),
            Cell::from(item.local_address.to_string()),
            Cell::from(item.service_address.to_string()),
            Cell::from(item.created_at.to_string()),
        ])
        .height(1)
    }

    fn matches_query(&self, item: &PortForwardingListItem, query: &str) -> bool {
        let lower = query.to_lowercase();
        item.display_name.to_lowercase().contains(&lower)
            || item.connection_name.to_lowercase().contains(&lower)
            || item.local_address.to_lowercase().contains(&lower)
            || item.service_address.to_lowercase().contains(&lower)
            || item.forward_type.to_lowercase().contains(&lower)
    }

    fn footer_hints(&self) -> &'static str {
        "Enter: Start/Stop   N: New   E: Edit   D: Delete   Q: Back   /: Search"
    }
}

pub fn draw_port_forwarding_list(
    area: Rect,
    port_forwards: &[PortForward],
    connections: &[Connection],
    selected_index: usize,
    search: &crate::SearchState,
    frame: &mut ratatui::Frame<'_>,
) {
    // Build the list items
    let items: Vec<PortForwardingListItem> = port_forwards
        .iter()
        .map(|pf| {
            let connection = connections
                .iter()
                .find(|c| c.id == pf.connection_id)
                .map(|c| c.display_name.as_str())
                .unwrap_or("Unknown");

            let status_icon = match pf.status {
                PortForwardStatus::Running => "●",
                PortForwardStatus::Stopped => "○",
                PortForwardStatus::Failed(_) => "✗",
            };

            let forward_type = match pf.forward_type {
                PortForwardType::Local => "Local",
                PortForwardType::Remote => "Remote",
                PortForwardType::Dynamic => "Dynamic",
            };

            PortForwardingListItem {
                status_icon: status_icon,
                forward_type: forward_type,
                display_name: pf.get_display_name(),
                connection_name: connection,
                local_address: pf.local_address(),
                service_address: pf.service_address(),
                created_at: pf
                    .created_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
            }
        })
        .collect();

    // Create the component
    let component = PortForwardingTableComponent;

    // Create state from current values
    let state = super::table::TableListState::from_parts(selected_index, search.clone());

    // Use the generic table renderer
    super::table_renderer::draw_table_list(area, &component, items, &state, frame, "Port Forwards");
}

pub fn draw_port_forwarding_form_popup(
    area: Rect,
    form: &mut PortForwardingForm,
    connections: &[Connection],
    is_new: bool,
    frame: &mut ratatui::Frame<'_>,
) -> Rect {
    let title = if is_new {
        "New Port Forward"
    } else {
        "Edit Port Forward"
    };

    // Responsive popup sizing (same as connection form)
    let (popup_w, popup_h) = calculate_responsive_popup_size(area);

    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;

    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    // Clear background behind popup
    frame.render_widget(Clear, popup);

    // Create main block with title
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

    // Get inner area for form content
    let inner = popup.inner(Margin::new(1, 1));

    // Build field list based on forward type
    let mut field_list = vec![];

    // Forward Type
    field_list.push((
        "Forward Type",
        None::<&TextArea>,
        form.focus == FocusField::ForwardType,
        FocusField::ForwardType,
    ));

    // Connection field
    field_list.push((
        "Connection",
        None::<&TextArea>,
        form.focus == FocusField::Connection,
        FocusField::Connection,
    ));

    // Add fields based on forward type
    match form.forward_type {
        PortForwardType::Local => {
            field_list.push((
                "Local Address",
                Some(&form.local_addr),
                form.focus == FocusField::LocalAddr,
                FocusField::LocalAddr,
            ));
            field_list.push((
                "Local Port",
                Some(&form.local_port),
                form.focus == FocusField::LocalPort,
                FocusField::LocalPort,
            ));
            field_list.push((
                "Service Host",
                Some(&form.service_host),
                form.focus == FocusField::ServiceHost,
                FocusField::ServiceHost,
            ));
            field_list.push((
                "Service Port",
                Some(&form.service_port),
                form.focus == FocusField::ServicePort,
                FocusField::ServicePort,
            ));
        }
        PortForwardType::Remote => {
            field_list.push((
                "Remote Bind Address (optional)",
                Some(&form.remote_bind_addr),
                form.focus == FocusField::RemoteBind,
                FocusField::RemoteBind,
            ));
            field_list.push((
                "Remote Port",
                Some(&form.local_port),
                form.focus == FocusField::LocalPort,
                FocusField::LocalPort,
            ));
            field_list.push((
                "Local Service Host",
                Some(&form.service_host),
                form.focus == FocusField::ServiceHost,
                FocusField::ServiceHost,
            ));
            field_list.push((
                "Local Service Port",
                Some(&form.service_port),
                form.focus == FocusField::ServicePort,
                FocusField::ServicePort,
            ));
        }
        PortForwardType::Dynamic => {
            field_list.push((
                "Local Address",
                Some(&form.local_addr),
                form.focus == FocusField::LocalAddr,
                FocusField::LocalAddr,
            ));
            field_list.push((
                "Local Port (SOCKS5)",
                Some(&form.local_port),
                form.focus == FocusField::LocalPort,
                FocusField::LocalPort,
            ));
        }
    }

    // Display Name (always last)
    field_list.push((
        "Display Name (optional)",
        Some(&form.display_name),
        form.focus == FocusField::DisplayName,
        FocusField::DisplayName,
    ));

    // Create responsive layout based on available space and number of fields
    let layout = create_responsive_form_layout_with_count(inner, field_list.len());

    // Render form fields
    for (idx, (label, textarea_opt, focused, field_type)) in field_list.iter().enumerate() {
        if idx >= layout.len() {
            break; // Skip if layout doesn't have enough space
        }

        if *field_type == FocusField::ForwardType {
            // Render forward type selector
            render_forward_type_selector(frame, layout[idx], form, *focused);
        } else if let Some(textarea) = textarea_opt {
            // Render text area field
            let mut widget = (*textarea).clone();
            let mut field_block = Block::default().borders(Borders::ALL).title(*label);
            if *focused {
                field_block = field_block.border_style(Style::default().fg(Color::Cyan));
            } else {
                // Hide cursor when not focused
                widget.set_cursor_style(Style::default().bg(Color::Reset));
            }

            widget.set_block(field_block);
            frame.render_widget(&widget, layout[idx]);
        } else if *label == "Connection" {
            // Render connection dropdown field
            let connection_text = connections
                .iter()
                .find(|c| c.id == form.connection_id)
                .map(|c| c.display_name.as_str())
                .unwrap_or(" Press Space to Select Connection");

            let mut field_block = Block::default().borders(Borders::ALL).title("Connection");

            if *focused {
                field_block = field_block.border_style(Style::default().fg(Color::Cyan));
            }

            let connection_paragraph = Paragraph::new(Line::from(Span::styled(
                connection_text,
                if *focused {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if !form.connection_id.is_empty() {
                    Style::default()
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            )))
            .block(field_block);

            frame.render_widget(connection_paragraph, layout[idx]);
        }
    }

    // Try to show error if space available
    let error_idx = field_list.len();
    if let Some(error) = &form.error
        && error_idx < layout.len()
    {
        let error_paragraph = Paragraph::new(Line::from(Span::styled(
            error,
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(error_paragraph, layout[error_idx]);
    }

    // Render the main block first
    frame.render_widget(Paragraph::new("").block(block), popup);

    // Render instructions inside the popup, just above the bottom border
    let instructions_area = Rect {
        x: popup.x + 2,
        y: popup.y + popup.height.saturating_sub(2), // Position just above bottom border
        width: popup.width.saturating_sub(4),
        height: 1,
    };
    let instructions = create_responsive_instructions(instructions_area.width);
    frame.render_widget(instructions, instructions_area);

    // Return the Connection field position (first field, index 0)
    if !layout.is_empty() {
        layout[0]
    } else {
        // Fallback to popup position if layout is empty
        popup
    }
}

// Calculate responsive popup size based on terminal dimensions
fn calculate_responsive_popup_size(area: Rect) -> (u16, u16) {
    let width = area.width;
    let height = area.height;

    // Responsive width calculation
    let popup_w = if width < 60 {
        // Very small screen: use most of the width
        (width as f32 * 0.9) as u16
    } else if width < 100 {
        // Small screen: use 70% of width
        (width as f32 * 0.7) as u16
    } else if width < 150 {
        // Medium screen: use 50% of width
        (width as f32 * 0.5) as u16
    } else {
        // Large screen: use 35% of width but cap at reasonable size
        ((width as f32 * 0.35) as u16).min(80)
    };

    // Responsive height calculation
    let popup_h = if height < 20 {
        // Very small height: use most available space
        height.saturating_sub(2)
    } else if height < 30 {
        // Small height: prioritize essential fields
        height.saturating_sub(4)
    } else {
        // Normal height: use ideal size
        23u16.min(height.saturating_sub(4))
    };

    (popup_w.max(30), popup_h.max(12)) // Ensure minimum usable size
}

// Create responsive layout based on available space
// Render the forward type selector with horizontal radio buttons
fn render_forward_type_selector(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    form: &PortForwardingForm,
    focused: bool,
) {
    let marker_fn = |typ: PortForwardType| {
        if form.forward_type == typ { "✓" } else { " " }
    };

    let text = vec![Line::from(vec![
        Span::styled(
            "Forward Type: ",
            if focused {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        Span::styled(
            format!("[{}] Local →  ", marker_fn(PortForwardType::Local)),
            if matches!(form.forward_type, PortForwardType::Local) {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        Span::styled(
            format!("[{}] Remote ←  ", marker_fn(PortForwardType::Remote)),
            if matches!(form.forward_type, PortForwardType::Remote) {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        Span::styled(
            format!("[{}] Dynamic ⇄", marker_fn(PortForwardType::Dynamic)),
            if matches!(form.forward_type, PortForwardType::Dynamic) {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ])];

    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, area);
}

// Create responsive form layout with dynamic field count
fn create_responsive_form_layout_with_count(inner: Rect, field_count: usize) -> Vec<Rect> {
    let available_height = inner.height;

    // Connection needs 3 lines, ForwardType needs 3 lines (with spacing), others need 2-3 lines
    // Reserve 2 lines at bottom for error + instructions

    let mut constraints = vec![];

    if available_height < 20 {
        // Compact mode - Connection gets 3 lines, ForwardType gets 2, others get 2
        for i in 0..field_count {
            if i == 0 {
                // ForwardType field - 2 lines with spacing
                constraints.push(Constraint::Length(1));
            } else {
                constraints.push(Constraint::Length(2));
            }
        }
        constraints.push(Constraint::Min(0)); // spacer
        constraints.push(Constraint::Length(2)); // error + instructions space
    } else {
        // Normal mode - Connection gets 3 lines, ForwardType gets 3 lines, others get 3 lines
        for i in 0..field_count {
            if i == 0 {
                // ForwardType field - 3 lines for better spacing from Connection
                constraints.push(Constraint::Length(2));
            } else {
                constraints.push(Constraint::Length(3));
            }
        }
        constraints.push(Constraint::Min(0)); // spacer
        constraints.push(Constraint::Length(2)); // error + instructions space
    }

    Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner)
        .to_vec()
}

// Create responsive instructions based on available width
fn create_responsive_instructions(width: u16) -> Paragraph<'static> {
    if width < 40 {
        // Very narrow: minimal instructions
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]))
    } else if width < 60 {
        // Narrow: short form
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Nav  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Save  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Cancel"),
        ]))
    } else {
        // Full width: complete instructions
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Tab/Shift+Tab",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Navigate  "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Save  "),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Cancel"),
        ]))
    }
}

impl From<&PortForward> for PortForwardingForm {
    fn from(pf: &PortForward) -> Self {
        PortForwardingForm::from_port_forward(pf)
    }
}

// Port forward delete confirmation popup renderer
pub fn draw_port_forward_delete_confirmation_popup(
    area: Rect,
    port_forward_name: &str,
    frame: &mut ratatui::Frame<'_>,
) {
    let popup_w = (area.width as f32 * 0.35) as u16; // 35% of screen width for more compact look
    let popup_h = 8u16.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "Delete Port Forward",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));

    let inner = popup.inner(Margin::new(1, 1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // warning message
            Constraint::Length(1), // port forward name
            Constraint::Length(1), // empty line
            Constraint::Length(1), // confirmation question
            Constraint::Min(1),    // spacer to push buttons to bottom
            Constraint::Length(1), // buttons hint (bottom-aligned)
        ])
        .split(inner);

    // Warning message
    let warning = Paragraph::new(Line::from(Span::styled(
        "⚠️  Are you sure you want to delete this port forward?",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(warning, layout[0]);

    // Port forward name
    let port_forward_info = Paragraph::new(Line::from(vec![
        Span::styled("Port Forward: ", Style::default().fg(Color::Gray)),
        Span::styled(
            port_forward_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(port_forward_info, layout[1]);

    // Empty line for spacing
    frame.render_widget(Paragraph::new(""), layout[2]);

    // Confirmation question
    let question = Paragraph::new(Line::from(Span::styled(
        "This action cannot be undone.",
        Style::default().fg(Color::Red),
    )));
    frame.render_widget(question, layout[3]);

    // Button hints
    let buttons = Paragraph::new(Line::from(vec![
        Span::styled(
            "Y",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Delete   ", Style::default().fg(Color::White)),
        Span::styled(
            "N",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Cancel   ", Style::default().fg(Color::White)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" - Cancel", Style::default().fg(Color::White)),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(buttons, layout[5]);

    frame.render_widget(Paragraph::new("").block(block), popup);
}

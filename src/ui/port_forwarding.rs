use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Table, TableState,
};
use tui_textarea::TextArea;

use crate::config::manager::{Connection, PortForward, PortForwardStatus};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusField {
    Connection,
    LocalAddr,
    LocalPort,
    ServiceHost,
    ServicePort,
    DisplayName,
}

#[derive(Clone, Debug)]
pub struct PortForwardingForm {
    pub id: Option<String>, // Store the ID when editing
    pub connection_id: String,
    pub local_addr: TextArea<'static>,
    pub local_port: TextArea<'static>,
    pub service_host: TextArea<'static>,
    pub service_port: TextArea<'static>,
    pub display_name: TextArea<'static>,
    pub focus: FocusField,
    pub error: Option<String>,
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

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());

        Self {
            id: None, // No ID for new port forwards
            connection_id: String::new(),
            local_addr,
            local_port,
            service_host,
            service_port,
            display_name,
            focus: FocusField::Connection,
            error: None,
        }
    }

    pub fn next(&mut self) {
        self.focus = match self.focus {
            FocusField::Connection => FocusField::LocalAddr,
            FocusField::LocalAddr => FocusField::LocalPort,
            FocusField::LocalPort => FocusField::ServiceHost,
            FocusField::ServiceHost => FocusField::ServicePort,
            FocusField::ServicePort => FocusField::DisplayName,
            FocusField::DisplayName => FocusField::Connection,
        };
    }

    pub fn prev(&mut self) {
        self.focus = match self.focus {
            FocusField::Connection => FocusField::DisplayName,
            FocusField::LocalAddr => FocusField::Connection,
            FocusField::LocalPort => FocusField::LocalAddr,
            FocusField::ServiceHost => FocusField::LocalPort,
            FocusField::ServicePort => FocusField::ServiceHost,
            FocusField::DisplayName => FocusField::ServicePort,
        };
    }

    pub fn focused_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            FocusField::Connection => None, // Handled by dropdown
            FocusField::LocalAddr => Some(&mut self.local_addr),
            FocusField::LocalPort => Some(&mut self.local_port),
            FocusField::ServiceHost => Some(&mut self.service_host),
            FocusField::ServicePort => Some(&mut self.service_port),
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

    pub fn validate(&self, connections: &[Connection]) -> Result<(), String> {
        if self.connection_id.trim().is_empty() {
            return Err("Connection is required".into());
        }

        if !connections.iter().any(|c| c.id == self.connection_id) {
            return Err("Selected connection not found".into());
        }

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

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());
        if let Some(name) = &pf.display_name {
            display_name.insert_str(name);
        }

        Self {
            id: Some(pf.id.clone()), // Preserve the ID when editing
            connection_id: pf.connection_id.clone(),
            local_addr,
            local_port,
            service_host,
            service_port,
            display_name,
            focus: FocusField::Connection,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PortForwardingListItem {
    pub status_icon: String,
    pub display_name: String,
    pub connection_name: String,
    pub local_address: String,
    pub service_address: String,
    pub created_at: String,
}

pub fn draw_port_forwarding_list(
    area: Rect,
    port_forwards: &[PortForward],
    connections: &[Connection],
    selected_index: usize,
    search_mode: bool,
    search_query: &str,
    frame: &mut ratatui::Frame<'_>,
) {
    let mut items: Vec<PortForwardingListItem> = port_forwards
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

            PortForwardingListItem {
                status_icon: status_icon.to_string(),
                display_name: pf.get_display_name(),
                connection_name: connection.to_string(),
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

    // Filter items based on search query
    if !search_query.is_empty() {
        items.retain(|item| {
            item.display_name
                .to_lowercase()
                .contains(&search_query.to_lowercase())
                || item
                    .connection_name
                    .to_lowercase()
                    .contains(&search_query.to_lowercase())
                || item
                    .local_address
                    .to_lowercase()
                    .contains(&search_query.to_lowercase())
                || item
                    .service_address
                    .to_lowercase()
                    .contains(&search_query.to_lowercase())
        });
    }

    let sel = if items.is_empty() {
        0
    } else {
        selected_index.min(items.len() - 1)
    };

    // In search mode, the area passed is already the table area, so no need for layout splitting
    let layout = if search_mode {
        vec![area] // Just use the entire area for the table
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area)
            .to_vec()
    };

    // Create table header
    let header = Row::new(vec![
        Cell::from("Status"),
        Cell::from("Name"),
        Cell::from("Connection"),
        Cell::from("Local"),
        Cell::from("Service"),
        Cell::from("Created"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // Create table rows
    let rows: Vec<Row> = items
        .iter()
        .map(|item| {
            let status_color = match item.status_icon.as_str() {
                "●" => Color::Green,
                "○" => Color::Gray,
                "✗" => Color::Red,
                _ => Color::White,
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    &item.status_icon,
                    Style::default().fg(status_color),
                )),
                Cell::from(item.display_name.clone()),
                Cell::from(item.connection_name.clone()),
                Cell::from(item.local_address.clone()),
                Cell::from(item.service_address.clone()),
                Cell::from(item.created_at.clone()),
            ])
            .height(1)
        })
        .collect();

    // Create the table
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),  // Status
            Constraint::Min(12),    // Name
            Constraint::Min(10),    // Connection
            Constraint::Min(12),    // Local
            Constraint::Min(12),    // Service
            Constraint::Length(16), // Created
            Constraint::Length(1),  // Scrollbar
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(format!(
        "Port Forwards ({}/{})",
        if !items.is_empty() { sel + 1 } else { 0 },
        items.len()
    )))
    .highlight_style(
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    // Render the table with state
    let mut table_state = TableState::default().with_selected(Some(sel));
    frame.render_stateful_widget(table, layout[0], &mut table_state);

    // Render vertical scrollbar only if content exceeds one page (visible rows)
    if !items.is_empty() {
        let inner_area = layout[0].inner(ratatui::layout::Margin::new(1, 2));
        // inner_area includes header row; visible rows are inner height - 1 (for header)
        let visible_rows = inner_area.height.saturating_sub(1) as usize;
        let content_length = items.len();
        if content_length > visible_rows {
            // Compute page-aware scrollbar positions
            let max_top = content_length.saturating_sub(visible_rows);
            let centered_top = sel.saturating_sub(visible_rows.saturating_sub(1) / 2);
            let top_index = centered_top.min(max_top);
            let total_positions = max_top.saturating_add(1);

            let mut scroll_state = ScrollbarState::new(total_positions).position(top_index);

            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);

            frame.render_stateful_widget(scrollbar, inner_area, &mut scroll_state);
        }
    }

    // Only render footer in normal mode (search mode footer is handled in main.rs)
    if !search_mode {
        let footer_area = layout[1];
        let footer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
            .split(footer_area);

        let hint_text = "Enter: Start/Stop   N: New   E: Edit   D: Delete   Esc: Back   /: Search";

        let left = Paragraph::new(Line::from(Span::styled(
            hint_text,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )))
        .alignment(Alignment::Left);

        let right = Paragraph::new(Line::from(Span::styled(
            format!("TermiRs v{}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )))
        .alignment(Alignment::Right);

        frame.render_widget(left, footer[0]);
        frame.render_widget(right, footer[1]);
    }
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

    // Create responsive layout based on available space
    let layout = create_responsive_form_layout(inner);

    // Render form fields based on available layout space
    let field_configs = [
        ("Connection", None, form.focus == FocusField::Connection),
        (
            "Local Address",
            Some(&form.local_addr),
            form.focus == FocusField::LocalAddr,
        ),
        (
            "Local Port",
            Some(&form.local_port),
            form.focus == FocusField::LocalPort,
        ),
        (
            "Service Host",
            Some(&form.service_host),
            form.focus == FocusField::ServiceHost,
        ),
        (
            "Service Port",
            Some(&form.service_port),
            form.focus == FocusField::ServicePort,
        ),
        (
            "Display Name (optional)",
            Some(&form.display_name),
            form.focus == FocusField::DisplayName,
        ),
    ];

    for (idx, (label, textarea_opt, focused)) in field_configs.iter().enumerate() {
        if idx >= layout.len() {
            break; // Skip if layout doesn't have enough space
        }

        if let Some(textarea) = textarea_opt {
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
                .unwrap_or("Press Space to Select Connection");

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
                } else {
                    Style::default()
                },
            )))
            .block(field_block);

            frame.render_widget(connection_paragraph, layout[idx]);
        }
    }

    // Try to show error if space available
    let error_idx = field_configs.len();
    if let Some(error) = &form.error {
        if error_idx < layout.len() {
            let error_paragraph = Paragraph::new(Line::from(Span::styled(
                error,
                Style::default().fg(Color::Red),
            )));
            frame.render_widget(error_paragraph, layout[error_idx]);
        }
    }

    // Render the main block first
    frame.render_widget(Paragraph::new("").block(block), popup);

    // Render instructions at the very bottom of the popup, overlapping the border
    let instructions_area = Rect {
        x: popup.x + 1,
        y: popup.y + popup.height - 2, // Position at bottom border line
        width: popup.width.saturating_sub(2),
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
fn create_responsive_form_layout(inner: Rect) -> Vec<Rect> {
    let available_height = inner.height;

    let layout_rects = if available_height < 15 {
        // Very compact layout: minimize spacing
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // connection
                Constraint::Length(2), // local address
                Constraint::Length(2), // local port
                Constraint::Length(2), // service host
                Constraint::Length(2), // service port
                Constraint::Length(2), // display name
                Constraint::Min(0),    // flexible spacer
            ])
            .split(inner)
    } else if available_height < 20 {
        // Compact layout: reduce field height slightly
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // connection
                Constraint::Length(2), // local address
                Constraint::Length(2), // local port
                Constraint::Length(2), // service host
                Constraint::Length(2), // service port
                Constraint::Length(2), // display name
                Constraint::Min(0),    // flexible spacer
                Constraint::Length(1), // error (if any)
            ])
            .split(inner)
    } else {
        // Normal layout: comfortable spacing
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // connection
                Constraint::Length(3), // local address
                Constraint::Length(3), // local port
                Constraint::Length(3), // service host
                Constraint::Length(3), // service port
                Constraint::Length(3), // display name
                Constraint::Min(0),    // flexible spacer
                Constraint::Length(1), // error (if any)
            ])
            .split(inner)
    };

    layout_rects.to_vec()
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

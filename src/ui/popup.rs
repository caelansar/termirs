use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tui_textarea::TextArea;

use crate::ui::connection::{ConnectionForm, FocusField};

// Error popup renderer
pub fn draw_error_popup(area: Rect, message: &str, frame: &mut ratatui::Frame<'_>) {
    let popup_w = (area.width as f32 * 0.45) as u16;
    let inner_w = popup_w.saturating_sub(2).max(1);
    let estimated_lines: u16 = message
        .lines()
        .map(|l| {
            let len = l.chars().count() as u16;
            if len == 0 { 1 } else { len.div_ceil(inner_w) }
        })
        .sum();
    let content_h = estimated_lines.max(1) + 4; // title + message + hint
    let popup_h = content_h.min(area.height.saturating_sub(2));
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
            "Error",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    let body = Paragraph::new(vec![
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(Color::Red),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Press Enter or Esc to dismiss",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

// Info popup renderer
pub fn draw_info_popup(area: Rect, message: &str, frame: &mut ratatui::Frame<'_>) {
    let popup_w = (area.width as f32 * 0.45) as u16;
    let inner_w = popup_w.saturating_sub(2).max(1);
    let estimated_lines: u16 = message
        .lines()
        .map(|l| {
            let len = l.chars().count() as u16;
            if len == 0 { 1 } else { len.div_ceil(inner_w) }
        })
        .sum();
    let content_h = estimated_lines.max(1) + 4; // title + message + hint
    let popup_h = content_h.min(area.height.saturating_sub(2));
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
            "Info",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
    let body = Paragraph::new(vec![
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(Color::Green),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Press Enter or Esc to dismiss",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

// Connecting popup renderer (shows cancellation hint at bottom)
pub fn draw_connecting_popup(area: Rect, message: &str, frame: &mut ratatui::Frame<'_>) {
    let popup_w = (area.width as f32 * 0.45) as u16;
    let inner_w = popup_w.saturating_sub(2).max(1);
    let estimated_lines: u16 = message
        .lines()
        .map(|l| {
            let len = l.chars().count() as u16;
            if len == 0 { 1 } else { len.div_ceil(inner_w) }
        })
        .sum();
    let content_h = estimated_lines.max(1) + 4; // title + message + hint
    let popup_h = content_h.min(area.height.saturating_sub(2));
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
            "Info",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    let body = Paragraph::new(vec![
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Press ESC to cancel",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

// Delete confirmation popup renderer
pub fn draw_delete_confirmation_popup(
    area: Rect,
    connection_name: &str,
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
            "Delete Connection",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));

    let inner = popup.inner(Margin::new(1, 1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // warning message
            Constraint::Length(1), // connection name
            Constraint::Length(1), // empty line
            Constraint::Length(1), // confirmation question
            Constraint::Min(1),    // spacer to push buttons to bottom
            Constraint::Length(1), // buttons hint (bottom-aligned)
        ])
        .split(inner);

    // Warning message
    let warning = Paragraph::new(Line::from(Span::styled(
        "⚠️  Are you sure you want to delete this connection?",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(warning, layout[0]);

    // Connection name
    let connection_info = Paragraph::new(Line::from(vec![
        Span::styled("Connection: ", Style::default().fg(Color::Gray)),
        Span::styled(
            connection_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(connection_info, layout[1]);

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

pub fn draw_connection_form_popup(
    area: Rect,
    form: &ConnectionForm,
    new: bool,
    frame: &mut ratatui::Frame<'_>,
) {
    draw_connection_form_popup_with_mode(area, form, new, frame);
}

fn draw_connection_form_popup_with_mode(
    area: Rect,
    form: &ConnectionForm,
    new: bool,
    frame: &mut ratatui::Frame<'_>,
) {
    let title = if new {
        "New SSH Connection / Import from SSH Config"
    } else {
        "Edit SSH Connection"
    };

    // Responsive popup sizing
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

    // Helper function to render text areas with responsive styling
    let mut render_textarea = |idx: usize, label: &str, textarea: &TextArea, focused: bool| {
        if idx >= layout.len() {
            return; // Skip if layout doesn't have enough space
        }

        let mut widget = textarea.clone();
        let mut field_block = Block::default().borders(Borders::ALL).title(label);
        if focused {
            field_block = field_block.border_style(Style::default().fg(Color::Cyan));
        } else {
            // Hide cursor when not focused
            widget.set_cursor_style(Style::default().bg(Color::Reset));
        }

        widget.set_block(field_block);
        frame.render_widget(&widget, layout[idx]);
    };

    // Render form fields based on available layout space
    let field_configs = [
        ("Host", &form.host, form.focus == FocusField::Host),
        ("Port", &form.port, form.focus == FocusField::Port),
        (
            "Username",
            &form.username,
            form.focus == FocusField::Username,
        ),
        (
            "Password",
            &form.password,
            form.focus == FocusField::Password,
        ),
        (
            "Private Key Path",
            &form.private_key_path,
            form.focus == FocusField::PrivateKeyPath,
        ),
        (
            "Display Name (optional)",
            &form.display_name,
            form.focus == FocusField::DisplayName,
        ),
    ];

    for (idx, (label, textarea, focused)) in field_configs.iter().enumerate() {
        render_textarea(idx, label, textarea, *focused);
    }

    // Try to show error if space available
    let error_idx = field_configs.len();
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

    // Render instructions at the very bottom of the popup, overlapping the border
    let instructions_area = Rect {
        x: popup.x + 1,
        y: popup.y + popup.height - 2, // Position at bottom border line
        width: popup.width.saturating_sub(2),
        height: 1,
    };
    let instructions = if new {
        create_responsive_instructions_with_import(instructions_area.width)
    } else {
        create_responsive_instructions(instructions_area.width)
    };
    frame.render_widget(instructions, instructions_area);
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
                Constraint::Length(2), // host
                Constraint::Length(2), // port
                Constraint::Length(2), // username
                Constraint::Length(2), // password
                Constraint::Length(2), // private key
                Constraint::Length(2), // display name
                Constraint::Min(0),    // flexible spacer
            ])
            .split(inner)
    } else if available_height < 20 {
        // Compact layout: reduce field height slightly
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // host
                Constraint::Length(2), // port
                Constraint::Length(2), // username
                Constraint::Length(2), // password
                Constraint::Length(2), // private key
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
                Constraint::Length(3), // host
                Constraint::Length(3), // port
                Constraint::Length(3), // username
                Constraint::Length(3), // password
                Constraint::Length(3), // private key
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

fn create_responsive_instructions_with_import(width: u16) -> Paragraph<'static> {
    if width < 50 {
        // Very narrow: minimal instructions
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Ctrl+L",
                Style::default()
                    .fg(Color::Yellow)
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
    } else if width < 80 {
        // Narrow: short form
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Ctrl+L",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Load  "),
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
                "Ctrl+L",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Load from SSH Config  "),
            Span::styled(
                "Tab",
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

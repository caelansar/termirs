use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

// Error popup renderer
pub fn draw_error_popup(area: Rect, message: &str, frame: &mut ratatui::Frame<'_>) {
    let popup_w = area.width.saturating_sub(4);
    let inner_w = popup_w.saturating_sub(2).max(1);
    let estimated_lines: u16 = message
        .lines()
        .map(|l| {
            let len = l.chars().count() as u16;
            if len == 0 {
                1
            } else {
                (len + inner_w - 1) / inner_w
            }
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
    let popup_w = area.width.saturating_sub(4);
    let inner_w = popup_w.saturating_sub(2).max(1);
    let estimated_lines: u16 = message
        .lines()
        .map(|l| {
            let len = l.chars().count() as u16;
            if len == 0 {
                1
            } else {
                (len + inner_w - 1) / inner_w
            }
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

// Delete confirmation popup renderer
pub fn draw_delete_confirmation_popup(
    area: Rect,
    connection_name: &str,
    frame: &mut ratatui::Frame<'_>,
) {
    let popup_w = area.width.saturating_sub(10).max(50);
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
            Constraint::Length(1), // buttons hint
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
    frame.render_widget(buttons, layout[4]);

    frame.render_widget(Paragraph::new("").block(block), popup);
}

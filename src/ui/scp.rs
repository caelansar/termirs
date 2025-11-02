use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};
use tui_textarea::TextArea;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpFocusField {
    LocalPath,
    RemotePath,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpMode {
    Send,
    Receive,
}

#[derive(Clone, Debug)]
pub struct ScpForm {
    pub local_path: TextArea<'static>,
    pub remote_path: TextArea<'static>,
    pub focus: ScpFocusField,
    pub mode: ScpMode,
}

impl ScpForm {
    pub fn new() -> Self {
        Self::new_with_mode(ScpMode::Send)
    }

    pub fn new_with_mode(mode: ScpMode) -> Self {
        let mut local_path = TextArea::default();
        let mut remote_path = TextArea::default();

        match mode {
            ScpMode::Send => {
                local_path.set_placeholder_text("Enter local file path");
                remote_path.set_placeholder_text("Enter remote file path");
            }
            ScpMode::Receive => {
                local_path.set_placeholder_text("Enter local destination path");
                remote_path.set_placeholder_text("Enter remote file path");
            }
        }

        local_path.set_cursor_line_style(Style::default());
        remote_path.set_cursor_line_style(Style::default());

        Self {
            local_path,
            remote_path,
            focus: match mode {
                ScpMode::Send => ScpFocusField::LocalPath,
                ScpMode::Receive => ScpFocusField::RemotePath,
            },
            mode,
        }
    }

    pub fn next(&mut self) {
        self.focus = match self.focus {
            ScpFocusField::LocalPath => ScpFocusField::RemotePath,
            ScpFocusField::RemotePath => ScpFocusField::LocalPath,
        };
    }

    pub fn prev(&mut self) {
        self.next();
    }

    pub fn focused_textarea_mut(&mut self) -> &mut TextArea<'static> {
        match self.focus {
            ScpFocusField::LocalPath => &mut self.local_path,
            ScpFocusField::RemotePath => &mut self.remote_path,
        }
    }

    pub fn get_local_path_value(&self) -> &str {
        &self.local_path.lines()[0]
    }

    pub fn get_remote_path_value(&self) -> &str {
        &self.remote_path.lines()[0]
    }
}

// SCP Progress popup renderer
pub fn draw_scp_progress_popup(
    area: Rect,
    progress: &crate::ScpProgress,
    frame: &mut ratatui::Frame<'_>,
) {
    let file_count = progress.files.len().max(1);
    let popup_w = (area.width as f32 * 0.45) as u16;
    let ideal_height = 4 + (file_count as u16) * 3;
    let popup_h = ideal_height.min(area.height.saturating_sub(2)).max(6);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    frame.render_widget(Clear, popup);

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "SFTP Transfers",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    frame.render_widget(outer, popup);

    let inner = popup.inner(Margin::new(1, 1));

    let mut constraints = vec![Constraint::Length(1)];
    for _ in &progress.files {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(1));

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Connection info
    let connection_info = Paragraph::new(Line::from(vec![
        Span::styled("Connection: ", Style::default().fg(Color::Gray)),
        Span::styled(
            progress.connection_name.clone(),
            Style::default().fg(Color::Cyan),
        ),
    ]));
    frame.render_widget(connection_info, layout[0]);

    for (idx, file) in progress.files.iter().enumerate() {
        let row = layout.get(idx + 1).copied().unwrap_or_else(|| Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 0,
        });
        if row.height < 3 {
            continue;
        }

        let file_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(row);

        let (status_label, status_style): (String, Style) = match &file.state {
            crate::TransferState::Pending => {
                ("Pending".to_string(), Style::default().fg(Color::Gray))
            }
            crate::TransferState::InProgress => (
                "In Progress".to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            crate::TransferState::Completed => (
                "Completed".to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            crate::TransferState::Failed(err) => (
                format!("Failed ({err})"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };

        let header = Paragraph::new(Line::from(vec![
            Span::styled(format!("{:<11}", status_label), status_style),
            Span::styled(file.display_name.clone(), Style::default().fg(Color::White)),
        ]));
        frame.render_widget(header, file_chunks[0]);

        let (from_path, to_path) = match file.mode {
            ScpMode::Send => (&file.local_path, &file.remote_path),
            ScpMode::Receive => (&file.remote_path, &file.local_path),
        };
        let path_line = Paragraph::new(Line::from(vec![
            Span::styled("From: ", Style::default().fg(Color::Gray)),
            Span::styled(from_path.clone(), Style::default().fg(Color::White)),
            Span::styled("  To: ", Style::default().fg(Color::Gray)),
            Span::styled(to_path.clone(), Style::default().fg(Color::White)),
        ]));
        frame.render_widget(path_line, file_chunks[1]);

        let ratio = file.ratio();
        let gauge_label = if let Some(total) = file.total_bytes {
            let percent = ratio * 100.0;
            Span::styled(
                format!(
                    "{} / {} ({percent:.1}%)",
                    format_bytes(file.transferred_bytes),
                    format_bytes(total)
                ),
                Style::default().fg(Color::White),
            )
        } else {
            Span::styled(
                format!("{} transferred", format_bytes(file.transferred_bytes)),
                Style::default().fg(Color::White),
            )
        };

        let gauge_color = match file.state {
            crate::TransferState::Pending
            | crate::TransferState::InProgress
            | crate::TransferState::Completed => Color::Cyan,
            crate::TransferState::Failed(_) => Color::Red,
        };

        let gauge = Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(gauge_color)
                    .bg(Color::Reset)
                    .add_modifier(Modifier::BOLD),
            )
            .ratio(ratio)
            .label(gauge_label)
            .use_unicode(true);
        frame.render_widget(gauge, file_chunks[2]);
    }

    if let Some(time_area) = layout.last().copied() {
        let elapsed = if progress.completed {
            if let Some(results) = &progress.completion_results {
                let mut max_time = progress.start_time.elapsed();
                if let Some(last) = results.iter().filter_map(|res| res.completed_at).max() {
                    max_time = last.duration_since(progress.start_time);
                }
                max_time
            } else {
                progress.start_time.elapsed()
            }
        } else {
            progress.start_time.elapsed()
        };

        let mut elapsed_text = format!("Elapsed: {:.1}s", elapsed.as_secs_f32());
        if progress.completed {
            elapsed_text.push_str("  •  Press Enter or Esc to close");
        }
        let time_info = Paragraph::new(Line::from(Span::styled(
            elapsed_text,
            Style::default().fg(Color::Gray),
        )));
        frame.render_widget(time_info, time_area);
    }
}

pub fn draw_scp_popup(area: Rect, form: &ScpForm, frame: &mut ratatui::Frame<'_>) -> (Rect, Rect) {
    let popup_w = (area.width as f32 * 0.35) as u16; // 35% of screen width for more compact look
    let popup_h = 9u16.min(area.height.saturating_sub(2)).max(7);
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    frame.render_widget(Clear, popup);

    let title = match form.mode {
        ScpMode::Send => "SFTP: Send File",
        ScpMode::Receive => "SFTP: Receive File",
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    frame.render_widget(outer, popup);

    let inner = popup.inner(Margin::new(1, 1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // local
            Constraint::Length(3), // remote
            Constraint::Length(1), // hint
        ])
        .split(inner);

    let mut render_textarea =
        |idx: usize, label: &str, textarea: &TextArea, focused: bool| -> Rect {
            let mut widget = textarea.clone();
            let mut block = Block::default().borders(Borders::ALL).title(label);
            if focused {
                block = block.border_style(Style::default().fg(Color::Cyan));
            } else {
                // Hide cursor when not focused
                widget.set_cursor_style(Style::default().bg(Color::Reset));
            }
            widget.set_block(block);
            frame.render_widget(&widget, layout[idx]);
            layout[idx]
        };

    let local_path_rect = render_textarea(
        0,
        "Local Path",
        &form.local_path,
        form.focus == ScpFocusField::LocalPath,
    );
    let remote_path_rect = render_textarea(
        1,
        "Remote Path",
        &form.remote_path,
        form.focus == ScpFocusField::RemotePath,
    );

    let hint_text = match form.mode {
        ScpMode::Send => {
            "Enter: Send   Ctrl+R: Receive Mode   Esc: Cancel   Tab: Complete   Up/Down: Switch Field"
        }
        ScpMode::Receive => {
            "Enter: Receive   Ctrl+R: Send Mode   Esc: Cancel   Tab: Complete   Up/Down: Switch Field"
        }
    };

    let hint = Paragraph::new(Line::from(Span::styled(
        hint_text,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )));
    frame.render_widget(hint, layout[2]);

    (local_path_rect, remote_path_rect)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit_index = 0;

    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

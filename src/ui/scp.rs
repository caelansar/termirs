use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};

use crate::ScpProgress;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpMode {
    Send,
    Receive,
}

// SCP Progress popup renderer
pub fn draw_scp_progress_popup(area: Rect, progress: &ScpProgress, frame: &mut ratatui::Frame<'_>) {
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
        let row = layout.get(idx + 1).copied().unwrap_or(Rect {
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
            Span::styled(format!("{status_label:<12}"), status_style),
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

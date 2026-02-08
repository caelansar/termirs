use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};
use std::borrow::Cow;

use crate::ScpProgress;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpMode {
    Send,
    Receive,
}

// SCP Progress popup renderer
pub fn draw_scp_progress_popup(
    area: Rect,
    progress: &mut ScpProgress,
    frame: &mut ratatui::Frame<'_>,
) {
    let file_count = progress.files.len();
    let popup_w = (area.width as f32 * 0.45) as u16;

    // Calculate how many files we can show given available space
    // Layout: 1 (connection) + N*3 (files) + optional 1 (scroll indicator) + 1 (elapsed)
    let max_popup_h = area.height.saturating_sub(2);
    // Reserve 4 lines for connection header, footer, and borders
    let available_for_files = max_popup_h.saturating_sub(6) as usize;
    let max_visible = (available_for_files / 3).max(1);
    let needs_scroll = file_count > max_visible;

    // Auto-scroll to keep the active (in-progress) file visible
    if needs_scroll {
        let active_idx = progress
            .files
            .iter()
            .position(|f| matches!(f.state, crate::TransferState::InProgress))
            .or_else(|| {
                progress
                    .files
                    .iter()
                    .position(|f| matches!(f.state, crate::TransferState::Pending))
            })
            .unwrap_or(progress.files.len().saturating_sub(1));

        if active_idx < progress.scroll_offset {
            progress.scroll_offset = active_idx;
        } else if active_idx >= progress.scroll_offset + max_visible {
            progress.scroll_offset = active_idx.saturating_sub(max_visible - 1);
        }
        // Clamp
        let max_offset = file_count.saturating_sub(max_visible);
        if progress.scroll_offset > max_offset {
            progress.scroll_offset = max_offset;
        }
    } else {
        progress.scroll_offset = 0;
    }

    let visible_count = file_count.min(max_visible);
    let has_above = needs_scroll && progress.scroll_offset > 0;
    let has_below = needs_scroll && progress.scroll_offset + visible_count < file_count;

    // Calculate popup height
    let content_lines = 1 // connection info
        + (visible_count as u16) * 3
        + if has_above { 1 } else { 0 }
        + if has_below { 1 } else { 0 }
        + 1; // elapsed time
    let ideal_height = content_lines + 2; // +2 for borders
    let popup_h = ideal_height.min(max_popup_h).max(6);

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

    // Build constraints for visible section
    let mut constraints = vec![Constraint::Length(1)]; // connection info
    if has_above {
        constraints.push(Constraint::Length(1));
    }
    for _ in 0..visible_count {
        constraints.push(Constraint::Length(3));
    }
    if has_below {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // elapsed time

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut layout_idx = 0;

    // Connection info
    let connection_info = Paragraph::new(Line::from(vec![
        Span::styled("Connection: ", Style::default().fg(Color::Gray)),
        Span::styled(&progress.connection_name, Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(connection_info, layout[layout_idx]);
    layout_idx += 1;

    // Scroll-up indicator
    if has_above {
        let above_count = progress.scroll_offset;
        let indicator = Paragraph::new(Line::from(Span::styled(
            format!("  ... {above_count} more above ..."),
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(indicator, layout[layout_idx]);
        layout_idx += 1;
    }

    // Visible files
    let visible_files =
        &progress.files[progress.scroll_offset..progress.scroll_offset + visible_count];
    for file in visible_files {
        let row = layout[layout_idx];
        layout_idx += 1;

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

        let (status_label, status_style) = match &file.state {
            crate::TransferState::Pending => {
                (Cow::Borrowed("Pending"), Style::default().fg(Color::Gray))
            }
            crate::TransferState::InProgress => (
                Cow::Borrowed("In Progress"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            crate::TransferState::Completed => (
                Cow::Borrowed("Completed"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            crate::TransferState::Failed(err) => (
                Cow::Owned(format!("Failed ({err})")),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        };

        let header = Paragraph::new(Line::from(vec![
            Span::styled(format!("{status_label:<12}"), status_style),
            Span::styled(&file.display_name, Style::default().fg(Color::White)),
        ]));
        frame.render_widget(header, file_chunks[0]);

        let (from_path, to_path) = match file.mode {
            ScpMode::Send => (&file.local_path, &file.remote_path),
            ScpMode::Receive => (&file.remote_path, &file.local_path),
        };
        let path_line = Paragraph::new(Line::from(vec![
            Span::styled("From: ", Style::default().fg(Color::Gray)),
            Span::styled(from_path.as_str(), Style::default().fg(Color::White)),
            Span::styled("  To: ", Style::default().fg(Color::Gray)),
            Span::styled(to_path.as_str(), Style::default().fg(Color::White)),
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

    // Scroll-down indicator
    if has_below {
        let below_count = file_count - (progress.scroll_offset + visible_count);
        let indicator = Paragraph::new(Line::from(Span::styled(
            format!("  ... {below_count} more below ..."),
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(indicator, layout[layout_idx]);
        layout_idx += 1;
    }

    // Elapsed time footer
    if let Some(time_area) = layout.get(layout_idx).copied() {
        // Determine elapsed time - freeze when all files hit 100% or when completed
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
        } else if let Some(done_at) = progress.all_files_done_at {
            // Freeze elapsed time when all files reached 100% (before ScpResult arrives)
            done_at.duration_since(progress.start_time)
        } else {
            progress.start_time.elapsed()
        };

        let mut elapsed_text = format!("Elapsed: {:.1}s", elapsed.as_secs_f32());
        if file_count > 0 {
            let completed = progress
                .files
                .iter()
                .filter(|f| {
                    matches!(
                        f.state,
                        crate::TransferState::Completed | crate::TransferState::Failed(_)
                    )
                })
                .count();
            elapsed_text = format!("{elapsed_text}  ({completed}/{file_count} files)");
        }
        // Show completion hint when all files are done or fully completed
        if progress.completed || progress.all_files_done_at.is_some() {
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

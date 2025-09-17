use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tui_textarea::TextArea;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpFocusField {
    LocalPath,
    RemotePath,
}

#[derive(Clone, Debug)]
pub struct ScpForm {
    pub local_path: TextArea<'static>,
    pub remote_path: TextArea<'static>,
    pub focus: ScpFocusField,
}

impl ScpForm {
    pub fn new() -> Self {
        let mut local_path = TextArea::default();
        local_path.set_placeholder_text("Enter local file path");
        local_path.set_cursor_line_style(Style::default());

        let mut remote_path = TextArea::default();
        remote_path.set_placeholder_text("Enter remote file path");
        remote_path.set_cursor_line_style(Style::default());

        Self {
            local_path,
            remote_path,
            focus: ScpFocusField::LocalPath,
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

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "SCP Transfer in Progress",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    frame.render_widget(outer, popup);

    let inner = popup.inner(Margin::new(1, 1));
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // connection info
            Constraint::Length(1), // local path
            Constraint::Length(1), // remote path
            Constraint::Length(1), // progress indicator
            Constraint::Length(1), // elapsed time
        ])
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

    // Local path
    let local_info = Paragraph::new(Line::from(vec![
        Span::styled("From: ", Style::default().fg(Color::Gray)),
        Span::styled(
            progress.local_path.clone(),
            Style::default().fg(Color::White),
        ),
    ]));
    frame.render_widget(local_info, layout[1]);

    // Remote path
    let remote_info = Paragraph::new(Line::from(vec![
        Span::styled("To: ", Style::default().fg(Color::Gray)),
        Span::styled(
            progress.remote_path.clone(),
            Style::default().fg(Color::White),
        ),
    ]));
    frame.render_widget(remote_info, layout[2]);

    // Progress indicator with spinner
    let spinner_char = progress.get_spinner_char();
    let progress_text = Paragraph::new(Line::from(vec![
        Span::styled("Uploading ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{}", spinner_char),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(progress_text, layout[3]);

    // Elapsed time
    let elapsed = progress.start_time.elapsed();
    let elapsed_text = format!("Elapsed: {:.1}s", elapsed.as_secs_f32());
    let time_info = Paragraph::new(Line::from(Span::styled(
        elapsed_text,
        Style::default().fg(Color::Gray),
    )));
    frame.render_widget(time_info, layout[4]);
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

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "SFTP: Send File",
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

    let hint = Paragraph::new(Line::from(Span::styled(
        "Enter: Send   Esc: Cancel   Tab: Complete   Up/Down: Switch Field",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )));
    frame.render_widget(hint, layout[2]);

    (local_path_rect, remote_path_rect)
}

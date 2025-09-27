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

    let title = match progress.mode {
        ScpMode::Send => "SFTP Send in Progress",
        ScpMode::Receive => "SFTP Receive in Progress",
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
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

    // Path info based on mode
    let (from_label, from_path, to_label, to_path) = match progress.mode {
        ScpMode::Send => (
            "From: ",
            &progress.local_path,
            "To: ",
            &progress.remote_path,
        ),
        ScpMode::Receive => (
            "From: ",
            &progress.remote_path,
            "To: ",
            &progress.local_path,
        ),
    };

    let local_info = Paragraph::new(Line::from(vec![
        Span::styled(from_label, Style::default().fg(Color::Gray)),
        Span::styled(from_path.clone(), Style::default().fg(Color::White)),
    ]));
    frame.render_widget(local_info, layout[1]);

    let remote_info = Paragraph::new(Line::from(vec![
        Span::styled(to_label, Style::default().fg(Color::Gray)),
        Span::styled(to_path.clone(), Style::default().fg(Color::White)),
    ]));
    frame.render_widget(remote_info, layout[2]);

    // Progress indicator with spinner
    let spinner_char = progress.get_spinner_char();
    let action_text = match progress.mode {
        ScpMode::Send => "Uploading ",
        ScpMode::Receive => "Downloading ",
    };

    let progress_text = Paragraph::new(Line::from(vec![
        Span::styled(action_text, Style::default().fg(Color::Yellow)),
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

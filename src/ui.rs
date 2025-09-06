use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use vt100::{Color as VtColor, Parser};

pub struct TerminalState {
    pub parser: Parser,
    pub last_change: Instant,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
            last_change: Instant::now(),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.last_change = Instant::now();
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.last_change = Instant::now();
    }
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(n) => Color::Indexed(n),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn draw_terminal(area: Rect, state: &TerminalState, frame: &mut ratatui::Frame<'_>) {
    let height = area.height;
    let width = area.width;
    let mut lines: Vec<Line> = Vec::with_capacity(height as usize);
    let screen = state.parser.screen();

    for row in 0..height {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();

        for col in 0..width {
            if let Some(cell) = screen.cell(row, col) {
                let mut fg = map_color(cell.fgcolor());
                let mut bg = map_color(cell.bgcolor());
                let bold = cell.bold();
                let italic = cell.italic();
                let underline = cell.underline();
                let inverse = cell.inverse();

                if inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let mut style = Style::default().fg(fg).bg(bg);
                if bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if italic {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if underline {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }

                let contents = cell.contents();
                let to_append = if contents.is_empty() { " " } else { contents };

                if style == current_style {
                    current_text.push_str(to_append);
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = style;
                    current_text.push_str(to_append);
                }
            } else {
                if current_style == Style::default() {
                    current_text.push(' ');
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = Style::default();
                    current_text.push(' ');
                }
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        lines.push(Line::from(spans));
    }

    let term_block = Block::default().borders(Borders::ALL).title("$ terminal");
    let para = Paragraph::new(lines).block(term_block);
    frame.render_widget(para, area);

    let (cur_row, cur_col) = screen.cursor_position();
    if !screen.hide_cursor() {
        let cursor_x = area.x + 1 + cur_col;
        let cursor_y = area.y + 1 + cur_row;
        frame.set_cursor(cursor_x, cursor_y);
    }
}

use ratatui::layout::{Constraint, Direction, Layout, Margin};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusField {
    Host,
    Port,
    Username,
    Password,
    DisplayName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionForm {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub display_name: String,
    pub focus: FocusField,
    pub error: Option<String>,
}

impl ConnectionForm {
    pub fn new() -> Self {
        Self {
            host: String::new(),
            port: String::new(),
            username: String::new(),
            password: String::new(),
            display_name: String::new(),
            focus: FocusField::Host,
            error: None,
        }
    }

    pub fn next(&mut self) {
        self.focus = match self.focus {
            FocusField::Host => FocusField::Port,
            FocusField::Port => FocusField::Username,
            FocusField::Username => FocusField::Password,
            FocusField::Password => FocusField::DisplayName,
            FocusField::DisplayName => FocusField::Host,
        };
    }

    pub fn prev(&mut self) {
        self.focus = match self.focus {
            FocusField::Host => FocusField::DisplayName,
            FocusField::Port => FocusField::Host,
            FocusField::Username => FocusField::Port,
            FocusField::Password => FocusField::Username,
            FocusField::DisplayName => FocusField::Password,
        };
    }

    pub fn focused_value_mut(&mut self) -> &mut String {
        match self.focus {
            FocusField::Host => &mut self.host,
            FocusField::Port => &mut self.port,
            FocusField::Username => &mut self.username,
            FocusField::Password => &mut self.password,
            FocusField::DisplayName => &mut self.display_name,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("Host is required".into());
        }
        if self.port.trim().is_empty() {
            return Err("Port is required".into());
        }
        if self.username.trim().is_empty() {
            return Err("Username is required".into());
        }
        if self.password.is_empty() {
            return Err("Password is required".into());
        }
        if self.port.parse::<u16>().is_err() {
            return Err("Port must be a number".into());
        }
        Ok(())
    }
}

pub fn draw_connection_form(area: Rect, form: &ConnectionForm, frame: &mut ratatui::Frame<'_>) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // host
            Constraint::Length(3), // port
            Constraint::Length(3), // username
            Constraint::Length(3), // password
            Constraint::Length(3), // display name (optional)
            Constraint::Length(1), // error line
            Constraint::Min(1),    // spacer
        ])
        .split(area);

    let mut render_input =
        |idx: usize, label: &str, value: &str, is_password: bool, focused: bool| {
            let mut block = Block::default().borders(Borders::ALL).title(label);
            if focused {
                block = block.border_style(Style::default().fg(Color::Cyan));
            } else {
                block = block.border_style(Style::default());
            }
            let shown = if is_password {
                "*".repeat(value.chars().count())
            } else {
                value.to_string()
            };
            let para = Paragraph::new(shown.clone()).block(block);
            frame.render_widget(para, layout[idx]);
            if focused {
                let area_box = layout[idx].inner(Margin::new(1, 1));
                let cursor_x = area_box.x + shown.len() as u16;
                let cursor_y = area_box.y;
                frame.set_cursor(cursor_x, cursor_y);
            }
        };

    render_input(1, "Host", &form.host, false, form.focus == FocusField::Host);
    render_input(2, "Port", &form.port, false, form.focus == FocusField::Port);
    render_input(
        3,
        "Username",
        &form.username,
        false,
        form.focus == FocusField::Username,
    );
    render_input(
        4,
        "Password",
        &form.password,
        true,
        form.focus == FocusField::Password,
    );
    render_input(
        5,
        "Display Name (optional)",
        &form.display_name,
        false,
        form.focus == FocusField::DisplayName,
    );
}

// Error popup renderer
use ratatui::widgets::Clear;

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
            Style::default().fg(Color::Gray),
        )),
    ])
    .wrap(ratatui::widgets::Wrap { trim: true })
    .block(block);
    frame.render_widget(body, popup);
}

// Add Main Menu renderer
use ratatui::widgets::{List, ListItem};

pub fn draw_main_menu(
    area: Rect,
    selected_index: usize,
    conn_count: usize,
    frame: &mut ratatui::Frame<'_>,
) {
    // Layout: header (fixed 3) + list (min 1)
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    // Header with saved connections count (fixed to 0 for now)
    let header_block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            "TermiRS SSH Client",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    let header_para = Paragraph::new(Line::from(Span::raw(format!(
        "{} saved connections",
        conn_count
    ))))
    .block(header_block);
    frame.render_widget(header_para, layout[0]);

    // Menu items
    let items = vec![
        ListItem::new("[V] View Saved Connections"),
        ListItem::new("[N] Create New Connection"),
        ListItem::new("[Q] Quit"),
    ];
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(
        list,
        layout[1],
        &mut ratatui::widgets::ListState::default().with_selected(Some(selected_index)),
    );
}

// Render the saved connections list

#[derive(Clone, Debug)]
pub struct ConnectionListItem<'a> {
    pub display_name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub created_at: String,
}

pub fn draw_connection_list(
    area: Rect,
    title: &str,
    items: &[ConnectionListItem<'_>],
    selected_index: usize,
    frame: &mut ratatui::Frame<'_>,
) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let header_block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    frame.render_widget(Paragraph::new("").block(header_block), layout[0]);

    let mut list_items: Vec<ListItem> = Vec::with_capacity(items.len());
    for it in items.iter() {
        let indicator = "● ";
        let header = Line::from(vec![
            Span::styled(indicator, Style::default().fg(Color::Green)),
            Span::styled(
                it.display_name,
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]);
        let meta1 = Line::from(vec![
            Span::raw("Host: "),
            Span::styled(it.host, Style::default().fg(Color::Cyan)),
            Span::raw("  Port: "),
            Span::styled(format!("{}", it.port), Style::default().fg(Color::Cyan)),
        ]);
        let meta2 = Line::from(vec![
            Span::raw("User: "),
            Span::styled(it.username, Style::default().fg(Color::Cyan)),
            Span::raw("  Created: "),
            Span::styled(it.created_at.clone(), Style::default().fg(Color::Gray)),
        ]);
        let text = vec![header, meta1, meta2];
        list_items.push(ListItem::new(text));
    }

    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Connection List"),
        )
        .highlight_style(
            Style::default()
                // .bg(Color::Blue)
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(
        list,
        layout[1],
        &mut ratatui::widgets::ListState::default().with_selected(Some(selected_index)),
    );
}

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
            parser: Parser::new(rows, cols, 10_000),
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

    pub fn scroll_by(&mut self, delta_lines: i32) {
        let current = self.parser.screen().scrollback() as i32;
        let target = current.saturating_add(delta_lines).max(0) as usize;
        self.parser.screen_mut().set_scrollback(target);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
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

// ===== SCP Popup =====

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScpFocusField {
    LocalPath,
    RemotePath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScpForm {
    pub local_path: String,
    pub remote_path: String,
    pub focus: ScpFocusField,
}

impl ScpForm {
    pub fn new() -> Self {
        Self {
            local_path: String::new(),
            remote_path: String::new(),
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

    pub fn focused_value_mut(&mut self) -> &mut String {
        match self.focus {
            ScpFocusField::LocalPath => &mut self.local_path,
            ScpFocusField::RemotePath => &mut self.remote_path,
        }
    }
}

// ===== Dropdown Component =====

#[derive(Clone, Debug)]
pub struct DropdownState {
    pub options: Vec<String>,
    pub selected: usize,
    pub visible: bool,
    pub anchor_rect: Rect,    // The input field this dropdown is anchored to
    pub scroll_offset: usize, // Track the scroll position
    pub max_visible_items: usize, // Maximum items to show at once
}

impl DropdownState {
    pub fn new(options: Vec<String>, anchor_rect: Rect) -> Self {
        Self {
            options,
            selected: 0,
            visible: true,
            anchor_rect,
            scroll_offset: 0,
            max_visible_items: 8, // Default to 8 visible items
        }
    }

    pub fn next(&mut self) {
        if !self.options.is_empty() {
            self.selected = (self.selected + 1) % self.options.len();
            self.update_scroll();
        }
    }

    pub fn prev(&mut self) {
        if !self.options.is_empty() {
            self.selected = if self.selected == 0 {
                self.options.len() - 1
            } else {
                self.selected - 1
            };
            self.update_scroll();
        }
    }

    /// Update scroll offset to keep selected item visible
    fn update_scroll(&mut self) {
        if self.options.is_empty() {
            return;
        }

        // If selected item is above the visible window, scroll up
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        // If selected item is below the visible window, scroll down
        else if self.selected >= self.scroll_offset + self.max_visible_items {
            self.scroll_offset = self.selected.saturating_sub(self.max_visible_items - 1);
        }
    }

    pub fn get_selected(&self) -> Option<&String> {
        self.options.get(self.selected)
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }
}

pub fn draw_dropdown(dropdown: &DropdownState, frame: &mut ratatui::Frame<'_>) {
    if !dropdown.visible || dropdown.options.is_empty() {
        return;
    }

    // Calculate dropdown position and size
    let visible_items = dropdown.options.len().min(dropdown.max_visible_items);
    let dropdown_height = visible_items as u16 + 2; // +2 for borders

    // Position dropdown below the anchor field
    let x = dropdown.anchor_rect.x;
    let y = dropdown.anchor_rect.y + dropdown.anchor_rect.height;
    let width = dropdown.anchor_rect.width;

    let dropdown_rect = Rect {
        x,
        y,
        width,
        height: dropdown_height,
    };

    // Clear the area first
    frame.render_widget(Clear, dropdown_rect);

    // Get the visible slice of options based on scroll offset
    let end_index =
        (dropdown.scroll_offset + dropdown.max_visible_items).min(dropdown.options.len());
    let visible_options = &dropdown.options[dropdown.scroll_offset..end_index];

    // Create list items for visible options only
    let list_items: Vec<ListItem> = visible_options
        .iter()
        .enumerate()
        .map(|(visible_index, option)| {
            let actual_index = dropdown.scroll_offset + visible_index;
            let style = if actual_index == dropdown.selected {
                Style::default()
                    .fg(Color::LightCyan)
                    // .bg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(option.clone(), style)))
        })
        .collect();

    // Create title with scroll indicators
    let title = if dropdown.options.len() > dropdown.max_visible_items {
        let has_more_above = dropdown.scroll_offset > 0;
        let has_more_below =
            dropdown.scroll_offset + dropdown.max_visible_items < dropdown.options.len();

        let scroll_indicator = match (has_more_above, has_more_below) {
            (true, true) => " ↑↓",
            (true, false) => " ↑",
            (false, true) => " ↓",
            (false, false) => "",
        };

        format!(
            "Options ({}/{}){}",
            dropdown.selected + 1,
            dropdown.options.len(),
            scroll_indicator
        )
    } else {
        format!(
            "Options ({}/{})",
            dropdown.selected + 1,
            dropdown.options.len()
        )
    };

    // Create the list widget
    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title),
    );

    frame.render_widget(list, dropdown_rect);
}

pub fn draw_scp_popup(area: Rect, form: &ScpForm, frame: &mut ratatui::Frame<'_>) -> (Rect, Rect) {
    let popup_w = area.width.saturating_sub(10).max(40);
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
            "SCP: Send File",
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

    let local_path_rect: Rect;
    let remote_path_rect: Rect;

    let mut render_input = |idx: usize, label: &str, value: &str, focused: bool| -> Rect {
        let mut block = Block::default().borders(Borders::ALL).title(label);
        if focused {
            block = block.border_style(Style::default().fg(Color::Cyan));
        }
        let para = Paragraph::new(value.to_string()).block(block);
        frame.render_widget(para, layout[idx]);
        if focused {
            let area_box = layout[idx].inner(Margin::new(1, 1));
            let cursor_x = area_box.x + value.chars().count() as u16;
            let cursor_y = area_box.y;
            frame.set_cursor(cursor_x, cursor_y);
        }
        layout[idx]
    };

    local_path_rect = render_input(
        0,
        "Local Path",
        &form.local_path,
        form.focus == ScpFocusField::LocalPath,
    );
    remote_path_rect = render_input(
        1,
        "Remote Path",
        &form.remote_path,
        form.focus == ScpFocusField::RemotePath,
    );

    let hint = Paragraph::new(Line::from(Span::styled(
        "Enter: Send   Esc: Cancel   Tab: Complete/Switch   Up/Down: Switch Field",
        Style::default().fg(Color::Gray),
    )));
    frame.render_widget(hint, layout[2]);

    (local_path_rect, remote_path_rect)
}

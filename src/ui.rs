use std::time::Instant;

use chrono::Local;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use tui_textarea::TextArea;
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
                let fg = map_color(cell.fgcolor());
                let bg = map_color(cell.bgcolor());
                let bold = cell.bold();
                let italic = cell.italic();
                let underline = cell.underline();
                let inverse = cell.inverse();
                let dim = cell.dim();

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
                if dim {
                    style = style.add_modifier(Modifier::DIM);
                }
                if inverse {
                    // Apply reverse video using a style modifier so default colors are inverted correctly
                    style = style.add_modifier(Modifier::REVERSED);
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
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

use ratatui::layout::{Constraint, Direction, Layout, Margin};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusField {
    Host,
    Port,
    Username,
    Password,
    PrivateKeyPath,
    DisplayName,
}

#[derive(Clone, Debug)]
pub struct ConnectionForm {
    pub host: TextArea<'static>,
    pub port: TextArea<'static>,
    pub username: TextArea<'static>,
    pub password: TextArea<'static>,
    pub private_key_path: TextArea<'static>,
    pub display_name: TextArea<'static>,
    pub focus: FocusField,
    pub error: Option<String>,
}

impl ConnectionForm {
    pub fn new() -> Self {
        let mut host = TextArea::default();
        host.set_placeholder_text("Enter hostname or IP address");
        host.set_cursor_line_style(Style::default());

        let mut port = TextArea::default();
        port.set_placeholder_text("22");
        port.set_cursor_line_style(Style::default());

        let mut username = TextArea::default();
        username.set_placeholder_text("Enter username");
        username.set_cursor_line_style(Style::default());

        let mut password = TextArea::default();
        password.set_placeholder_text("Enter password");
        password.set_mask_char('*');
        password.set_cursor_line_style(Style::default());

        let mut private_key_path = TextArea::default();
        private_key_path.set_placeholder_text(
            "Enter private key path (at least one of password or key path is required)",
        );
        private_key_path.set_cursor_line_style(Style::default());

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());

        Self {
            host,
            port,
            username,
            password,
            private_key_path,
            display_name,
            focus: FocusField::Host,
            error: None,
        }
    }

    pub fn next(&mut self) {
        self.focus = match self.focus {
            FocusField::Host => FocusField::Port,
            FocusField::Port => FocusField::Username,
            FocusField::Username => FocusField::Password,
            FocusField::Password => FocusField::PrivateKeyPath,
            FocusField::PrivateKeyPath => FocusField::DisplayName,
            FocusField::DisplayName => FocusField::Host,
        };
    }

    pub fn prev(&mut self) {
        self.focus = match self.focus {
            FocusField::Host => FocusField::DisplayName,
            FocusField::Port => FocusField::Host,
            FocusField::Username => FocusField::Port,
            FocusField::Password => FocusField::Username,
            FocusField::PrivateKeyPath => FocusField::Password,
            FocusField::DisplayName => FocusField::PrivateKeyPath,
        };
    }

    pub fn focused_textarea_mut(&mut self) -> &mut TextArea<'static> {
        match self.focus {
            FocusField::Host => &mut self.host,
            FocusField::Port => &mut self.port,
            FocusField::Username => &mut self.username,
            FocusField::Password => &mut self.password,
            FocusField::PrivateKeyPath => &mut self.private_key_path,
            FocusField::DisplayName => &mut self.display_name,
        }
    }

    pub fn get_host_value(&self) -> String {
        self.host.lines()[0].clone()
    }

    pub fn get_port_value(&self) -> String {
        self.port.lines()[0].clone()
    }

    pub fn get_username_value(&self) -> String {
        self.username.lines()[0].clone()
    }

    pub fn get_password_value(&self) -> String {
        self.password.lines()[0].clone()
    }

    pub fn get_private_key_path_value(&self) -> String {
        self.private_key_path.lines()[0].clone()
    }

    pub fn get_display_name_value(&self) -> String {
        self.display_name.lines()[0].clone()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.get_host_value().trim().is_empty() {
            return Err("Host is required".into());
        }
        if self.get_port_value().trim().is_empty() {
            return Err("Port is required".into());
        }
        if self.get_username_value().trim().is_empty() {
            return Err("Username is required".into());
        }
        if self.get_password_value().is_empty() && self.get_private_key_path_value().is_empty() {
            return Err("Password or private key is required".into());
        }
        if !self.get_port_value().is_empty() && self.get_port_value().parse::<u16>().is_err() {
            return Err("Port must be a number".into());
        }
        Ok(())
    }

    pub fn from_connection(conn: &crate::config::manager::Connection) -> Self {
        let mut host = TextArea::default();
        host.set_placeholder_text("Enter hostname or IP address");
        host.set_cursor_line_style(Style::default());
        host.insert_str(conn.host.clone());

        let mut port = TextArea::default();
        port.set_placeholder_text("22");
        port.set_cursor_line_style(Style::default());
        port.insert_str(conn.port.to_string());

        let mut username = TextArea::default();
        username.set_placeholder_text("Enter username");
        username.set_cursor_line_style(Style::default());
        username.insert_str(conn.username.clone());

        let mut password = TextArea::default();
        password.set_placeholder_text("Enter password");
        password.set_mask_char('*');
        password.set_cursor_line_style(Style::default());
        // Don't prefill password for security

        let mut private_key_path = TextArea::default();
        private_key_path.set_placeholder_text("Enter private key path (optional)");
        private_key_path.set_cursor_line_style(Style::default());
        if let crate::config::manager::AuthMethod::PublicKey {
            private_key_path: path,
            ..
        } = &conn.auth_method
        {
            private_key_path.insert_str(path.clone());
        }

        let mut display_name = TextArea::default();
        display_name.set_placeholder_text("Enter display name (optional)");
        display_name.set_cursor_line_style(Style::default());
        display_name.insert_str(conn.display_name.clone());

        Self {
            host,
            port,
            username,
            password,
            private_key_path,
            display_name,
            focus: FocusField::Host,
            error: None,
        }
    }
}

pub fn draw_connection_form(area: Rect, form: &ConnectionForm, frame: &mut ratatui::Frame<'_>) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(3), // host
            Constraint::Length(3), // port
            Constraint::Length(3), // username
            Constraint::Length(3), // password
            Constraint::Length(3), // private key path
            Constraint::Length(3), // display name (optional)
            Constraint::Length(1), // error line
            Constraint::Min(1),    // spacer
        ])
        .split(area);

    let mut render_textarea = |idx: usize, label: &str, textarea: &TextArea, focused: bool| {
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
    };

    render_textarea(1, "Host", &form.host, form.focus == FocusField::Host);
    render_textarea(2, "Port", &form.port, form.focus == FocusField::Port);
    render_textarea(
        3,
        "Username",
        &form.username,
        form.focus == FocusField::Username,
    );
    render_textarea(
        4,
        "Password",
        &form.password,
        form.focus == FocusField::Password,
    );
    render_textarea(
        5,
        "Private Key Path",
        &form.private_key_path,
        form.focus == FocusField::PrivateKeyPath,
    );
    render_textarea(
        6,
        "Display Name (optional)",
        &form.display_name,
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

// Add Main Menu renderer
use ratatui::widgets::{List, ListItem};

use crate::config::manager::{AuthMethod, Connection};

// Render the saved connections list

#[derive(Clone, Debug)]
pub struct ConnectionListItem<'a> {
    pub display_name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub created_at: String,
    pub auth_method: &'a str,
    pub last_used: Option<String>,
}

pub fn draw_connection_list(
    area: Rect,
    conns: &[Connection],
    selected_index: usize,
    frame: &mut ratatui::Frame<'_>,
) {
    let title = format!("Saved Connections ({} connections)", conns.len());
    let items: Vec<ConnectionListItem> = conns
        .iter()
        .map(|c| ConnectionListItem {
            display_name: &c.display_name,
            host: &c.host,
            port: c.port,
            username: &c.username,
            created_at: c
                .created_at
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            auth_method: match &c.auth_method {
                AuthMethod::Password(_) => "password",
                AuthMethod::PublicKey { .. } => "public key",
            },
            last_used: c
                .last_used
                .map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()),
        })
        .collect();
    let sel = if items.is_empty() {
        0
    } else {
        selected_index.min(items.len() - 1)
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
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
        let meta3 = Line::from(vec![
            Span::raw("Auth: "),
            Span::styled(it.auth_method, Style::default().fg(Color::Cyan)),
            Span::raw("  Last Used: "),
            Span::styled(
                it.last_used.clone().unwrap_or_default(),
                Style::default().fg(Color::Gray),
            ),
        ]);
        let text = vec![header, meta1, meta2, meta3];
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
        &mut ratatui::widgets::ListState::default().with_selected(Some(sel)),
    );

    let footer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(layout[2]);

    let left = Paragraph::new(Line::from(Span::styled(
        "Enter: Connect   Esc: Cancel   K/↑: Up   J/↓: Down   N: New   S: SCP   D: Delete   E: Edit",
        Style::default().fg(Color::White).add_modifier(Modifier::DIM),
    ))).alignment(Alignment::Left);
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

// ===== SCP Popup =====

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

    pub fn get_local_path_value(&self) -> String {
        self.local_path.lines()[0].clone()
    }

    pub fn get_remote_path_value(&self) -> String {
        self.remote_path.lines()[0].clone()
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
    pub scrollbar_state: ScrollbarState, // State for the scrollbar widget
}

impl DropdownState {
    pub fn new(options: Vec<String>, anchor_rect: Rect) -> Self {
        let content_length = options.len();
        Self {
            options,
            selected: 0,
            visible: true,
            anchor_rect,
            scroll_offset: 0,
            max_visible_items: 8, // Default to 8 visible items
            scrollbar_state: ScrollbarState::new(content_length).position(0),
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

        // Update scrollbar state to reflect current position
        self.scrollbar_state = self.scrollbar_state.position(self.selected);
    }

    pub fn get_selected(&self) -> Option<&String> {
        self.options.get(self.selected)
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

    // Split the dropdown area to make room for scrollbar if needed
    let show_scrollbar = dropdown.options.len() > dropdown.max_visible_items;
    let (list_area, scrollbar_area) = if show_scrollbar {
        // Get the inner area (inside borders) first
        let inner_area = Rect {
            x: dropdown_rect.x + 1,
            y: dropdown_rect.y + 1,
            width: dropdown_rect.width.saturating_sub(2),
            height: dropdown_rect.height.saturating_sub(2),
        };

        // Reserve 1 column for scrollbar on the right inside the borders
        let list_area = Rect {
            x: dropdown_rect.x,
            y: dropdown_rect.y,
            width: dropdown_rect.width.saturating_sub(1), // Make room for scrollbar
            height: dropdown_rect.height,
        };

        let scrollbar_area = Rect {
            x: inner_area.x + inner_area.width.saturating_sub(1), // Position inside right border
            y: inner_area.y,
            width: 1,
            height: inner_area.height,
        };
        (list_area, Some(scrollbar_area))
    } else {
        (dropdown_rect, None)
    };

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

    // Create title - simpler now since we have visual scrollbar
    let title = format!(
        "Options ({}/{})",
        dropdown.selected + 1,
        dropdown.options.len()
    );

    // Create the list widget
    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(title),
    );

    frame.render_widget(list, list_area);

    // Render scrollbar if needed
    if let Some(scrollbar_area) = scrollbar_area {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None) // Remove symbols to fit better inside borders
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .style(Style::default().fg(Color::Cyan));

        frame.render_stateful_widget(
            scrollbar,
            scrollbar_area,
            &mut dropdown.scrollbar_state.clone(),
        );
    }
}

// SCP Progress popup renderer
pub fn draw_scp_progress_popup(
    area: Rect,
    progress: &crate::ScpProgress,
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

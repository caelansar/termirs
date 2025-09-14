use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use tui_textarea::TextArea;

use crate::config::manager::{AuthMethod, Connection};

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

    pub fn from_connection(conn: &Connection) -> Self {
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
        if let AuthMethod::PublicKey {
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
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

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
                .title(format!("Connection List ({} connections)", items.len())),
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
        layout[0],
        &mut ratatui::widgets::ListState::default().with_selected(Some(sel)),
    );

    let footer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(layout[1]);

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

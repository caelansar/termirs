use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    TableState,
};
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
        private_key_path.set_placeholder_text("At least one of password or key path is required");
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

        let mut private_key_path = TextArea::default();
        private_key_path.set_placeholder_text("Enter private key path (optional)");
        private_key_path.set_cursor_line_style(Style::default());
        match &conn.auth_method {
            AuthMethod::Password(pwd) => {
                password.insert_str("*".repeat(pwd.len()));
            }
            AuthMethod::PublicKey {
                private_key_path: path,
                ..
            } => {
                private_key_path.insert_str(path);
            }
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
    pub name: &'a str,
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
    search_mode: bool,
    search_query: &str,
    frame: &mut ratatui::Frame<'_>,
) {
    let mut items: Vec<ConnectionListItem> = conns
        .iter()
        .map(|c| ConnectionListItem {
            name: &c.display_name,
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

    // Filter items based on search query
    if !search_query.is_empty() {
        items.retain(|item| {
            item.host
                .to_lowercase()
                .contains(&search_query.to_lowercase())
                || item
                    .username
                    .to_lowercase()
                    .contains(&search_query.to_lowercase())
                || item
                    .name
                    .to_lowercase()
                    .contains(&search_query.to_lowercase())
        });
    }

    let sel = if items.is_empty() {
        0
    } else {
        selected_index.min(items.len() - 1)
    };

    // In search mode, the area passed is already the table area, so no need for layout splitting
    let layout = if search_mode {
        vec![area] // Just use the entire area for the table
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area)
            .to_vec()
    };

    // Create table header
    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Host"),
        Cell::from("Port"),
        Cell::from("User"),
        Cell::from("Auth"),
        Cell::from("Created"),
        Cell::from("Last Used"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // Create table rows
    let rows: Vec<Row> = items
        .iter()
        .map(|item| {
            Row::new(vec![
                Cell::from(item.name),
                Cell::from(item.host),
                Cell::from(item.port.to_string()),
                Cell::from(item.username),
                Cell::from(item.auth_method),
                Cell::from(item.created_at.clone()),
                Cell::from(
                    item.last_used
                        .clone()
                        .unwrap_or_else(|| "Never".to_string()),
                ),
            ])
            .height(1)
        })
        .collect();

    // Create the table
    let table = Table::new(
        rows,
        [
            Constraint::Min(8),     // Name
            Constraint::Min(8),     // Host (reduced from 15 to 8)
            Constraint::Length(6),  // Port
            Constraint::Min(6),     // User (reduced from 12 to 6)
            Constraint::Length(12), // Auth
            Constraint::Length(16), // Created
            Constraint::Length(16), // Last Used
            Constraint::Length(1),  // Scrollbar
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Connection List ({} connections)", items.len())),
    )
    .highlight_style(
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    // Render the table with state
    let mut table_state = TableState::default().with_selected(Some(sel));
    frame.render_stateful_widget(table, layout[0], &mut table_state);

    // Render vertical scrollbar only if content exceeds one page (visible rows)
    if !items.is_empty() {
        let inner_area = layout[0].inner(ratatui::layout::Margin::new(1, 2));
        // inner_area includes header row; visible rows are inner height - 1 (for header)
        let visible_rows = inner_area.height.saturating_sub(1) as usize;
        let content_length = items.len();
        if content_length > visible_rows {
            // Compute page-aware scrollbar positions
            let max_top = content_length.saturating_sub(visible_rows);
            let centered_top = sel.saturating_sub(visible_rows.saturating_sub(1) / 2);
            let top_index = centered_top.min(max_top);
            let total_positions = max_top.saturating_add(1);

            let mut scroll_state = ScrollbarState::new(total_positions).position(top_index);

            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);

            frame.render_stateful_widget(scrollbar, inner_area, &mut scroll_state);
        }
    }

    // Only render footer in normal mode (search mode footer is handled in main.rs)
    if !search_mode {
        let footer_area = layout[1];
        let footer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(footer_area);

        let hint_text = "Enter: Connect   Esc: Cancel   K/↑: Up   J/↓: Down   N: New   S: SCP   D: Delete   E: Edit   /: Search";

        let left = Paragraph::new(Line::from(Span::styled(
            hint_text,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        )))
        .alignment(Alignment::Left);

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
}

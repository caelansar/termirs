use chrono::Local;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row};
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

impl Default for ConnectionForm {
    fn default() -> Self {
        Self::new()
    }
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
        password.set_mask_char('\u{2022}');
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

    pub fn get_host_value(&self) -> &str {
        &self.host.lines()[0]
    }

    pub fn get_port_value(&self) -> &str {
        &self.port.lines()[0]
    }

    pub fn get_username_value(&self) -> &str {
        &self.username.lines()[0]
    }

    pub fn get_password_value(&self) -> &str {
        &self.password.lines()[0]
    }

    pub fn get_private_key_path_value(&self) -> &str {
        &self.private_key_path.lines()[0]
    }

    pub fn get_display_name_value(&self) -> &str {
        &self.display_name.lines()[0]
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.get_host_value().trim().is_empty() {
            return Err("Host is required".into());
        }
        if self.get_username_value().trim().is_empty() {
            return Err("Username is required".into());
        }
        let port_str = self.get_port_value().trim();
        if !port_str.is_empty() && port_str.parse::<u16>().is_err() {
            return Err("Port must be a number".into());
        }
        Ok(())
    }

    fn from_connection(conn: &Connection) -> Self {
        let mut host = TextArea::default();
        host.set_placeholder_text("Enter hostname or IP address");
        host.set_cursor_line_style(Style::default());
        host.insert_str(&conn.host);

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
        password.set_mask_char('\u{2022}');
        password.set_cursor_line_style(Style::default());

        let mut private_key_path = TextArea::default();
        private_key_path.set_placeholder_text("Enter private key path (optional)");
        private_key_path.set_cursor_line_style(Style::default());
        match &conn.auth_method {
            AuthMethod::Password(pwd) => {
                password.insert_str(pwd);
            }
            AuthMethod::PublicKey {
                private_key_path: path,
                ..
            } => {
                private_key_path.insert_str(path);
            }
            AuthMethod::AutoLoadKey => {
                // No fields to populate for auto-load key
            }
            AuthMethod::None => {
                // No fields to populate for none auth
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
    search: &crate::SearchState,
    frame: &mut ratatui::Frame<'_>,
    choose_connection_mode: bool,
) {
    // Build the list items
    let items: Vec<ConnectionListItem> = conns
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
                AuthMethod::AutoLoadKey => "auto-load key",
                AuthMethod::None => "none",
            },
            last_used: c
                .last_used
                .map(|d| d.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()),
        })
        .collect();

    // Create the component with appropriate footer hints based on mode
    let component = if choose_connection_mode {
        ConnectionTableComponentWithMode {
            hints: "Enter: Select   K/↑: Up   J/↓: Down   /: Search",
        }
    } else {
        ConnectionTableComponentWithMode {
            hints: "Enter: Connect   K/↑: Up   J/↓: Down   N: New   I: File Explorer   P: Port Forward   D: Delete   E: Edit   /: Search",
        }
    };

    // Create state from current values
    let state = super::table::TableListState::from_parts(selected_index, search.clone());

    // Determine title based on mode
    let title = if choose_connection_mode {
        "Choose Connection"
    } else {
        "Connection List"
    };

    // Use the generic table renderer
    super::table_renderer::draw_table_list(area, &component, items, &state, frame, title);
}

// Helper component that allows customizing footer hints
struct ConnectionTableComponentWithMode {
    hints: &'static str,
}

impl super::table::TableListComponent<7> for ConnectionTableComponentWithMode {
    type Item<'a> = ConnectionListItem<'a>;

    const HEADER_LABELS: &'static [&'static str; 7] = &[
        "Name",
        "Host",
        "Port",
        "User",
        "Auth",
        "Created",
        "Last Used",
    ];

    const COLUMN_CONSTRAINTS: &'static [Constraint; 7] = &[
        Constraint::Min(8),     // Name
        Constraint::Min(8),     // Host
        Constraint::Length(6),  // Port
        Constraint::Min(6),     // User
        Constraint::Length(16), // Auth
        Constraint::Length(16), // Created
        Constraint::Length(16), // Last Used
    ];

    fn render_row(&self, item: &ConnectionListItem<'_>) -> Row<'static> {
        Row::new(vec![
            Cell::from(item.name.to_string()),
            Cell::from(item.host.to_string()),
            Cell::from(item.port.to_string()),
            Cell::from(item.username.to_string()),
            Cell::from(item.auth_method.to_string()),
            Cell::from(item.created_at.clone()),
            Cell::from(item.last_used.clone().unwrap_or("Never".into())),
        ])
        .height(1)
    }

    fn matches_query(&self, item: &ConnectionListItem<'_>, query: &str) -> bool {
        let lower = query.to_lowercase();
        item.host.to_lowercase().contains(&lower)
            || item.username.to_lowercase().contains(&lower)
            || item.name.to_lowercase().contains(&lower)
    }

    fn footer_hints(&self) -> &'static str {
        self.hints
    }
}

impl From<&Connection> for ConnectionForm {
    fn from(conn: &Connection) -> Self {
        ConnectionForm::from_connection(conn)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn test_connection_form() {
        let conn = Connection {
            id: "1".to_string(),
            created_at: Utc::now(),
            last_used: None,
            public_key: None,
            display_name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "test".to_string(),
            auth_method: AuthMethod::Password("test".to_string().into()),
        };

        let form = ConnectionForm::from_connection(&conn);
        assert_eq!(form.get_password_value(), "test");
    }
}

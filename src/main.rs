mod config;
mod error;
mod key_event;
mod ssh_client;
mod ui;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::event::{self, DisableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;

use error::{AppError, Result};
use ssh_client::SshClient;
use ui::{
    ConnectionForm, ConnectionListItem, DropdownState, ScpForm, TerminalState,
    draw_connection_form, draw_connection_list, draw_dropdown, draw_error_popup, draw_info_popup,
    draw_main_menu, draw_scp_popup, draw_terminal,
};

use config::manager::{ConfigManager, Connection};

#[derive(Clone)]
pub(crate) enum AppMode {
    MainMenu {
        selected: usize,
    },
    ConnectionList {
        selected: usize,
    },
    FormNew {
        form: ConnectionForm,
    },
    FormEdit {
        form: ConnectionForm,
        original: Connection,
    },
    Connected {
        client: SshClient,
        state: Arc<Mutex<TerminalState>>,
    },
}

/// App is the main application
pub(crate) struct App {
    pub(crate) mode: AppMode,
    pub(crate) error: Option<AppError>,
    pub(crate) info: Option<String>,
    pub(crate) config: ConfigManager,
    pub(crate) scp_form: Option<ScpForm>,
    pub(crate) dropdown: Option<DropdownState>,
}

impl App {
    fn new() -> Result<Self> {
        Ok(Self {
            mode: AppMode::MainMenu { selected: 0 },
            error: None,
            info: None,
            config: ConfigManager::new()?,
            scp_form: None,
            dropdown: None,
        })
    }

    pub(crate) fn go_to_connected(&mut self, client: SshClient, state: Arc<Mutex<TerminalState>>) {
        self.mode = AppMode::Connected { client, state };
    }

    pub(crate) fn go_to_main_menu(&mut self) {
        self.mode = AppMode::MainMenu { selected: 0 };
    }

    pub(crate) fn go_to_connection_list(&mut self) {
        self.mode = AppMode::ConnectionList { selected: 0 };
    }
}

fn main() -> Result<()> {
    // Setup Crossterm terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, DisableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;

    // UI event/render loop
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(10);

    loop {
        // main entry point for drawing to the terminal
        terminal.draw(|f| {
            let size = f.size();
            match &app.mode {
                AppMode::MainMenu { selected } => {
                    let conns = app.config.connections();
                    draw_main_menu(size, *selected, conns.len(), f);
                }
                AppMode::ConnectionList { selected } => {
                    let conns = app.config.connections();
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
                        })
                        .collect();
                    let sel = if items.is_empty() {
                        0
                    } else {
                        (*selected).min(items.len() - 1)
                    };
                    draw_connection_list(size, &title, &items, sel, f);
                }
                AppMode::FormNew { form } => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(1)])
                        .split(size);

                    let title_block = Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .title(
                            Line::from("New SSH Connection").style(
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        );
                    f.render_widget(title_block, layout[0]);

                    draw_connection_form(layout[1], &form, f);
                }
                AppMode::FormEdit { form, .. } => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(1)])
                        .split(size);

                    let title_block = Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .title(
                            Line::from("Edit SSH Connection").style(
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        );
                    f.render_widget(title_block, layout[0]);

                    draw_connection_form(layout[1], &form, f);
                }
                AppMode::Connected { client, state } => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(1)])
                        .split(size);

                    let title_block = Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .title(
                            Line::from("title").style(
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        );
                    f.render_widget(title_block, layout[0]);

                    let inner = layout[1].inner(Margin::new(1, 1));
                    if let Ok(mut guard) = state.lock() {
                        if guard.parser.screen().size() != (inner.height, inner.width) {
                            guard.resize(inner.height, inner.width);
                            client.request_size(inner.width, inner.height);
                        }
                        draw_terminal(layout[1], &guard, f);
                    }
                }
            }

            // Overlay error popup if any
            if let Some(err) = &app.error {
                draw_error_popup(size, &err.to_string(), f);
            }

            // Overlay info popup if any
            if let Some(msg) = &app.info {
                draw_info_popup(size, msg, f);
            }

            // Overlay SCP popup if any
            let mut scp_input_rects: Option<(Rect, Rect)> = None;
            if let Some(form) = &app.scp_form {
                scp_input_rects = Some(draw_scp_popup(size, form, f));
            }

            // Update dropdown anchor rect if SCP popup is visible and dropdown exists
            if let (Some(dropdown), Some((local_rect, _remote_rect))) =
                (&mut app.dropdown, scp_input_rects)
            {
                dropdown.anchor_rect = local_rect;
            }

            // Overlay dropdown if any
            if let Some(dropdown) = &app.dropdown {
                draw_dropdown(dropdown, f);
            }
        })?;

        // Input handling
        while crossterm::event::poll(Duration::from_millis(1))? {
            // true guarantees that read function call won't block.
            match event::read()? {
                Event::Key(key) => match crate::key_event::handle_key_event(&mut app, key) {
                    crate::key_event::KeyFlow::Continue => {}
                    crate::key_event::KeyFlow::Quit => {
                        disable_raw_mode().ok();
                        execute!(
                            terminal.backend_mut(),
                            LeaveAlternateScreen,
                            DisableMouseCapture
                        )
                        .ok();
                        return Ok(());
                    }
                },
                Event::Paste(data) => {
                    crate::key_event::handle_paste_event(&mut app, &data);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

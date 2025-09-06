mod config;
mod error;
mod ssh_client;
mod ui;

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;

use error::{AppError, Result};
use ssh_client::SshClient;
use ui::{
    ConnectionForm, ConnectionListItem, TerminalState, draw_connection_form, draw_connection_list,
    draw_error_popup, draw_main_menu, draw_terminal,
};

use config::manager::{ConfigManager, Connection};

#[derive(Clone)]
enum AppMode {
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
struct App {
    mode: AppMode,
    error: Option<AppError>,
    config: ConfigManager,
}

impl App {
    fn new() -> Result<Self> {
        Ok(Self {
            mode: AppMode::MainMenu { selected: 0 },
            error: None,
            config: ConfigManager::new()?,
        })
    }

    fn go_to_connected(&mut self, client: SshClient, state: Arc<Mutex<TerminalState>>) {
        self.mode = AppMode::Connected { client, state };
    }

    fn go_to_main_menu(&mut self) {
        self.mode = AppMode::MainMenu { selected: 0 };
    }

    fn go_to_connection_list(&mut self) {
        self.mode = AppMode::ConnectionList { selected: 0 };
    }
}

fn main() -> Result<()> {
    // Setup Crossterm terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
        })?;

        // Input handling
        while crossterm::event::poll(Duration::from_millis(1))? {
            // true guarantees that read function call won't block.
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // If error popup is visible, handle dismissal only
                    if app.error.is_some() {
                        match key.code {
                            KeyCode::Enter | KeyCode::Esc => {
                                app.error = None;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match &mut app.mode {
                        AppMode::MainMenu { selected } => {
                            const NUM_ITEMS: usize = 3;
                            match key.code {
                                KeyCode::Char('k') | KeyCode::Up => {
                                    *selected = if *selected == 0 {
                                        NUM_ITEMS - 1
                                    } else {
                                        *selected - 1
                                    };
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    *selected = (*selected + 1) % NUM_ITEMS;
                                }
                                KeyCode::Char('v') | KeyCode::Char('V') => {
                                    app.mode = AppMode::ConnectionList { selected: 0 };
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') => {
                                    app.mode = AppMode::FormNew {
                                        form: ConnectionForm::new(),
                                    };
                                }
                                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                                    // restore terminal
                                    disable_raw_mode().ok();
                                    execute!(
                                        terminal.backend_mut(),
                                        LeaveAlternateScreen,
                                        DisableMouseCapture
                                    )
                                    .ok();
                                    return Ok(());
                                }
                                KeyCode::Enter => match *selected {
                                    0 => {
                                        app.mode = AppMode::ConnectionList { selected: 0 };
                                    }
                                    1 => {
                                        app.mode = AppMode::FormNew {
                                            form: ConnectionForm::new(),
                                        };
                                    }
                                    2 => {
                                        disable_raw_mode().ok();
                                        execute!(
                                            terminal.backend_mut(),
                                            LeaveAlternateScreen,
                                            DisableMouseCapture
                                        )
                                        .ok();
                                        return Ok(());
                                    }
                                    _ => {}
                                },
                                _ => {}
                            }
                        }
                        AppMode::ConnectionList { selected } => {
                            let len = app.config.connections().len();
                            if len == 0 {
                                match key.code {
                                    KeyCode::Esc => app.go_to_main_menu(),
                                    _ => {}
                                }
                                continue;
                            }
                            match key.code {
                                KeyCode::Char('k') | KeyCode::Up => {
                                    *selected = if *selected == 0 {
                                        len - 1
                                    } else {
                                        *selected - 1
                                    };
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    *selected = (*selected + 1) % len;
                                }
                                KeyCode::Enter => {
                                    let conn = app.config.connections()[*selected].clone();
                                    match SshClient::connect(&conn) {
                                        Ok(client) => {
                                            let state =
                                                Arc::new(Mutex::new(TerminalState::new(30, 100)));
                                            let app_reader = state.clone();
                                            let client_reader = client.channel.clone();
                                            thread::spawn(move || {
                                                let mut buf = [0u8; 8192];
                                                loop {
                                                    let n = {
                                                        let mut ch = match client_reader.lock() {
                                                            Ok(guard) => guard,
                                                            Err(_) => break,
                                                        };
                                                        match ch.read(&mut buf) {
                                                            Ok(0) => return,
                                                            Ok(n) => n,
                                                            Err(_) => 0,
                                                        }
                                                    };
                                                    if n > 0 {
                                                        if let Ok(mut guard) = app_reader.lock() {
                                                            guard.process_bytes(&buf[..n]);
                                                        }
                                                    } else {
                                                        std::thread::sleep(Duration::from_millis(
                                                            10,
                                                        ));
                                                    }
                                                }
                                            });
                                            let _ = app.config.touch_last_used(&conn.id);
                                            app.go_to_connected(client, state);
                                        }
                                        Err(e) => {
                                            app.error = Some(e);
                                        }
                                    }
                                }
                                KeyCode::Char('e') | KeyCode::Char('E') => {
                                    let original = app.config.connections()[*selected].clone();
                                    let mut form = ConnectionForm::new();
                                    form.host = original.host.clone();
                                    form.port = original.port.to_string();
                                    form.username = original.username.clone();
                                    form.display_name = original.display_name.clone();
                                    form.password.clear(); // keep empty to mean "unchanged"
                                    app.mode = AppMode::FormEdit { form, original };
                                }
                                KeyCode::Char('d') | KeyCode::Char('D') => {
                                    let id = app.config.connections()[*selected].id.clone();
                                    match app.config.remove_connection(&id) {
                                        Ok(_) => {
                                            // persist changes
                                            if let Err(e) = app.config.save() {
                                                app.error = Some(e);
                                            }
                                            let new_len = app.config.connections().len();
                                            if new_len == 0 {
                                                *selected = 0;
                                            } else if *selected >= new_len {
                                                *selected = new_len - 1;
                                            }
                                        }
                                        Err(e) => app.error = Some(e),
                                    }
                                }
                                KeyCode::Esc => {
                                    app.go_to_main_menu();
                                }
                                _ => {}
                            }
                        }
                        AppMode::FormNew { form } => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.go_to_main_menu();
                                }
                                KeyCode::Tab | KeyCode::Down => {
                                    form.next();
                                }
                                KeyCode::BackTab | KeyCode::Up => {
                                    form.prev();
                                }
                                KeyCode::Enter => {
                                    match form.validate() {
                                        Ok(_) => {
                                            let user = form.username.trim().to_string();
                                            let pass = form.password.clone();

                                            let mut conn = Connection::new(
                                                form.host.trim().to_string(),
                                                form.port.parse::<u16>().unwrap_or(22),
                                                user,
                                                pass,
                                            );
                                            if !form.display_name.trim().is_empty() {
                                                conn.set_display_name(
                                                    form.display_name.trim().to_string(),
                                                );
                                            }
                                            match SshClient::connect(&conn) {
                                                Ok(client) => {
                                                    if let Err(e) =
                                                        app.config.add_connection(conn.clone())
                                                    {
                                                        app.error = Some(e);
                                                    }

                                                    let state = Arc::new(Mutex::new(
                                                        TerminalState::new(30, 100),
                                                    ));
                                                    // Reader thread
                                                    let app_reader = state.clone();
                                                    let client_reader = client.channel.clone();
                                                    thread::spawn(move || {
                                                        let mut buf = [0u8; 8192];
                                                        loop {
                                                            let n = {
                                                                let mut ch =
                                                                    match client_reader.lock() {
                                                                        Ok(guard) => guard,
                                                                        Err(_) => break,
                                                                    };
                                                                match ch.read(&mut buf) {
                                                                    Ok(0) => return,
                                                                    Ok(n) => n,
                                                                    Err(_) => 0,
                                                                }
                                                            };
                                                            if n > 0 {
                                                                if let Ok(mut guard) =
                                                                    app_reader.lock()
                                                                {
                                                                    guard.process_bytes(&buf[..n]);
                                                                }
                                                            } else {
                                                                std::thread::sleep(
                                                                    Duration::from_millis(10),
                                                                );
                                                            }
                                                        }
                                                    });
                                                    form.error = None;
                                                    app.go_to_connected(client, state);
                                                }
                                                Err(e) => {
                                                    app.error = Some(e);
                                                }
                                            }
                                        }
                                        Err(msg) => {
                                            app.error = Some(AppError::ValidationError(msg));
                                        }
                                    }
                                }
                                KeyCode::Backspace => {
                                    let s = form.focused_value_mut();
                                    s.pop();
                                }
                                KeyCode::Char(ch) => {
                                    let s = form.focused_value_mut();
                                    s.push(ch);
                                }
                                _ => {}
                            }
                        }
                        AppMode::FormEdit { form, original } => {
                            match key.code {
                                KeyCode::Esc => {
                                    app.go_to_connection_list();
                                }
                                KeyCode::Tab | KeyCode::Down => {
                                    form.next();
                                }
                                KeyCode::BackTab | KeyCode::Up => {
                                    form.prev();
                                }
                                KeyCode::Enter => {
                                    // Validate fields (password optional)
                                    if form.host.trim().is_empty() {
                                        app.error = Some(AppError::ValidationError(
                                            "Host is required".into(),
                                        ));
                                        continue;
                                    }
                                    if form.port.trim().is_empty() {
                                        app.error = Some(AppError::ValidationError(
                                            "Port is required".into(),
                                        ));
                                        continue;
                                    }
                                    let parsed_port = match form.port.parse::<u16>() {
                                        Ok(p) => p,
                                        Err(_) => {
                                            app.error = Some(AppError::ValidationError(
                                                "Port must be a number".into(),
                                            ));
                                            continue;
                                        }
                                    };
                                    if form.username.trim().is_empty() {
                                        app.error = Some(AppError::ValidationError(
                                            "Username is required".into(),
                                        ));
                                        continue;
                                    }

                                    let new_password = if form.password.is_empty() {
                                        original.password.clone()
                                    } else {
                                        form.password.clone()
                                    };

                                    let mut updated = original.clone();
                                    updated.host = form.host.trim().to_string();
                                    updated.port = parsed_port;
                                    updated.username = form.username.trim().to_string();
                                    updated.password = new_password;
                                    updated.display_name = form.display_name.trim().to_string();

                                    if let Err(e) = updated.validate() {
                                        app.error = Some(e);
                                        continue;
                                    }

                                    match app.config.update_connection(updated) {
                                        Ok(_) => {
                                            // persist changes
                                            if let Err(e) = app.config.save() {
                                                app.error = Some(e);
                                            }
                                            app.go_to_connection_list();
                                        }
                                        Err(e) => app.error = Some(e),
                                    }
                                }
                                KeyCode::Backspace => {
                                    let s = form.focused_value_mut();
                                    s.pop();
                                }
                                KeyCode::Char(ch) => {
                                    let s = form.focused_value_mut();
                                    s.push(ch);
                                }
                                _ => {}
                            }
                        }
                        AppMode::Connected { client, state } => match key.code {
                            KeyCode::Esc => {
                                let in_alt = state
                                    .lock()
                                    .ok()
                                    .map(|g| g.parser.screen().alternate_screen())
                                    .unwrap_or(false);
                                if in_alt {
                                    // jump to bottom before sending input
                                    if let Ok(mut guard) = state.lock() {
                                        if guard.parser.screen().scrollback() > 0 {
                                            guard.scroll_to_bottom();
                                        }
                                    }
                                    client.write_all(&[0x1b])?;
                                } else {
                                    client.close();
                                    app.go_to_connection_list();
                                }
                            }
                            KeyCode::Enter => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\r")?;
                            }
                            KeyCode::Backspace => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(&[0x7f])?;
                            }
                            KeyCode::Left => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\x1b[D")?;
                            }
                            KeyCode::Right => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\x1b[C")?;
                            }
                            KeyCode::Up => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\x1b[A")?;
                            }
                            KeyCode::Down => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\x1b[B")?;
                            }
                            KeyCode::Tab => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(b"\t")?;
                            }
                            KeyCode::PageUp => {
                                if let Ok(mut guard) = state.lock() {
                                    let rows = guard.parser.screen().size().0;
                                    let page = (rows.saturating_sub(1)) as i32;
                                    guard.scroll_by(page);
                                }
                            }
                            KeyCode::PageDown => {
                                if let Ok(mut guard) = state.lock() {
                                    let rows = guard.parser.screen().size().0;
                                    let page = (rows.saturating_sub(1)) as i32;
                                    guard.scroll_by(-page);
                                }
                            }
                            KeyCode::Home => {
                                if let Ok(mut guard) = state.lock() {
                                    // jump to top of scrollback
                                    let top = usize::MAX; // set_scrollback will clamp
                                    guard.parser.screen_mut().set_scrollback(top);
                                }
                            }
                            KeyCode::End => {
                                if let Ok(mut guard) = state.lock() {
                                    guard.scroll_to_bottom();
                                }
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(&[0x03])?;
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                client.write_all(&[0x04])?;
                            }
                            KeyCode::Char(ch_) => {
                                if let Ok(mut guard) = state.lock() {
                                    if guard.parser.screen().scrollback() > 0 {
                                        guard.scroll_to_bottom();
                                    }
                                }
                                let mut tmp = [0u8; 4];
                                let s = ch_.encode_utf8(&mut tmp);
                                let _ = client.write_all(s.as_bytes());
                            }
                            _ => {}
                        },
                    }
                }
                Event::Paste(data) => match &mut app.mode {
                    AppMode::FormNew { form } => {
                        let s = form.focused_value_mut();
                        s.push_str(&data);
                    }
                    AppMode::FormEdit { form, .. } => {
                        let s = form.focused_value_mut();
                        s.push_str(&data);
                    }
                    AppMode::Connected { client, state } => {
                        if let Ok(mut guard) = state.lock() {
                            if guard.parser.screen().scrollback() > 0 {
                                guard.scroll_to_bottom();
                            }
                        }
                        client.write_all(data.as_bytes())?;
                    }
                    AppMode::MainMenu { .. } => {}
                    AppMode::ConnectionList { .. } => {}
                },
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    ..
                }) => {
                    if let AppMode::Connected { state, .. } = &mut app.mode {
                        if let Ok(mut guard) = state.lock() {
                            guard.scroll_by(-3);
                        }
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    ..
                }) => {
                    if let AppMode::Connected { state, .. } = &mut app.mode {
                        if let Ok(mut guard) = state.lock() {
                            guard.scroll_by(3);
                        }
                    }
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

mod async_ssh_client;
mod config;
mod error;
mod key_event;
mod ui;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Margin;
use ratatui::prelude::Backend;
use tokio::{select, sync::mpsc, time};
use tui_textarea::TextArea;

use async_ssh_client::SshSession;
pub(crate) use async_ssh_client::expand_tilde;
use config::manager::{ConfigManager, Connection};
use error::{AppError, Result};
use ui::{
    ConnectionForm, DropdownState, ScpForm, TerminalState, draw_connection_form_popup,
    draw_connection_list, draw_delete_confirmation_popup, draw_dropdown_with_rect,
    draw_error_popup, draw_info_popup, draw_scp_popup, draw_scp_progress_popup, draw_terminal,
};

impl crate::async_ssh_client::ByteProcessor for TerminalState {
    fn process_bytes(&mut self, bytes: &[u8]) {
        TerminalState::process_bytes(self, bytes);
    }
}

/// Result of SCP transfer operation
#[derive(Debug, Clone)]
pub(crate) enum ScpResult {
    Success {
        local_path: String,
        remote_path: String,
    },
    Error {
        error: String,
    },
}

#[derive(Debug)]
enum AppEvent {
    Input(Event),
    Tick,
}

pub(crate) enum AppMode {
    ConnectionList {
        selected: usize,
        search_mode: bool,
        search_input: TextArea<'static>,
    },
    FormNew {
        form: ConnectionForm,
        current_selected: usize,
    },
    FormEdit {
        form: ConnectionForm,
        original: Connection,
        current_selected: usize,
    },
    Connected {
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
    },
    ScpForm {
        form: ScpForm,
        dropdown: Option<DropdownState>,
        current_selected: usize,
    },
    ScpProgress {
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        current_selected: usize,
    },
    DeleteConfirmation {
        connection_name: String,
        connection_id: String,
        current_selected: usize,
    },
}

/// SCP transfer progress state
#[derive(Clone, Debug)]
pub(crate) struct ScpProgress {
    pub(crate) local_path: String,
    pub(crate) remote_path: String,
    pub(crate) connection_name: String,
    pub(crate) start_time: std::time::Instant,
    pub(crate) spinner_state: usize, // For rotating spinner animation
    pub(crate) tick_counter: usize,  // Counter to slow down spinner updates
}

impl ScpProgress {
    pub(crate) fn new(local_path: String, remote_path: String, connection_name: String) -> Self {
        Self {
            local_path,
            remote_path,
            connection_name,
            start_time: std::time::Instant::now(),
            spinner_state: 0,
            tick_counter: 0,
        }
    }

    pub(crate) fn tick(&mut self) {
        self.tick_counter += 1;
        // Update spinner every 20 ticks (200ms at 10ms tick rate)
        if self.tick_counter % 20 == 0 {
            self.spinner_state = (self.spinner_state + 1) % 4;
        }
    }

    pub(crate) fn get_spinner_char(&self) -> char {
        match self.spinner_state {
            0 => '|',
            1 => '/',
            2 => '-',
            3 => '\\',
            _ => '|',
        }
    }
}

/// App is the main application
pub(crate) struct App<B: Backend + Write> {
    pub(crate) mode: AppMode,
    pub(crate) error: Option<AppError>,
    pub(crate) info: Option<String>,
    pub(crate) config: ConfigManager,
    terminal: Terminal<B>,
}

impl<B: Backend + Write> Drop for App<B> {
    fn drop(&mut self) {
        disable_raw_mode().ok();
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen,).ok();
    }
}

fn create_search_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type to search connections (Name | Host | User)");
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea
}

impl<B: Backend + Write> App<B> {
    fn new(terminal: Terminal<B>) -> Result<Self> {
        Ok(Self {
            mode: AppMode::ConnectionList {
                selected: 0,
                search_mode: false,
                search_input: create_search_textarea(),
            },
            error: None,
            info: None,
            config: ConfigManager::new()?,
            terminal,
        })
    }

    pub(crate) fn init_terminal(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            DisableMouseCapture
        )?;

        Ok(())
    }

    pub(crate) fn go_to_connected(
        &mut self,
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
    ) {
        self.mode = AppMode::Connected {
            name,
            client,
            state,
            current_selected,
        };
    }

    pub(crate) fn go_to_form_new(&mut self) {
        self.mode = AppMode::FormNew {
            form: ConnectionForm::new(),
            current_selected: self.current_selected(),
        };
    }

    pub(crate) fn go_to_form_edit(&mut self, form: ConnectionForm, original: Connection) {
        self.mode = AppMode::FormEdit {
            form,
            original,
            current_selected: self.current_selected(),
        };
    }

    pub(crate) fn go_to_connection_list_with_selected(&mut self, selected: usize) {
        self.mode = AppMode::ConnectionList {
            selected,
            search_mode: false,
            search_input: create_search_textarea(),
        };
    }

    pub(crate) fn go_to_scp_form(&mut self, current_selected: usize) {
        self.mode = AppMode::ScpForm {
            form: ScpForm::new(),
            dropdown: None,
            current_selected,
        };
    }

    pub(crate) fn go_to_scp_progress(
        &mut self,
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        current_selected: usize,
    ) {
        self.mode = AppMode::ScpProgress {
            progress,
            receiver,
            current_selected,
        };
    }

    pub(crate) fn go_to_delete_confirmation(
        &mut self,
        connection_name: String,
        connection_id: String,
        current_selected: usize,
    ) {
        self.mode = AppMode::DeleteConfirmation {
            connection_name,
            connection_id,
            current_selected,
        };
    }

    pub(crate) fn current_selected(&self) -> usize {
        match &self.mode {
            AppMode::ConnectionList { selected, .. } => {
                let len = self.config.connections().len();
                if len == 0 {
                    0
                } else {
                    (*selected).min(len - 1)
                }
            }
            AppMode::FormEdit {
                current_selected, ..
            }
            | AppMode::Connected {
                current_selected, ..
            }
            | AppMode::ScpForm {
                current_selected, ..
            }
            | AppMode::ScpProgress {
                current_selected, ..
            }
            | AppMode::DeleteConfirmation {
                current_selected, ..
            } => *current_selected,
            _ => 0,
        }
    }
}

pub fn init_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // intentionally ignore errors here since we're already in a panic
        eprintln!("Panic hook");
        let _ = restore_tui();
        original_hook(panic_info);
    }));
}

fn restore_tui() -> std::io::Result<()> {
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_panic_hook();

    // Setup Crossterm terminal
    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(terminal)?;
    app.init_terminal()?;

    // async event channel
    let (tx, mut rx) = mpsc::channel::<AppEvent>(100);

    // ticker
    let mut ticker = time::interval(Duration::from_millis(10));
    let tx_tick = tx.clone();

    // asynchronous: keyboard/terminal event listening
    let mut event_stream = event::EventStream::new();
    tokio::spawn(async move {
        loop {
            select! {
                maybe_ev = event_stream.next() => {
                    let ev = match maybe_ev {
                        None => break,
                        Some(Err(_)) => break,
                        Some(Ok(e)) => e,
                    };
                    if tx.send(AppEvent::Input(ev)).await.is_err() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if tx_tick.send(AppEvent::Tick).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // run app loop
    let res = run_app(&mut app, &mut rx).await;

    // app drop restores terminal
    drop(app);

    res
}

async fn run_app<B: Backend + Write>(
    app: &mut App<B>,
    rx: &mut mpsc::Receiver<AppEvent>,
) -> Result<()> {
    loop {
        if let AppMode::Connected { client, state, .. } = &app.mode {
            let size = app.terminal.size()?;
            // Calculate inner area for terminal content (accounting for borders)
            let h = size.height.saturating_sub(2); // Top and bottom borders
            let w = size.width.saturating_sub(2); // Left and right borders
            if let Ok(guard) = state.lock() {
                if guard.parser.screen().size() != (h, w) {
                    client.request_size(w, h).await;
                }
            }
        }

        // render a frame (sync)
        app.terminal.draw(|f| {
            let size = f.area();
            match &mut app.mode {
                AppMode::ConnectionList {
                    selected,
                    search_mode,
                    search_input,
                } => {
                    let conns = app.config.connections();
                    let search_query = &search_input.lines()[0];

                    if *search_mode {
                        // In search mode: custom layout with table, search input, and footer
                        let layout = ratatui::layout::Layout::default()
                            .direction(ratatui::layout::Direction::Vertical)
                            .constraints([
                                ratatui::layout::Constraint::Min(1),    // Table area
                                ratatui::layout::Constraint::Length(3), // Search input area
                                ratatui::layout::Constraint::Length(1), // Footer area
                            ])
                            .split(size);

                        // Render the table in the first area
                        draw_connection_list(
                            layout[0],
                            conns,
                            *selected,
                            *search_mode,
                            &search_query,
                            f,
                        );

                        // Render search input in the second area
                        search_input.set_block(
                            ratatui::widgets::Block::default()
                                .borders(ratatui::widgets::Borders::ALL)
                                .title("Search")
                                .style(
                                    ratatui::style::Style::default()
                                        .fg(ratatui::style::Color::Cyan),
                                ),
                        );
                        f.render_widget(&*search_input, layout[1]);

                        // Render footer in the third area
                        let footer = ratatui::layout::Layout::default()
                            .direction(ratatui::layout::Direction::Horizontal)
                            .constraints([
                                ratatui::layout::Constraint::Percentage(50),
                                ratatui::layout::Constraint::Percentage(50),
                            ])
                            .split(layout[2]);

                        let hint_text =
                            "Enter: Apply Search   Esc: Exit Search   Arrow Keys: Move Cursor";
                        let left = ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                            ratatui::text::Span::styled(
                                hint_text,
                                ratatui::style::Style::default()
                                    .fg(ratatui::style::Color::White)
                                    .add_modifier(ratatui::style::Modifier::DIM),
                            ),
                        ))
                        .alignment(ratatui::layout::Alignment::Left);

                        let right = ratatui::widgets::Paragraph::new(ratatui::text::Line::from(
                            ratatui::text::Span::styled(
                                format!("TermiRs v{}", env!("CARGO_PKG_VERSION")),
                                ratatui::style::Style::default()
                                    .fg(ratatui::style::Color::White)
                                    .add_modifier(ratatui::style::Modifier::DIM),
                            ),
                        ))
                        .alignment(ratatui::layout::Alignment::Right);

                        f.render_widget(left, footer[0]);
                        f.render_widget(right, footer[1]);
                    } else {
                        // Normal mode: let draw_connection_list handle everything
                        draw_connection_list(
                            size,
                            conns,
                            *selected,
                            *search_mode,
                            &search_query,
                            f,
                        );
                    }
                }
                AppMode::FormNew {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = app.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::FormEdit {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = app.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::Connected { name, state, .. } => {
                    let inner = size.inner(Margin::new(1, 1));
                    if let Ok(mut guard) = state.lock() {
                        if guard.parser.screen().size() != (inner.height, inner.width) {
                            guard.resize(inner.height, inner.width);
                        }
                        draw_terminal(size, &guard, name, f);
                    }
                }
                AppMode::ScpForm {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = app.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::ScpProgress {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = app.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::DeleteConfirmation {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = app.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
            }

            // Overlay info popup if any
            if let Some(msg) = &app.info {
                draw_info_popup(size, msg, f);
            }

            // Overlay SCP popup if in SCP form mode
            if let AppMode::ScpForm { form, dropdown, .. } = &mut app.mode {
                let scp_input_rects = draw_scp_popup(size, form, f);

                // Update dropdown anchor rect if dropdown exists
                if let Some(dropdown) = dropdown {
                    draw_dropdown_with_rect(dropdown, scp_input_rects.0, f);
                }
            }

            // Overlay SCP progress popup if in SCP progress mode
            if let AppMode::ScpProgress { progress, .. } = &app.mode {
                draw_scp_progress_popup(size, progress, f);
            }

            // Overlay delete confirmation popup if in delete confirmation mode
            if let AppMode::DeleteConfirmation {
                connection_name, ..
            } = &app.mode
            {
                draw_delete_confirmation_popup(size, connection_name, f);
            }

            // Overlay connection form popup if in form mode
            if let AppMode::FormNew { form, .. } = &app.mode {
                draw_connection_form_popup(size, form, true, f);
            }
            if let AppMode::FormEdit { form, .. } = &mut app.mode {
                draw_connection_form_popup(size, form, false, f);
            }

            // Overlay error popup if any (always on top)
            if let Some(err) = &app.error {
                draw_error_popup(size, &err.to_string(), f);
            }
        })?;

        // wait for an event (asynchronous)
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break, // exit if channel is closed
        };

        match ev {
            AppEvent::Tick => {
                // Update spinner animation for SCP progress and handle results
                if let AppMode::ScpProgress {
                    progress,
                    receiver,
                    current_selected,
                } = &mut app.mode
                {
                    progress.tick();

                    // Drain any SCP results
                    match receiver.try_recv() {
                        Ok(result) => {
                            let current_selected = *current_selected;

                            // Handle the result
                            match result {
                                ScpResult::Success {
                                    local_path,
                                    remote_path,
                                } => {
                                    app.info = Some(format!(
                                        "SCP upload completed from {} to {}",
                                        local_path, remote_path
                                    ));
                                }
                                ScpResult::Error { error } => {
                                    app.error = Some(AppError::SshConnectionError(error));
                                }
                            }

                            // Go back to connection list
                            app.go_to_connection_list_with_selected(current_selected);
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            let current_selected = *current_selected;
                            app.error = Some(AppError::SshConnectionError(
                                "SCP transfer task disconnected unexpectedly".to_string(),
                            ));
                            app.go_to_connection_list_with_selected(current_selected);
                        }
                    }
                }
            }
            AppEvent::Input(ev) => match ev {
                Event::Key(key) => match crate::key_event::handle_key_event(app, key).await {
                    crate::key_event::KeyFlow::Continue => {}
                    crate::key_event::KeyFlow::Quit => {
                        return Ok(());
                    }
                },
                Event::Paste(data) => {
                    crate::key_event::handle_paste_event(app, &data).await;
                }
                Event::Resize(_, _) => {}
                _ => {}
            },
        }
    }
    Ok(())
}

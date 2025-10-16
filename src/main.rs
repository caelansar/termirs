mod async_ssh_client;
mod config;
mod error;
mod filesystem;
mod key_event;
mod ui;

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crossterm::cursor::Show;
use crossterm::event::{self, DisableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
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
    draw_error_popup, draw_file_explorer, draw_info_popup, draw_scp_popup, draw_scp_progress_popup,
    draw_terminal, rect_with_top_margin,
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
        mode: crate::ui::ScpMode,
        local_path: String,
        remote_path: String,
        destination_filename: String, // Filename to select after transfer
    },
    Error {
        error: String,
    },
}

#[derive(Debug)]
enum AppEvent {
    Input(Event),
    Tick,
    Disconnect,
}

/// Enum to track where to return after SCP operations
/// Which pane is currently active in the file explorer
#[derive(Clone, Debug)]
pub(crate) enum FileExplorerPane {
    Local,
    Remote,
}

/// Copy operation state for file transfer
#[derive(Clone, Debug)]
pub(crate) struct CopyOperation {
    pub(crate) source_path: String,
    pub(crate) source_name: String,
    pub(crate) direction: CopyDirection,
}

/// Direction of file transfer
#[derive(Clone, Debug)]
pub(crate) enum CopyDirection {
    LocalToRemote,
    RemoteToLocal,
}

pub(crate) enum ScpReturnMode {
    ConnectionList {
        current_selected: usize,
    },
    Connected {
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
        cancel_token: tokio_util::sync::CancellationToken,
    },
    FileExplorer {
        connection_name: String,
        local_explorer: ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>,
        remote_explorer: ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>,
        active_pane: FileExplorerPane,
        copy_operation: Option<CopyOperation>,
        return_to: usize,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
    },
}

impl ScpReturnMode {
    /// Clone the return mode while dropping non-cloneable channel handles.
    pub(crate) fn clone_without_channel(&self) -> Self {
        match self {
            ScpReturnMode::ConnectionList { current_selected } => ScpReturnMode::ConnectionList {
                current_selected: *current_selected,
            },
            ScpReturnMode::Connected {
                name,
                client,
                state,
                current_selected,
                cancel_token,
            } => ScpReturnMode::Connected {
                name: name.clone(),
                client: client.clone(),
                state: state.clone(),
                current_selected: *current_selected,
                cancel_token: cancel_token.clone(),
            },
            ScpReturnMode::FileExplorer {
                connection_name,
                local_explorer,
                remote_explorer,
                active_pane,
                copy_operation,
                return_to,
                sftp_session,
                ssh_connection,
                ..
            } => ScpReturnMode::FileExplorer {
                connection_name: connection_name.clone(),
                local_explorer: local_explorer.clone(),
                remote_explorer: remote_explorer.clone(),
                active_pane: active_pane.clone(),
                copy_operation: copy_operation.clone(),
                return_to: *return_to,
                sftp_session: sftp_session.clone(),
                ssh_connection: ssh_connection.clone(),
                channel: None,
            },
        }
    }
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
        cancel_token: tokio_util::sync::CancellationToken, // Token to cancel the read task
    },
    ScpForm {
        form: ScpForm,
        dropdown: Option<DropdownState>,
        return_mode: ScpReturnMode,
        channel: Option<russh::Channel<russh::client::Msg>>,
    },
    ScpProgress {
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        return_mode: ScpReturnMode,
    },
    DeleteConfirmation {
        connection_name: String,
        connection_id: String,
        current_selected: usize,
    },
    FileExplorer {
        connection_name: String,
        local_explorer: ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>,
        remote_explorer: ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>,
        active_pane: FileExplorerPane,
        copy_operation: Option<CopyOperation>,
        return_to: usize,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
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
    pub(crate) mode: crate::ui::ScpMode, // Send or Receive mode
}

impl ScpProgress {
    #[allow(unused)]
    pub(crate) fn new(local_path: String, remote_path: String, connection_name: String) -> Self {
        Self::new_with_mode(
            local_path,
            remote_path,
            connection_name,
            crate::ui::ScpMode::Send,
        )
    }

    pub(crate) fn new_with_mode(
        local_path: String,
        remote_path: String,
        connection_name: String,
        mode: crate::ui::ScpMode,
    ) -> Self {
        Self {
            local_path,
            remote_path,
            connection_name,
            start_time: std::time::Instant::now(),
            spinner_state: 0,
            tick_counter: 0,
            mode,
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
    needs_redraw: bool, // Track if UI needs redrawing
    event_tx: Option<tokio::sync::mpsc::Sender<AppEvent>>, // Event sender for SSH disconnect
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
            needs_redraw: true, // Initial redraw needed
            event_tx: None,     // Will be set later
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

    pub(crate) fn set_event_sender(&mut self, sender: tokio::sync::mpsc::Sender<AppEvent>) {
        self.event_tx = Some(sender);
    }

    pub(crate) fn go_to_connected(
        &mut self,
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
        cancel_token: tokio_util::sync::CancellationToken,
    ) {
        self.mode = AppMode::Connected {
            name,
            client,
            state,
            current_selected,
            cancel_token,
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_form_new(&mut self) {
        self.mode = AppMode::FormNew {
            form: ConnectionForm::new(),
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_form_edit(&mut self, form: ConnectionForm, original: Connection) {
        self.mode = AppMode::FormEdit {
            form,
            original,
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_connection_list_with_selected(&mut self, selected: usize) {
        self.mode = AppMode::ConnectionList {
            selected,
            search_mode: false,
            search_input: create_search_textarea(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_scp_form(&mut self, current_selected: usize) {
        self.mode = AppMode::ScpForm {
            form: ScpForm::new(),
            dropdown: None,
            return_mode: ScpReturnMode::ConnectionList { current_selected },
            channel: None,
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_scp_form_from_connected(
        &mut self,
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
        channel: Option<russh::Channel<russh::client::Msg>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) {
        self.mode = AppMode::ScpForm {
            form: ScpForm::new(),
            dropdown: None,
            channel,
            return_mode: ScpReturnMode::Connected {
                name,
                client,
                state,
                current_selected,
                cancel_token,
            },
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_scp_progress(
        &mut self,
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        return_mode: ScpReturnMode,
    ) {
        self.mode = AppMode::ScpProgress {
            progress,
            receiver,
            return_mode,
        };
        self.needs_redraw = true; // Mode change requires redraw
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
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) async fn go_to_file_explorer(
        &mut self,
        conn: Connection,
        return_to: usize,
    ) -> Result<()> {
        // For SFTP, we need to create a new session directly since we need both the session and channel
        // We'll use the existing sftp_send_file pattern but adapt it for our needs
        let (sftp_session, channel) = Self::create_sftp_session(&conn).await?;
        let sftp_session = Arc::new(sftp_session);

        // Initialize local file explorer
        // Use current directory as it's more reliable than HOME which might be on a slow network mount
        let local_start_dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "/tmp".to_string());

        let local_explorer = ratatui_explorer::FileExplorer::with_fs(
            Arc::new(ratatui_explorer::LocalFileSystem),
            local_start_dir.clone(),
        )
        .await
        .map_err(|e| {
            AppError::SftpError(format!(
                "Failed to initialize local explorer from '{}': {}",
                local_start_dir, e
            ))
        })?;

        // Initialize remote file explorer (start from home directory)
        // Canonicalize the remote home path to get the absolute path
        let remote_home_canonical = sftp_session.canonicalize(".").await.map_err(|e| {
            AppError::SftpError(format!("Failed to resolve remote home directory: {}", e))
        })?;

        let sftp_fs = crate::filesystem::SftpFileSystem::new(sftp_session.clone());
        let remote_explorer = ratatui_explorer::FileExplorer::with_fs(
            Arc::new(sftp_fs),
            remote_home_canonical.clone(),
        )
        .await
        .map_err(|e| {
            AppError::SftpError(format!(
                "Failed to initialize remote explorer from '{}': {}",
                remote_home_canonical, e
            ))
        })?;

        // Transition to FileExplorer mode
        self.mode = AppMode::FileExplorer {
            connection_name: conn.display_name.clone(),
            local_explorer,
            remote_explorer,
            active_pane: FileExplorerPane::Local,
            copy_operation: None,
            return_to,
            sftp_session,
            ssh_connection: conn,
            channel: Some(channel),
        };
        self.needs_redraw = true;
        Ok(())
    }

    async fn create_sftp_session(
        conn: &Connection,
    ) -> Result<(
        russh_sftp::client::SftpSession,
        russh::Channel<russh::client::Msg>,
    )> {
        // Create a new SSH session specifically for SFTP
        let (session, _server_key) = SshSession::new_session_with_timeout(
            conn,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await?;

        // Open a channel for SFTP
        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;

        // Create and initialize SFTP session
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::SftpError(format!("SFTP session creation failed: {}", e)))?;

        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;

        Ok((sftp, channel))
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
            | AppMode::DeleteConfirmation {
                current_selected, ..
            } => *current_selected,
            AppMode::ScpForm { return_mode, .. } | AppMode::ScpProgress { return_mode, .. } => {
                match return_mode {
                    ScpReturnMode::ConnectionList { current_selected } => *current_selected,
                    ScpReturnMode::Connected {
                        current_selected, ..
                    } => *current_selected,
                    ScpReturnMode::FileExplorer { return_to, .. } => *return_to,
                }
            }
            AppMode::FileExplorer { return_to, .. } => *return_to,
            AppMode::FormNew { .. } => 0,
        }
    }

    /// Mark that UI needs redrawing
    pub(crate) fn mark_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Check if redraw is needed and mark as drawn
    pub(crate) fn should_redraw(&mut self) -> bool {
        let should = self.needs_redraw;
        self.needs_redraw = false;
        should
    }

    /// Set error and mark for redraw
    pub(crate) fn set_error(&mut self, error: AppError) {
        self.error = Some(error);
        self.needs_redraw = true;
    }

    /// Set info and mark for redraw  
    pub(crate) fn set_info(&mut self, info: String) {
        self.info = Some(info);
        self.needs_redraw = true;
    }

    fn draw(&mut self) -> Result<()> {
        self.terminal.draw(|f| {
            let size = f.area();
            match &mut self.mode {
                AppMode::ConnectionList {
                    selected,
                    search_mode,
                    search_input,
                } => {
                    let conns = self.config.connections();
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
                            search_query,
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
                        draw_connection_list(size, conns, *selected, *search_mode, search_query, f);
                    }
                }
                AppMode::FormNew {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::FormEdit {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::Connected { name, state, .. } => {
                    let inner = rect_with_top_margin(size, 1);
                    if let Ok(mut guard) = state.try_lock() {
                        if guard.parser.screen().size() != (inner.height, inner.width) {
                            guard.resize(inner.height, inner.width);
                        }
                        draw_terminal(size, &mut guard, name, f);
                    }
                }
                AppMode::ScpForm { return_mode, .. } => {
                    // Render appropriate background based on return mode
                    match return_mode {
                        ScpReturnMode::ConnectionList { current_selected } => {
                            let conns = self.config.connections();
                            draw_connection_list(size, conns, *current_selected, false, "", f);
                        }
                        ScpReturnMode::Connected { name, state, .. } => {
                            let inner = rect_with_top_margin(size, 1);
                            if let Ok(mut guard) = state.try_lock() {
                                if guard.parser.screen().size() != (inner.height, inner.width) {
                                    guard.resize(inner.height, inner.width);
                                }
                                draw_terminal(size, &mut guard, name, f);
                            }
                        }
                        ScpReturnMode::FileExplorer {
                            connection_name,
                            local_explorer,
                            remote_explorer,
                            active_pane,
                            copy_operation,
                            ..
                        } => {
                            draw_file_explorer(
                                f,
                                size,
                                connection_name,
                                local_explorer,
                                remote_explorer,
                                active_pane,
                                copy_operation,
                            );
                        }
                    }
                }
                AppMode::ScpProgress { return_mode, .. } => {
                    // Render appropriate background based on return mode
                    match return_mode {
                        ScpReturnMode::ConnectionList { current_selected } => {
                            let conns = self.config.connections();
                            draw_connection_list(size, conns, *current_selected, false, "", f);
                        }
                        ScpReturnMode::Connected { name, state, .. } => {
                            let inner = rect_with_top_margin(size, 1);
                            if let Ok(mut guard) = state.try_lock() {
                                if guard.parser.screen().size() != (inner.height, inner.width) {
                                    guard.resize(inner.height, inner.width);
                                }
                                draw_terminal(size, &mut guard, name, f);
                            }
                        }
                        ScpReturnMode::FileExplorer {
                            connection_name,
                            local_explorer,
                            remote_explorer,
                            active_pane,
                            copy_operation,
                            ..
                        } => {
                            draw_file_explorer(
                                f,
                                size,
                                connection_name,
                                local_explorer,
                                remote_explorer,
                                active_pane,
                                copy_operation,
                            );
                        }
                    }
                }
                AppMode::DeleteConfirmation {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f);
                }
                AppMode::FileExplorer {
                    connection_name,
                    local_explorer,
                    remote_explorer,
                    active_pane,
                    copy_operation,
                    ..
                } => {
                    draw_file_explorer(
                        f,
                        size,
                        connection_name,
                        local_explorer,
                        remote_explorer,
                        active_pane,
                        copy_operation,
                    );
                }
            }

            // Overlay info popup if any
            if let Some(msg) = &self.info {
                draw_info_popup(size, msg, f);
            }

            // Overlay SCP popup if in SCP form mode
            if let AppMode::ScpForm { form, dropdown, .. } = &mut self.mode {
                let scp_input_rects = draw_scp_popup(size, form, f);

                // Update dropdown anchor rect if dropdown exists
                if let Some(dropdown) = dropdown {
                    draw_dropdown_with_rect(dropdown, scp_input_rects.0, f);
                }
            }

            // Overlay SCP progress popup if in SCP progress mode
            if let AppMode::ScpProgress { progress, .. } = &self.mode {
                draw_scp_progress_popup(size, progress, f);
            }

            // Overlay delete confirmation popup if in delete confirmation mode
            if let AppMode::DeleteConfirmation {
                connection_name, ..
            } = &self.mode
            {
                draw_delete_confirmation_popup(size, connection_name, f);
            }

            // Overlay connection form popup if in form mode
            if let AppMode::FormNew { form, .. } = &self.mode {
                draw_connection_form_popup(size, form, true, f);
            }
            if let AppMode::FormEdit { form, .. } = &mut self.mode {
                draw_connection_form_popup(size, form, false, f);
            }

            // Overlay error popup if any (always on top)
            if let Some(err) = &self.error {
                draw_error_popup(size, &err.to_string(), f);
            }
        })?;

        Ok(())
    }

    async fn run(&mut self, rx: &mut mpsc::Receiver<AppEvent>) -> Result<()> {
        loop {
            // Check terminal size changes and update SSH session if needed
            let mut terminal_size_changed = false;
            let mut has_terminal_updates = false;

            if let AppMode::Connected { client, state, .. } = &self.mode {
                let size = self.terminal.size()?;
                // Calculate inner area for terminal content (accounting for borders)
                let h = size.height.saturating_sub(1); // Top borders
                let w = size.width;
                let guard = state.lock().await;
                if guard.parser.screen().size() != (h, w) {
                    client.request_size(w, h).await;
                    terminal_size_changed = true;
                }

                // Check if terminal content has been updated recently
                // Only redraw if content changed within last few milliseconds
                let time_since_update = guard.last_change.elapsed();
                if time_since_update.as_millis() < 100 {
                    has_terminal_updates = true;
                }
            }

            // Only render when needed
            if self.should_redraw() || terminal_size_changed || has_terminal_updates {
                self.draw()?;
            }

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
                        return_mode,
                    } = &mut self.mode
                    {
                        progress.tick();
                        // Mark redraw after handling the progress update

                        // Drain any SCP results
                        match receiver.try_recv() {
                            Ok(result) => {
                                // Clone the return_mode before we change self.mode
                                let return_mode = return_mode.clone_without_channel();

                                // Check if transfer was successful before handling the result
                                let transfer_successful =
                                    matches!(result, ScpResult::Success { .. });

                                // Handle the result and extract destination filename
                                let destination_filename = if let ScpResult::Success {
                                    ref destination_filename,
                                    ..
                                } = result
                                {
                                    Some(destination_filename.clone())
                                } else {
                                    None
                                };

                                // Handle the result
                                match result {
                                    ScpResult::Success {
                                        mode,
                                        local_path,
                                        remote_path,
                                        ..
                                    } => match mode {
                                        crate::ui::ScpMode::Send => {
                                            self.set_info(format!(
                                                    "SCP transfer completed from {local_path} to {remote_path}"
                                                ));
                                        }
                                        crate::ui::ScpMode::Receive => {
                                            self.set_info(format!(
                                                    "SCP transfer completed from {remote_path} to {local_path}"
                                                ));
                                        }
                                    },
                                    ScpResult::Error { error } => {
                                        self.set_error(AppError::SshConnectionError(error));
                                    }
                                }

                                // Return to the appropriate mode
                                match return_mode {
                                    ScpReturnMode::ConnectionList { current_selected } => {
                                        self.go_to_connection_list_with_selected(current_selected);
                                    }
                                    ScpReturnMode::Connected {
                                        name,
                                        client,
                                        state,
                                        current_selected,
                                        cancel_token,
                                    } => {
                                        self.go_to_connected(
                                            name,
                                            client,
                                            state,
                                            current_selected,
                                            cancel_token,
                                        );
                                    }
                                    ScpReturnMode::FileExplorer {
                                        connection_name,
                                        mut local_explorer,
                                        mut remote_explorer,
                                        active_pane,
                                        copy_operation,
                                        return_to,
                                        sftp_session,
                                        ssh_connection,
                                        channel,
                                    } => {
                                        // If transfer was successful, refresh the destination pane and select the transferred file
                                        if transfer_successful {
                                            // The active_pane is the destination pane (where paste was executed)
                                            match active_pane {
                                                FileExplorerPane::Local => {
                                                    // Refresh local pane
                                                    let local_cwd =
                                                        local_explorer.cwd().to_path_buf();
                                                    if let Err(e) =
                                                        local_explorer.set_cwd(local_cwd).await
                                                    {
                                                        self.set_error(AppError::SftpError(
                                                            format!(
                                                                "Failed to refresh local pane: {}",
                                                                e
                                                            ),
                                                        ));
                                                    } else if let Some(ref filename) =
                                                        destination_filename
                                                    {
                                                        // Select the transferred file
                                                        local_explorer.select_file(filename);
                                                    }
                                                }
                                                FileExplorerPane::Remote => {
                                                    // Refresh remote pane
                                                    let remote_cwd =
                                                        remote_explorer.cwd().to_path_buf();
                                                    if let Err(e) =
                                                        remote_explorer.set_cwd(remote_cwd).await
                                                    {
                                                        self.set_error(AppError::SftpError(
                                                            format!(
                                                                "Failed to refresh remote pane: {}",
                                                                e
                                                            ),
                                                        ));
                                                    } else if let Some(ref filename) =
                                                        destination_filename
                                                    {
                                                        // Select the transferred file
                                                        remote_explorer.select_file(filename);
                                                    }
                                                }
                                            }
                                        }

                                        // Return to file explorer mode
                                        self.mode = AppMode::FileExplorer {
                                            connection_name,
                                            local_explorer,
                                            remote_explorer,
                                            active_pane,
                                            copy_operation,
                                            return_to,
                                            sftp_session,
                                            ssh_connection,
                                            channel,
                                        };
                                    }
                                }
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                // Clone the return_mode before we change self.mode
                                let return_mode = return_mode.clone_without_channel();

                                self.set_error(AppError::SshConnectionError(
                                    "SCP transfer task disconnected unexpectedly".to_string(),
                                ));

                                // Return to the appropriate mode
                                match return_mode {
                                    ScpReturnMode::ConnectionList { current_selected } => {
                                        self.go_to_connection_list_with_selected(current_selected);
                                    }
                                    ScpReturnMode::Connected {
                                        name,
                                        client,
                                        state,
                                        current_selected,
                                        cancel_token,
                                    } => {
                                        self.go_to_connected(
                                            name,
                                            client,
                                            state,
                                            current_selected,
                                            cancel_token,
                                        );
                                    }
                                    ScpReturnMode::FileExplorer {
                                        connection_name,
                                        local_explorer,
                                        remote_explorer,
                                        active_pane,
                                        copy_operation,
                                        return_to,
                                        sftp_session,
                                        ssh_connection,
                                        channel,
                                    } => {
                                        // Return to file explorer mode
                                        self.mode = AppMode::FileExplorer {
                                            connection_name,
                                            local_explorer,
                                            remote_explorer,
                                            active_pane,
                                            copy_operation,
                                            return_to,
                                            sftp_session,
                                            ssh_connection,
                                            channel,
                                        };
                                    }
                                }
                            }
                        }

                        // Mark redraw after all progress handling is done
                        self.mark_redraw();
                    }
                }
                AppEvent::Input(ev) => {
                    self.mark_redraw(); // Input events typically need redraw
                    match ev {
                        Event::Key(key) => {
                            match crate::key_event::handle_key_event(self, key).await {
                                crate::key_event::KeyFlow::Continue => {}
                                crate::key_event::KeyFlow::Quit => {
                                    return Ok(());
                                }
                            }
                        }
                        Event::Paste(data) => {
                            crate::key_event::handle_paste_event(self, &data).await;
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                AppEvent::Disconnect => {
                    // SSH connection has been disconnected (e.g., user typed 'exit')
                    // Automatically return to the connection list
                    if let AppMode::Connected {
                        current_selected,
                        cancel_token,
                        ..
                    } = &self.mode
                    {
                        let current_selected = *current_selected;
                        // Cancel the read task
                        cancel_token.cancel();
                        // Go back to connection list
                        self.go_to_connection_list_with_selected(current_selected);
                        self.mark_redraw();
                    }
                }
            }
        }
        Ok(())
    }
}

fn init_panic_hook() {
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
    execute!(std::io::stdout(), LeaveAlternateScreen, Show)?;
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

    // Set the event sender in the app
    app.set_event_sender(tx.clone());

    // ticker - reduced frequency for better performance
    let mut ticker = time::interval(Duration::from_millis(50)); // Changed from 10ms to 50ms
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
    let res = app.run(&mut rx).await;

    // app drop restores terminal
    drop(app);

    res
}

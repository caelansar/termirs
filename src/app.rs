use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use arboard::Clipboard;
use ratatui::Terminal;
use ratatui::layout::Rect;
use ratatui::prelude::Backend;
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::async_ssh_client::SshSession;
use crate::config::manager::{ConfigManager, Connection};
use crate::error::{AppError, Result};
use crate::events::AppEvent;
use crate::mode_state::{
    DeleteConfirmationState, FormWithConnectionSelector, ListSelectionState, SourceSelectorState,
};
use crate::search_state::SearchState;
use crate::terminal::{
    LastMouseClick, MouseClickClass, SelectionAutoScroll, SelectionEndpoint,
    SelectionScrollDirection, TerminalPoint, compute_selection_for_view, make_selection_endpoint,
};
use crate::transfer::{ScpProgress, ScpResult};
use crate::ui::{
    ConnectionForm, DeleteConfirmationConfig, TerminalState, draw_connecting_popup,
    draw_connection_form_popup, draw_connection_list, draw_delete_confirmation_popup,
    draw_error_popup, draw_file_explorer, draw_info_popup, draw_port_forwarding_form_popup,
    draw_port_forwarding_list, draw_scp_progress_popup, draw_terminal, rect_with_top_margin,
};

/// Enum to track where to return after SCP operations
/// Which pane is currently active in the file explorer
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum FileExplorerPane {
    Local,
    RemoteSsh {
        connection_name: String,
        connection: Connection,
    },
}

/// Enum to track which pane (left/right) is active
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivePane {
    Left,
    Right,
}

/// Wrapper enum for left pane explorer (can be Local or Remote)
pub enum LeftExplorer {
    Local(ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>),
    Remote(ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>),
}

impl LeftExplorer {
    /// Get the current file/directory
    pub fn current(&self) -> &ratatui_explorer::File {
        match self {
            LeftExplorer::Local(explorer) => explorer.current(),
            LeftExplorer::Remote(explorer) => explorer.current(),
        }
    }

    /// Get current working directory
    pub fn cwd(&self) -> &std::path::Path {
        match self {
            LeftExplorer::Local(explorer) => explorer.cwd(),
            LeftExplorer::Remote(explorer) => explorer.cwd(),
        }
    }

    /// Set current working directory
    pub async fn set_cwd(&mut self, path: std::path::PathBuf) -> std::io::Result<()> {
        match self {
            LeftExplorer::Local(explorer) => explorer.set_cwd(path).await,
            LeftExplorer::Remote(explorer) => explorer.set_cwd(path).await,
        }
    }

    /// Handle input
    pub async fn handle(&mut self, input: ratatui_explorer::Input) -> std::io::Result<()> {
        match self {
            LeftExplorer::Local(explorer) => explorer.handle(input).await,
            LeftExplorer::Remote(explorer) => explorer.handle(input).await,
        }
    }

    /// Set search filter
    pub fn set_search_filter(&mut self, filter: Option<String>) {
        match self {
            LeftExplorer::Local(explorer) => explorer.set_search_filter(filter),
            LeftExplorer::Remote(explorer) => explorer.set_search_filter(filter),
        }
    }

    /// Get all files in the current directory
    pub fn files(&self) -> Vec<&ratatui_explorer::File> {
        match self {
            LeftExplorer::Local(explorer) => explorer.files(),
            LeftExplorer::Remote(explorer) => explorer.files(),
        }
    }

    /// Set selected index
    pub fn set_selected_idx(&mut self, idx: usize) {
        match self {
            LeftExplorer::Local(explorer) => explorer.set_selected_idx(idx),
            LeftExplorer::Remote(explorer) => explorer.set_selected_idx(idx),
        }
    }

    /// Set show hidden files
    pub async fn set_show_hidden(&mut self, show: bool) -> std::io::Result<()> {
        match self {
            LeftExplorer::Local(explorer) => explorer.set_show_hidden(show).await,
            LeftExplorer::Remote(explorer) => explorer.set_show_hidden(show).await,
        }
    }

    /// Get show hidden setting
    pub fn show_hidden(&self) -> bool {
        match self {
            LeftExplorer::Local(explorer) => explorer.show_hidden(),
            LeftExplorer::Remote(explorer) => explorer.show_hidden(),
        }
    }

    /// Select a file by name
    pub fn select_file(&mut self, filename: &str) -> bool {
        match self {
            LeftExplorer::Local(explorer) => explorer.select_file(filename),
            LeftExplorer::Remote(explorer) => explorer.select_file(filename),
        }
    }
}

impl Clone for LeftExplorer {
    fn clone(&self) -> Self {
        match self {
            LeftExplorer::Local(explorer) => LeftExplorer::Local(explorer.clone()),
            LeftExplorer::Remote(explorer) => LeftExplorer::Remote(explorer.clone()),
        }
    }
}

/// Copy operation state for file transfer
#[derive(Clone, Debug)]
pub struct CopyOperation {
    pub source_path: String,
    pub source_name: String,
    pub direction: CopyDirection,
    pub is_dir: bool,
}

/// Direction of file transfer (pane-based, not filesystem-based)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyDirection {
    LeftToRight, // From left pane to right pane
    RightToLeft, // From right pane to left pane
}

#[allow(clippy::large_enum_variant)]
pub enum ScpReturnMode {
    #[allow(dead_code)]
    ConnectionList { current_selected: usize },
    #[allow(dead_code)]
    Connected {
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
        cancel_token: tokio_util::sync::CancellationToken,
    },
    FileExplorer {
        connection_name: String,

        left_pane: FileExplorerPane,
        left_explorer: LeftExplorer,
        left_sftp: Option<(Arc<russh_sftp::client::SftpSession>, Connection)>,
        left_session: Option<
            Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,
        >,

        remote_explorer: ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
        ssh_session:
            Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,

        active_pane: ActivePane,
        copy_buffer: Vec<CopyOperation>,
        return_to: usize,
        search: SearchState,
    },
}

/// Track where a connection was initiated from
#[derive(Clone)]
pub enum ConnectingSource {
    FormNew {
        auto_auth: bool,
        form: ConnectionForm,
    },
    FormEdit {
        form: ConnectionForm,
        original: Connection,
    },
    ConnectionList {
        file_explorer: bool,
    },
}

#[allow(clippy::large_enum_variant)]
pub enum AppMode {
    ConnectionList(ListSelectionState),
    FormNew {
        auto_auth: bool,
        form: ConnectionForm,
        current_selected: usize,
    },
    FormEdit {
        form: ConnectionForm,
        original: Connection,
        current_selected: usize,
    },
    Connecting {
        connection: Connection,
        connection_name: String,
        return_to: usize,
        return_from: ConnectingSource,
        cancel_token: tokio_util::sync::CancellationToken,
        receiver: mpsc::Receiver<Result<SshSession>>,
    },
    Connected {
        name: String,
        client: SshSession,
        state: Arc<Mutex<TerminalState>>,
        current_selected: usize,
        cancel_token: tokio_util::sync::CancellationToken, // Token to cancel the read task
    },
    ScpProgress {
        progress: ScpProgress,
        return_mode: Option<ScpReturnMode>,
    },
    DeleteConfirmation {
        connection_name: String,
        connection_id: String,
        current_selected: usize,
    },
    FileExplorer {
        connection_name: String, // Right pane connection name (original)

        // Left pane - switchable between Local and SSH
        left_pane: FileExplorerPane,
        left_explorer: LeftExplorer,
        left_sftp: Option<(Arc<russh_sftp::client::SftpSession>, Connection)>,
        left_session: Option<
            Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,
        >,

        // Right pane - always Remote SSH (original connection from entry)
        remote_explorer: ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
        ssh_session:
            Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,

        active_pane: ActivePane,
        copy_buffer: Vec<CopyOperation>,
        return_to: usize,
        search: SearchState,

        // UI overlays
        source_selector: SourceSelectorState,
        delete_confirmation: DeleteConfirmationState,
    },
    PortForwardingList(ListSelectionState),
    PortForwardingFormNew(FormWithConnectionSelector<crate::ui::PortForwardingForm>),
    PortForwardingFormEdit(FormWithConnectionSelector<crate::ui::PortForwardingForm>),
    PortForwardDeleteConfirmation {
        port_forward_name: String,
        port_forward_id: String,
        current_selected: usize,
    },
}

pub fn create_search_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type to search connections (Name | Host | User)");
    textarea.set_cursor_line_style(ratatui::style::Style::default());
    textarea
}

/// App is the main application
pub struct App<B: Backend + Write> {
    pub mode: AppMode,
    pub error: Option<AppError>,
    pub info: Option<String>,
    pub config: ConfigManager,
    pub port_forwarding_runtime: crate::async_ssh_client::PortForwardingRuntime,
    terminal: Terminal<B>,
    needs_redraw: bool, // Track if UI needs redrawing
    event_tx: Option<tokio::sync::mpsc::Sender<AppEvent>>, // Event sender for SSH disconnect
    tick_control_tx: Option<tokio::sync::mpsc::Sender<crate::events::TickControl>>, // Tick control sender
    mouse_capture_enabled: bool,
    terminal_viewport: Rect,
    selection_anchor: Option<SelectionEndpoint>,
    selection_tail: Option<SelectionEndpoint>,
    selection_dragging: bool,
    selection_auto_scroll: Option<SelectionAutoScroll>,
    last_click: Option<LastMouseClick>,
    selection_force_nonempty: bool,
}

impl<B: Backend + Write> Drop for App<B> {
    fn drop(&mut self) {
        use crossterm::event::{DisableBracketedPaste, DisableMouseCapture};
        use crossterm::execute;
        use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

        disable_raw_mode().ok();
        #[cfg(target_os = "windows")]
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen,).ok();
        #[cfg(not(target_os = "windows"))]
        execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
        )
        .ok();
    }
}

impl<B: Backend + Write> App<B> {
    pub fn new(terminal: Terminal<B>) -> Result<Self> {
        Ok(Self {
            mode: AppMode::ConnectionList(ListSelectionState::new(0)),
            error: None,
            info: None,
            config: ConfigManager::new()?,
            port_forwarding_runtime: crate::async_ssh_client::PortForwardingRuntime::new(),
            terminal,
            needs_redraw: true,    // Initial redraw needed
            event_tx: None,        // Will be set later
            tick_control_tx: None, // Will be set later
            mouse_capture_enabled: false,
            terminal_viewport: Rect::default(),
            selection_anchor: None,
            selection_tail: None,
            selection_dragging: false,
            selection_auto_scroll: None,
            last_click: None,
            selection_force_nonempty: false,
        })
    }

    pub fn init_terminal(&mut self) -> Result<()> {
        use crossterm::ExecutableCommand;
        use crossterm::event::DisableMouseCapture;
        use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

        enable_raw_mode().inspect_err(|e| tracing::error!("Error enabling raw mode: {}", e))?;
        self.terminal
            .backend_mut()
            .execute(EnterAlternateScreen)
            .inspect_err(|e| {
                tracing::error!(
                    "Error executing EnterAlternateScreen terminal command: {}",
                    e
                )
            })?;

        #[cfg(not(target_os = "windows"))]
        self.terminal
            .backend_mut()
            .execute(crossterm::event::EnableBracketedPaste)
            .inspect_err(|e| {
                tracing::error!(
                    "Error executing EnableBracketedPaste terminal command: {}",
                    e
                )
            })?;

        #[cfg(not(target_os = "windows"))]
        self.terminal
            .backend_mut()
            .execute(DisableMouseCapture)
            .inspect_err(|e| {
                tracing::error!(
                    "Error executing DisableMouseCapture terminal command: {}",
                    e
                )
            })?;

        self.mouse_capture_enabled = false;
        self.terminal_viewport = Rect::default();
        self.selection_anchor = None;
        self.selection_tail = None;
        self.selection_dragging = false;
        self.selection_auto_scroll = None;
        self.last_click = None;
        self.selection_force_nonempty = false;

        Ok(())
    }

    pub fn set_event_sender(&mut self, sender: tokio::sync::mpsc::Sender<AppEvent>) {
        self.event_tx = Some(sender);
    }

    pub fn get_event_sender(&self) -> Option<tokio::sync::mpsc::Sender<AppEvent>> {
        self.event_tx.clone()
    }

    pub fn set_tick_control_sender(
        &mut self,
        sender: tokio::sync::mpsc::Sender<crate::events::TickControl>,
    ) {
        self.tick_control_tx = Some(sender);
    }

    pub fn start_ticker(&self) {
        if let Some(tx) = &self.tick_control_tx {
            let _ = tx.try_send(crate::events::TickControl::Start);
        }
    }

    pub fn stop_ticker(&self) {
        if let Some(tx) = &self.tick_control_tx {
            let _ = tx.try_send(crate::events::TickControl::Stop);
        }
    }

    pub fn send_event(&self, event: AppEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(event);
        }
    }

    fn mode_needs_ticker(&self) -> bool {
        matches!(
            self.mode,
            AppMode::ScpProgress { .. } | AppMode::Connecting { .. }
        )
    }

    /// Get the terminal size for SSH PTY (cols, rows), accounting for borders
    pub fn ssh_terminal_size(&self) -> Result<(u16, u16)> {
        let size = self.terminal.size()?;
        // Account for top border (1 row for title bar)
        let rows = size.height.saturating_sub(1);
        let cols = size.width;
        Ok((cols, rows))
    }

    fn set_mouse_capture(&mut self, enable: bool) -> Result<()> {
        #[cfg(target_os = "windows")]
        return Ok(());

        use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
        use crossterm::execute;

        if enable {
            if !self.mouse_capture_enabled {
                execute!(self.terminal.backend_mut(), EnableMouseCapture)?;
                self.mouse_capture_enabled = true;
            }
        } else if self.mouse_capture_enabled {
            execute!(self.terminal.backend_mut(), DisableMouseCapture)?;
            self.mouse_capture_enabled = false;
        }
        Ok(())
    }

    #[inline]
    fn update_mouse_capture_mode(&mut self) -> Result<()> {
        let should_enable = matches!(self.mode, AppMode::Connected { .. });
        self.set_mouse_capture(should_enable)?;
        Ok(())
    }

    pub fn clear_selection(&mut self) {
        if self.selection_anchor.is_some()
            || self.selection_tail.is_some()
            || self.selection_dragging
        {
            self.selection_anchor = None;
            self.selection_tail = None;
            self.selection_dragging = false;
            self.selection_auto_scroll = None;
            self.selection_force_nonempty = false;
            self.mark_redraw();
        }
    }

    pub fn is_selecting(&self) -> bool {
        self.selection_dragging
    }

    pub fn start_selection(&mut self, point: SelectionEndpoint) {
        self.selection_anchor = Some(point);
        self.selection_tail = Some(point);
        self.selection_dragging = true;
        self.selection_force_nonempty = false;
        self.mark_redraw();
    }

    pub fn update_selection(&mut self, point: SelectionEndpoint) {
        if self.selection_anchor.is_some() {
            self.selection_tail = Some(point);
            self.mark_redraw();
        }
    }

    pub fn finish_selection(&mut self) {
        if self.selection_anchor.is_some() && self.selection_tail.is_some() {
            self.selection_dragging = false;
            self.mark_redraw();
        }
    }

    pub fn selection_endpoints(&self) -> Option<(SelectionEndpoint, SelectionEndpoint)> {
        let anchor = self.selection_anchor?;
        let tail = self.selection_tail?;
        if anchor == tail && !self.selection_force_nonempty {
            None
        } else {
            Some((anchor, tail))
        }
    }

    pub fn selection_text(&self, state: &TerminalState) -> Option<String> {
        let (anchor, tail) = self.selection_endpoints()?;
        crate::terminal::selection::collect_selection_text(
            &state.terminal,
            state.scrollback(),
            anchor,
            tail,
        )
    }

    pub fn begin_selection_auto_scroll(
        &mut self,
        direction: SelectionScrollDirection,
        view_row: u16,
        view_col: u16,
    ) {
        self.selection_auto_scroll = Some(SelectionAutoScroll {
            direction,
            view_row,
            view_col,
        });
        self.start_ticker();
    }

    pub fn stop_selection_auto_scroll(&mut self) {
        if self.selection_auto_scroll.is_some() {
            self.selection_auto_scroll = None;
            // Only stop if no other features need ticking
            if !self.mode_needs_ticker() {
                self.stop_ticker();
            }
        }
    }

    pub fn register_left_click(&mut self, point: TerminalPoint) -> MouseClickClass {
        let now = Instant::now();
        let mut click_class = MouseClickClass::Single;
        let mut click_count = 1;

        if let Some(last) = self.last_click {
            let within_window =
                now.duration_since(last.time) <= TerminalPoint::DOUBLE_CLICK_MAX_INTERVAL;
            if within_window && last.point == point {
                let next_count = last.count.saturating_add(1);
                match next_count {
                    2 => {
                        click_class = MouseClickClass::Double;
                        click_count = 2;
                    }
                    3 => {
                        click_class = MouseClickClass::Triple;
                        click_count = 3;
                    }
                    _ => {
                        click_class = MouseClickClass::Single;
                        click_count = 1;
                    }
                }
            }
        }

        self.last_click = Some(LastMouseClick {
            point,
            time: now,
            count: click_count,
        });

        if matches!(click_class, MouseClickClass::Triple) {
            // Reset sequence after triple click so future clicks start fresh.
            self.last_click = Some(LastMouseClick {
                point,
                time: now,
                count: 0,
            });
        }

        click_class
    }

    pub fn clear_click_tracking(&mut self) {
        self.last_click = None;
    }

    pub fn force_selection_nonempty(&mut self) {
        self.selection_force_nonempty = true;
    }

    pub fn copy_text_to_clipboard(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        match Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(err) = clipboard.set_text(text.trim_end()) {
                    self.set_error(AppError::ClipboardError(err.to_string()));
                }
            }
            Err(err) => {
                self.set_error(AppError::ClipboardError(err.to_string()));
            }
        }
    }

    pub fn viewport_cell_at(&self, column: u16, row: u16) -> Option<TerminalPoint> {
        let viewport = self.terminal_viewport;
        if viewport.width == 0 || viewport.height == 0 {
            return None;
        }
        if column < viewport.x
            || row < viewport.y
            || column >= viewport.x + viewport.width
            || row >= viewport.y + viewport.height
        {
            return None;
        }
        Some(TerminalPoint {
            row: row - viewport.y,
            col: column - viewport.x,
        })
    }

    pub fn clamp_point_to_viewport(
        &self,
        column: u16,
        row: u16,
    ) -> Option<(TerminalPoint, Option<SelectionScrollDirection>)> {
        let viewport = self.terminal_viewport;
        if viewport.width == 0 || viewport.height == 0 {
            return None;
        }
        let mut clamped_col = column;
        if clamped_col < viewport.x {
            clamped_col = viewport.x;
        } else if clamped_col >= viewport.x + viewport.width {
            clamped_col = viewport.x + viewport.width - 1;
        }

        let top = viewport.y;
        let bottom = viewport.y + viewport.height - 1;
        let mut clamped_row = row;
        let mut direction = None;
        if clamped_row <= top {
            clamped_row = top;
            direction = Some(SelectionScrollDirection::Up);
        } else if clamped_row >= bottom {
            clamped_row = bottom;
            direction = Some(SelectionScrollDirection::Down);
        }

        self.viewport_cell_at(clamped_col, clamped_row)
            .map(|point| (point, direction))
    }

    pub fn go_to_connected(
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
        self.clear_selection();
        // Stop ticker - terminal updates are now event-driven via TerminalUpdate
        // Selection auto-scroll can re-enable ticker if needed
        if self.selection_auto_scroll.is_none() {
            self.stop_ticker();
        }
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub fn go_to_form_new(&mut self) {
        self.clear_selection();
        self.mode = AppMode::FormNew {
            auto_auth: false,
            form: ConnectionForm::new(),
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub fn go_to_form_edit(&mut self, form: ConnectionForm, original: Connection) {
        self.clear_selection();
        self.mode = AppMode::FormEdit {
            form,
            original,
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub fn go_to_connection_list_with_selected(&mut self, selected: usize) {
        self.clear_selection();
        self.mode = AppMode::ConnectionList(ListSelectionState::new(selected));
        // Stop ticker when returning to connection list (idle state)
        self.stop_ticker();
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub fn go_to_scp_progress(&mut self, progress: ScpProgress, return_mode: ScpReturnMode) {
        self.mode = AppMode::ScpProgress {
            progress,
            return_mode: Some(return_mode),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub fn go_to_delete_confirmation(
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

    pub async fn go_to_port_forwarding_list(&mut self) {
        self.go_to_port_forwarding_list_with_selected(0).await;
    }

    pub async fn go_to_port_forwarding_list_with_selected(&mut self, selected: usize) {
        // Sync port forwarding status before showing the list
        crate::key_event::port_forwarding::sync_port_forwarding_status(self).await;

        self.mode = AppMode::PortForwardingList(ListSelectionState::new(selected));
        self.needs_redraw = true;
    }

    pub fn go_to_port_forwarding_form_new(&mut self) {
        self.mode = AppMode::PortForwardingFormNew(FormWithConnectionSelector::new(
            crate::ui::PortForwardingForm::new(),
            self.current_selected(),
        ));
        self.needs_redraw = true;
    }

    pub fn go_to_port_forwarding_form_edit(&mut self, form: crate::ui::PortForwardingForm) {
        self.mode = AppMode::PortForwardingFormEdit(FormWithConnectionSelector::new(
            form,
            self.current_selected(),
        ));
        self.needs_redraw = true;
    }

    pub fn go_to_port_forward_delete_confirmation(
        &mut self,
        port_forward_name: String,
        port_forward_id: String,
        current_selected: usize,
    ) {
        self.mode = AppMode::PortForwardDeleteConfirmation {
            port_forward_name,
            port_forward_id,
            current_selected,
        };
        self.needs_redraw = true;
    }

    pub fn go_to_connecting(
        &mut self,
        connection: Connection,
        connection_name: String,
        return_to: usize,
        return_from: ConnectingSource,
        cancel_token: tokio_util::sync::CancellationToken,
        receiver: mpsc::Receiver<Result<SshSession>>,
    ) {
        self.mode = AppMode::Connecting {
            connection,
            connection_name,
            return_to,
            return_from,
            cancel_token,
            receiver,
        };
        self.start_ticker();
        self.needs_redraw = true;
    }

    pub async fn go_to_file_explorer(&mut self, conn: Connection, return_to: usize) -> Result<()> {
        // For SFTP, we need to create a new session directly since we need both the session and channel
        // We'll use the existing sftp_send_file pattern but adapt it for our needs
        let (sftp_session, channel, ssh_session) = Self::create_sftp_session(&conn).await?;
        let sftp_session = Arc::new(sftp_session);

        // Initialize local file explorer
        // Use current directory as it's more reliable than HOME which might be on a slow network mount
        let local_start_dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .ok()
            .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_else(|| "/tmp".to_string());

        let local_explorer = ratatui_explorer::FileExplorer::with_fs(
            Arc::new(ratatui_explorer::LocalFileSystem),
            local_start_dir.clone(),
        )
        .await
        .map_err(|e| {
            AppError::SftpError(format!(
                "Failed to initialize local explorer from '{local_start_dir}': {e}"
            ))
        })?;

        // Initialize remote file explorer (start from home directory)
        // Canonicalize the remote home path to get the absolute path
        let remote_home_canonical = sftp_session.canonicalize(".").await.map_err(|e| {
            AppError::SftpError(format!("Failed to resolve remote home directory: {e}"))
        })?;

        let sftp_fs = crate::filesystem::SftpFileSystem::new(sftp_session.clone());
        let remote_explorer = ratatui_explorer::FileExplorer::with_fs(
            Arc::new(sftp_fs),
            remote_home_canonical.clone(),
        )
        .await
        .map_err(|e| {
            AppError::SftpError(format!(
                "Failed to initialize remote explorer from '{remote_home_canonical}': {e}"
            ))
        })?;

        // Transition to FileExplorer mode
        self.mode = AppMode::FileExplorer {
            connection_name: conn.display_name.clone(),

            // Left pane starts as Local
            left_pane: FileExplorerPane::Local,
            left_explorer: LeftExplorer::Local(local_explorer),
            left_sftp: None,
            left_session: None,

            // Right pane is the original SSH connection
            remote_explorer,
            sftp_session,
            ssh_connection: conn,
            channel: Some(channel),
            ssh_session,

            active_pane: ActivePane::Left,
            copy_buffer: Vec::new(),
            return_to,
            search: SearchState::Off,

            source_selector: SourceSelectorState::new(),
            delete_confirmation: DeleteConfirmationState::new(),
        };
        self.needs_redraw = true;
        self.stop_ticker();
        Ok(())
    }

    /// Switch left pane to local filesystem
    pub async fn switch_left_pane_to_local(&mut self) {
        if let AppMode::FileExplorer {
            left_pane,
            left_explorer,
            left_sftp,
            ..
        } = &mut self.mode
        {
            // Check if already local
            if matches!(left_pane, FileExplorerPane::Local) {
                return;
            }

            // Create local explorer
            let local_start_dir = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .ok()
                .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "/tmp".to_string());

            match ratatui_explorer::FileExplorer::with_fs(
                Arc::new(ratatui_explorer::LocalFileSystem),
                local_start_dir.clone(),
            )
            .await
            {
                Ok(local_explorer) => {
                    *left_pane = FileExplorerPane::Local;
                    *left_explorer = LeftExplorer::Local(local_explorer);
                    *left_sftp = None; // Drop old SFTP session
                    self.needs_redraw = true;
                }
                Err(e) => {
                    self.error = Some(AppError::SftpError(format!(
                        "Failed to initialize local explorer: {e}"
                    )));
                }
            }
        }
    }

    /// Switch left pane to an SSH connection
    pub async fn switch_left_pane_to_ssh(&mut self, conn: Connection) {
        if let AppMode::FileExplorer {
            left_pane,
            left_explorer,
            left_sftp,
            left_session,
            ssh_connection: right_conn,
            ..
        } = &mut self.mode
        {
            // Validate: left and right cannot be the same connection
            if conn.id == right_conn.id {
                self.error = Some(AppError::SftpError(
                    "Left and right panes cannot use the same SSH connection".to_string(),
                ));
                return;
            }

            // Check if already using this connection
            if let FileExplorerPane::RemoteSsh { connection, .. } = left_pane
                && connection.id == conn.id
            {
                return;
            }

            // Switch left pane to SSH connection
            match Self::setup_left_ssh_pane(
                &conn,
                left_pane,
                left_explorer,
                left_sftp,
                left_session,
            )
            .await
            {
                Ok(()) => {
                    self.needs_redraw = true;
                }
                Err(e) => {
                    self.error = Some(e);
                }
            }
        }
    }

    /// Setup left pane to connect to an SSH server
    async fn setup_left_ssh_pane(
        conn: &Connection,
        left_pane: &mut FileExplorerPane,
        left_explorer: &mut LeftExplorer,
        left_sftp: &mut Option<(Arc<russh_sftp::client::SftpSession>, Connection)>,
        left_session: &mut Option<
            Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,
        >,
    ) -> Result<()> {
        // Create SFTP session
        let (sftp_session, _explorer_channel, ssh_session) = Self::create_sftp_session(conn)
            .await
            .map_err(|e| AppError::SftpError(format!("Failed to create SFTP session: {e}")))?;
        let sftp_session = Arc::new(sftp_session);

        // Get home directory
        let remote_home = sftp_session.canonicalize(".").await.map_err(|e| {
            AppError::SftpError(format!("Failed to get remote home directory: {e}"))
        })?;

        // Create file explorer for the remote filesystem
        let sftp_fs = crate::filesystem::SftpFileSystem::new(sftp_session.clone());
        let remote_explorer =
            ratatui_explorer::FileExplorer::with_fs(Arc::new(sftp_fs), remote_home.clone())
                .await
                .map_err(|e| {
                    AppError::SftpError(format!("Failed to initialize remote explorer: {e}"))
                })?;

        // Update state
        *left_pane = FileExplorerPane::RemoteSsh {
            connection_name: conn.display_name.clone(),
            connection: conn.clone(),
        };
        *left_explorer = LeftExplorer::Remote(remote_explorer);
        *left_sftp = Some((sftp_session, conn.clone()));
        *left_session = Some(ssh_session);

        Ok(())
    }

    async fn create_sftp_session(
        conn: &Connection,
    ) -> Result<(
        russh_sftp::client::SftpSession,
        russh::Channel<russh::client::Msg>,
        Arc<tokio::sync::Mutex<russh::client::Handle<crate::async_ssh_client::SshClient>>>,
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
            .map_err(|e| AppError::SftpError(format!("SFTP session creation failed: {e}")))?;

        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;

        // Wrap session in Arc<Mutex> for shared access
        let session = Arc::new(tokio::sync::Mutex::new(session));

        Ok((sftp, channel, session))
    }

    pub fn current_selected(&self) -> usize {
        match &self.mode {
            AppMode::ConnectionList(state) => {
                let len = self.config.connections().len();
                if len == 0 {
                    0
                } else {
                    state.selected.min(len - 1)
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
            AppMode::Connecting { return_to, .. } => *return_to,
            AppMode::ScpProgress { return_mode, .. } => match return_mode {
                Some(ScpReturnMode::ConnectionList { current_selected }) => *current_selected,
                Some(ScpReturnMode::Connected {
                    current_selected, ..
                }) => *current_selected,
                Some(ScpReturnMode::FileExplorer { return_to, .. }) => *return_to,
                None => 0,
            },
            AppMode::FileExplorer { return_to, .. } => *return_to,
            AppMode::FormNew { .. } => 0,
            AppMode::PortForwardingList(state) => {
                let len = self.config.port_forwards().len();
                if len == 0 {
                    0
                } else {
                    state.selected.min(len - 1)
                }
            }
            AppMode::PortForwardingFormNew(state) | AppMode::PortForwardingFormEdit(state) => {
                state.current_selected
            }
            AppMode::PortForwardDeleteConfirmation {
                current_selected, ..
            } => *current_selected,
        }
    }

    /// Mark that UI needs redrawing
    pub fn mark_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Check if redraw is needed and mark as drawn
    pub fn should_redraw(&mut self) -> bool {
        let should = self.needs_redraw;
        self.needs_redraw = false;
        should
    }

    /// Set error and mark for redraw
    pub fn set_error(&mut self, error: AppError) {
        self.error = Some(error);
        self.needs_redraw = true;
    }

    /// Set info and mark for redraw
    #[allow(dead_code)]
    pub fn set_info(&mut self, info: String) {
        self.info = Some(info);
        self.needs_redraw = true;
    }

    fn draw(&mut self) -> Result<()> {
        let selection_anchor = self.selection_anchor;
        let selection_tail = self.selection_tail;
        let selection_forced = self.selection_force_nonempty;
        let mut new_viewport = Rect::default();

        self.terminal.draw(|f| {
            let size = f.area();
            match &mut self.mode {
                AppMode::ConnectionList(state) => {
                    let conns = self.config.connections();

                    draw_connection_list(size, conns, state.selected, &state.search, f, false);
                }
                AppMode::FormNew {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(
                        size,
                        conns,
                        *current_selected,
                        &SearchState::Off,
                        f,
                        false,
                    );
                }
                AppMode::FormEdit {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(
                        size,
                        conns,
                        *current_selected,
                        &SearchState::Off,
                        f,
                        false,
                    );
                }
                AppMode::Connecting {
                    return_from,
                    return_to,
                    ..
                } => {
                    // Render appropriate background based on return_from
                    match return_from {
                        ConnectingSource::FormNew { form, .. } => {
                            let conns = self.config.connections();
                            draw_connection_list(
                                size,
                                conns,
                                *return_to,
                                &SearchState::Off,
                                f,
                                false,
                            );
                            draw_connection_form_popup(size, form, true, f);
                        }
                        ConnectingSource::FormEdit { form, .. } => {
                            let conns = self.config.connections();
                            draw_connection_list(
                                size,
                                conns,
                                *return_to,
                                &SearchState::Off,
                                f,
                                false,
                            );
                            draw_connection_form_popup(size, form, false, f);
                        }
                        ConnectingSource::ConnectionList { .. } => {
                            let conns = self.config.connections();
                            draw_connection_list(
                                size,
                                conns,
                                *return_to,
                                &SearchState::Off,
                                f,
                                false,
                            );
                        }
                    }
                }
                AppMode::Connected { name, state, .. } => {
                    let inner = rect_with_top_margin(size, 1);
                    new_viewport = inner;
                    if let Ok(mut guard) = state.try_lock() {
                        if guard.screen_size() != (inner.height, inner.width) {
                            guard.resize(inner.height, inner.width);
                        }
                        let selection = compute_selection_for_view(
                            selection_anchor,
                            selection_tail,
                            &guard,
                            inner.width,
                            selection_forced,
                        );
                        draw_terminal(size, &mut guard, name, f, selection);
                    }
                }
                AppMode::ScpProgress { return_mode, .. } => {
                    // Render appropriate background based on return mode
                    match return_mode {
                        Some(ScpReturnMode::ConnectionList { current_selected }) => {
                            let conns = self.config.connections();
                            draw_connection_list(
                                size,
                                conns,
                                *current_selected,
                                &SearchState::Off,
                                f,
                                false,
                            );
                        }
                        Some(ScpReturnMode::Connected { name, state, .. }) => {
                            let inner = rect_with_top_margin(size, 1);
                            new_viewport = inner;
                            if let Ok(mut guard) = state.try_lock() {
                                if guard.screen_size() != (inner.height, inner.width) {
                                    guard.resize(inner.height, inner.width);
                                }
                                let selection = compute_selection_for_view(
                                    selection_anchor,
                                    selection_tail,
                                    &guard,
                                    inner.width,
                                    selection_forced,
                                );
                                draw_terminal(size, &mut guard, name, f, selection);
                            }
                        }
                        Some(ScpReturnMode::FileExplorer {
                            connection_name,
                            left_pane,
                            left_explorer,
                            remote_explorer,
                            active_pane,
                            copy_buffer,
                            search,
                            ..
                        }) => {
                            draw_file_explorer(
                                f,
                                size,
                                connection_name,
                                left_pane,
                                left_explorer,
                                remote_explorer,
                                active_pane,
                                copy_buffer,
                                search,
                                self.config.have_nerd_font(),
                            );
                        }
                        None => {}
                    }
                }
                AppMode::DeleteConfirmation {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(
                        size,
                        conns,
                        *current_selected,
                        &SearchState::Off,
                        f,
                        false,
                    );
                }
                AppMode::FileExplorer {
                    connection_name,
                    left_pane,
                    left_explorer,
                    remote_explorer,
                    active_pane,
                    copy_buffer,
                    search,
                    source_selector,
                    ssh_connection,
                    delete_confirmation,
                    ..
                } => {
                    draw_file_explorer(
                        f,
                        size,
                        connection_name,
                        left_pane,
                        left_explorer,
                        remote_explorer,
                        active_pane,
                        copy_buffer,
                        search,
                        self.config.have_nerd_font(),
                    );

                    // Draw source selector popup if active
                    if source_selector.showing {
                        let connections = self.config.connections();
                        crate::ui::draw_connection_selector_popup(
                            f,
                            size,
                            connections,
                            source_selector.selected,
                            Some(ssh_connection.id.as_str()),
                            true,
                            " Select Left Pane Source ",
                            &source_selector.search,
                        );
                    }

                    // Draw delete confirmation popup if active
                    if delete_confirmation.showing {
                        draw_delete_confirmation_popup(
                            f,
                            size,
                            &crate::ui::DeleteConfirmationConfig::FILE,
                            &delete_confirmation.file_name,
                        );
                    }
                }
                AppMode::PortForwardingList(state) => {
                    let connections = self.config.connections();
                    let port_forwards = self.config.port_forwards();

                    draw_port_forwarding_list(
                        size,
                        port_forwards,
                        connections,
                        state.selected,
                        &state.search,
                        f,
                    );
                }
                AppMode::PortForwardingFormNew(state) | AppMode::PortForwardingFormEdit(state) => {
                    // Render the port forwarding list background
                    let port_forwards = self.config.port_forwards();
                    let connections = self.config.connections();
                    draw_port_forwarding_list(
                        size,
                        port_forwards,
                        connections,
                        state.current_selected,
                        &SearchState::Off,
                        f,
                    );
                }
                AppMode::PortForwardDeleteConfirmation {
                    current_selected, ..
                } => {
                    // Render the port forwarding list background first
                    let port_forwards = self.config.port_forwards();
                    let connections = self.config.connections();
                    draw_port_forwarding_list(
                        size,
                        port_forwards,
                        connections,
                        *current_selected,
                        &SearchState::Off,
                        f,
                    );
                }
            }

            // Overlay port forwarding form popup if in port forwarding form mode
            if let AppMode::PortForwardingFormNew(state) = &mut self.mode {
                let connections = self.config.connections();
                draw_port_forwarding_form_popup(size, &mut state.form, connections, true, f);
            }
            if let AppMode::PortForwardingFormEdit(state) = &mut self.mode {
                let connections = self.config.connections();
                draw_port_forwarding_form_popup(size, &mut state.form, connections, false, f);
            }

            // Overlay port forwarding connection selector popup when active
            if let AppMode::PortForwardingFormNew(state) = &mut self.mode
                && state.connection_selector.showing
            {
                let connections = self.config.connections();
                crate::ui::draw_connection_selector_popup(
                    f,
                    size,
                    connections,
                    state.connection_selector.selected,
                    None,
                    false,
                    " Choose Connection ",
                    &state.connection_selector.search,
                );
            }
            if let AppMode::PortForwardingFormEdit(state) = &mut self.mode
                && state.connection_selector.showing
            {
                let connections = self.config.connections();
                crate::ui::draw_connection_selector_popup(
                    f,
                    size,
                    connections,
                    state.connection_selector.selected,
                    None,
                    false,
                    " Choose Connection ",
                    &state.connection_selector.search,
                );
            }

            // Overlay SCP progress popup if in SCP progress mode
            if let AppMode::ScpProgress { progress, .. } = &mut self.mode {
                draw_scp_progress_popup(size, progress, f);
            }

            // Overlay delete confirmation popup if in delete confirmation mode
            if let AppMode::DeleteConfirmation {
                connection_name, ..
            } = &self.mode
            {
                draw_delete_confirmation_popup(
                    f,
                    size,
                    &DeleteConfirmationConfig::CONNECTION,
                    connection_name,
                );
            }

            // Overlay port forward delete confirmation popup if in port forward delete confirmation mode
            if let AppMode::PortForwardDeleteConfirmation {
                port_forward_name, ..
            } = &self.mode
            {
                draw_delete_confirmation_popup(
                    f,
                    size,
                    &DeleteConfirmationConfig::PORT_FORWARD,
                    port_forward_name,
                );
            }

            // Overlay connection form popup if in form mode
            if let AppMode::FormNew { form, .. } = &self.mode {
                draw_connection_form_popup(size, form, true, f);
            }
            if let AppMode::FormEdit { form, .. } = &mut self.mode {
                draw_connection_form_popup(size, form, false, f);
            }

            // Overlay connecting popup if in connecting mode
            if let AppMode::Connecting {
                connection_name, ..
            } = &self.mode
            {
                let message = format!("Connecting to {connection_name}...");
                draw_connecting_popup(size, &message, f);
            }

            // Overlay info popup if any
            if let Some(msg) = &self.info {
                draw_info_popup(size, msg, f);
            }

            // Overlay error popup if any (always on top)
            if let Some(err) = &self.error {
                draw_error_popup(size, &err.to_string(), f);
            }
        })?;

        self.terminal_viewport = new_viewport;

        Ok(())
    }

    pub async fn run(&mut self, rx: &mut mpsc::Receiver<AppEvent>) -> Result<()> {
        let mut pending_event: Option<AppEvent> = None;
        loop {
            self.update_mouse_capture_mode()?;

            // Check terminal size changes and update SSH session if needed
            let mut terminal_size_changed = false;

            if let AppMode::Connected { client, state, .. } = &self.mode {
                let size = self.terminal.size()?;
                // Calculate inner area for terminal content (accounting for borders)
                let h = size.height.saturating_sub(1); // Top borders
                let w = size.width;
                let mut guard = state.lock().await;
                if guard.screen_size() != (h, w) {
                    // Resize both local terminal state and remote PTY
                    guard.resize(h, w);
                    client.request_size(w, h).await;
                    terminal_size_changed = true;
                }
            }

            // Only render when needed
            // Terminal content updates now trigger via TerminalUpdate event (event-driven)
            if self.should_redraw() || terminal_size_changed {
                self.draw()?;
            }

            // wait for an event (asynchronous)
            let ev = if let Some(pending) = pending_event.take() {
                pending
            } else {
                match rx.recv().await {
                    Some(e) => e,
                    None => {
                        tracing::warn!("App event channel closed");
                        break; // exit if channel is closed
                    }
                }
            };

            match ev {
                AppEvent::Tick => {
                    // Safety net: should only receive ticks when features are active
                    let needs_tick = self.selection_auto_scroll.is_some()
                        || matches!(
                            self.mode,
                            AppMode::ScpProgress { .. } | AppMode::Connecting { .. }
                        );

                    if !needs_tick {
                        // Skip tick processing - no active features need it
                        tracing::warn!("Received tick when no feature needs it");
                        continue;
                    }

                    if self.selection_dragging
                        && let Some(auto) = self.selection_auto_scroll
                    {
                        let state_arc = if let AppMode::Connected { state, .. } = &self.mode {
                            Some(state.clone())
                        } else {
                            None
                        };
                        if let Some(state_arc) = state_arc {
                            let mut guard = state_arc.lock().await;
                            let delta = match auto.direction {
                                SelectionScrollDirection::Up => 1,
                                SelectionScrollDirection::Down => -1,
                            };
                            guard.scroll_by(delta);
                            let (height, width) = guard.screen_size();
                            let endpoint = if height > 0 && width > 0 {
                                let target_row = auto.view_row.min(height.saturating_sub(1));
                                let target_col = auto.view_col.min(width.saturating_sub(1));
                                make_selection_endpoint(&guard, target_row, target_col)
                            } else {
                                None
                            };
                            self.mark_redraw();
                            drop(guard);
                            if let Some(endpoint) = endpoint {
                                self.update_selection(endpoint);
                            }
                        }
                    }

                    // Handle connection result polling in Connecting mode
                    if let AppMode::Connecting {
                        connection,
                        return_from,
                        return_to,
                        receiver,
                        ..
                    } = &mut self.mode
                    {
                        match receiver.try_recv() {
                            Ok(result) => {
                                match result {
                                    Ok(mut client) => {
                                        // Connection successful - extract data and transition to Connected mode
                                        let conn = connection.clone();
                                        let return_to = *return_to;

                                        // Save the server key if it was received
                                        if conn.public_key.is_none()
                                            && let Some(server_key) = client.get_server_key()
                                            && let Some(stored_conn) = self
                                                .config
                                                .connections_mut()
                                                .iter_mut()
                                                .find(|c| c.id == conn.id)
                                        {
                                            stored_conn.public_key = Some(server_key.to_string());
                                            let _ = self.config.save();
                                        }

                                        if let ConnectingSource::ConnectionList { file_explorer } =
                                            return_from
                                        {
                                            if *file_explorer {
                                                let conn = connection.clone();
                                                self.go_to_file_explorer(conn, return_to).await?;
                                                continue;
                                            }
                                        }

                                        // Handle based on source
                                        if let ConnectingSource::FormNew { .. } = return_from {
                                            // Save the connection (only for new connections)
                                            let mut conn_to_save = conn.clone();
                                            if let Some(server_key) = client.get_server_key() {
                                                conn_to_save.public_key =
                                                    Some(server_key.to_string());
                                            }
                                            if let Err(e) = self.config.add_connection(conn_to_save)
                                            {
                                                self.set_error(e);
                                                self.go_to_form_new();
                                                continue;
                                            }
                                        }

                                        let scrollback = self.config.terminal_scrollback_lines();
                                        let (cols, rows) =
                                            self.ssh_terminal_size().unwrap_or((80, 24));
                                        tracing::info!(
                                            "Creating TerminalState with {} rows x {} cols",
                                            rows,
                                            cols
                                        );
                                        let state = Arc::new(Mutex::new(
                                            TerminalState::new_with_scrollback(
                                                rows, cols, scrollback,
                                            ),
                                        ));
                                        let app_reader = state.clone();
                                        let reader =
                                            client.take_reader().expect("reader already taken");
                                        let cancel_token =
                                            tokio_util::sync::CancellationToken::new();
                                        let cancel_for_task = cancel_token.clone();
                                        let event_tx = self.event_tx.clone();
                                        tokio::spawn(async move {
                                            SshSession::read_loop(
                                                reader,
                                                app_reader,
                                                cancel_for_task,
                                                event_tx,
                                            )
                                            .await;
                                        });

                                        let _ = self.config.touch_last_used(&conn.id);
                                        self.go_to_connected(
                                            conn.display_name.clone(),
                                            client,
                                            state,
                                            return_to,
                                            cancel_token,
                                        );
                                    }
                                    Err(e) => {
                                        // Connection failed - clone data before setting error
                                        let return_from = return_from.clone();
                                        let return_to = *return_to;

                                        // Now show error and return to previous mode
                                        self.set_error(e);
                                        match return_from {
                                            ConnectingSource::FormNew { auto_auth, form } => {
                                                self.mode = AppMode::FormNew {
                                                    auto_auth,
                                                    form,
                                                    current_selected: return_to,
                                                };
                                            }
                                            ConnectingSource::FormEdit { form, original } => {
                                                self.mode = AppMode::FormEdit {
                                                    form,
                                                    original,
                                                    current_selected: return_to,
                                                };
                                            }
                                            ConnectingSource::ConnectionList { .. } => {
                                                self.go_to_connection_list_with_selected(return_to);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                                // Still waiting for connection result
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                // Connection task was cancelled or dropped
                                let return_from = return_from.clone();
                                let return_to = *return_to;
                                match return_from {
                                    ConnectingSource::FormNew { auto_auth, form } => {
                                        self.mode = AppMode::FormNew {
                                            auto_auth,
                                            form,
                                            current_selected: return_to,
                                        };
                                    }
                                    ConnectingSource::FormEdit { form, original } => {
                                        self.mode = AppMode::FormEdit {
                                            form,
                                            original,
                                            current_selected: return_to,
                                        };
                                    }
                                    ConnectingSource::ConnectionList { .. } => {
                                        self.go_to_connection_list_with_selected(return_to);
                                    }
                                }
                            }
                        }
                    }
                }
                AppEvent::Input(ev) => {
                    use crossterm::event::Event;

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
                        Event::Mouse(mouse) => {
                            crate::key_event::handle_mouse_event(self, mouse).await;
                        }
                        Event::Paste(data) => {
                            crate::key_event::handle_paste_event(self, &data).await;
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                AppEvent::TerminalUpdate => {
                    // Terminal has received data from SSH - mark redraw to update display
                    // Coalesce rapid terminal updates to prevent rendering lag
                    // during high SSH data throughput. Drain all pending
                    // TerminalUpdate events from the channel, then draw once.
                    loop {
                        match rx.try_recv() {
                            Ok(AppEvent::TerminalUpdate) => {} // skip redundant
                            Ok(other) => {
                                pending_event = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    self.mark_redraw();
                }
                AppEvent::SftpProgress(result) => {
                    if let AppMode::ScpProgress { progress, .. } = &mut self.mode {
                        match result {
                            ScpResult::Progress(update) => {
                                progress.update_progress(update);
                                self.mark_redraw();
                            }
                            ScpResult::Completed(file_results) => {
                                let mut pending_error: Option<AppError> = None;
                                let mut all_success = true;
                                let mut failure_lines = Vec::new();
                                let mut last_success_destination = None;

                                for (idx, file_result) in file_results.iter().enumerate() {
                                    progress.mark_completed(
                                        idx,
                                        file_result.success,
                                        file_result.error.clone(),
                                    );

                                    let (from, to) = match file_result.mode {
                                        crate::ui::ScpMode::Send => (
                                            file_result.local_path.clone(),
                                            file_result.remote_path.clone(),
                                        ),
                                        crate::ui::ScpMode::Receive => (
                                            file_result.remote_path.clone(),
                                            file_result.local_path.clone(),
                                        ),
                                    };

                                    if file_result.success {
                                        last_success_destination =
                                            Some(file_result.destination_filename.clone());
                                    } else {
                                        all_success = false;
                                        let err = file_result
                                            .error
                                            .clone()
                                            .unwrap_or_else(|| "unknown error".into());
                                        failure_lines
                                            .push(format!(" from {from} to {to} (FAILED: {err})"));
                                    }
                                }

                                if !all_success {
                                    let mut message = String::from("sftp transfer issues:");
                                    for line in failure_lines {
                                        message.push('\n');
                                        message.push_str(&line);
                                    }
                                    pending_error = Some(AppError::SshConnectionError(message));
                                }

                                progress.completed = true;
                                progress.last_success_destination = last_success_destination;
                                progress.completion_results = Some(file_results);
                                self.mark_redraw();

                                if let Some(err) = pending_error {
                                    self.set_error(err);
                                }
                            }
                            ScpResult::Error { error } => {
                                for idx in 0..progress.files.len() {
                                    progress.mark_completed(idx, false, Some(error.clone()));
                                }
                                let pending_error = AppError::SshConnectionError(error.clone());
                                progress.completed = true;
                                progress.completion_results = None;
                                self.mark_redraw();
                                self.set_error(pending_error);
                            }
                        }
                    }
                }
                AppEvent::Disconnect => {
                    // SSH connection has been disconnected (e.g., user typed 'exit')
                    // Automatically return to the connection list
                    tracing::info!("SSH connection disconnected");
                    if let AppMode::Connected {
                        current_selected,
                        cancel_token,
                        name,
                        client,
                        ..
                    } = &self.mode
                    {
                        tracing::debug!("Closing connection to '{}'", name);
                        let current_selected = *current_selected;
                        // Cancel the read task
                        cancel_token.cancel();
                        // Close the SSH connection
                        if let Err(e) = client.close().await {
                            tracing::error!("Error closing SSH connection: {}", e);
                        }
                        // Go back to connection list
                        self.go_to_connection_list_with_selected(current_selected);
                        self.stop_ticker();
                        self.mark_redraw();
                    }
                }
            }
        }
        Ok(())
    }
}

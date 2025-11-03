mod async_ssh_client;
mod config;
mod error;
mod filesystem;
mod key_event;
mod ui;

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use arboard::Clipboard;
use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::prelude::Backend;
use tokio::{select, sync::mpsc, time};
use tui_textarea::TextArea;

use async_ssh_client::SshSession;
pub(crate) use async_ssh_client::expand_tilde;
use config::manager::{ConfigManager, Connection};
use error::{AppError, Result};
use ui::{
    ConnectionForm, TerminalSelection, TerminalState, draw_connection_form_popup,
    draw_connection_list, draw_delete_confirmation_popup, draw_error_popup, draw_file_explorer,
    draw_info_popup, draw_port_forward_delete_confirmation_popup, draw_port_forwarding_form_popup,
    draw_port_forwarding_list, draw_scp_progress_popup, draw_search_overlay, draw_terminal,
    rect_with_top_margin,
};

impl crate::async_ssh_client::ByteProcessor for TerminalState {
    fn process_bytes(&mut self, bytes: &[u8]) {
        TerminalState::process_bytes(self, bytes);
    }
}

/// Result summary of SFTP transfer operations
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum ScpResult {
    Completed(Vec<ScpFileResult>),
    Error { error: String },
}

/// Outcome for a single file within a batch transfer
#[derive(Debug, Clone)]
pub(crate) struct ScpFileResult {
    pub(crate) mode: crate::ui::ScpMode,
    pub(crate) local_path: String,
    pub(crate) remote_path: String,
    pub(crate) destination_filename: String,
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
    pub(crate) completed_at: Option<std::time::Instant>,
}

/// Byte-level progress updates for SFTP transfers (per file)
#[derive(Debug, Clone)]
pub(crate) struct ScpTransferProgress {
    pub(crate) file_index: usize,
    pub(crate) transferred_bytes: u64,
    pub(crate) total_bytes: Option<u64>,
}

#[derive(Debug)]
enum AppEvent {
    Input(Event),
    Tick,
    Disconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalPoint {
    row: u16,
    col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionEndpoint {
    rev_row: i64,
    col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionAutoScroll {
    direction: SelectionScrollDirection,
    view_row: u16,
    view_col: u16,
}

#[derive(Clone, Copy, Debug)]
struct LastMouseClick {
    point: TerminalPoint,
    time: Instant,
    count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseClickClass {
    Single,
    Double,
    Triple,
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

/// Specification for a single file transfer within a batch
#[derive(Clone, Debug)]
pub(crate) struct ScpTransferSpec {
    pub(crate) mode: crate::ui::ScpMode,
    pub(crate) local_path: String,
    pub(crate) remote_path: String,
    pub(crate) display_name: String,
    pub(crate) destination_filename: String,
}

/// Direction of file transfer
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyDirection {
    LocalToRemote,
    RemoteToLocal,
}

pub(crate) enum ScpReturnMode {
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
        local_explorer: ratatui_explorer::FileExplorer<ratatui_explorer::LocalFileSystem>,
        remote_explorer: ratatui_explorer::FileExplorer<crate::filesystem::SftpFileSystem>,
        active_pane: FileExplorerPane,
        copy_buffer: Vec<CopyOperation>,
        return_to: usize,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
        search_mode: bool,
        search_query: String,
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
                copy_buffer,
                return_to,
                sftp_session,
                ssh_connection,
                search_mode,
                search_query,
                ..
            } => ScpReturnMode::FileExplorer {
                connection_name: connection_name.clone(),
                local_explorer: local_explorer.clone(),
                remote_explorer: remote_explorer.clone(),
                active_pane: active_pane.clone(),
                copy_buffer: copy_buffer.clone(),
                return_to: *return_to,
                sftp_session: sftp_session.clone(),
                ssh_connection: ssh_connection.clone(),
                channel: None,
                search_mode: *search_mode,
                search_query: search_query.clone(),
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
        auto_auth: bool,
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
    ScpProgress {
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        progress_updates: mpsc::Receiver<ScpTransferProgress>,
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
        copy_buffer: Vec<CopyOperation>,
        return_to: usize,
        sftp_session: Arc<russh_sftp::client::SftpSession>,
        ssh_connection: Connection,
        channel: Option<russh::Channel<russh::client::Msg>>,
        search_mode: bool,
        search_query: String,
    },
    PortForwardingList {
        selected: usize,
        search_mode: bool,
        search_input: TextArea<'static>,
    },
    PortForwardingFormNew {
        form: crate::ui::PortForwardingForm,
        current_selected: usize, // Port forwarding list position
        select_connection_mode: bool,
        connection_selected: usize, // Connection list position
        connection_search_mode: bool,
        connection_search_input: TextArea<'static>,
    },
    PortForwardingFormEdit {
        form: crate::ui::PortForwardingForm,
        current_selected: usize, // Port forwarding list position
        select_connection_mode: bool,
        connection_selected: usize, // Connection list position
        connection_search_mode: bool,
        connection_search_input: TextArea<'static>,
    },
    PortForwardDeleteConfirmation {
        port_forward_name: String,
        port_forward_id: String,
        current_selected: usize,
    },
}

/// Per-file progress snapshot
#[derive(Clone, Debug)]
pub(crate) struct ScpFileProgress {
    pub(crate) local_path: String,
    pub(crate) remote_path: String,
    pub(crate) display_name: String,
    pub(crate) mode: crate::ui::ScpMode, // Send or Receive mode
    pub(crate) transferred_bytes: u64,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) state: TransferState,
}

impl ScpFileProgress {
    pub(crate) fn from_spec(spec: &ScpTransferSpec) -> Self {
        Self {
            local_path: spec.local_path.clone(),
            remote_path: spec.remote_path.clone(),
            display_name: spec.display_name.clone(),
            mode: spec.mode.clone(),
            transferred_bytes: 0,
            total_bytes: None,
            state: TransferState::Pending,
        }
    }

    pub(crate) fn ratio(&self) -> f64 {
        match self.total_bytes {
            Some(total) if total > 0 => {
                (self.transferred_bytes as f64 / total as f64).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

/// Aggregate transfer progress for a batch of files
#[derive(Clone, Debug)]
pub(crate) struct ScpProgress {
    pub(crate) connection_name: String,
    pub(crate) start_time: std::time::Instant,
    pub(crate) files: Vec<ScpFileProgress>,
    pub(crate) completed: bool,
    pub(crate) completion_results: Option<Vec<ScpFileResult>>,
    pub(crate) last_success_destination: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum TransferState {
    Pending,
    InProgress,
    Completed,
    Failed(String),
}

impl ScpProgress {
    pub(crate) fn new(connection_name: String, files: Vec<ScpFileProgress>) -> Self {
        Self {
            connection_name,
            start_time: std::time::Instant::now(),
            files,
            completed: false,
            completion_results: None,
            last_success_destination: None,
        }
    }

    pub(crate) fn update_progress(&mut self, update: ScpTransferProgress) {
        if let Some(file) = self.files.get_mut(update.file_index) {
            if matches!(
                file.state,
                TransferState::Pending | TransferState::InProgress
            ) {
                file.transferred_bytes = update.transferred_bytes;
                if update.total_bytes.is_some() {
                    file.total_bytes = update.total_bytes;
                }

                // Check if transfer is complete (100%)
                if let Some(total) = file.total_bytes {
                    if file.transferred_bytes >= total && total > 0 {
                        file.state = TransferState::Completed;
                    } else {
                        file.state = TransferState::InProgress;
                    }
                } else {
                    file.state = TransferState::InProgress;
                }
            }
        }
    }

    pub(crate) fn mark_completed(&mut self, index: usize, success: bool, error: Option<String>) {
        if let Some(file) = self.files.get_mut(index) {
            if success {
                file.state = TransferState::Completed;
            } else {
                let message = error.unwrap_or_else(|| "Unknown error".to_string());
                file.state = TransferState::Failed(message);
            }
            if let Some(total) = file.total_bytes {
                file.transferred_bytes = total;
            }
        }
    }
}

/// App is the main application
pub(crate) struct App<B: Backend + Write> {
    pub(crate) mode: AppMode,
    pub(crate) error: Option<AppError>,
    pub(crate) info: Option<String>,
    pub(crate) config: ConfigManager,
    pub(crate) port_forwarding_runtime: async_ssh_client::PortForwardingRuntime,
    terminal: Terminal<B>,
    needs_redraw: bool, // Track if UI needs redrawing
    event_tx: Option<tokio::sync::mpsc::Sender<AppEvent>>, // Event sender for SSH disconnect
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
        disable_raw_mode().ok();
        execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
        )
        .ok();
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
            port_forwarding_runtime: async_ssh_client::PortForwardingRuntime::new(),
            terminal,
            needs_redraw: true, // Initial redraw needed
            event_tx: None,     // Will be set later
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

    pub(crate) fn init_terminal(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            DisableMouseCapture
        )?;

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

    pub(crate) fn set_event_sender(&mut self, sender: tokio::sync::mpsc::Sender<AppEvent>) {
        self.event_tx = Some(sender);
    }

    fn set_mouse_capture(&mut self, enable: bool) -> Result<()> {
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

    pub(crate) fn clear_selection(&mut self) {
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

    pub(crate) fn is_selecting(&self) -> bool {
        self.selection_dragging
    }

    pub(crate) fn start_selection(&mut self, point: SelectionEndpoint) {
        self.selection_anchor = Some(point);
        self.selection_tail = Some(point);
        self.selection_dragging = true;
        self.selection_force_nonempty = false;
        self.mark_redraw();
    }

    pub(crate) fn update_selection(&mut self, point: SelectionEndpoint) {
        if self.selection_anchor.is_some() {
            self.selection_tail = Some(point);
            self.mark_redraw();
        }
    }

    pub(crate) fn finish_selection(&mut self) {
        if self.selection_anchor.is_some() && self.selection_tail.is_some() {
            self.selection_dragging = false;
            self.mark_redraw();
        }
    }

    pub(crate) fn selection_endpoints(&self) -> Option<(SelectionEndpoint, SelectionEndpoint)> {
        let anchor = self.selection_anchor?;
        let tail = self.selection_tail?;
        if anchor == tail && !self.selection_force_nonempty {
            None
        } else {
            Some((anchor, tail))
        }
    }

    pub(crate) fn selection_text(&self, state: &TerminalState) -> Option<String> {
        let (anchor, tail) = self.selection_endpoints()?;
        collect_selection_text(state.parser.screen(), anchor, tail)
    }

    pub(crate) fn begin_selection_auto_scroll(
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
    }

    pub(crate) fn stop_selection_auto_scroll(&mut self) {
        if self.selection_auto_scroll.is_some() {
            self.selection_auto_scroll = None;
        }
    }

    pub(crate) fn register_left_click(&mut self, point: TerminalPoint) -> MouseClickClass {
        const DOUBLE_CLICK_MAX_INTERVAL: Duration = Duration::from_millis(350);
        let now = Instant::now();
        let mut click_class = MouseClickClass::Single;
        let mut click_count = 1;

        if let Some(last) = self.last_click {
            let within_window = now.duration_since(last.time) <= DOUBLE_CLICK_MAX_INTERVAL;
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

    pub(crate) fn clear_click_tracking(&mut self) {
        self.last_click = None;
    }

    pub(crate) fn force_selection_nonempty(&mut self) {
        self.selection_force_nonempty = true;
    }

    pub(crate) fn copy_text_to_clipboard(&mut self, text: String) {
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

    pub(crate) fn viewport_cell_at(&self, column: u16, row: u16) -> Option<TerminalPoint> {
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

    pub(crate) fn clamp_point_to_viewport(
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
        self.clear_selection();
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_form_new(&mut self) {
        self.clear_selection();
        self.mode = AppMode::FormNew {
            auto_auth: false,
            form: ConnectionForm::new(),
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_form_edit(&mut self, form: ConnectionForm, original: Connection) {
        self.clear_selection();
        self.mode = AppMode::FormEdit {
            form,
            original,
            current_selected: self.current_selected(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_connection_list_with_selected(&mut self, selected: usize) {
        self.clear_selection();
        self.mode = AppMode::ConnectionList {
            selected,
            search_mode: false,
            search_input: create_search_textarea(),
        };
        self.needs_redraw = true; // Mode change requires redraw
    }

    pub(crate) fn go_to_scp_progress(
        &mut self,
        progress: ScpProgress,
        receiver: mpsc::Receiver<ScpResult>,
        progress_updates: mpsc::Receiver<ScpTransferProgress>,
        return_mode: ScpReturnMode,
    ) {
        self.mode = AppMode::ScpProgress {
            progress,
            receiver,
            progress_updates,
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

    pub(crate) async fn go_to_port_forwarding_list(&mut self) {
        self.go_to_port_forwarding_list_with_selected(0).await;
    }

    pub(crate) async fn go_to_port_forwarding_list_with_selected(&mut self, selected: usize) {
        // Sync port forwarding status before showing the list
        crate::key_event::port_forwarding::sync_port_forwarding_status(self).await;

        self.mode = AppMode::PortForwardingList {
            selected,
            search_mode: false,
            search_input: create_search_textarea(),
        };
        self.needs_redraw = true;
    }

    pub(crate) fn go_to_port_forwarding_form_new(&mut self) {
        self.mode = AppMode::PortForwardingFormNew {
            form: crate::ui::PortForwardingForm::new(),
            current_selected: self.current_selected(),
            select_connection_mode: false,
            connection_selected: 0,
            connection_search_mode: false,
            connection_search_input: create_search_textarea(),
        };
        self.needs_redraw = true;
    }

    pub(crate) fn go_to_port_forwarding_form_edit(
        &mut self,
        form: crate::ui::PortForwardingForm,
        _original: crate::config::manager::PortForward,
    ) {
        self.mode = AppMode::PortForwardingFormEdit {
            form,
            current_selected: self.current_selected(),
            select_connection_mode: false,
            connection_selected: 0,
            connection_search_mode: false,
            connection_search_input: create_search_textarea(),
        };
        self.needs_redraw = true;
    }

    pub(crate) fn go_to_port_forward_delete_confirmation(
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
            copy_buffer: Vec::new(),
            return_to,
            sftp_session,
            ssh_connection: conn,
            channel: Some(channel),
            search_mode: false,
            search_query: String::new(),
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
            AppMode::ScpProgress { return_mode, .. } => match return_mode {
                ScpReturnMode::ConnectionList { current_selected } => *current_selected,
                ScpReturnMode::Connected {
                    current_selected, ..
                } => *current_selected,
                ScpReturnMode::FileExplorer { return_to, .. } => *return_to,
            },
            AppMode::FileExplorer { return_to, .. } => *return_to,
            AppMode::FormNew { .. } => 0,
            AppMode::PortForwardingList { selected, .. } => {
                let len = self.config.port_forwards().len();
                if len == 0 {
                    0
                } else {
                    (*selected).min(len - 1)
                }
            }
            AppMode::PortForwardingFormNew { .. } => 0,
            AppMode::PortForwardingFormEdit {
                current_selected, ..
            }
            | AppMode::PortForwardDeleteConfirmation {
                current_selected, ..
            } => *current_selected,
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
    #[allow(dead_code)]
    pub(crate) fn set_info(&mut self, info: String) {
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
                AppMode::ConnectionList {
                    selected,
                    search_mode,
                    search_input,
                } => {
                    let conns = self.config.connections();
                    let search_query = search_input.lines()[0].to_string();
                    let search_query_ref = search_query.as_str();

                    if *search_mode {
                        draw_search_overlay(
                            f,
                            size,
                            search_input,
                            "Enter: Apply Search   Esc: Exit Search   Arrow Keys: Move Cursor",
                            [
                                ratatui::layout::Constraint::Percentage(50),
                                ratatui::layout::Constraint::Percentage(50),
                            ],
                            |area, frame| {
                                draw_connection_list(
                                    area,
                                    conns,
                                    *selected,
                                    *search_mode,
                                    search_query_ref,
                                    frame,
                                    false,
                                );
                            },
                        );
                    } else {
                        // Normal mode: let draw_connection_list handle everything
                        draw_connection_list(
                            size,
                            conns,
                            *selected,
                            *search_mode,
                            search_query_ref,
                            f,
                            false,
                        );
                    }
                }
                AppMode::FormNew {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f, false);
                }
                AppMode::FormEdit {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f, false);
                }
                AppMode::Connected { name, state, .. } => {
                    let inner = rect_with_top_margin(size, 1);
                    new_viewport = inner;
                    if let Ok(mut guard) = state.try_lock() {
                        if guard.parser.screen().size() != (inner.height, inner.width) {
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
                        ScpReturnMode::ConnectionList { current_selected } => {
                            let conns = self.config.connections();
                            draw_connection_list(
                                size,
                                conns,
                                *current_selected,
                                false,
                                "",
                                f,
                                false,
                            );
                        }
                        ScpReturnMode::Connected { name, state, .. } => {
                            let inner = rect_with_top_margin(size, 1);
                            new_viewport = inner;
                            if let Ok(mut guard) = state.try_lock() {
                                if guard.parser.screen().size() != (inner.height, inner.width) {
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
                        ScpReturnMode::FileExplorer {
                            connection_name,
                            local_explorer,
                            remote_explorer,
                            active_pane,
                            copy_buffer,
                            search_mode,
                            search_query,
                            ..
                        } => {
                            draw_file_explorer(
                                f,
                                size,
                                connection_name,
                                local_explorer,
                                remote_explorer,
                                active_pane,
                                copy_buffer,
                                *search_mode,
                                search_query,
                            );
                        }
                    }
                }
                AppMode::DeleteConfirmation {
                    current_selected, ..
                } => {
                    // Render the connection list background first
                    let conns = self.config.connections();
                    draw_connection_list(size, conns, *current_selected, false, "", f, false);
                }
                AppMode::FileExplorer {
                    connection_name,
                    local_explorer,
                    remote_explorer,
                    active_pane,
                    copy_buffer,
                    search_mode,
                    search_query,
                    ..
                } => {
                    draw_file_explorer(
                        f,
                        size,
                        connection_name,
                        local_explorer,
                        remote_explorer,
                        active_pane,
                        copy_buffer,
                        *search_mode,
                        search_query,
                    );
                }
                AppMode::PortForwardingList {
                    selected,
                    search_mode,
                    search_input,
                } => {
                    let connections = self.config.connections();
                    let port_forwards = self.config.port_forwards();
                    let search_query = search_input.lines()[0].to_string();
                    let search_query_ref = search_query.as_str();

                    if *search_mode {
                        draw_search_overlay(
                            f,
                            size,
                            search_input,
                            "Enter: Apply Search   Esc: Exit Search   Arrow Keys: Move Cursor",
                            [
                                ratatui::layout::Constraint::Percentage(50),
                                ratatui::layout::Constraint::Percentage(50),
                            ],
                            |area, frame| {
                                draw_port_forwarding_list(
                                    area,
                                    port_forwards,
                                    connections,
                                    *selected,
                                    *search_mode,
                                    search_query_ref,
                                    frame,
                                );
                            },
                        );
                    } else {
                        // Normal mode: let draw_port_forwarding_list handle everything
                        draw_port_forwarding_list(
                            size,
                            port_forwards,
                            connections,
                            *selected,
                            *search_mode,
                            search_query_ref,
                            f,
                        );
                    }
                }
                AppMode::PortForwardingFormNew {
                    current_selected,
                    select_connection_mode,
                    connection_selected,
                    connection_search_mode,
                    connection_search_input,
                    ..
                } => {
                    if *select_connection_mode {
                        // Render connection list for selection
                        let connections = self.config.connections();
                        // Always use the search query from input, regardless of search mode
                        let search_query = connection_search_input.lines()[0].to_string();
                        let search_query_ref = search_query.as_str();

                        if *connection_search_mode {
                            draw_search_overlay(
                                f,
                                size,
                                connection_search_input,
                                "Enter: Select   Esc: Cancel Search   K/↑: Up   J/↓: Down",
                                [
                                    ratatui::layout::Constraint::Percentage(80),
                                    ratatui::layout::Constraint::Percentage(20),
                                ],
                                |area, frame| {
                                    draw_connection_list(
                                        area,
                                        connections,
                                        *connection_selected,
                                        *connection_search_mode,
                                        search_query_ref,
                                        frame,
                                        true,
                                    );
                                },
                            );
                        } else {
                            // Normal mode: just render connection list (still filtered by search query)
                            draw_connection_list(
                                size,
                                connections,
                                *connection_selected,
                                *connection_search_mode,
                                search_query_ref,
                                f,
                                true,
                            );
                        }
                    } else {
                        // Render the port forwarding list background first
                        let port_forwards = self.config.port_forwards();
                        let connections = self.config.connections();
                        draw_port_forwarding_list(
                            size,
                            port_forwards,
                            connections,
                            *current_selected,
                            false,
                            "",
                            f,
                        );
                    }
                }
                AppMode::PortForwardingFormEdit {
                    current_selected,
                    select_connection_mode,
                    connection_selected,
                    connection_search_mode,
                    connection_search_input,
                    ..
                } => {
                    if *select_connection_mode {
                        // Render connection list for selection
                        let connections = self.config.connections();
                        // Always use the search query from input, regardless of search mode
                        let search_query = connection_search_input.lines()[0].to_string();
                        let search_query_ref = search_query.as_str();

                        if *connection_search_mode {
                            draw_search_overlay(
                                f,
                                size,
                                connection_search_input,
                                "Enter: Select   Esc: Cancel Search   K/↑: Up   J/↓: Down",
                                [
                                    ratatui::layout::Constraint::Percentage(80),
                                    ratatui::layout::Constraint::Percentage(20),
                                ],
                                |area, frame| {
                                    draw_connection_list(
                                        area,
                                        connections,
                                        *connection_selected,
                                        *connection_search_mode,
                                        search_query_ref,
                                        frame,
                                        true,
                                    );
                                },
                            );
                        } else {
                            // Normal mode: just render connection list (still filtered by search query)
                            draw_connection_list(
                                size,
                                connections,
                                *connection_selected,
                                *connection_search_mode,
                                search_query_ref,
                                f,
                                true,
                            );
                        }
                    } else {
                        // Render the port forwarding list background first
                        let port_forwards = self.config.port_forwards();
                        let connections = self.config.connections();
                        draw_port_forwarding_list(
                            size,
                            port_forwards,
                            connections,
                            *current_selected,
                            false,
                            "",
                            f,
                        );
                    }
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
                        false,
                        "",
                        f,
                    );
                }
            }

            // Overlay port forwarding form popup if in port forwarding form mode and not selecting connection
            if let AppMode::PortForwardingFormNew {
                form,
                select_connection_mode,
                ..
            } = &mut self.mode
            {
                if !*select_connection_mode {
                    let connections = self.config.connections();
                    draw_port_forwarding_form_popup(size, form, connections, true, f);
                }
            }
            if let AppMode::PortForwardingFormEdit {
                form,
                select_connection_mode,
                ..
            } = &mut self.mode
            {
                if !*select_connection_mode {
                    let connections = self.config.connections();
                    draw_port_forwarding_form_popup(size, form, connections, false, f);
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

            // Overlay port forward delete confirmation popup if in port forward delete confirmation mode
            if let AppMode::PortForwardDeleteConfirmation {
                port_forward_name, ..
            } = &self.mode
            {
                draw_port_forward_delete_confirmation_popup(size, port_forward_name, f);
            }

            // Overlay connection form popup if in form mode
            if let AppMode::FormNew { form, .. } = &self.mode {
                draw_connection_form_popup(size, form, true, f);
            }
            if let AppMode::FormEdit { form, .. } = &mut self.mode {
                draw_connection_form_popup(size, form, false, f);
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

    async fn run(&mut self, rx: &mut mpsc::Receiver<AppEvent>) -> Result<()> {
        loop {
            self.update_mouse_capture_mode()?;

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
                    if self.selection_dragging {
                        if let Some(auto) = self.selection_auto_scroll {
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
                                let (height, width) = guard.parser.screen().size();
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
                    }

                    // Update spinner animation for SCP progress and handle results
                    let mut progress_needs_redraw = false;
                    if let AppMode::ScpProgress {
                        progress,
                        receiver,
                        progress_updates,
                        return_mode,
                    } = &mut self.mode
                    {
                        progress_needs_redraw = true;
                        while let Ok(update) = progress_updates.try_recv() {
                            progress.update_progress(update);
                            progress_needs_redraw = true;
                        }
                        // Mark redraw after handling the progress update

                        // Drain any SCP results
                        match receiver.try_recv() {
                            Ok(result) => {
                                let mut pending_error: Option<AppError> = None;

                                match result {
                                    ScpResult::Completed(file_results) => {
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
                                                failure_lines.push(format!(
                                                    " from {from} to {to} (FAILED: {err})"
                                                ));
                                            }
                                        }

                                        if !all_success {
                                            let mut message = String::from("sftp transfer issues:");
                                            for line in failure_lines {
                                                message.push('\n');
                                                message.push_str(&line);
                                            }
                                            pending_error =
                                                Some(AppError::SshConnectionError(message));
                                        }

                                        progress.completed = true;
                                        progress.last_success_destination =
                                            last_success_destination;
                                        progress.completion_results = Some(file_results);
                                        progress_needs_redraw = true;
                                    }
                                    ScpResult::Error { error } => {
                                        for idx in 0..progress.files.len() {
                                            progress.mark_completed(
                                                idx,
                                                false,
                                                Some(error.clone()),
                                            );
                                        }
                                        pending_error =
                                            Some(AppError::SshConnectionError(error.clone()));
                                        progress.completed = true;
                                        progress.completion_results = None;
                                        progress_needs_redraw = true;
                                    }
                                }

                                if let Some(err) = pending_error {
                                    self.set_error(err);
                                }
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                if !progress.completed {
                                    // Clone the return_mode before we change self.mode
                                    let return_mode = return_mode.clone_without_channel();

                                    self.set_error(AppError::SshConnectionError(
                                        "SCP transfer task disconnected unexpectedly".to_string(),
                                    ));

                                    // Return to the appropriate mode
                                    match return_mode {
                                        ScpReturnMode::ConnectionList { current_selected } => {
                                            self.go_to_connection_list_with_selected(
                                                current_selected,
                                            );
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
                                            copy_buffer,
                                            return_to,
                                            sftp_session,
                                            ssh_connection,
                                            channel,
                                            search_mode,
                                            search_query,
                                        } => {
                                            self.mode = AppMode::FileExplorer {
                                                connection_name,
                                                local_explorer,
                                                remote_explorer,
                                                active_pane,
                                                copy_buffer,
                                                return_to,
                                                sftp_session,
                                                ssh_connection,
                                                channel,
                                                search_mode,
                                                search_query,
                                            };
                                        }
                                    }
                                }
                            }
                        }

                        // Mark redraw after all progress handling is done
                    }
                    if progress_needs_redraw {
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

fn order_selection_endpoints(
    anchor: SelectionEndpoint,
    tail: SelectionEndpoint,
) -> (SelectionEndpoint, SelectionEndpoint) {
    if anchor.rev_row > tail.rev_row {
        (anchor, tail)
    } else if anchor.rev_row < tail.rev_row {
        (tail, anchor)
    } else if anchor.col <= tail.col {
        (anchor, tail)
    } else {
        (tail, anchor)
    }
}

fn compute_rev_from_view(height: u16, scrollback: usize, view_row: u16) -> i64 {
    if height == 0 {
        return 0;
    }
    let clamped_row = view_row.min(height.saturating_sub(1));
    i64::from(height - 1 - clamped_row) + scrollback as i64
}

fn rev_to_view_row(state: &TerminalState, rev_row: i64) -> Option<u16> {
    rev_to_view_row_on_screen(state.parser.screen(), rev_row)
}

fn rev_to_view_row_on_screen(screen: &vt100::Screen, rev_row: i64) -> Option<u16> {
    let (height, _) = screen.size();
    if height == 0 {
        return None;
    }
    let scrollback = screen.scrollback() as i64;
    let row = (height as i64 - 1) - (rev_row - scrollback);
    if row < 0 || row >= height as i64 {
        None
    } else {
        Some(row as u16)
    }
}

fn visible_rev_bounds(state: &TerminalState) -> Option<(i64, i64)> {
    let (height, _) = state.parser.screen().size();
    if height == 0 {
        return None;
    }
    let scrollback = state.parser.screen().scrollback() as i64;
    let min_rev = scrollback;
    let max_rev = scrollback + height as i64 - 1;
    Some((min_rev, max_rev))
}

fn compute_selection_for_view(
    anchor: Option<SelectionEndpoint>,
    tail: Option<SelectionEndpoint>,
    state: &TerminalState,
    width: u16,
    force_nonempty: bool,
) -> Option<TerminalSelection> {
    let (anchor, tail) = match (anchor, tail) {
        (Some(a), Some(b)) => (a, b),
        _ => return None,
    };
    if anchor == tail && !force_nonempty {
        return None;
    }
    if width == 0 {
        return None;
    }
    let (top, bottom) = order_selection_endpoints(anchor, tail);
    let (visible_min, visible_max) = visible_rev_bounds(state)?;
    if top.rev_row < visible_min || bottom.rev_row > visible_max {
        return None;
    }
    let clamped_top = top.rev_row.clamp(visible_min, visible_max);
    let clamped_bottom = bottom.rev_row.clamp(visible_min, visible_max);
    if clamped_top < clamped_bottom {
        return None;
    }
    let start_row = rev_to_view_row(state, clamped_top)?;
    let end_row = rev_to_view_row(state, clamped_bottom)?;

    let start_col = if top.rev_row == clamped_top {
        top.col.min(width.saturating_sub(1))
    } else {
        0
    };
    let end_col = if bottom.rev_row == clamped_bottom {
        bottom.col.saturating_add(1).min(width)
    } else {
        width
    };

    if start_row == end_row {
        if start_col >= end_col {
            return None;
        }
    }

    Some(TerminalSelection {
        start_row,
        start_col,
        end_row,
        end_col,
    })
}

pub(crate) fn make_selection_endpoint(
    state: &TerminalState,
    view_row: u16,
    view_col: u16,
) -> Option<SelectionEndpoint> {
    let (height, width) = state.parser.screen().size();
    if height == 0 || width == 0 {
        return None;
    }
    let clamped_col = view_col.min(width.saturating_sub(1));
    let rev_row = compute_rev_from_view(height, state.parser.screen().scrollback(), view_row);
    Some(SelectionEndpoint {
        rev_row,
        col: clamped_col,
    })
}

fn collect_selection_text(
    screen: &vt100::Screen,
    anchor: SelectionEndpoint,
    tail: SelectionEndpoint,
) -> Option<String> {
    let (height, width) = screen.size();
    if height == 0 || width == 0 {
        return None;
    }

    let (top, bottom) = order_selection_endpoints(anchor, tail);
    let mut current_rev = top.rev_row;
    let mut result = String::new();

    while current_rev >= bottom.rev_row {
        if current_rev < 0 {
            break;
        }

        let view_row = match rev_to_view_row_on_screen(screen, current_rev) {
            Some(row) => row,
            None => {
                if current_rev == bottom.rev_row {
                    break;
                }
                current_rev -= 1;
                continue;
            }
        };

        let mut start_col = if current_rev == top.rev_row {
            top.col
        } else {
            0
        };
        let mut end_col = if current_rev == bottom.rev_row {
            bottom.col.saturating_add(1)
        } else {
            width
        };

        start_col = start_col.min(width);
        end_col = end_col.min(width);

        if end_col > start_col {
            let segment = extract_screen_segment(screen, view_row, start_col, end_col);
            result.push_str(&segment);
        }

        if current_rev == bottom.rev_row {
            break;
        }

        if !screen.row_wrapped(view_row) {
            result.push('\n');
        }

        if current_rev == i64::MIN {
            break;
        }
        current_rev -= 1;
    }

    Some(result)
}

fn extract_screen_segment(
    screen: &vt100::Screen,
    row: u16,
    start_col: u16,
    end_col: u16,
) -> String {
    let mut text = String::new();
    let mut col = start_col;
    while col < end_col {
        if let Some(cell) = screen.cell(row, col) {
            if cell.is_wide_continuation() {
                col = col.saturating_add(1);
                continue;
            }
            if cell.has_contents() {
                text.push_str(cell.contents());
                let advance = if cell.is_wide() { 2 } else { 1 };
                col = col.saturating_add(advance);
                continue;
            }
        }
        text.push(' ');
        col = col.saturating_add(1);
    }
    text
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn selection_above_viewport_is_hidden() {
        let state = TerminalState::new(5, 10);
        let endpoint = SelectionEndpoint {
            rev_row: 10,
            col: 3,
        };
        let result = compute_selection_for_view(Some(endpoint), Some(endpoint), &state, 10, false);
        assert!(result.is_none());
    }

    #[test]
    fn selection_below_viewport_is_hidden() {
        let state = TerminalState::new(5, 10);
        let endpoint = SelectionEndpoint {
            rev_row: -1,
            col: 0,
        };
        let result = compute_selection_for_view(Some(endpoint), Some(endpoint), &state, 10, false);
        assert!(result.is_none());
    }

    #[test]
    fn selection_overlapping_viewport_is_rendered() {
        let mut state = TerminalState::new(5, 10);
        state.parser.screen_mut().set_scrollback(2);
        let anchor = SelectionEndpoint { rev_row: 7, col: 4 };
        let tail = SelectionEndpoint { rev_row: 4, col: 5 };
        let selection = compute_selection_for_view(Some(anchor), Some(tail), &state, 10, false)
            .expect("selection should be visible");
        assert_eq!(selection.start_row, 0);
        assert_eq!(selection.end_col, 6);
    }

    #[test]
    fn selection_in_alternate_screen_copies_text() {
        let mut state = TerminalState::new(5, 20);
        state.process_bytes(b"\x1b[?1049h");
        state.process_bytes(b"first line in vim");
        state.process_bytes(b"\r\nsecond row");

        let anchor = make_selection_endpoint(&state, 0, 0).unwrap();
        let tail = make_selection_endpoint(&state, 1, 6).unwrap();
        let text =
            collect_selection_text(state.parser.screen(), anchor, tail).expect("text available");

        assert!(text.contains("first line"));
        assert!(text.contains("second"));
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

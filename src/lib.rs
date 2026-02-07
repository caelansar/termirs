mod async_ssh_client;
mod config;
mod error;
mod filesystem;
mod key_event;
mod mode_state;
mod search_state;
mod ui;

// New modules from refactoring
mod app;
mod events;
mod terminal;
mod transfer;
mod utils;

// Re-export commonly used types
pub use app::{
    ActivePane, App, AppMode, ConnectingSource, CopyDirection, CopyOperation, FileExplorerPane,
    LeftExplorer, ScpReturnMode, create_search_textarea,
};
pub use async_ssh_client::expand_tilde;
pub use error::{AppError, Result};
pub use events::{AppEvent, TickControl};
pub use mode_state::{
    ConnectionSelectorState, DeleteConfirmationState, FormWithConnectionSelector,
    ListSelectionState, SourceSelectorState,
};
pub use search_state::SearchState;
pub use transfer::{
    ScpFileProgress, ScpFileResult, ScpProgress, ScpResult, ScpTransferProgress, ScpTransferSpec,
    TransferState,
};
pub use utils::{init_panic_hook, init_tracing};

// Implement ByteProcessor for TerminalState
impl async_ssh_client::ByteProcessor for ui::TerminalState {
    fn process_bytes(&mut self, bytes: &[u8]) {
        self.process_bytes(bytes);
    }
}

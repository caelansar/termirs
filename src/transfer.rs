/// Result summary of SFTP transfer operations
#[derive(Debug, Clone)]
pub enum ScpResult {
    Progress(ScpTransferProgress),
    Completed(Vec<ScpFileResult>),
    Error { error: String },
}

/// Outcome for a single file within a batch transfer
#[derive(Debug, Clone)]
pub struct ScpFileResult {
    pub mode: crate::ui::ScpMode,
    pub local_path: String,
    pub remote_path: String,
    pub destination_filename: String,
    pub success: bool,
    pub error: Option<String>,
    pub completed_at: Option<std::time::Instant>,
}

/// Byte-level progress updates for SFTP transfers (per file)
#[derive(Debug, Clone)]
pub struct ScpTransferProgress {
    pub file_index: usize,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// Specification for a single file transfer within a batch
#[derive(Clone, Debug)]
pub struct ScpTransferSpec {
    pub mode: crate::ui::ScpMode,
    pub local_path: String,
    pub remote_path: String,
    pub display_name: String,
    pub destination_filename: String,
    pub is_ssh_to_ssh: bool, // True if transferring between two SSH hosts
}

/// Per-file progress snapshot
#[derive(Clone, Debug)]
pub struct ScpFileProgress {
    pub local_path: String,
    pub remote_path: String,
    pub display_name: String,
    pub mode: crate::ui::ScpMode, // Send or Receive mode
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub state: TransferState,
}

impl ScpFileProgress {
    pub fn from_spec(spec: &ScpTransferSpec) -> Self {
        Self {
            local_path: spec.local_path.clone(),
            remote_path: spec.remote_path.clone(),
            display_name: spec.display_name.clone(),
            mode: spec.mode,
            transferred_bytes: 0,
            total_bytes: None,
            state: TransferState::Pending,
        }
    }

    pub fn ratio(&self) -> f64 {
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
pub struct ScpProgress {
    pub connection_name: String,
    pub start_time: std::time::Instant,
    pub files: Vec<ScpFileProgress>,
    pub completed: bool,
    pub completion_results: Option<Vec<ScpFileResult>>,
    pub last_success_destination: Option<String>,
    /// Tracks when all files reached 100% transfer
    pub all_files_done_at: Option<std::time::Instant>,
    /// Viewport scroll offset for rendering large file lists
    pub scroll_offset: usize,
}

#[derive(Clone, Debug)]
pub enum TransferState {
    Pending,
    InProgress,
    Completed,
    Failed(String),
}

impl ScpProgress {
    pub fn new(connection_name: String, files: Vec<ScpFileProgress>) -> Self {
        Self {
            connection_name,
            start_time: std::time::Instant::now(),
            files,
            completed: false,
            completion_results: None,
            last_success_destination: None,
            all_files_done_at: None,
            scroll_offset: 0,
        }
    }

    /// Check if all files have finished transferring (100% or failed)
    pub fn all_files_finished(&self) -> bool {
        self.files
            .iter()
            .all(|f| matches!(f.state, TransferState::Completed | TransferState::Failed(_)))
    }

    pub fn update_progress(&mut self, update: ScpTransferProgress) {
        if let Some(file) = self.files.get_mut(update.file_index)
            && matches!(
                file.state,
                TransferState::Pending | TransferState::InProgress
            )
        {
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

        // Track when all files reach 100% (to freeze elapsed time display)
        if self.all_files_done_at.is_none() && self.all_files_finished() {
            self.all_files_done_at = Some(std::time::Instant::now());
        }
    }

    pub fn mark_completed(&mut self, index: usize, success: bool, error: Option<String>) {
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

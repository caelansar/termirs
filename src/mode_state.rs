use crate::ActivePane;
use crate::search_state::SearchState;

/// Shared state for list-based views with selection and search
///
/// This pattern is used by ConnectionList and PortForwardingList modes,
/// eliminating duplication of the selected + search pattern.
#[derive(Clone, Debug)]
pub struct ListSelectionState {
    pub selected: usize,
    pub search: SearchState,
}

impl ListSelectionState {
    /// Create a new list selection state with the given selected index
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            search: SearchState::Off,
        }
    }
}

/// State for connection selector popup
///
/// Used in FileExplorer and PortForwardingForm modes to provide a consistent
/// connection selection experience.
#[derive(Clone, Debug)]
pub struct ConnectionSelectorState {
    pub showing: bool,
    pub selected: usize,
    pub search: SearchState,
}

impl ConnectionSelectorState {
    /// Create a new connection selector state (hidden by default)
    pub fn new() -> Self {
        Self {
            showing: false,
            selected: 0,
            search: SearchState::Off,
        }
    }

    /// Show the connection selector popup
    pub fn show(&mut self) {
        self.showing = true;
    }

    /// Hide the connection selector popup
    pub fn hide(&mut self) {
        self.showing = false;
    }
}

impl Default for ConnectionSelectorState {
    fn default() -> Self {
        Self::new()
    }
}

/// State for delete confirmation popup
///
/// Used in FileExplorer mode to confirm file deletions.
#[derive(Clone, Debug)]
pub struct DeleteConfirmationState {
    pub showing: bool,
    pub file_name: String,
    pub pane: ActivePane,
}

impl DeleteConfirmationState {
    /// Create a new delete confirmation state (hidden by default)
    pub fn new() -> Self {
        Self {
            showing: false,
            file_name: String::new(),
            pane: ActivePane::Left,
        }
    }

    /// Show the delete confirmation popup
    pub fn show(&mut self, file_name: String, pane: ActivePane) {
        self.showing = true;
        self.file_name = file_name;
        self.pane = pane;
    }

    /// Hide the delete confirmation popup
    pub fn hide(&mut self) {
        self.showing = false;
    }
}

impl Default for DeleteConfirmationState {
    fn default() -> Self {
        Self::new()
    }
}

/// Source selector popup state for FileExplorer
///
/// Used when opening multiple connections in split pane mode.
#[derive(Clone, Debug)]
pub struct SourceSelectorState {
    pub showing: bool,
    pub selected: usize,
    pub search: SearchState,
}

impl SourceSelectorState {
    /// Create a new source selector state (hidden by default)
    pub fn new() -> Self {
        Self {
            showing: false,
            selected: 0,
            search: SearchState::Off,
        }
    }

    /// Show the source selector popup
    pub fn show(&mut self) {
        self.showing = true;
    }

    /// Hide the source selector popup
    pub fn hide(&mut self) {
        self.showing = false;
    }
}

impl Default for SourceSelectorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Generic form state with connection selector
///
/// This pattern is used by PortForwardingFormNew and PortForwardingFormEdit
/// to unify the connection selector logic.
#[derive(Clone, Debug)]
pub struct FormWithConnectionSelector<F> {
    pub form: F,
    pub current_selected: usize,
    pub connection_selector: ConnectionSelectorState,
}

impl<F> FormWithConnectionSelector<F> {
    /// Create a new form with connection selector
    pub fn new(form: F, current_selected: usize) -> Self {
        Self {
            form,
            current_selected,
            connection_selector: ConnectionSelectorState::new(),
        }
    }
}

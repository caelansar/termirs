pub mod selection;

pub use selection::{
    LastMouseClick, MouseClickClass, SelectionAutoScroll, SelectionEndpoint,
    SelectionScrollDirection, TerminalPoint, compute_selection_for_view, make_selection_endpoint,
};

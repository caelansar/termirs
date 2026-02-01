/// Shared table component abstraction for reusable table views.
///
/// This module provides a trait-based abstraction for creating table components
/// with built-in support for navigation, search, filtering, and rendering.
///
/// # Example
///
/// ```ignore
/// use termirs::ui::table::{TableListComponent, TableListState};
/// use ratatui::widgets::{Row, Cell};
/// use ratatui::layout::Constraint;
///
/// struct MyItem<'a> {
///     name: &'a str,
///     value: &'a str,
/// }
///
/// struct MyTableComponent;
///
/// impl TableListComponent<3> for MyTableComponent {
///     type Item<'a> = MyItem<'a>;
///
///     const HEADER_LABELS: &'static [&'static str; 3] = &["Name", "Value", ""];
///     const COLUMN_CONSTRAINTS: &'static [Constraint; 3] = &[
///         Constraint::Min(10),
///         Constraint::Min(10),
///         Constraint::Length(1), // Scrollbar
///     ];
///
///     fn render_row(&self, item: &MyItem<'_>) -> Row<'static> {
///         Row::new(vec![
///             Cell::from(item.name.to_string()),
///             Cell::from(item.value.to_string()),
///         ])
///     }
///
///     fn matches_query(&self, item: &MyItem<'_>, query: &str) -> bool {
///         item.name.to_lowercase().contains(&query.to_lowercase())
///     }
///
///     fn footer_hints(&self) -> &'static str {
///         "Enter: Select  K/↑: Up  J/↓: Down  /: Search"
///     }
/// }
/// ```
use crate::SearchState;

/// Core trait for table components.
///
/// Implement this trait to create a new table-based view with automatic
/// support for navigation, searching, and rendering.
pub trait TableListComponent<const N: usize> {
    /// The type of items displayed in each row.
    /// Use a lifetime parameter to support borrowed data.
    type Item<'a>;

    /// Column header labels displayed at the top of the table.
    const HEADER_LABELS: &'static [&'static str; N];

    /// Column width constraints for the table layout.
    /// Should include an extra `Constraint::Length(1)` for the scrollbar column.
    const COLUMN_CONSTRAINTS: &'static [Constraint; N];

    /// Render a single row for the given item.
    ///
    /// # Arguments
    /// * `item` - The item to render
    ///
    /// # Returns
    /// A `Row` widget ready to be displayed in the table
    fn render_row(&self, item: &Self::Item<'_>) -> Row<'static>;

    /// Check if an item matches the search query.
    ///
    /// # Arguments
    /// * `item` - The item to check
    /// * `query` - The search query string
    ///
    /// # Returns
    /// `true` if the item matches the query, `false` otherwise
    fn matches_query(&self, item: &Self::Item<'_>, query: &str) -> bool;

    /// Footer hints displayed at the bottom of the table.
    ///
    /// # Returns
    /// A string describing available keyboard shortcuts
    fn footer_hints(&self) -> &'static str;

    /// Format the table title showing current selection and total count.
    ///
    /// # Arguments
    /// * `current` - Currently selected index (0-based)
    /// * `total` - Total number of items
    ///
    /// # Returns
    /// A formatted string like "(1/10)" or "(0/0)" for empty lists
    fn table_title(&self, current: usize, total: usize) -> String {
        format!("({}/{})", if total > 0 { current + 1 } else { 0 }, total)
    }
}

// Re-export ratatui types for convenience
pub use ratatui::{layout::Constraint, widgets::Row};

/// State management for table lists with search support.
///
/// This struct manages the selection index and search state for a table component.
/// It provides methods for navigation (scrolling) and search state management.
#[derive(Clone, Debug)]
pub struct TableListState {
    /// Currently selected row index (0-based)
    pub selected: usize,
    /// Current search state (Off, On, or Applied)
    pub search: SearchState,
}

impl TableListState {
    /// Create a new table list state with default values.
    pub fn new() -> Self {
        Self {
            selected: 0,
            search: SearchState::default(),
        }
    }

    /// Create a new table list state from existing values.
    ///
    /// # Arguments
    /// * `selected` - Initial selection index
    /// * `search` - Initial search state
    pub fn from_parts(selected: usize, search: SearchState) -> Self {
        Self { selected, search }
    }

    /// Scroll up by one item, wrapping to the end if at the beginning.
    ///
    /// # Arguments
    /// * `len` - Total number of items in the list
    ///
    /// # Behavior
    /// - If `len` is 0, selection is set to 0
    /// - If at the first item (0), wraps to last item (`len - 1`)
    /// - Otherwise, decrements selection by 1
    pub fn scroll_up(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected = (self.selected - 1).min(len - 1);
        }
    }

    /// Scroll down by one item, wrapping to the beginning if at the end.
    ///
    /// # Arguments
    /// * `len` - Total number of items in the list
    ///
    /// # Behavior
    /// - If `len` is 0, selection is set to 0
    /// - Uses modulo arithmetic to wrap around: `(selected + 1) % len`
    pub fn scroll_down(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1) % len;
        }
    }

    /// Reset selection to the first item (index 0).
    ///
    /// This is typically called when the search query changes to provide
    /// a consistent user experience.
    pub fn reset_selection(&mut self) {
        self.selected = 0;
    }
}

impl Default for TableListState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_down_wraps_correctly() {
        let mut state = TableListState::new();

        // Empty list
        state.scroll_down(0);
        assert_eq!(state.selected, 0);

        // Normal scrolling
        state.scroll_down(5);
        assert_eq!(state.selected, 1);
        state.scroll_down(5);
        assert_eq!(state.selected, 2);

        // Wrap around
        state.selected = 4;
        state.scroll_down(5);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_scroll_up_wraps_correctly() {
        let mut state = TableListState::new();

        // Empty list
        state.scroll_up(0);
        assert_eq!(state.selected, 0);

        // Wrap from first to last
        state.selected = 0;
        state.scroll_up(5);
        assert_eq!(state.selected, 4);

        // Normal scrolling
        state.scroll_up(5);
        assert_eq!(state.selected, 3);
        state.scroll_up(5);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn test_reset_selection() {
        let mut state = TableListState::new();
        state.selected = 42;
        state.reset_selection();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_table_title_formatting() {
        struct DummyComponent;
        impl TableListComponent<1> for DummyComponent {
            type Item<'a> = &'a str;
            const HEADER_LABELS: &'static [&'static str; 1] = &["Test"];
            const COLUMN_CONSTRAINTS: &'static [Constraint; 1] = &[Constraint::Min(10)];

            fn render_row(&self, _item: &Self::Item<'_>) -> Row<'static> {
                Row::default()
            }

            fn matches_query(&self, _item: &Self::Item<'_>, _query: &str) -> bool {
                true
            }

            fn footer_hints(&self) -> &'static str {
                "Test"
            }
        }

        let component = DummyComponent;

        // Empty list
        assert_eq!(component.table_title(0, 0), "(0/0)");

        // First item selected
        assert_eq!(component.table_title(0, 10), "(1/10)");

        // Last item selected
        assert_eq!(component.table_title(9, 10), "(10/10)");
    }
}

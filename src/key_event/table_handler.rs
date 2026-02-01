/// Shared key event handlers for table components.
///
/// This module provides reusable key event handling for search and navigation,
/// reducing code duplication across different table views.
use crossterm::event::{KeyCode, KeyEvent};

use crate::ui::table::TableListState;

/// Handle search-related key events.
///
/// Processes keyboard input when search is active (On) or applied:
/// - **Char(c)**: Add character to search query and reset selection
/// - **Backspace**: Remove last character from query and reset selection
/// - **Esc**: Clear query (if non-empty) or deactivate search, reset selection
/// - **Enter**: Apply the search filter
///
/// # Arguments
/// * `state` - The table list state containing selection and search
///
/// # Returns
/// `true` if the event was handled, `false` otherwise
///
/// # Example
/// ```ignore
/// use termirs::ui::table::TableListState;
/// use termirs::key_event::KeyFlow;
/// use crossterm::event::KeyEvent;
///
/// let mut state = TableListState::default();
/// let key = KeyEvent::from(crossterm::event::KeyCode::Char('a'));
/// let mut app = todo!();
///
/// if handle_search_keys(&mut state, key) {
///     app.mark_redraw();
///     return KeyFlow::Continue;
/// }
/// ```
pub fn handle_search_keys(state: &mut TableListState, key: KeyEvent) -> bool {
    // Handle typing in search mode
    if state.search.is_on() {
        match key.code {
            KeyCode::Char(c) => {
                if let Some(query) = state.search.query_mut() {
                    query.push(c);
                }
                state.reset_selection();
                return true;
            }
            KeyCode::Backspace => {
                if let Some(query) = state.search.query_mut() {
                    query.pop();
                }
                state.reset_selection();
                return true;
            }
            KeyCode::Esc => {
                if !state.search.query().is_empty() {
                    state.search.clear_query();
                } else {
                    state.search.deactivate();
                }
                state.reset_selection();
                return true;
            }
            KeyCode::Enter => {
                state.search.apply();
                // Keep current selection when applying filter
                return true;
            }
            _ => return false,
        }
    }

    // Handle Esc when search filter is applied (but not actively editing)
    if matches!(state.search, crate::SearchState::Applied { .. }) {
        if key.code == KeyCode::Esc {
            state.search.deactivate();
            state.reset_selection();
            return true;
        }
    }

    false
}

/// Handle navigation key events (up, down, search activation).
///
/// Processes keyboard input for navigating the table:
/// - **k** or **Up**: Scroll up (wraps to end)
/// - **j** or **Down**: Scroll down (wraps to beginning)
/// - **/**: Activate search mode and reset selection
///
/// # Arguments
/// * `state` - The table list state containing selection and search
/// * `key` - The keyboard event to process
/// * `list_len` - The length of the (filtered) list
///
/// # Returns
/// `true` if the event was handled, `false` otherwise
///
/// # Example
/// ```ignore
/// use termirs::ui::table::TableListState;
/// use termirs::key_event::KeyFlow;
/// use crossterm::event::KeyEvent;
///
/// let mut state = TableListState::default();
/// let key = KeyEvent::from(crossterm::event::KeyCode::Char('j'));
/// let mut app = todo!();
/// fn get_filtered_list_length() -> usize { 10 }
///
/// let len = get_filtered_list_length();
/// if handle_navigation_keys(&mut state, key, len) {
///     app.mark_redraw();
///     return KeyFlow::Continue;
/// }
/// ```
pub fn handle_navigation_keys(state: &mut TableListState, key: KeyEvent, list_len: usize) -> bool {
    match key.code {
        KeyCode::Char('k') | KeyCode::Up => {
            state.scroll_up(list_len);
            true
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.scroll_down(list_len);
            true
        }
        KeyCode::Char('/') => {
            state.search.activate();
            state.reset_selection();
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchState;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn test_search_keys_char_input() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::new_on(),
        };

        assert!(handle_search_keys(&mut state, make_key(KeyCode::Char('a'))));
        assert_eq!(state.search.query(), "a");
        assert_eq!(state.selected, 0); // Reset on query change

        state.selected = 3;
        assert!(handle_search_keys(&mut state, make_key(KeyCode::Char('b'))));
        assert_eq!(state.search.query(), "ab");
        assert_eq!(state.selected, 0); // Reset again
    }

    #[test]
    fn test_search_keys_backspace() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::with_query("test".to_string()),
        };

        assert!(handle_search_keys(&mut state, make_key(KeyCode::Backspace)));
        assert_eq!(state.search.query(), "tes");
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_search_keys_esc_clears_query() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::with_query("test".to_string()),
        };

        // First Esc clears query
        assert!(handle_search_keys(&mut state, make_key(KeyCode::Esc)));
        assert_eq!(state.search.query(), "");
        assert!(state.search.is_on()); // Still in search mode
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_search_keys_esc_deactivates_when_empty() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::new_on(),
        };

        // Esc with empty query deactivates search
        assert!(handle_search_keys(&mut state, make_key(KeyCode::Esc)));
        assert!(state.search.is_off());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_search_keys_enter_applies_filter() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::with_query("test".to_string()),
        };

        assert!(handle_search_keys(&mut state, make_key(KeyCode::Enter)));
        assert!(matches!(state.search, SearchState::Applied { .. }));
        assert_eq!(state.selected, 5); // Selection preserved on apply
    }

    #[test]
    fn test_search_keys_esc_clears_applied_filter() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Applied {
                query: "test".to_string(),
            },
        };

        assert!(handle_search_keys(&mut state, make_key(KeyCode::Esc)));
        assert!(state.search.is_off());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_search_keys_ignores_when_off() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Off,
        };

        assert!(!handle_search_keys(
            &mut state,
            make_key(KeyCode::Char('a'))
        ));
        assert_eq!(state.selected, 5); // Unchanged
        assert!(state.search.is_off());
    }

    #[test]
    fn test_navigation_keys_scroll_up() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Off,
        };

        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('k')),
            10
        ));
        assert_eq!(state.selected, 4);

        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Up),
            10
        ));
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn test_navigation_keys_scroll_down() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Off,
        };

        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('j')),
            10
        ));
        assert_eq!(state.selected, 6);

        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Down),
            10
        ));
        assert_eq!(state.selected, 7);
    }

    #[test]
    fn test_navigation_keys_wraps_correctly() {
        let mut state = TableListState {
            selected: 0,
            search: SearchState::Off,
        };

        // Scroll up from first wraps to last
        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('k')),
            10
        ));
        assert_eq!(state.selected, 9);

        // Scroll down from last wraps to first
        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('j')),
            10
        ));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_navigation_keys_slash_activates_search() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Off,
        };

        assert!(handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('/')),
            10
        ));
        assert!(state.search.is_on());
        assert_eq!(state.selected, 0); // Reset on search activation
    }

    #[test]
    fn test_navigation_keys_ignores_other_keys() {
        let mut state = TableListState {
            selected: 5,
            search: SearchState::Off,
        };

        assert!(!handle_navigation_keys(
            &mut state,
            make_key(KeyCode::Char('a')),
            10
        ));
        assert_eq!(state.selected, 5); // Unchanged
    }
}

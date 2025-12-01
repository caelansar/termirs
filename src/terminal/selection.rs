use std::time::{Duration, Instant};

use crate::ui::TerminalState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalPoint {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionEndpoint {
    pub rev_row: i64,
    pub col: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionAutoScroll {
    pub direction: SelectionScrollDirection,
    pub view_row: u16,
    pub view_col: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct LastMouseClick {
    pub point: TerminalPoint,
    pub time: Instant,
    pub count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseClickClass {
    Single,
    Double,
    Triple,
}

impl TerminalPoint {
    pub const DOUBLE_CLICK_MAX_INTERVAL: Duration = Duration::from_millis(350);
}

pub fn order_selection_endpoints(
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

pub fn compute_rev_from_view(height: u16, scrollback: usize, view_row: u16) -> i64 {
    if height == 0 {
        return 0;
    }
    let clamped_row = view_row.min(height.saturating_sub(1));
    i64::from(height - 1 - clamped_row) + scrollback as i64
}

pub fn rev_to_view_row(state: &TerminalState, rev_row: i64) -> Option<u16> {
    rev_to_view_row_on_screen(state.parser.screen(), rev_row)
}

pub fn rev_to_view_row_on_screen(screen: &vt100::Screen, rev_row: i64) -> Option<u16> {
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

pub fn visible_rev_bounds(state: &TerminalState) -> Option<(i64, i64)> {
    let (height, _) = state.parser.screen().size();
    if height == 0 {
        return None;
    }
    let scrollback = state.parser.screen().scrollback() as i64;
    let min_rev = scrollback;
    let max_rev = scrollback + height as i64 - 1;
    Some((min_rev, max_rev))
}

pub fn compute_selection_for_view(
    anchor: Option<SelectionEndpoint>,
    tail: Option<SelectionEndpoint>,
    state: &TerminalState,
    width: u16,
    force_nonempty: bool,
) -> Option<crate::ui::TerminalSelection> {
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

    if start_row == end_row && start_col >= end_col {
        return None;
    }

    Some(crate::ui::TerminalSelection {
        start_row,
        start_col,
        end_row,
        end_col,
    })
}

pub fn make_selection_endpoint(
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

pub fn collect_selection_text(
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
mod tests {
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

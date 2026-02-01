/// Generic table rendering with scrollbar and footer support.
///
/// This module provides reusable rendering functions for table components,
/// including smart scrollbar positioning and three-state footer rendering.
use super::table::{TableListComponent, TableListState};
use crate::SearchState;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    prelude::Stylize,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
        TableState,
    },
};

/// Draw a generic table list with filtering, scrolling, and footer.
///
/// This is the main rendering function that handles:
/// - Filtering items based on search query
/// - Rendering table with header and rows
/// - Smart scrollbar visibility and positioning
/// - Three-state footer (search active/applied/normal)
///
/// # Arguments
/// * `area` - The rectangular area to render the table in
/// * `component` - The table component implementing TableListComponent
/// * `items` - All items to display (before filtering)
/// * `state` - The table list state (selection + search)
/// * `frame` - The ratatui frame to render to
/// * `title` - The base title for the table (e.g., "Connection List")
///
/// # Example
/// ```ignore
/// use termirs::ui::table::TableListState;
/// use termirs::SearchState;
///
/// let component = ConnectionTableComponent;
/// let items = vec![/* ... */];
/// let selected = 0;
/// let search = SearchState::Off;
/// let state = TableListState::from_parts(selected, search);
///
/// draw_table_list(
///     area,
///     &component,
///     items,
///     &state,
///     frame,
///     "Connection List",
/// );
/// ```
pub fn draw_table_list<'a, T, const N: usize>(
    area: Rect,
    component: &T,
    items: Vec<T::Item<'a>>,
    state: &TableListState,
    frame: &mut Frame<'_>,
    title: &str,
) where
    T: TableListComponent<N> + 'a,
{
    // Filter and render rows in one pass to avoid lifetime issues
    // Since render_row returns Row<'static>, we own the row data
    let rows_and_items: Vec<(ratatui::widgets::Row<'static>, bool)> = items
        .into_iter()
        .map(|item| {
            let matches = state.search.query().is_empty()
                || component.matches_query(&item, state.search.query());
            let row = component.render_row(&item);
            (row, matches)
        })
        .collect();

    // Filter to only keep matching rows
    let rows: Vec<_> = rows_and_items
        .into_iter()
        .filter_map(|(row, matches)| if matches { Some(row) } else { None })
        .collect();

    // Save the length before moving rows
    let rows_len = rows.len();

    // Clamp selection to valid range
    let sel = if rows_len == 0 {
        0
    } else {
        state.selected.min(rows_len - 1)
    };

    // Layout for table and footer
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // Create table header
    let header = ratatui::widgets::Row::new(
        T::HEADER_LABELS
            .iter()
            .map(|&label| ratatui::widgets::Cell::from(label))
            .collect::<Vec<_>>(),
    )
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // Create the table with component-specific constraints
    let table = Table::new(
        rows,
        T::COLUMN_CONSTRAINTS
            .iter()
            .chain(std::iter::once(&Constraint::Length(1))),
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(format!(
        "{} {}",
        title,
        component.table_title(sel, rows_len)
    )))
    .row_highlight_style(
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    // Render the table with state
    let mut table_state = TableState::default().with_selected(Some(sel));
    frame.render_stateful_widget(table, layout[0], &mut table_state);

    // Render scrollbar if content exceeds visible area
    render_scrollbar(frame, layout[0], sel, rows_len);

    // Render footer based on search state
    render_footer(frame, layout[1], &state.search, component.footer_hints());
}

/// Render vertical scrollbar only if content exceeds visible rows.
///
/// The scrollbar uses page-aware centering to keep the selected item
/// visible in the middle of the viewport when possible.
///
/// # Arguments
/// * `frame` - The ratatui frame to render to
/// * `table_area` - The area occupied by the table widget
/// * `selected` - Currently selected index
/// * `total_items` - Total number of items in the list
fn render_scrollbar(frame: &mut Frame<'_>, table_area: Rect, selected: usize, total_items: usize) {
    if total_items == 0 {
        return;
    }

    // Calculate visible rows (accounting for borders and header)
    let inner_area = table_area.inner(Margin::new(1, 2));
    let visible_rows = inner_area.height.saturating_sub(1) as usize; // -1 for header

    // Only show scrollbar if content exceeds visible area
    if total_items <= visible_rows {
        return;
    }

    // Compute page-aware scrollbar positions
    // Try to center the selected item in the viewport
    let max_top = total_items.saturating_sub(visible_rows);
    let centered_top = selected.saturating_sub(visible_rows.saturating_sub(1) / 2);
    let top_index = centered_top.min(max_top);
    let total_positions = max_top.saturating_add(1);

    let mut scroll_state = ScrollbarState::new(total_positions).position(top_index);

    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);

    frame.render_stateful_widget(scrollbar, inner_area, &mut scroll_state);
}

/// Render footer based on search state.
///
/// Three possible states:
/// 1. **Search Active (On)**: Shows "Search: " + query or placeholder
/// 2. **Search Applied**: Shows "Searched: xxx" + hint to clear
/// 3. **Normal Mode**: Shows keyboard hints + version
///
/// # Arguments
/// * `frame` - The ratatui frame to render to
/// * `footer_area` - The area for the footer
/// * `search` - Current search state
/// * `hints` - Keyboard hints to display in normal mode
fn render_footer(frame: &mut Frame<'_>, footer_area: Rect, search: &SearchState, hints: &str) {
    if search.is_on() {
        // Search mode: show search input with placeholder
        let mut spans = vec![Span::styled(
            "Search: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )];

        if search.query().is_empty() {
            spans.push(Span::styled(
                "Type to filter items",
                Style::default().fg(Color::DarkGray).dim(),
            ));
        } else {
            spans.push(Span::raw(search.query()));
        }

        let search_line =
            Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Reset));
        frame.render_widget(search_line, footer_area);
    } else if matches!(search, SearchState::Applied { .. }) {
        // Applied filter: show "Searched: xxx"
        let search_line = Paragraph::new(Line::from(vec![
            Span::styled(
                "Searched: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(search.query(), Style::default().fg(Color::Yellow)),
            Span::styled(
                "   Press Esc to clear",
                Style::default().fg(Color::DarkGray).dim(),
            ),
        ]))
        .style(Style::default().bg(Color::Reset));
        frame.render_widget(search_line, footer_area);
    } else {
        // Normal mode: show hints + version
        render_normal_footer(frame, footer_area, hints);
    }
}

/// Render normal mode footer with hints and version.
///
/// Layout: 80% hints (left-aligned) + 20% version (right-aligned)
fn render_normal_footer(frame: &mut Frame<'_>, footer_area: Rect, hints: &str) {
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
        .split(footer_area);

    let left = Paragraph::new(Line::from(Span::styled(
        hints,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(Alignment::Left);

    let right = Paragraph::new(Line::from(Span::styled(
        format!("TermiRs v{}", env!("CARGO_PKG_VERSION")),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM),
    )))
    .alignment(Alignment::Right);

    frame.render_widget(left, footer_layout[0]);
    frame.render_widget(right, footer_layout[1]);
}

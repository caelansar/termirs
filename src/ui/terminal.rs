use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::Instant;

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use wezterm_surface::CursorVisibility;
use wezterm_term::color::{ColorAttribute, ColorPalette};
use wezterm_term::config::TerminalConfiguration;
use wezterm_term::{Intensity, Terminal as WezTerminal, TerminalSize, Underline};

use crate::config::manager::DEFAULT_TERMINAL_SCROLLBACK_LINES;

/// Simple configuration for the wezterm terminal
#[derive(Debug)]
struct SimpleConfig {
    scrollback_size: usize,
}

impl TerminalConfiguration for SimpleConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }

    fn scrollback_size(&self) -> usize {
        self.scrollback_size
    }
}

/// Represents a single search match position in the terminal
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub row: usize,     // Absolute row (including scrollback)
    pub start_col: u16, // Start column
    pub end_col: u16,   // End column (exclusive)
}

/// Search state for terminal content
#[derive(Clone)]
pub struct TerminalSearch {
    pub query: String,
    pub active: bool,
    pub inputting: bool, // true = typing query, false = navigation mode
    pub matches: Vec<SearchMatch>,
    pub current_idx: usize,
    pub max_scrollback: usize, // cached max scrollback for coordinate conversion
    pub needs_initial_scroll: bool, // true when search is confirmed and needs auto-scroll check
    dirty: bool,
    last_query: String,
}

impl Default for TerminalSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalSearch {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            active: false,
            inputting: false,
            matches: Vec::new(),
            current_idx: 0,
            max_scrollback: 0,
            needs_initial_scroll: false,
            dirty: true,
            last_query: String::new(),
        }
    }

    /// Enter search mode (starts in input phase)
    pub fn enter(&mut self) {
        self.active = true;
        self.inputting = true;
        self.query.clear();
        self.matches.clear();
        self.current_idx = 0;
        self.needs_initial_scroll = false;
        self.dirty = true;
        self.last_query.clear();
    }

    /// Exit search mode completely
    pub fn exit(&mut self) {
        self.active = false;
        self.inputting = false;
        self.query.clear();
        self.matches.clear();
        self.current_idx = 0;
        self.needs_initial_scroll = false;
        self.dirty = true;
        self.last_query.clear();
    }

    /// Confirm query and enter navigation mode
    pub fn confirm(&mut self) {
        self.inputting = false;
        self.needs_initial_scroll = true;
    }

    /// Go back to input mode (e.g., to edit query)
    pub fn edit(&mut self) {
        self.inputting = true;
    }

    /// Check if in input phase
    pub fn is_inputting(&self) -> bool {
        self.active && self.inputting
    }

    /// Check if in navigation phase
    pub fn is_navigating(&self) -> bool {
        self.active && !self.inputting
    }

    /// Mark search as dirty (needs recomputation)
    pub fn mark_dirty(&mut self) {
        if self.active && !self.query.is_empty() {
            self.dirty = true;
        }
    }

    /// Check if search needs recomputation
    pub fn needs_update(&self) -> bool {
        self.active && self.dirty
    }

    /// Navigate to next match
    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_idx = (self.current_idx + 1) % self.matches.len();
        }
    }

    /// Navigate to previous match
    pub fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            self.current_idx = if self.current_idx == 0 {
                self.matches.len() - 1
            } else {
                self.current_idx - 1
            };
        }
    }

    /// Get current match if any
    pub fn current_match(&self) -> Option<&SearchMatch> {
        self.matches.get(self.current_idx)
    }

    /// Update query and mark dirty if changed
    pub fn set_query(&mut self, query: String) {
        if query != self.query {
            self.query = query;
            self.dirty = true;
        }
    }

    /// Append string to query
    pub fn push_str(&mut self, str: &str) {
        self.query.push_str(str);
        self.dirty = true;
    }

    /// Append character to query
    pub fn push_char(&mut self, ch: char) {
        self.query.push(ch);
        self.dirty = true;
    }

    /// Remove last character from query
    pub fn pop_char(&mut self) {
        if self.query.pop().is_some() {
            self.dirty = true;
        }
    }
}

/// Find all matches of a pattern in multi-line text.
///
/// Returns a vector of (line_index, start_col, end_col) for each match.
/// Uses smart case matching (case-insensitive if pattern is all lowercase).
///
/// # Arguments
/// * `text` - The multi-line text to search in
/// * `pattern` - The regex pattern to search for
///
/// # Returns
/// A vector of (line_index, start_col, end_col) tuples where line_index is 0-based
pub fn find_matches_in_text(text: &str, pattern: &str) -> Vec<(usize, u16, u16)> {
    if pattern.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let matcher = match RegexMatcherBuilder::new()
        .case_smart(true)
        .line_terminator(Some(b'\n'))
        .build(pattern)
    {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    let mut matches = Vec::new();
    let _ = Searcher::new().search_slice(
        &matcher,
        text.as_bytes(),
        UTF8(|line_num, line| {
            // line_num is 1-based, convert to 0-based
            let line_idx = (line_num as usize).saturating_sub(1);
            let mut start = 0;
            while let Ok(Some(mat)) = matcher.find(&line.as_bytes()[start..]) {
                let match_start = start + mat.start();
                let match_end = start + mat.end();
                matches.push((line_idx, match_start as u16, match_end as u16));
                start = match_start + 1;
                if start >= line.len() {
                    break;
                }
            }
            Ok(true)
        }),
    );

    matches
}

pub struct TerminalState {
    pub terminal: WezTerminal,
    pub last_change: Instant,
    pub search: TerminalSearch,
    cached_lines: Vec<Line<'static>>,
    row_hashes: Vec<u64>,
    cached_height: u16,
    cached_width: u16,
    cache_invalidated: bool,
    scrollback_limit: usize,
    /// Current scrollback offset (0 = at bottom, positive = scrolled up)
    scrollback_offset: usize,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::new_with_scrollback(rows, cols, DEFAULT_TERMINAL_SCROLLBACK_LINES)
    }

    pub fn new_with_scrollback(rows: u16, cols: u16, scrollback_limit: usize) -> Self {
        let limit = scrollback_limit.max(1);
        let size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        let config = Arc::new(SimpleConfig {
            scrollback_size: limit,
        });
        let terminal = WezTerminal::new(size, config, "termirs", "0.1", Box::new(std::io::sink()));

        Self {
            terminal,
            last_change: Instant::now(),
            search: TerminalSearch::new(),
            cached_lines: Vec::new(),
            row_hashes: Vec::new(),
            cached_height: 0,
            cached_width: 0,
            cache_invalidated: true,
            scrollback_limit: limit,
            scrollback_offset: 0,
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        self.terminal.resize(size);
        self.last_change = Instant::now();
        self.invalidate_cache();
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        self.terminal.advance_bytes(data);
        self.last_change = Instant::now();
        self.invalidate_cache();
        self.search.mark_dirty();
    }

    /// Get the current scrollback offset
    pub fn scrollback(&self) -> usize {
        self.scrollback_offset
    }

    /// Get the maximum scrollback available
    pub fn max_scrollback(&self) -> usize {
        let screen = self.terminal.screen();
        let total_lines = screen.scrollback_rows();
        let phys_rows = screen.physical_rows;
        total_lines.saturating_sub(phys_rows)
    }

    pub fn scroll_by(&mut self, delta_lines: i32) {
        let max_sb = self.max_scrollback();
        let current = self.scrollback_offset as i32;
        let target = current.saturating_add(delta_lines).max(0) as usize;
        self.scrollback_offset = target.min(max_sb);
        self.invalidate_cache();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scrollback_offset = 0;
        self.invalidate_cache();
    }

    #[allow(dead_code)]
    pub fn clear_history(&mut self) {
        let screen = self.terminal.screen();
        let rows = screen.physical_rows;
        let cols = screen.physical_cols;
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        let config = Arc::new(SimpleConfig {
            scrollback_size: self.scrollback_limit,
        });
        self.terminal = WezTerminal::new(size, config, "termirs", "0.1", Box::new(std::io::sink()));
        self.cached_lines.clear();
        self.row_hashes.clear();
        self.cached_height = 0;
        self.cached_width = 0;
        self.scrollback_offset = 0;
        self.invalidate_cache();
        self.last_change = Instant::now();
    }

    fn ensure_cache_dimensions(&mut self, height: u16, width: u16) {
        if self.cached_height != height || self.cached_width != width {
            self.cached_height = height;
            self.cached_width = width;
            self.cached_lines.resize(height as usize, Line::default());
            self.row_hashes.resize(height as usize, 0);
            self.invalidate_cache();
        }
    }

    fn invalidate_cache(&mut self) {
        self.cache_invalidated = true;
    }

    fn rebuild_cache(&mut self) {
        if !self.cache_invalidated {
            return;
        }
        let screen = self.terminal.screen();
        let height = self.cached_height as usize;
        let width = self.cached_width as usize;

        // Get the visible lines based on scrollback offset
        let total_lines = screen.scrollback_rows();
        let phys_rows = screen.physical_rows;
        let start_row = total_lines
            .saturating_sub(phys_rows)
            .saturating_sub(self.scrollback_offset);
        let end_row = start_row + height.min(phys_rows);

        let lines = screen.lines_in_phys_range(start_row..end_row);

        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx >= height {
                break;
            }
            let new_hash = compute_row_hash_wez(line, width);
            if self.row_hashes[row_idx] != new_hash || self.cache_invalidated {
                let built_line = build_line_wez(line, width);
                self.cached_lines[row_idx] = built_line;
                self.row_hashes[row_idx] = new_hash;
            }
        }
        self.cache_invalidated = false;
    }

    fn cached_lines(&mut self, height: u16, width: u16) -> &[Line<'static>] {
        self.ensure_cache_dimensions(height, width);
        self.rebuild_cache();
        &self.cached_lines
    }

    /// Run search on terminal content and update matches
    /// Searches ALL content including scrollback history
    pub fn update_search(&mut self) {
        self.update_search_with_finder(find_matches_in_text);
    }

    /// Run search on terminal content using a custom match finder function
    /// This allows for dependency injection in tests
    fn update_search_with_finder<F>(&mut self, find_matches: F)
    where
        F: Fn(&str, &str) -> Vec<(usize, u16, u16)>,
    {
        if !self.search.needs_update() {
            return;
        }

        self.search.matches.clear();
        self.search.dirty = false;

        if self.search.query.is_empty() {
            self.search.last_query.clear();
            return;
        }

        let screen = self.terminal.screen();
        let height = screen.physical_rows;
        let width = screen.physical_cols;

        // Get total lines in scrollback + visible
        let total_lines = screen.scrollback_rows();
        let max_scrollback = total_lines.saturating_sub(height);

        // Get all lines at once
        let all_lines = screen.lines_in_phys_range(0..total_lines);

        // Collect all row texts with their absolute row indices
        let mut row_texts: Vec<String> = Vec::with_capacity(total_lines);
        let mut abs_row_map: Vec<usize> = Vec::with_capacity(total_lines);

        for (abs_row, line) in all_lines.iter().enumerate() {
            let row_text = extract_line_text_wez(line, width);
            if !row_text.trim().is_empty() {
                row_texts.push(row_text);
                abs_row_map.push(abs_row);
            }
        }

        // Build the complete text with newlines and search once
        let full_text = row_texts.join("\n");
        let all_matches = find_matches(&full_text, &self.search.query);

        // Map line indices back to absolute row indices
        for (line_idx, start_col, end_col) in all_matches {
            if let Some(&abs_row) = abs_row_map.get(line_idx) {
                self.search.matches.push(SearchMatch {
                    row: abs_row,
                    start_col,
                    end_col,
                });
            }
        }

        // Preserve current index if valid, otherwise reset
        if self.search.current_idx >= self.search.matches.len() {
            self.search.current_idx = 0;
        }

        // Store max_scrollback for later use in coordinate conversion
        self.search.max_scrollback = max_scrollback;

        self.search.last_query = self.search.query.clone();
    }

    /// Scroll to make current match visible
    pub fn scroll_to_current_match(&mut self) {
        if let Some(mat) = self.search.current_match().cloned() {
            let max_scrollback = self.search.max_scrollback;

            // Calculate the scrollback position needed to show this match
            // Match at absolute row `mat.row` should be visible in the viewport
            if mat.row < max_scrollback {
                // Match is in scrollback history
                // To show row mat.row at the top of the screen:
                // scrollback = max_scrollback - mat.row
                let target_scrollback = max_scrollback - mat.row;
                self.scrollback_offset = target_scrollback;
            } else {
                // Match is in current screen area
                // Make sure we're scrolled to bottom to see it
                self.scrollback_offset = 0;
            }
            self.invalidate_cache();
        }
    }

    /// Check if any search match is visible in the current viewport
    pub fn has_visible_match(&self) -> bool {
        if self.search.matches.is_empty() {
            return false;
        }

        let max_scrollback = self.search.max_scrollback;
        let current_scrollback = self.scrollback_offset;
        let height = self.terminal.screen().physical_rows;

        // Calculate the range of absolute rows visible in the current view
        // When scrollback = S, we see rows from (max_scrollback - S) to (max_scrollback - S + height - 1)
        let view_start_abs = max_scrollback.saturating_sub(current_scrollback);
        let view_end_abs = view_start_abs + height;

        // Check if any match falls within the visible range
        self.search
            .matches
            .iter()
            .any(|mat| mat.row >= view_start_abs && mat.row < view_end_abs)
    }

    /// Get the maximum scrollback value (for coordinate calculations)
    pub fn get_max_scrollback(&self) -> usize {
        self.search.max_scrollback
    }

    /// Get screen size as (rows, cols)
    pub fn screen_size(&self) -> (u16, u16) {
        let screen = self.terminal.screen();
        (screen.physical_rows as u16, screen.physical_cols as u16)
    }

    /// Check if alternate screen is active
    pub fn is_alternate_screen(&self) -> bool {
        self.terminal.is_alt_screen_active()
    }

    /// Check if application cursor keys mode is active
    pub fn application_cursor_keys(&self) -> bool {
        self.terminal.get_application_cursor_keys()
    }

    /// Get cursor position as (row, col)
    pub fn cursor_position(&self) -> (u16, u16) {
        let cursor = self.terminal.cursor_pos();
        (cursor.y as u16, cursor.x as u16)
    }

    /// Check if cursor should be hidden
    pub fn hide_cursor(&self) -> bool {
        self.terminal.cursor_pos().visibility != CursorVisibility::Visible
    }
}

/// Convert wezterm ColorAttribute to ratatui Color
fn map_color_wez(color: &ColorAttribute) -> Color {
    match color {
        ColorAttribute::Default => Color::Reset,
        ColorAttribute::PaletteIndex(idx) => Color::Indexed(*idx),
        ColorAttribute::TrueColorWithDefaultFallback(c)
        | ColorAttribute::TrueColorWithPaletteFallback(c, _) => {
            let (r, g, b, _) = c.to_tuple_rgba();
            Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        }
    }
}

/// Extract text content from a wezterm Line
fn extract_line_text_wez(line: &wezterm_term::Line, width: usize) -> String {
    let mut text = String::with_capacity(width);
    let mut col = 0usize;

    for cell in line.visible_cells() {
        if col >= width {
            break;
        }

        let cell_width = cell.width();
        let cell_text = cell.str();

        if cell_text.is_empty() || cell_text == " " {
            text.push(' ');
        } else {
            text.push_str(cell_text);
        }

        col += cell_width;
    }

    // Pad with spaces to reach width
    while text.len() < width {
        text.push(' ');
    }

    text
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalSelection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

pub fn draw_terminal(
    area: Rect,
    state: &mut TerminalState,
    name: &str,
    frame: &mut ratatui::Frame<'_>,
    selection: Option<TerminalSelection>,
) {
    // Update search matches if needed
    state.update_search();

    // Auto-scroll to first match if search was just confirmed and no matches are visible
    if state.search.needs_initial_scroll && !state.search.matches.is_empty() {
        if !state.has_visible_match() {
            state.scroll_to_current_match();
        }
        state.search.needs_initial_scroll = false;
    }

    // Build title based on search state
    let (title, title_style) = if state.search.active {
        let search = &state.search;
        let title_text = if search.is_inputting() {
            if search.query.is_empty() {
                " SEARCHING: _ ".to_string()
            } else {
                format!(" SEARCHING: {}_ ", search.query)
            }
        } else {
            // Navigation mode
            if search.matches.is_empty() {
                format!(" SEARCHING: {} (no matches) ", search.query)
            } else {
                format!(
                    " SEARCHING: {} ({}/{}) ",
                    search.query,
                    search.current_idx + 1,
                    search.matches.len()
                )
            }
        };
        let style = Style::default()
            .fg(Color::White)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        (title_text, style)
    } else {
        (
            format!("Connected to {name}"),
            Style::default().fg(Color::Cyan),
        )
    };

    // Render the block with appropriate title
    let mut term_block = Block::default()
        .borders(Borders::TOP)
        .title(Span::styled(title, title_style));
    if state.search.active {
        term_block = term_block.border_style(Style::default().fg(Color::Yellow));
    } else {
        term_block = term_block.border_style(Style::default().fg(Color::Cyan));
    }

    frame.render_widget(&term_block, area);

    // Get the inner area for terminal content
    let inner = term_block.inner(area);
    let height = inner.height;
    let width = inner.width;
    let (cur_row, cur_col) = state.cursor_position();
    let hide_cursor = state.hide_cursor();
    let lines = state.cached_lines(height, width);

    // Render terminal rows using a lightweight cached widget
    let widget = CachedTerminalWidget { lines };
    frame.render_widget(widget, inner);

    // Highlight search matches
    if state.search.active && !state.search.matches.is_empty() {
        highlight_search_matches(frame.buffer_mut(), inner, state);
    }

    if let Some(selection) = selection {
        highlight_selection(frame.buffer_mut(), inner, selection);
    }

    if !hide_cursor && !state.search.active {
        // Use inner area coordinates (already accounts for borders)
        let cursor_x = inner.x + cur_col;
        let cursor_y = inner.y + cur_row;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Highlight search matches in the terminal buffer
fn highlight_search_matches(buf: &mut Buffer, area: Rect, state: &TerminalState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let current_match_idx = state.search.current_idx;
    let max_scrollback = state.search.max_scrollback;
    let current_scrollback = state.scrollback_offset;
    let height = area.height as usize;

    // Calculate the range of absolute rows visible in the current view
    // When scrollback = S, we see rows from (max_scrollback - S) to (max_scrollback - S + height - 1)
    let view_start_abs = max_scrollback.saturating_sub(current_scrollback);
    let view_end_abs = view_start_abs + height;

    // First pass: render all non-current matches with Yellow
    for (idx, mat) in state.search.matches.iter().enumerate() {
        if idx == current_match_idx {
            continue; // Skip current match in first pass
        }

        // Check if this match is in the current view
        if mat.row < view_start_abs || mat.row >= view_end_abs {
            continue;
        }

        // Convert absolute row to view row
        let view_row = (mat.row - view_start_abs) as u16;
        let y = area.y + view_row;

        // Clamp columns to area bounds
        let start_col = mat.start_col.min(area.width.saturating_sub(1));
        let end_col = mat.end_col.min(area.width);

        if start_col >= end_col {
            continue;
        }

        for col in start_col..end_col {
            let x = area.x + col;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(Color::Yellow);
                cell.set_fg(Color::Black);
            }
        }
    }

    // Second pass: render current match with LightGreen (ensures it overrides Yellow)
    if let Some(mat) = state.search.matches.get(current_match_idx) {
        // Check if current match is in the current view
        if mat.row >= view_start_abs && mat.row < view_end_abs {
            let view_row = (mat.row - view_start_abs) as u16;
            let y = area.y + view_row;

            let start_col = mat.start_col.min(area.width.saturating_sub(1));
            let end_col = mat.end_col.min(area.width);

            if start_col < end_col {
                for col in start_col..end_col {
                    let x = area.x + col;
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(Color::LightGreen);
                        cell.set_fg(Color::Black);
                    }
                }
            }
        }
    }
}

struct CachedTerminalWidget<'a> {
    lines: &'a [Line<'static>],
}

impl<'a> Widget for CachedTerminalWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let height = area.height.min(self.lines.len() as u16);
        for row in 0..height {
            let line = &self.lines[row as usize];
            buf.set_line(area.x, area.y + row, line, area.width);
        }
    }
}

fn highlight_selection(buf: &mut Buffer, area: Rect, selection: TerminalSelection) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let start_row = selection.start_row.min(area.height.saturating_sub(1));
    let end_row = selection.end_row.min(area.height.saturating_sub(1));

    for row in start_row..=end_row {
        let (col_start, col_end) = selection_bounds_for_row(selection, row, area.width);
        if col_start >= col_end {
            continue;
        }
        let y = area.y + row;
        let x_start = area.x + col_start;
        let x_end = area.x + col_end;
        for x in x_start..x_end {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(Color::DarkGray);
            }
        }
    }
}

fn selection_bounds_for_row(selection: TerminalSelection, row: u16, width: u16) -> (u16, u16) {
    let mut start_col = 0;
    let mut end_col = width;

    if row == selection.start_row {
        start_col = selection.start_col.min(width);
    }
    if row == selection.end_row {
        end_col = selection.end_col.min(width);
    }

    // Handle selection entirely within one row
    if selection.start_row == selection.end_row {
        start_col = selection.start_col.min(width);
        end_col = selection.end_col.min(width);
    }

    (start_col, end_col)
}

/// Compute a hash for a wezterm Line for cache invalidation
fn compute_row_hash_wez(line: &wezterm_term::Line, width: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut col = 0usize;

    for cell in line.visible_cells() {
        if col >= width {
            break;
        }

        let attrs = cell.attrs();
        hash_color_wez(&mut hasher, &attrs.foreground());
        hash_color_wez(&mut hasher, &attrs.background());
        hasher.write_u8((attrs.intensity() == Intensity::Bold) as u8);
        hasher.write_u8(attrs.italic() as u8);
        hasher.write_u8((attrs.underline() != Underline::None) as u8);
        hasher.write_u8(attrs.reverse() as u8);
        hasher.write_u8((attrs.intensity() == Intensity::Half) as u8);

        let contents = cell.str();
        hasher.write_usize(contents.len());
        hasher.write(contents.as_bytes());

        col += cell.width();
    }

    // Hash remaining empty columns
    while col < width {
        hasher.write_u8(0);
        col += 1;
    }

    hasher.finish()
}

/// Hash a wezterm ColorAttribute
fn hash_color_wez(hasher: &mut DefaultHasher, color: &ColorAttribute) {
    match color {
        ColorAttribute::Default => hasher.write_u8(0),
        ColorAttribute::PaletteIndex(n) => {
            hasher.write_u8(1);
            hasher.write_u8(*n);
        }
        ColorAttribute::TrueColorWithDefaultFallback(c)
        | ColorAttribute::TrueColorWithPaletteFallback(c, _) => {
            hasher.write_u8(2);
            let (r, g, b, _) = c.to_tuple_rgba();
            hasher.write_u8((r * 255.0) as u8);
            hasher.write_u8((g * 255.0) as u8);
            hasher.write_u8((b * 255.0) as u8);
        }
    }
}

/// Build a ratatui Line from a wezterm Line
fn build_line_wez(line: &wezterm_term::Line, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default();
    let mut current_text = String::new();
    let mut col = 0usize;

    for cell in line.visible_cells() {
        if col >= width {
            break;
        }

        let cell_width = cell.width();
        let attrs = cell.attrs();

        let fg = map_color_wez(&attrs.foreground());
        let bg = map_color_wez(&attrs.background());
        let bold = attrs.intensity() == Intensity::Bold;
        let italic = attrs.italic();
        let underline = attrs.underline() != Underline::None;
        let inverse = attrs.reverse();
        let dim = attrs.intensity() == Intensity::Half;

        let mut style = Style::default().fg(fg).bg(bg);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if dim {
            style = style.add_modifier(Modifier::DIM);
        }
        if inverse {
            style = style.add_modifier(Modifier::REVERSED);
        }

        let contents = cell.str();
        let to_append = if contents.is_empty() { " " } else { contents };

        if style == current_style {
            current_text.push_str(to_append);
        } else {
            if !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(to_append);
        }

        // For wide characters, account for the extra column
        for _ in 1..cell_width {
            if col + 1 < width {
                // Wide character continuation - no additional text needed
            }
        }

        col += cell_width;
    }

    // Fill remaining columns with spaces
    while col < width {
        if current_style == Style::default() {
            current_text.push(' ');
        } else {
            if !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = Style::default();
            current_text.push(' ');
        }
        col += 1;
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, current_style));
    }

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "profiling helper; run explicitly when needed"]
    fn profile_dirty_row_cache_under_sustained_output() {
        let mut state = TerminalState::new(24, 80);
        let line = format!("{}\r\n", "ping output with ansi ".repeat(4));
        let payload = line.repeat(10);
        let iterations = 5_000;

        // Warm-up to populate cache
        state.process_bytes(payload.as_bytes());
        let _ = state.cached_lines(24, 80);

        let start_active = Instant::now();
        for _ in 0..iterations {
            state.process_bytes(payload.as_bytes());
            let _ = state.cached_lines(24, 80);
        }
        let active_duration = start_active.elapsed();

        // Measure cost when no new bytes arrive but redraws continue
        let start_idle = Instant::now();
        for _ in 0..iterations {
            let _ = state.cached_lines(24, 80);
        }
        let idle_duration = start_idle.elapsed();

        println!(
            "sustained-output: {} iterations with updates in {:?} ({:.4?}/iter); idle redraws in {:?} ({:.4?}/iter)",
            iterations,
            active_duration,
            active_duration / iterations,
            idle_duration,
            idle_duration / iterations
        );
    }

    #[test]
    fn test_find_matches_in_text_single_match() {
        let text = "hello world";
        let matches = find_matches_in_text(text, "world");
        // (line_idx, start_col, end_col)
        assert_eq!(matches, vec![(0, 6, 11)]);
    }

    #[test]
    fn test_find_matches_in_text_multiline() {
        let text = "hello world\nwow hello world";
        let matches = find_matches_in_text(text, "world");
        // Line 0: "hello world" -> match at 6-11
        // Line 1: "wow hello world" -> match at 10-15
        assert_eq!(matches, vec![(0, 6, 11), (1, 10, 15)]);
    }

    #[test]
    fn test_find_matches_in_text_multiple_matches() {
        let text = "foo bar foo baz foo";
        let matches = find_matches_in_text(text, "foo");
        assert_eq!(matches, vec![(0, 0, 3), (0, 8, 11), (0, 16, 19)]);
    }

    #[test]
    fn test_find_matches_in_text_no_matches() {
        let text = "hello world";
        let matches = find_matches_in_text(text, "xyz");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_in_text_empty_pattern() {
        let text = "hello world";
        let matches = find_matches_in_text(text, "");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_in_text_empty_text() {
        let matches = find_matches_in_text("", "pattern");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_in_text_case_insensitive() {
        // Smart case: lowercase pattern should match case-insensitively
        let text = "Hello HELLO hello";
        let matches = find_matches_in_text(text, "hello");
        assert_eq!(matches, vec![(0, 0, 5), (0, 6, 11), (0, 12, 17)]);
    }

    #[test]
    fn test_find_matches_in_text_case_sensitive_with_uppercase() {
        // Smart case: pattern with uppercase should match case-sensitively
        let text = "Hello HELLO hello";
        let matches = find_matches_in_text(text, "Hello");
        assert_eq!(matches, vec![(0, 0, 5)]);
    }

    #[test]
    fn test_find_matches_in_text_regex_pattern() {
        let text = "foo123 bar456 baz789";
        let matches = find_matches_in_text(text, r"\d+");
        // The implementation finds overlapping matches by advancing 1 byte after each match
        assert_eq!(
            matches,
            vec![
                (0, 3, 6),
                (0, 4, 6),
                (0, 5, 6),
                (0, 10, 13),
                (0, 11, 13),
                (0, 12, 13),
                (0, 17, 20),
                (0, 18, 20),
                (0, 19, 20)
            ]
        );
    }

    #[test]
    fn test_find_matches_in_text_overlapping_potential() {
        // Pattern "aa" in "aaaa" - should find non-overlapping matches
        let text = "aaaa";
        let matches = find_matches_in_text(text, "aa");
        // With start = match_start + 1, we get overlapping positions
        assert_eq!(matches, vec![(0, 0, 2), (0, 1, 3), (0, 2, 4)]);
    }

    #[test]
    fn test_find_matches_in_text_special_chars() {
        let text = "path/to/file.rs";
        let matches = find_matches_in_text(text, r"\.rs");
        assert_eq!(matches, vec![(0, 12, 15)]);
    }

    #[test]
    fn test_find_matches_in_text_invalid_regex() {
        let text = "hello world";
        // Invalid regex pattern should return empty
        let matches = find_matches_in_text(text, "[invalid");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matches_in_text_word_boundary() {
        let text = "the them there";
        let matches = find_matches_in_text(text, r"\bthe\b");
        assert_eq!(matches, vec![(0, 0, 3)]);
    }

    #[test]
    fn test_find_matches_in_text_at_boundaries() {
        // Test matching at start and end of text
        let text = "foo bar foo";
        let matches = find_matches_in_text(text, "foo");
        assert_eq!(matches, vec![(0, 0, 3), (0, 8, 11)]);
    }

    #[test]
    fn test_find_matches_in_text_whole_text() {
        let text = "exact";
        let matches = find_matches_in_text(text, "exact");
        assert_eq!(matches, vec![(0, 0, 5)]);
    }

    #[test]
    fn test_find_matches_in_text_multiline_multiple_matches() {
        let text = "foo bar\nbaz foo\nfoo end";
        let matches = find_matches_in_text(text, "foo");
        // Line 0: "foo bar" -> match at 0-3
        // Line 1: "baz foo" -> match at 4-7
        // Line 2: "foo end" -> match at 0-3
        assert_eq!(matches, vec![(0, 0, 3), (1, 4, 7), (2, 0, 3)]);
    }
}

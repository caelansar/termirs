use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::time::Instant;

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::Searcher;
use grep_searcher::sinks::UTF8;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use ratatui::{buffer::Buffer, style::Style as RatStyle};
use vt100::{Color as VtColor, Parser};

use crate::config::manager::DEFAULT_TERMINAL_SCROLLBACK_LINES;

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
            while let Ok(Some(mat)) = matcher.find(line[start..].as_bytes()) {
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
    pub parser: Parser,
    pub last_change: Instant,
    pub search: TerminalSearch,
    cached_lines: Vec<Line<'static>>,
    row_hashes: Vec<u64>,
    cached_height: u16,
    cached_width: u16,
    cache_invalidated: bool,
    scrollback_limit: usize,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::new_with_scrollback(rows, cols, DEFAULT_TERMINAL_SCROLLBACK_LINES)
    }

    pub fn new_with_scrollback(rows: u16, cols: u16, scrollback_limit: usize) -> Self {
        let limit = scrollback_limit.max(1);
        Self {
            parser: Parser::new(rows, cols, limit),
            last_change: Instant::now(),
            search: TerminalSearch::new(),
            cached_lines: Vec::new(),
            row_hashes: Vec::new(),
            cached_height: 0,
            cached_width: 0,
            cache_invalidated: true,
            scrollback_limit: limit,
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.last_change = Instant::now();
        self.invalidate_cache();
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.last_change = Instant::now();
        self.invalidate_cache();
        self.search.mark_dirty();
    }

    pub fn scroll_by(&mut self, delta_lines: i32) {
        let current = self.parser.screen().scrollback() as i32;
        let target = current.saturating_add(delta_lines).max(0) as usize;
        self.parser.screen_mut().set_scrollback(target);
        self.invalidate_cache();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
        self.invalidate_cache();
    }

    #[allow(dead_code)]
    pub fn clear_history(&mut self) {
        let (rows, cols) = self.parser.screen().size();
        self.parser = Parser::new(rows, cols, self.scrollback_limit);
        self.cached_lines.clear();
        self.row_hashes.clear();
        self.cached_height = 0;
        self.cached_width = 0;
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
        let screen = self.parser.screen();
        let height = self.cached_height;
        let width = self.cached_width;
        for row in 0..height {
            let row_idx = row as usize;
            let new_hash = compute_row_hash(screen, row, width);
            if self.row_hashes[row_idx] != new_hash || self.cache_invalidated {
                let line = build_line(screen, row, width);
                self.cached_lines[row_idx] = line;
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

        let (height, width) = self.parser.screen().size();

        // Save current scrollback position
        let original_scrollback = self.parser.screen().scrollback();

        // Find the maximum scrollback (total scrollback buffer length)
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max_scrollback = self.parser.screen().scrollback();

        // Calculate total rows: scrollback buffer + visible screen
        let total_rows = max_scrollback + height as usize;

        // Collect all row texts with their absolute row indices
        // This allows us to search the entire content at once
        let mut row_texts: Vec<String> = Vec::with_capacity(total_rows);
        let mut abs_row_map: Vec<usize> = Vec::with_capacity(total_rows);

        for abs_row in 0..total_rows {
            // Calculate which scrollback position and view row we need
            let (scrollback_pos, view_row) = if abs_row < max_scrollback {
                // This row is in scrollback history
                let sb = max_scrollback - abs_row;
                (sb, 0u16)
            } else {
                // This row is in the current screen
                (0, (abs_row - max_scrollback) as u16)
            };

            // Set scrollback to access this row
            self.parser.screen_mut().set_scrollback(scrollback_pos);

            let row_text = extract_visible_row_text(self.parser.screen(), view_row, width);
            if !row_text.trim().is_empty() {
                row_texts.push(row_text);
                abs_row_map.push(abs_row);
            }
        }

        // Restore original scrollback position
        self.parser.screen_mut().set_scrollback(original_scrollback);

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
                self.parser.screen_mut().set_scrollback(target_scrollback);
            } else {
                // Match is in current screen area
                // Make sure we're scrolled to bottom to see it
                self.parser.screen_mut().set_scrollback(0);
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
        let current_scrollback = self.parser.screen().scrollback();
        let height = self.parser.screen().size().0 as usize;

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
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(n) => Color::Indexed(n),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Extract text content from a visible terminal row
fn extract_visible_row_text(screen: &vt100::Screen, row: u16, width: u16) -> String {
    let mut text = String::with_capacity(width as usize);

    for col in 0..width {
        if let Some(cell) = screen.cell(row, col) {
            // Skip wide character continuation cells
            if cell.is_wide_continuation() {
                continue;
            }
            let contents = cell.contents();
            if contents.is_empty() {
                text.push(' ');
            } else {
                text.push_str(contents);
            }
        } else {
            text.push(' ');
        }
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
    let screen = state.parser.screen();
    let (cur_row, cur_col) = screen.cursor_position();
    let hide_cursor = screen.hide_cursor();
    let lines = state.cached_lines(height, width);

    // Render terminal rows using a lightweight cached widget
    let widget = CachedTerminalWidget { lines };
    frame.render_widget(widget.bg(Color::Red), inner);

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
    let current_scrollback = state.parser.screen().scrollback();
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

impl<'a> CachedTerminalWidget<'a> {
    fn bg(self, color: Color) -> CachedTerminalWidgetWithBg<'a> {
        CachedTerminalWidgetWithBg {
            inner: self,
            background: color,
        }
    }
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

struct CachedTerminalWidgetWithBg<'a> {
    inner: CachedTerminalWidget<'a>,
    background: Color,
}

impl<'a> Widget for CachedTerminalWidgetWithBg<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, RatStyle::default().bg(self.background));
        self.inner.render(area, buf);
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

fn compute_row_hash(screen: &vt100::Screen, row: u16, width: u16) -> u64 {
    let mut hasher = DefaultHasher::new();
    for col in 0..width {
        match screen.cell(row, col) {
            Some(cell) => {
                hash_color(&mut hasher, cell.fgcolor());
                hash_color(&mut hasher, cell.bgcolor());
                hasher.write_u8(cell.bold() as u8);
                hasher.write_u8(cell.italic() as u8);
                hasher.write_u8(cell.underline() as u8);
                hasher.write_u8(cell.inverse() as u8);
                hasher.write_u8(cell.dim() as u8);
                let contents = cell.contents();
                hasher.write_usize(contents.len());
                hasher.write(contents.as_bytes());
            }
            None => {
                hasher.write_u8(0);
            }
        }
    }
    hasher.finish()
}

fn hash_color(hasher: &mut DefaultHasher, color: VtColor) {
    match color {
        VtColor::Default => hasher.write_u8(0),
        VtColor::Idx(n) => {
            hasher.write_u8(1);
            hasher.write_u8(n);
        }
        VtColor::Rgb(r, g, b) => {
            hasher.write_u8(2);
            hasher.write_u8(r);
            hasher.write_u8(g);
            hasher.write_u8(b);
        }
    }
}

fn build_line(screen: &vt100::Screen, row: u16, width: u16) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default();
    let mut current_text = String::new();

    for col in 0..width {
        if let Some(cell) = screen.cell(row, col) {
            // Skip wide character continuation cells to render the actual wide character once
            if cell.is_wide_continuation() {
                continue;
            }
            let fg = map_color(cell.fgcolor());
            let bg = map_color(cell.bgcolor());
            let bold = cell.bold();
            let italic = cell.italic();
            let underline = cell.underline();
            let inverse = cell.inverse();
            let dim = cell.dim();

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

            let contents = cell.contents();
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
        } else if current_style == Style::default() {
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

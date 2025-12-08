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
        self.dirty = true;
        self.last_query.clear();
    }

    /// Confirm query and enter navigation mode
    pub fn confirm(&mut self) {
        self.inputting = false;
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
        if !self.search.needs_update() {
            return;
        }

        self.search.matches.clear();
        self.search.dirty = false;

        if self.search.query.is_empty() {
            self.search.last_query.clear();
            return;
        }

        // Build regex matcher - escape special characters to treat as literal search
        let escaped_query = escape_regex_special_chars(&self.search.query);
        let matcher = match RegexMatcherBuilder::new()
            .case_smart(true)
            .line_terminator(Some(b'\n'))
            .build(&escaped_query)
        {
            Ok(m) => m,
            Err(_) => {
                // If even escaped pattern fails, give up
                return;
            }
        };

        let (height, width) = self.parser.screen().size();

        // Save current scrollback position
        let original_scrollback = self.parser.screen().scrollback();

        // Find the maximum scrollback (total scrollback buffer length)
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max_scrollback = self.parser.screen().scrollback();

        // Calculate total rows: scrollback buffer + visible screen
        let total_rows = max_scrollback + height as usize;

        // Search through ALL content by iterating with different scrollback positions
        // We'll search from oldest (top) to newest (bottom)
        for abs_row in 0..total_rows {
            // Calculate which scrollback position and view row we need
            let (scrollback_pos, view_row) = if abs_row < max_scrollback {
                // This row is in scrollback history
                // To see row abs_row, we need scrollback = max_scrollback - abs_row
                // and view_row = 0
                let sb = max_scrollback - abs_row;
                (sb, 0u16)
            } else {
                // This row is in the current screen
                // scrollback = 0, view_row = abs_row - max_scrollback
                (0, (abs_row - max_scrollback) as u16)
            };

            // Set scrollback to access this row
            self.parser.screen_mut().set_scrollback(scrollback_pos);

            let row_text = extract_visible_row_text(self.parser.screen(), view_row, width);
            if row_text.trim().is_empty() {
                continue;
            }

            // Search this row using grep_searcher
            let mut row_matches: Vec<(u16, u16)> = Vec::new();
            let _ = Searcher::new().search_slice(
                &matcher,
                row_text.as_bytes(),
                UTF8(|_line_num, line| {
                    let mut start = 0;
                    while let Ok(Some(mat)) = matcher.find(line[start..].as_bytes()) {
                        let match_start = start + mat.start();
                        let match_end = start + mat.end();
                        row_matches.push((match_start as u16, match_end as u16));
                        start = match_start + 1;
                        if start >= line.len() {
                            break;
                        }
                    }
                    Ok(true)
                }),
            );

            // Add matches with absolute row index
            for (start_col, end_col) in row_matches {
                self.search.matches.push(SearchMatch {
                    row: abs_row,
                    start_col,
                    end_col,
                });
            }
        }

        // Restore original scrollback position
        self.parser.screen_mut().set_scrollback(original_scrollback);

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

/// Escape regex special characters to treat pattern as literal
fn escape_regex_special_chars(pattern: &str) -> String {
    let mut escaped = String::with_capacity(pattern.len() * 2);
    for ch in pattern.chars() {
        match ch {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
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

    for (idx, mat) in state.search.matches.iter().enumerate() {
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

        // Use different highlight for current match vs other matches
        let highlight_color = if idx == current_match_idx {
            Color::LightGreen // Current match - bright green
        } else {
            Color::Yellow // Other matches - yellow
        };

        for col in start_col..end_col {
            let x = area.x + col;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(highlight_color);
                cell.set_fg(Color::Black);
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
}

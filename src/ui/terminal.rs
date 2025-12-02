use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use ratatui::{buffer::Buffer, style::Style as RatStyle};
use vt100::{Color as VtColor, Parser};

use crate::config::manager::DEFAULT_TERMINAL_SCROLLBACK_LINES;

pub struct TerminalState {
    pub parser: Parser,
    pub last_change: Instant,
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
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(n) => Color::Indexed(n),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
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
    // Render the block separately
    let term_block = Block::default()
        .borders(Borders::TOP)
        .title(format!("Connected to {name}"))
        .fg(Color::Cyan);

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

    if let Some(selection) = selection {
        highlight_selection(frame.buffer_mut(), inner, selection);
    }

    if !hide_cursor {
        // Use inner area coordinates (already accounts for borders)
        let cursor_x = inner.x + cur_col;
        let cursor_y = inner.y + cur_row;
        frame.set_cursor_position((cursor_x, cursor_y));
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

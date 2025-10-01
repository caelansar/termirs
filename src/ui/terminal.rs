use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use vt100::{Color as VtColor, Parser};

pub struct TerminalState {
    pub parser: Parser,
    pub last_change: Instant,
}

impl TerminalState {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 10_000),
            last_change: Instant::now(),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.last_change = Instant::now();
    }

    pub fn process_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.last_change = Instant::now();
    }

    pub fn scroll_by(&mut self, delta_lines: i32) {
        let current = self.parser.screen().scrollback() as i32;
        let target = current.saturating_add(delta_lines).max(0) as usize;
        self.parser.screen_mut().set_scrollback(target);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(n) => Color::Indexed(n),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn draw_terminal(
    area: Rect,
    state: &TerminalState,
    name: &str,
    frame: &mut ratatui::Frame<'_>,
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
    let mut lines: Vec<Line> = Vec::with_capacity(height as usize);
    let screen = state.parser.screen();

    for row in 0..height {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();

        for col in 0..width {
            if let Some(cell) = screen.cell(row, col) {
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
                    // Apply reverse video using a style modifier so default colors are inverted correctly
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
        // eprintln!("row: {}, spans: {:?}", row, spans);
        lines.push(Line::from(spans));
    }

    // Render paragraph directly in the inner area (without block)
    let para = Paragraph::new(lines);
    frame.render_widget(para.bg(Color::Red), inner);

    let (cur_row, cur_col) = screen.cursor_position();
    if !screen.hide_cursor() {
        // Use inner area coordinates (already accounts for borders)
        let cursor_x = inner.x + cur_col;
        let cursor_y = inner.y + cur_row;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

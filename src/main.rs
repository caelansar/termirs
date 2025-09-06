use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use vt100::{Parser, Color as VtColor};

struct TerminalState {
    parser: Parser,
    // last time we requested redraw due to output
    last_change: Instant,
}

impl TerminalState {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
            last_change: Instant::now(),
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.last_change = Instant::now();
    }

    fn process_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.last_change = Instant::now();
    }
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(n) => Color::Indexed(n),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn draw_terminal(area: Rect, state: &TerminalState, frame: &mut ratatui::Frame<'_>) {
    // Compose styled lines from vt100 screen cells to preserve colors and attributes
    let height = area.height;
    let width = area.width;
    let mut lines: Vec<Line> = Vec::with_capacity(height as usize);
    let screen = state.parser.screen();

    for row in 0..height {
        let mut spans: Vec<Span> = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();

        for col in 0..width {
            if let Some(cell) = screen.cell(row, col) {
                let mut fg = map_color(cell.fgcolor());
                let mut bg = map_color(cell.bgcolor());
                let bold = cell.bold();
                let italic = cell.italic();
                let underline = cell.underline();
                let inverse = cell.inverse();

                if inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let mut style = Style::default().fg(fg).bg(bg);
                if bold { style = style.add_modifier(Modifier::BOLD); }
                if italic { style = style.add_modifier(Modifier::ITALIC); }
                if underline { style = style.add_modifier(Modifier::UNDERLINED); }

                let contents = cell.contents();

                if style == current_style {
                    current_text.push_str(contents);
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = style;
                    current_text.push_str(contents);
                }
            } else {
                // out of bounds -> fill space
                if current_style == Style::default() {
                    current_text.push(' ');
                } else {
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = Style::default();
                    current_text.push(' ');
                }
            }
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }
        lines.push(Line::from(spans));
    }

    let term_block = Block::default().borders(Borders::ALL).title("$ shell");
    let para = Paragraph::new(lines).block(term_block);
    frame.render_widget(para, area);

    // Draw cursor if visible
    let (cur_row, cur_col) = screen.cursor_position();
    if !screen.hide_cursor() {
        // convert to frame coordinates (inside the area, account for borders drawn by Block)
        let cursor_x = area.x + 1 + cur_col;
        let cursor_y = area.y + 1 + cur_row;
        frame.set_cursor(cursor_x, cursor_y);
    }
}

fn main() -> Result<()> {
    // Setup Crossterm terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Spawn a login shell inside a PTY
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(
            PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            }
        )
        .context("open PTY")?;

    let cmd = if let Ok(shell) = std::env::var("SHELL") { CommandBuilder::new(shell) } else { CommandBuilder::new("/bin/bash") };
    // Start the child process attached to the PTY slave
    let child = pty_pair
        .slave
        .spawn_command(cmd)
        .context("spawn shell in PTY")?;

    // We'll read from the PTY master in a background thread
    let mut reader = pty_pair.master.try_clone_reader()?;
    let mut writer = pty_pair.master.take_writer()?;

    // Shared vt100 state
    let app_state = Arc::new(Mutex::new(TerminalState::new(30, 100)));

    // Reader thread: pump PTY bytes into vt100 parser
    let app_state_reader = app_state.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut guard) = app_state_reader.lock() {
                        guard.process_bytes(&buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    // UI event/render loop
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(16);

    loop {
        terminal.draw(|f| {
            let size = f.size();
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .split(size);

            // Title area
            let title_block = Block::default()
                .borders(Borders::ALL)
                .title(Line::from("title").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
            f.render_widget(title_block, layout[0]);

            // Terminal area (inner without borders affects vt100 size)
            let inner = layout[1].inner(Margin::new(1, 1));
            // Ensure vt100 knows current size
            if let Ok(mut guard) = app_state.lock() {
                if guard.parser.screen().size() != (inner.height, inner.width) {
                    guard.resize(inner.height, inner.width);
                    // also send resize to PTY
                    let _ = pty_pair.master.resize(PtySize {
                        rows: inner.height,
                        cols: inner.width,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                draw_terminal(layout[1], &guard, f);
            }
        })?;

        // Input handling
        while crossterm::event::poll(Duration::from_millis(1))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Map a few special keys; otherwise forward UTF-8
                    match key.code {
                        KeyCode::Esc => {
                            // Exit only when not in alternate screen (i.e., likely at main shell)
                            let in_alt = app_state
                                .lock()
                                .ok()
                                .map(|g| g.parser.screen().alternate_screen())
                                .unwrap_or(false);
                            if in_alt {
                                writer.write_all(&[0x1b])?; // forward ESC to app like vim
                                writer.flush()?;
                            } else {
                                disable_raw_mode().ok();
                                execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
                                drop(child);
                                return Ok(());
                            }
                        }
                        KeyCode::Enter => {
                            writer.write_all(b"\r")?;
                            writer.flush()?;
                        }
                        KeyCode::Backspace => {
                            writer.write_all(&[0x7f])?; // DEL
                            writer.flush()?;
                        }
                        KeyCode::Left => { writer.write_all(b"\x1b[D")?; }
                        KeyCode::Right => { writer.write_all(b"\x1b[C")?; }
                        KeyCode::Up => { writer.write_all(b"\x1b[A")?; }
                        KeyCode::Down => { writer.write_all(b"\x1b[B")?; }
                        KeyCode::Tab => { writer.write_all(b"\t")?; }
                        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            writer.write_all(&[0x03])?; // ETX
                        }
                        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            writer.write_all(&[0x04])?; // EOT
                        }
                        KeyCode::Char(ch) => {
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            writer.write_all(s.as_bytes())?;
                        }
                        _ => {}
                    }
                    writer.flush().ok();
                }
                Event::Paste(data) => {
                    writer.write_all(data.as_bytes())?;
                    writer.flush()?;
                }
                Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, .. }) => {
                    // best-effort: send shift-page-down is non-trivial; ignore
                }
                Event::Resize(_, _) => {
                    // next draw will resize
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use ssh2::Session;
use vt100::{Color as VtColor, Parser};

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

    // SSH connection details (PoC hardcoded as requested)
    let ssh_host = "127.0.0.1:2222";
    let ssh_user = "dockeruser";
    let ssh_pass = "dockerpass";

    // Establish SSH session and interactive shell with a PTY
    let tcp = TcpStream::connect(ssh_host).context("connect SSH host")?;
    tcp.set_nodelay(true).ok();
    let mut sess = Session::new().context("new SSH session")?;
    sess.set_tcp_stream(tcp);
    sess.handshake().context("ssh handshake")?;
    sess.userauth_password(ssh_user, ssh_pass).context("ssh auth")?;
    if !sess.authenticated() {
        anyhow::bail!("SSH authentication failed");
    }

    let mut channel = sess.channel_session().context("open channel")?;
    channel
        .request_pty(
            "xterm-256color",
            None,
            Some((100, 30, 0, 0)), // will resize immediately after first draw
        )
        .context("request pty")?;
    channel.shell().context("start remote shell")?;

    // Set non-blocking to allow read polling without stalling writes
    sess.set_blocking(false);

    // Shared vt100 state
    let app_state = Arc::new(Mutex::new(TerminalState::new(30, 100)));

    // Wrap channel in Arc<Mutex<..>> for concurrent read/write/resize
    let channel_arc = Arc::new(Mutex::new(channel));

    // Reader thread: pump SSH channel bytes into vt100 parser
    let app_state_reader = app_state.clone();
    let channel_reader = channel_arc.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            // lock briefly for a nonblocking read
            let n = {
                let mut ch = match channel_reader.lock() {
                    Ok(guard) => guard,
                    Err(_) => break,
                };
                match ch.read(&mut buf) {
                    Ok(0) => return, // channel closed
                    Ok(n) => n,
                    Err(_) => 0,
                }
            };

            if n > 0 {
                if let Ok(mut guard) = app_state_reader.lock() {
                    guard.process_bytes(&buf[..n]);
                }
            } else {
                thread::sleep(Duration::from_millis(10));
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
            // Ensure vt100 knows current size and propagate to remote PTY
            if let Ok(mut guard) = app_state.lock() {
                if guard.parser.screen().size() != (inner.height, inner.width) {
                    guard.resize(inner.height, inner.width);
                    if let Ok(mut ch) = channel_arc.lock() {
                        let _ = ch.request_pty_size(inner.width as u32, inner.height as u32, None, None);
                    }
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
                                if let Ok(mut ch) = channel_arc.lock() {
                                    ch.write_all(&[0x1b])?;
                                    ch.flush().ok();
                                }
                            } else {
                                disable_raw_mode().ok();
                                execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
                                // best-effort close channel
                                if let Ok(mut ch) = channel_arc.lock() {
                                    let _ = ch.send_eof();
                                    let _ = ch.close();
                                }
                                return Ok(());
                            }
                        }
                        KeyCode::Enter => {
                            if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\r")?; ch.flush().ok(); }
                        }
                        KeyCode::Backspace => {
                            if let Ok(mut ch) = channel_arc.lock() { ch.write_all(&[0x7f])?; ch.flush().ok(); }
                        }
                        KeyCode::Left => { if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\x1b[D")?; } }
                        KeyCode::Right => { if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\x1b[C")?; } }
                        KeyCode::Up => { if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\x1b[A")?; } }
                        KeyCode::Down => { if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\x1b[B")?; } }
                        KeyCode::Tab => { if let Ok(mut ch) = channel_arc.lock() { ch.write_all(b"\t")?; } }
                        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            if let Ok(mut ch) = channel_arc.lock() { ch.write_all(&[0x03])?; }
                        }
                        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            if let Ok(mut ch) = channel_arc.lock() { ch.write_all(&[0x04])?; }
                        }
                        KeyCode::Char(ch_) => {
                            let mut buf = [0u8; 4];
                            let s = ch_.encode_utf8(&mut buf);
                            if let Ok(mut ch) = channel_arc.lock() { ch.write_all(s.as_bytes())?; }
                        }
                        _ => {}
                    }
                }
                Event::Paste(data) => {
                    if let Ok(mut ch) = channel_arc.lock() { ch.write_all(data.as_bytes())?; ch.flush().ok(); }
                }
                Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, .. }) => {
                    // ignore for now
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

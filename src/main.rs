mod ui;
mod ssh_client;

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::Terminal;

use ssh_client::SshClient;
use ui::{TerminalState, draw_terminal};

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

    let client = SshClient::connect(ssh_host, ssh_user, ssh_pass)?;

    // Shared vt100 state
    let app_state = Arc::new(Mutex::new(TerminalState::new(30, 100)));

    // Reader thread: pump SSH channel bytes into vt100 parser
    let app_state_reader = app_state.clone();
    let client_reader = client.channel.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = {
                let mut ch = match client_reader.lock() { Ok(guard) => guard, Err(_) => break };
                match ch.read(&mut buf) { Ok(0) => return, Ok(n) => n, Err(_) => 0 }
            };
            if n > 0 {
                if let Ok(mut guard) = app_state_reader.lock() { guard.process_bytes(&buf[..n]); }
            } else {
                std::thread::sleep(Duration::from_millis(10));
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
                .constraints([ Constraint::Length(3), Constraint::Min(1) ])
                .split(size);

            let title_block = Block::default().borders(ratatui::widgets::Borders::ALL)
                .title(Line::from("title").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
            f.render_widget(title_block, layout[0]);

            let inner = layout[1].inner(Margin::new(1, 1));
            if let Ok(mut guard) = app_state.lock() {
                if guard.parser.screen().size() != (inner.height, inner.width) {
                    guard.resize(inner.height, inner.width);
                    client.request_size(inner.width, inner.height);
                }
                draw_terminal(layout[1], &guard, f);
            }
        })?;

        // Input handling
        while crossterm::event::poll(Duration::from_millis(1))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Esc => {
                            let in_alt = app_state.lock().ok().map(|g| g.parser.screen().alternate_screen()).unwrap_or(false);
                            if in_alt { client.write_all(&[0x1b])?; }
                            else {
                                disable_raw_mode().ok();
                                execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
                                client.close();
                                return Ok(());
                            }
                        }
                        KeyCode::Enter => { client.write_all(b"\r")?; }
                        KeyCode::Backspace => { client.write_all(&[0x7f])?; }
                        KeyCode::Left => { client.write_all(b"\x1b[D")?; }
                        KeyCode::Right => { client.write_all(b"\x1b[C")?; }
                        KeyCode::Up => { client.write_all(b"\x1b[A")?; }
                        KeyCode::Down => { client.write_all(b"\x1b[B")?; }
                        KeyCode::Tab => { client.write_all(b"\t")?; }
                        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => { client.write_all(&[0x03])?; }
                        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => { client.write_all(&[0x04])?; }
                        KeyCode::Char(ch_) => { let mut tmp=[0u8;4]; let s=ch_.encode_utf8(&mut tmp); client.write_all(s.as_bytes())?; }
                        _ => {}
                    }
                }
                Event::Paste(data) => { client.write_all(data.as_bytes())?; }
                Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, .. }) => {}
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate { last_tick = Instant::now(); }
    }
}

mod ui;
mod ssh_client;

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::Terminal;

use ssh_client::SshClient;
use ui::{TerminalState, draw_terminal, ConnectionForm, draw_connection_form};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AppMode { Form, Connected }

fn main() -> Result<()> {
	// Setup Crossterm terminal
	enable_raw_mode()?;
	let mut stdout = std::io::stdout();
	execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	// App state
	let mut mode = AppMode::Form;
	let mut form = ConnectionForm::new();
	let mut client: Option<SshClient> = None;
	let mut app_state: Option<Arc<Mutex<TerminalState>>> = None;

	// UI event/render loop
	let mut last_tick = Instant::now();
	let tick_rate = Duration::from_millis(16);

	loop {
		terminal.draw(|f| {
			let size = f.size();
			match mode {
				AppMode::Form => {
					let layout = Layout::default()
						.direction(Direction::Vertical)
						.constraints([ Constraint::Length(3), Constraint::Min(1) ])
						.split(size);

					let title_block = Block::default().borders(ratatui::widgets::Borders::ALL)
						.title(Line::from("New SSH Connection").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
					f.render_widget(title_block, layout[0]);

					draw_connection_form(layout[1], &form, f);
				}
				AppMode::Connected => {
					let layout = Layout::default()
						.direction(Direction::Vertical)
						.constraints([ Constraint::Length(3), Constraint::Min(1) ])
						.split(size);

					let title_block = Block::default().borders(ratatui::widgets::Borders::ALL)
						.title(Line::from("title").style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
					f.render_widget(title_block, layout[0]);

					if let (Some(app), Some(cli)) = (app_state.as_ref(), client.as_ref()) {
						let inner = layout[1].inner(Margin::new(1, 1));
						if let Ok(mut guard) = app.lock() {
							if guard.parser.screen().size() != (inner.height, inner.width) {
								guard.resize(inner.height, inner.width);
								cli.request_size(inner.width, inner.height);
							}
							draw_terminal(layout[1], &guard, f);
						}
					}
				}
			}
		})?;

		// Input handling
		while crossterm::event::poll(Duration::from_millis(1))? {
			match event::read()? {
				Event::Key(key) if key.kind == KeyEventKind::Press => {
					match mode {
						AppMode::Form => {
							match key.code {
								KeyCode::Esc => {
									disable_raw_mode().ok();
									execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
									return Ok(());
								}
								KeyCode::Tab => { form.next(); }
								KeyCode::BackTab => { form.prev(); }
								KeyCode::Enter => {
									match form.validate() {
										Ok(_) => {
											let host = form.host_port();
											let user = form.username.trim().to_string();
											let pass = form.password.clone();
											match SshClient::connect(&host, &user, &pass) {
												Ok(cli) => {
													let app = Arc::new(Mutex::new(TerminalState::new(30, 100)));
													// Reader thread
													let app_reader = app.clone();
													let client_reader = cli.channel.clone();
													thread::spawn(move || {
														let mut buf = [0u8; 8192];
														loop {
															let n = {
																let mut ch = match client_reader.lock() { Ok(guard) => guard, Err(_) => break };
																match ch.read(&mut buf) { Ok(0) => return, Ok(n) => n, Err(_) => 0 }
															};
														if n > 0 {
															if let Ok(mut guard) = app_reader.lock() { guard.process_bytes(&buf[..n]); }
														} else {
															std::thread::sleep(Duration::from_millis(10));
														}
													}
													});
													client = Some(cli);
													app_state = Some(app);
													form.error = None;
													mode = AppMode::Connected;
												}
												Err(e) => { form.error = Some(format!("{}", e)); }
											}
										}
										Err(msg) => { form.error = Some(msg); }
									}
								}
								KeyCode::Backspace => { let s = form.focused_value_mut(); s.pop(); }
								KeyCode::Char(ch) => {
									let s = form.focused_value_mut(); s.push(ch);
								}
								_ => {}
							}
						}
						AppMode::Connected => {
							if let Some(cli) = client.as_ref() {
								match key.code {
									KeyCode::Esc => {
										let in_alt = app_state.as_ref().and_then(|a| a.lock().ok()).map(|g| g.parser.screen().alternate_screen()).unwrap_or(false);
										if in_alt { cli.write_all(&[0x1b])?; }
										else {
											disable_raw_mode().ok();
											execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
											cli.close();
											return Ok(());
										}
									}
									KeyCode::Enter => { cli.write_all(b"\r")?; }
									KeyCode::Backspace => { cli.write_all(&[0x7f])?; }
									KeyCode::Left => { cli.write_all(b"\x1b[D")?; }
									KeyCode::Right => { cli.write_all(b"\x1b[C")?; }
									KeyCode::Up => { cli.write_all(b"\x1b[A")?; }
									KeyCode::Down => { cli.write_all(b"\x1b[B")?; }
									KeyCode::Tab => { cli.write_all(b"\t")?; }
									KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => { cli.write_all(&[0x03])?; }
									KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => { cli.write_all(&[0x04])?; }
									KeyCode::Char(ch_) => { let mut tmp=[0u8;4]; let s=ch_.encode_utf8(&mut tmp); cli.write_all(s.as_bytes())?; }
									_ => {}
								}
							}
						}
					}
				}
				Event::Paste(data) => {
					match mode {
						AppMode::Form => { let s = form.focused_value_mut(); s.push_str(&data); }
						AppMode::Connected => { if let Some(cli) = client.as_ref() { cli.write_all(data.as_bytes())?; } }
					}
				}
				Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, .. }) => {}
				Event::Resize(_, _) => {}
				_ => {}
			}
		}

		if last_tick.elapsed() >= tick_rate { last_tick = Instant::now(); }
	}
}

use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::{
    event::{self, Event as CtEvent, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::{select, sync::mpsc, time};

type Term = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug)]
enum AppEvent {
    Input(CtEvent),
    Tick,
}

struct App {
    ticks: u64,
    msg: String,
}

impl App {
    fn new() -> Self {
        Self {
            ticks: 0,
            msg:
                "Press q to exit; any key will update message; every 250ms will automatically tick"
                    .into(),
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        self.msg = format!("Received key: {:?}", code);
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
    }
}

fn init_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(mut terminal: Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1) initialize terminal
    let mut terminal = init_terminal()?;

    // 2) async event channel
    let (tx, mut rx) = mpsc::channel::<AppEvent>(100);

    // 3) async ticker
    let mut ticker = time::interval(time::Duration::from_millis(250));
    let tx_tick = tx.clone();

    // 4) asynchronous: keyboard/terminal event listening (in blocking task)
    let tx_input = tx.clone();

    let mut event_stream = event::EventStream::new();
    tokio::spawn(async move {
        loop {
            select! {
                event_result = event_stream.next() => {
                    let event = match event_result {
                        None => break,
                        Some(Err(_)) => break, // IO error on stdin
                        Some(Ok(event)) => event,
                    };
                    tx_input.send(AppEvent::Input(event)).await.unwrap();
                }
                _ = ticker.tick() => {
                    tx_tick.send(AppEvent::Tick).await.unwrap();
                }
            }
        }
    });

    // 5) application state and main loop (sync rendering + asynchronous events)
    let mut app = App::new();

    let res = run_app(&mut terminal, &mut rx, &mut app).await;

    // 6) clean up terminal
    restore_terminal(terminal)?;

    res
}

async fn run_app(
    terminal: &mut Term,
    rx: &mut mpsc::Receiver<AppEvent>,
    app: &mut App,
) -> Result<()> {
    loop {
        // render a frame (sync)
        terminal.draw(|f| ui(f, app))?;

        // wait for an event (asynchronous)
        let ev = match rx.recv().await {
            Some(e) => e,
            None => break, // exit if channel is closed
        };

        match ev {
            AppEvent::Tick => app.on_tick(),
            AppEvent::Input(CtEvent::Key(k)) => {
                if k.code == KeyCode::Char('q') {
                    break;
                }
                app.on_key(k.code);
            }
            _ => {}
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let area = f.size();

    let block = Block::default()
        .title("ratatui + tokio (async)")
        .borders(Borders::ALL);
    let inner = block.inner(area);

    let text = vec![
        Line::from(app.msg.as_str()),
        Line::from(""),
        Line::from(format!("Tick count: {}", app.ticks)),
        Line::from(""),
        Line::from("Hint: press q to exit program"),
    ];

    let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);
}

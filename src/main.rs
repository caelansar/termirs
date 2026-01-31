use std::time::Duration;

use clap::Parser;
use crossterm::event;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::{select, sync::mpsc, time};

use termirs::{App, AppEvent, Result, TickControl, init_panic_hook, init_tracing};

/// A modern, async SSH terminal client
#[derive(Parser, Debug)]
#[command(name = "termirs")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Enable logging to termirs.log file
    #[arg(short, long)]
    log: bool,

    /// Set log level (off, error, warn, info, debug, trace)
    /// This option requires --log to be enabled
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize tracing if logging is enabled
    if args.log {
        init_tracing(&args.log_level)?;
        tracing::info!("Starting termirs SSH client v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!("Logging enabled at level: {}", args.log_level);
    }

    init_panic_hook();

    // Setup Crossterm terminal
    tracing::debug!("Initializing terminal backend");
    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(terminal)?;
    app.init_terminal()?;
    tracing::info!("Terminal initialized successfully");

    // async event channel
    let (tx, mut rx) = mpsc::channel::<AppEvent>(100);
    let (tick_control_tx, mut tick_control_rx) = mpsc::channel::<TickControl>(10);

    // Set the event sender in the app
    app.set_event_sender(tx.clone());
    app.set_tick_control_sender(tick_control_tx);

    // ticker - 50ms interval, conditionally enabled
    let mut ticker = time::interval(Duration::from_millis(50));
    let tx_tick = tx.clone();

    // asynchronous: keyboard/terminal event listening
    let mut event_stream = event::EventStream::new();
    tokio::spawn(async move {
        let mut tick_enabled = false; // Start with ticker disabled

        loop {
            select! {
                maybe_ev = event_stream.next() => {
                    let ev = match maybe_ev {
                        None => break,
                        Some(Err(_)) => break,
                        Some(Ok(e)) => e,
                    };
                    if tx.send(AppEvent::Input(ev)).await.is_err() {
                        break;
                    }
                }
                _ = ticker.tick(), if tick_enabled => {
                    // Only fires when tick_enabled is true
                    if tx_tick.send(AppEvent::Tick).await.is_err() {
                        break;
                    }
                }
                Some(control) = tick_control_rx.recv() => {
                    match control {
                        TickControl::Start => tick_enabled = true,
                        TickControl::Stop => tick_enabled = false,
                    }
                }
            }
        }
    });

    // run app loop
    tracing::info!("Starting main event loop");
    let res = app
        .run(&mut rx)
        .await
        .inspect_err(|e| tracing::error!("Error in main event loop: {}", e));

    // app drop restores terminal
    tracing::info!("Application shutting down");
    drop(app);

    res
}

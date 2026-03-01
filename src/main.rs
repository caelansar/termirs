use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use crossterm::event;
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

    // Shared flag: the blocking poll thread only reads events when true.
    let input_enabled = Arc::new(AtomicBool::new(true));
    let input_enabled_poller = input_enabled.clone();

    // Shared flag: set to false on shutdown so the blocking thread exits.
    let running = Arc::new(AtomicBool::new(true));
    let running_poller = running.clone();

    // Sync event polling on a dedicated blocking thread.
    // Uses crossterm's sync poll/read API directly, avoiding EventStream's
    // background thread and its global mutex deadlock.
    let tx_input = tx.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if !running_poller.load(Ordering::SeqCst) {
                break;
            }
            if !input_enabled_poller.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match event::poll(Duration::from_millis(10)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if tx_input.blocking_send(AppEvent::Input(ev)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error reading event: {:?}", e);
                        break;
                    }
                },
                Ok(false) => {} // no event within 10ms
                Err(e) => {
                    tracing::error!("Error polling event: {:?}", e);
                    break;
                }
            }
        }
    });

    // Tick and control handling
    tokio::spawn(async move {
        let mut tick_enabled = false;
        loop {
            select! {
                _ = ticker.tick(), if tick_enabled => {
                    if tx_tick.send(AppEvent::Tick).await.is_err() {
                        break;
                    }
                }
                result = tick_control_rx.recv() => {
                    match result {
                        Some(control) => match control {
                            TickControl::Start => tick_enabled = true,
                            TickControl::Stop => tick_enabled = false,
                            TickControl::PauseInput => {
                                input_enabled.store(false, Ordering::SeqCst);
                                tracing::info!("Pausing input polling");
                            },
                            TickControl::ResumeInput => {
                                input_enabled.store(true, Ordering::SeqCst);
                                tracing::info!("Resuming input polling");
                            },
                        },
                        None => break, // channel closed, shut down
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

    // Signal the blocking poll thread to exit.
    running.store(false, Ordering::SeqCst);

    // app drop restores terminal
    tracing::info!("Application shutting down");
    drop(app);

    res
}

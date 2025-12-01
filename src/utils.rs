use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::error::{AppError, Result};

pub fn init_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // intentionally ignore errors here since we're already in a panic
        eprintln!("Panic hook");
        let _ = restore_tui();
        original_hook(panic_info);
    }));
}

pub fn restore_tui() -> std::io::Result<()> {
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen, Show)?;
    Ok(())
}

pub fn init_tracing(log_level: &str) -> Result<()> {
    // Create a file appender that writes to termirs.log in the current directory
    let file_appender = tracing_appender::rolling::never(".", "termirs.log");

    // Create a non-blocking writer for better performance
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Build the subscriber with environment filter support
    // Priority: RUST_LOG env var > command line arg > default (info)
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    // Configure the formatter with timestamps and target info
    let fmt_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(true)
        .with_ansi(false); // Disable ANSI colors in log file

    // Initialize the global subscriber
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| AppError::ConfigError(format!("Failed to initialize tracing: {}", e)))?;

    // Keep the guard alive for the duration of the program
    // We intentionally leak it here since logging should last the entire program
    std::mem::forget(_guard);

    Ok(())
}

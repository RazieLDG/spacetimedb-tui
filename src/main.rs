/// SpacetimeDB TUI — entry point.
///
/// Responsibilities:
/// 1. Parse CLI arguments and build [`Config`].
/// 2. Initialise the `tracing` subscriber.
/// 3. Set up the crossterm raw-mode terminal.
/// 4. Run the async event loop via [`App`].
/// 5. Restore the terminal on exit — even if the loop panics.
mod api;
mod app;
mod config;
mod effects;
mod state;
mod terminal;
mod ui;
mod user_config;

use std::io;

use anyhow::{Context, Result};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing::info;

use crate::{
    api::SpacetimeClient,
    app::App,
    config::Config,
    terminal::{CrosstermTerminalOps, TerminalGuard},
};

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let config = Config::parse().context("Failed to parse configuration")?;
    init_tracing(&config.log_level);
    install_panic_logger();

    info!(
        server_url = %config.server_url,
        database   = ?config.database,
        "SpacetimeDB TUI starting"
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build Tokio runtime")?;

    rt.block_on(async_main(config))
}

// ── Async entry point ─────────────────────────────────────────────────────────

async fn async_main(config: Config) -> Result<()> {
    // Build the HTTP client.
    let client = SpacetimeClient::new(config.server_url.clone(), config.auth_token.clone())
        .context("Failed to create SpacetimeDB HTTP client")?;

    // Build the application.
    let mut app = App::new(&config, client);

    // Pre-select the database from the CLI flag if provided.
    if let Some(db) = &config.database {
        app.state.databases.push(db.clone());
        app.state.select_database(0);
    }

    // Acquire raw mode, alternate screen, mouse capture, and a hidden
    // cursor. The guard owns these transitions and restores them exactly
    // once on drop — on normal return, error return, or unwind alike.
    let _terminal_guard =
        TerminalGuard::enter(CrosstermTerminalOps::new()).context("Failed to set up terminal")?;

    // Create the Ratatui terminal (the guard already prepared the tty).
    let mut terminal = setup_terminal().context("Failed to set up terminal")?;

    // Run the event loop. `_terminal_guard` stays alive until after this
    // returns, then its `Drop` restores the terminal exactly once.
    app.run(&mut terminal).await
}

// ── Terminal setup / teardown ─────────────────────────────────────────────────

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn setup_terminal() -> Result<Term> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create ratatui Terminal")?;
    terminal.clear().context("Failed to clear terminal")?;
    Ok(terminal)
}

// ── Tracing ───────────────────────────────────────────────────────────────────

fn init_tracing(level: &str) {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

/// Route every panic (including ones that fire inside `tokio::spawn`
/// background tasks) through `tracing::error!` so they show up in the
/// log output instead of getting silently dropped by the default
/// hook while the TUI is in raw mode and nothing is visible anyway.
///
/// We wrap the default hook rather than replacing it so the original
/// backtrace-printing behaviour still fires when running outside the
/// TUI (e.g. during `cargo test`).
fn install_panic_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        tracing::error!(
            target: "spacetimedb_tui::panic",
            "panic at {location}: {payload}"
        );
        default_hook(info);
    }));
}

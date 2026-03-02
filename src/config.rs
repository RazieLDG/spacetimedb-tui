//! Configuration for the SpacetimeDB TUI.
//!
//! [`Config`] is built from CLI arguments (via [`clap`]) and optional
//! environment variables.  It is constructed once at startup and then passed
//! (by reference or `Arc`) wherever it is needed.

use anyhow::{bail, Result};
use clap::Parser;

// ---------------------------------------------------------------------------
// CLI argument definition
// ---------------------------------------------------------------------------

/// Command-line arguments parsed by clap.
#[derive(Debug, Parser)]
#[command(
    name = "spacetimedb-tui",
    version,
    author,
    about = "A terminal user interface for SpacetimeDB",
    long_about = None,
)]
pub struct Cli {
    /// Hostname or IP address of the SpacetimeDB server.
    #[arg(
        short = 'H',
        long,
        default_value = "localhost",
        env = "SPACETIMEDB_HOST",
        help = "SpacetimeDB server hostname"
    )]
    pub host: String,

    /// HTTP port of the SpacetimeDB server.
    #[arg(
        short,
        long,
        default_value_t = 3000,
        env = "SPACETIMEDB_PORT",
        help = "SpacetimeDB server port"
    )]
    pub port: u16,

    /// Database (module) name to connect to on startup.
    #[arg(
        short,
        long,
        env = "SPACETIMEDB_DATABASE",
        help = "Database / module name to open on startup"
    )]
    pub database: Option<String>,

    /// Authentication token.
    #[arg(
        short,
        long,
        env = "SPACETIMEDB_TOKEN",
        help = "Bearer token for authentication"
    )]
    pub token: Option<String>,

    /// Use TLS (wss:// / https://).
    #[arg(long, default_value_t = false, help = "Use TLS for the connection")]
    pub tls: bool,

    /// Log level filter for the TUI's own log output (not module logs).
    #[arg(
        long,
        default_value = "warn",
        env = "RUST_LOG",
        help = "Tracing log level (error/warn/info/debug/trace)"
    )]
    pub log_level: String,

    /// Colour theme.
    #[arg(
        long,
        default_value = "dark",
        help = "Colour theme: dark, light, or high-contrast"
    )]
    pub theme: ThemeName,
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Named colour themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ThemeName {
    Dark,
    Light,
    HighContrast,
}

impl std::fmt::Display for ThemeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeName::Dark => write!(f, "dark"),
            ThemeName::Light => write!(f, "light"),
            ThemeName::HighContrast => write!(f, "high-contrast"),
        }
    }
}

/// A set of ratatui `Color` values for a named theme.
///
/// Using `u8` RGB triples rather than `ratatui::style::Color` directly so
/// that `config.rs` does not need to depend on ratatui (keeping the layer
/// boundary clean).  The UI layer converts these to `Color::Rgb(r, g, b)`.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    // Backgrounds
    pub bg_primary: (u8, u8, u8),
    pub bg_secondary: (u8, u8, u8),
    pub bg_selected: (u8, u8, u8),
    // Foregrounds
    pub fg_primary: (u8, u8, u8),
    pub fg_secondary: (u8, u8, u8),
    pub fg_muted: (u8, u8, u8),
    // Accent / highlight
    pub accent: (u8, u8, u8),
    pub highlight: (u8, u8, u8),
    // Status colours
    pub success: (u8, u8, u8),
    pub warning: (u8, u8, u8),
    pub error: (u8, u8, u8),
    pub info: (u8, u8, u8),
    // Border
    pub border_normal: (u8, u8, u8),
    pub border_focused: (u8, u8, u8),
}

impl ThemeColors {
    pub fn dark() -> Self {
        Self {
            bg_primary: (18, 18, 18),
            bg_secondary: (28, 28, 30),
            bg_selected: (44, 62, 80),
            fg_primary: (220, 220, 220),
            fg_secondary: (180, 180, 180),
            fg_muted: (120, 120, 120),
            accent: (97, 175, 239),
            highlight: (229, 192, 123),
            success: (152, 195, 121),
            warning: (229, 192, 123),
            error: (224, 108, 117),
            info: (86, 182, 194),
            border_normal: (60, 60, 60),
            border_focused: (97, 175, 239),
        }
    }

    pub fn light() -> Self {
        Self {
            bg_primary: (248, 248, 248),
            bg_secondary: (235, 235, 235),
            bg_selected: (200, 220, 240),
            fg_primary: (30, 30, 30),
            fg_secondary: (80, 80, 80),
            fg_muted: (160, 160, 160),
            accent: (0, 100, 200),
            highlight: (160, 100, 0),
            success: (0, 140, 0),
            warning: (180, 120, 0),
            error: (200, 0, 0),
            info: (0, 120, 160),
            border_normal: (180, 180, 180),
            border_focused: (0, 100, 200),
        }
    }

    pub fn high_contrast() -> Self {
        Self {
            bg_primary: (0, 0, 0),
            bg_secondary: (20, 20, 20),
            bg_selected: (0, 80, 160),
            fg_primary: (255, 255, 255),
            fg_secondary: (220, 220, 220),
            fg_muted: (180, 180, 180),
            accent: (0, 200, 255),
            highlight: (255, 220, 0),
            success: (0, 255, 0),
            warning: (255, 200, 0),
            error: (255, 0, 0),
            info: (0, 200, 255),
            border_normal: (120, 120, 120),
            border_focused: (255, 255, 255),
        }
    }

    pub fn for_theme(theme: ThemeName) -> Self {
        match theme {
            ThemeName::Dark => Self::dark(),
            ThemeName::Light => Self::light(),
            ThemeName::HighContrast => Self::high_contrast(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Resolved application configuration, derived from [`Cli`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Full HTTP base URL, e.g. `http://localhost:3000`.
    pub server_url: String,
    /// Full WebSocket base URL, e.g. `ws://localhost:3000`.
    pub ws_url: String,
    /// Database / module to open on startup (may be `None`).
    pub database: Option<String>,
    /// Authentication token (may be `None` for unauthenticated access).
    pub auth_token: Option<String>,
    /// Resolved colour theme.
    pub theme: ThemeColors,
    /// Theme name (for display / serialisation).
    pub theme_name: ThemeName,
    /// Tracing log level string.
    pub log_level: String,
}

impl Config {
    /// Build a [`Config`] from parsed CLI arguments.
    ///
    /// # Errors
    /// Returns an error if the port is 0 or the host is empty.
    pub fn from_cli(cli: Cli) -> Result<Self> {
        if cli.host.trim().is_empty() {
            bail!("--host must not be empty");
        }
        if cli.port == 0 {
            bail!("--port must be a non-zero port number");
        }

        let scheme = if cli.tls { "https" } else { "http" };
        let ws_scheme = if cli.tls { "wss" } else { "ws" };

        let server_url = format!("{}://{}:{}", scheme, cli.host, cli.port);
        let ws_url = format!("{}://{}:{}", ws_scheme, cli.host, cli.port);

        Ok(Self {
            server_url,
            ws_url,
            database: cli.database,
            auth_token: cli.token,
            theme: ThemeColors::for_theme(cli.theme),
            theme_name: cli.theme,
            log_level: cli.log_level,
        })
    }

    /// Parse CLI args from `std::env::args()` and build a [`Config`].
    pub fn parse() -> Result<Self> {
        let cli = Cli::parse();
        Self::from_cli(cli)
    }

    /// Whether TLS is in use (inferred from the scheme in `server_url`).
    pub fn uses_tls(&self) -> bool {
        self.server_url.starts_with("https://")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cli(host: &str, port: u16, database: Option<&str>, tls: bool) -> Cli {
        Cli {
            host: host.to_string(),
            port,
            database: database.map(str::to_owned),
            token: None,
            tls,
            log_level: "warn".to_string(),
            theme: ThemeName::Dark,
        }
    }

    #[test]
    fn test_config_http() {
        let cfg = Config::from_cli(make_cli("localhost", 3000, None, false)).unwrap();
        assert_eq!(cfg.server_url, "http://localhost:3000");
        assert_eq!(cfg.ws_url, "ws://localhost:3000");
        assert!(!cfg.uses_tls());
    }

    #[test]
    fn test_config_tls() {
        let cfg = Config::from_cli(make_cli("example.com", 443, Some("mydb"), true)).unwrap();
        assert_eq!(cfg.server_url, "https://example.com:443");
        assert_eq!(cfg.ws_url, "wss://example.com:443");
        assert!(cfg.uses_tls());
        assert_eq!(cfg.database.as_deref(), Some("mydb"));
    }

    #[test]
    fn test_config_empty_host_is_error() {
        let result = Config::from_cli(make_cli("", 3000, None, false));
        assert!(result.is_err());
    }

    #[test]
    fn test_config_zero_port_is_error() {
        let result = Config::from_cli(make_cli("localhost", 0, None, false));
        assert!(result.is_err());
    }

    #[test]
    fn test_theme_colors_dark() {
        let t = ThemeColors::dark();
        // Spot-check a few fields are non-zero.
        assert_ne!(t.accent, (0, 0, 0));
        assert_ne!(t.error, (0, 0, 0));
    }
}

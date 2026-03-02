# 🛸 spacetimedb-tui

> **A blazing-fast, keyboard-driven terminal UI for exploring, querying, and monitoring SpacetimeDB 2.0 — right from your shell.**

[![Crates.io](https://img.shields.io/crates/v/spacetimedb-tui.svg)](https://crates.io/crates/spacetimedb-tui)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/alice-ai/spacetimedb-tui/ci.yml?branch=main)](https://github.com/alice-ai/spacetimedb-tui/actions)
[![SpacetimeDB](https://img.shields.io/badge/SpacetimeDB-2.0-blueviolet)](https://spacetimedb.com)
[![Rust](https://img.shields.io/badge/Rust-1.78%2B-orange)](https://www.rust-lang.org)

---

## 📺 TUI Layout

```
╔══════════════════════════════════════════════════════════════════════════════════╗
║  🛸 spacetimedb-tui  │  host: localhost:3000  │  db: my_game_db  │  ● LIVE     ║
╠══════════════╦═══════════════════════════════════════════╦═══════════════════════╣
║  DATABASES   ║  TABLE: player                            ║  METRICS              ║
║ ─────────── ║ ──────────────────────────────────────── ║ ───────────────────── ║
║ ▶ my_game_db ║  id   │ username    │ score │ online      ║  Connections:    128  ║
║   lobby_db   ║ ──────┼─────────────┼───────┼──────────  ║  Queries/sec:    4.2  ║
║   test_db    ║  1    │ alice       │ 9800  │ true        ║  Rows written:  1.2M  ║
║              ║  2    │ bob_builder │ 4200  │ false       ║  Uptime:      3d 14h  ║
║  TABLES      ║  3    │ carol99     │ 7350  │ true        ║  Mem usage:    512MB  ║
║ ─────────── ║  4    │ dave_x      │ 1100  │ true        ║  CPU:           2.4%  ║
║ ▶ player     ║  5    │ eve_online  │ 6600  │ false       ║ ─────────────────── ║
║   inventory  ║  ...                                      ║  MODULES              ║
║   match      ║                                           ║ ─────────────────── ║
║   session    ║  Rows: 1,024  │ Page 1/11  │ ↑↓ scroll   ║  game_logic  v1.4.2  ║
║   event_log  ║                                           ║  auth        v2.0.1  ║
║              ╠═══════════════════════════════════════════╣  matchmaker  v0.9.8  ║
║  [F1] Help   ║  SQL ▶  SELECT * FROM player LIMIT 5;    ║                       ║
║  [F5] Refresh║  ────────────────────────────────────────║  [Tab] cycle panels  ║
╚══════════════╩═══════════════════════════════════════════╩═══════════════════════╝
 [q]uit [Tab]focus [/]search [s]ort [r]efresh [x]SQL [l]ogs [m]etrics [?]help
```

---

## ✨ Features

| Feature | Description |
|---|---|
| 🗄️ **Database Browser** | Navigate all your SpacetimeDB databases and schemas in a tree-style sidebar. Switch between databases instantly without leaving the terminal. |
| 📊 **Live Table Viewer** | Stream real-time row updates from any table via SpacetimeDB subscriptions. Rows highlight on insert, update, and delete — no polling needed. |
| 🖥️ **SQL Console** | Write and execute ad-hoc SQL queries with inline syntax highlighting, query history (↑/↓), and results rendered in a scrollable grid. |
| 📜 **Log Viewer** | Tail and filter structured logs emitted by your SpacetimeDB modules. Supports level filtering (`INFO`, `WARN`, `ERROR`, `DEBUG`) and regex search. |
| 📈 **Metrics Dashboard** | Live sparklines and counters for connections, queries/sec, memory, CPU, and row throughput — all sourced from the SpacetimeDB metrics API. |
| 🔬 **Module Inspector** | Browse deployed WASM modules: view reducer signatures, scheduled reducers, table definitions, and module version history. |
| ⌨️ **Keyboard-First UX** | Every action is reachable without a mouse. Vim-style navigation, fuzzy search, and modal panels keep your hands on the keyboard. |
| 🎨 **Theming** | Ships with `dark`, `light`, and `solarized` themes. Fully customisable via a TOML config file. |
| 🔐 **Auth Support** | Connects with SpacetimeDB identity tokens. Reads credentials from env vars or a local config file — no plaintext passwords in shell history. |

---

## 🚀 Installation

### Prerequisites

- **Rust 1.78+** — install via [rustup](https://rustup.rs)
- A running **SpacetimeDB 2.0** instance (local or remote)

---

### Option 1 — Install from Crates.io

```bash
cargo install spacetimedb-tui
```

The `stdb-tui` binary will be placed in `~/.cargo/bin/`. Make sure that directory is on your `$PATH`.

---

### Option 2 — Build from Source

```bash
# 1. Clone the repository
git clone https://github.com/alice-ai/spacetimedb-tui.git
cd spacetimedb-tui

# 2. Build an optimised release binary
cargo build --release

# 3. (Optional) copy to a directory on your PATH
cp target/release/stdb-tui ~/.local/bin/stdb-tui
```

> **Tip:** Use `cargo build --release --features tls` to enable TLS support for remote SpacetimeDB instances served over `wss://`.

---

### Option 3 — Nix Flake

```bash
nix run github:alice-ai/spacetimedb-tui
```

---

## 🖥️ Usage

### Basic Invocation

```bash
# Connect to a local SpacetimeDB instance on the default port
stdb-tui

# Specify host, port, and database
stdb-tui --host localhost --port 3000 --database my_game_db

# Connect to a remote instance with an auth token
stdb-tui --host db.example.com --port 443 --database prod_db --token $STDB_TOKEN

# Open directly in the SQL console panel
stdb-tui --panel sql --database my_game_db

# Use a custom config file
stdb-tui --config ~/.config/stdb-tui/custom.toml
```

---

### CLI Reference

| Flag | Short | Default | Description |
|---|---|---|---|
| `--host <HOST>` | `-H` | `localhost` | SpacetimeDB server hostname or IP address |
| `--port <PORT>` | `-p` | `3000` | SpacetimeDB server port |
| `--database <DB>` | `-d` | *(none)* | Database to select on startup |
| `--token <TOKEN>` | `-t` | *(env: `STDB_TOKEN`)* | SpacetimeDB identity/auth token |
| `--panel <PANEL>` | | `browser` | Starting panel: `browser`, `sql`, `logs`, `metrics`, `modules` |
| `--config <PATH>` | `-c` | `~/.config/stdb-tui/config.toml` | Path to configuration file |
| `--theme <THEME>` | | `dark` | UI theme: `dark`, `light`, `solarized` |
| `--no-live` | | *(off)* | Disable live subscription updates (read-only polling mode) |
| `--log-level <LVL>` | | `info` | Internal log verbosity: `error`, `warn`, `info`, `debug`, `trace` |
| `--version` | `-V` | | Print version and exit |
| `--help` | `-h` | | Print help and exit |

---

### Example Commands

```bash
# Browse a local development database
stdb-tui -H localhost -p 3000 -d dev_db

# Tail logs from a production module
stdb-tui -H prod.example.com -p 443 -d prod_db --panel logs --token $STDB_TOKEN

# Run in polling mode (no WebSocket subscription)
stdb-tui --no-live -d my_game_db

# Use a staging environment defined in config
stdb-tui --config ~/.config/stdb-tui/staging.toml
```

---

## ⌨️ Key Bindings

### Global

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Cycle focus between panels (forward / backward) |
| `q` | Quit the application |
| `?` | Toggle help overlay |
| `F1` | Toggle help overlay (alias) |
| `F5` | Force refresh current panel |
| `Ctrl+c` | Quit immediately |
| `Ctrl+l` | Redraw / clear screen |
| `Ctrl+r` | Reconnect to SpacetimeDB |
| `1`–`5` | Jump directly to panel 1–5 |

---

### Navigation

| Key | Action |
|---|---|
| `↑` / `k` | Move selection up |
| `↓` / `j` | Move selection down |
| `←` / `h` | Move selection left / collapse tree node |
| `→` / `l` | Move selection right / expand tree node |
| `g` / `Home` | Jump to first row |
| `G` / `End` | Jump to last row |
| `PgUp` / `Ctrl+u` | Scroll up half a page |
| `PgDn` / `Ctrl+d` | Scroll down half a page |
| `Enter` | Select / confirm / open |
| `Esc` | Cancel / close modal / deselect |

---

### Database Browser

| Key | Action |
|---|---|
| `/` | Fuzzy-search databases and tables |
| `r` | Refresh database/table list |
| `o` | Open selected table in Live Table Viewer |
| `x` | Open selected table in SQL Console |
| `i` | Inspect selected table schema |
| `Space` | Toggle tree node expand/collapse |

---

### Live Table Viewer

| Key | Action |
|---|---|
| `s` | Cycle sort column (asc → desc → none) |
| `S` | Open sort configuration dialog |
| `f` | Open column filter dialog |
| `F` | Clear all active filters |
| `/` | Search within visible rows |
| `n` / `N` | Jump to next / previous search match |
| `c` | Copy selected cell value to clipboard |
| `C` | Copy entire selected row as JSON |
| `e` | Export current view to CSV |
| `L` | Toggle live subscription on/off |
| `r` | Manually refresh rows |
| `←` / `→` | Scroll columns left / right |
| `[` / `]` | Resize focused column narrower / wider |

---

### SQL Console

| Key | Action |
|---|---|
| `Enter` | Execute query (when in input mode) |
| `Ctrl+Enter` | Execute multi-line query |
| `↑` / `↓` | Navigate query history |
| `Ctrl+k` | Clear the SQL input buffer |
| `Ctrl+e` | Open query in `$EDITOR` |
| `Ctrl+s` | Save query to history file |
| `Tab` | Auto-complete table / column names |
| `F9` | Execute and show execution plan (`EXPLAIN`) |
| `e` | Export result set to CSV |
| `c` | Copy result set to clipboard as TSV |

---

### Log Viewer

| Key | Action |
|---|---|
| `f` | Open log level filter dialog |
| `/` | Search / filter by regex pattern |
| `n` / `N` | Jump to next / previous match |
| `p` | Pause / resume log streaming |
| `c` | Clear log buffer |
| `w` | Toggle line-wrap |
| `e` | Export visible logs to file |
| `t` | Toggle timestamp display format (relative / absolute) |

---

### Metrics Dashboard

| Key | Action |
|---|---|
| `r` | Force metrics refresh |
| `i` | Change refresh interval |
| `+` / `-` | Zoom sparkline time window in / out |
| `h` | Toggle historical data overlay |
| `e` | Export metrics snapshot to JSON |

---

### Module Inspector

| Key | Action |
|---|---|
| `Enter` | Inspect selected module / reducer |
| `b` | Go back to module list |
| `c` | Copy reducer signature to clipboard |
| `d` | Show module deployment history |
| `r` | Refresh module list |

---

## 🪐 SpacetimeDB 2.0 Compatibility

`spacetimedb-tui` is built and tested against **SpacetimeDB 2.0** and its stable HTTP + WebSocket APIs.

| SpacetimeDB Version | Supported | Notes |
|---|---|---|
| **2.0.x** | ✅ Full support | Primary target. All features available. |
| **1.x** | ⚠️ Partial | Live subscriptions and module inspection may not work. |
| **< 1.0** | ❌ Not supported | Legacy API incompatibilities. |

### What uses SpacetimeDB 2.0 APIs

- **Live Table Viewer** — uses the SpacetimeDB 2.0 subscription WebSocket protocol (`/database/subscribe`)
- **SQL Console** — uses the `/database/sql` REST endpoint
- **Log Viewer** — streams from the `/database/logs` endpoint
- **Metrics Dashboard** — polls the `/metrics` Prometheus-compatible endpoint
- **Module Inspector** — reads from `/database/schema` and `/database/module_def`

> ⚠️ **Note:** SpacetimeDB 2.0 introduced breaking changes to the subscription protocol and schema endpoints. If you are running SpacetimeDB 1.x, upgrade your server before using `spacetimedb-tui` for the best experience.

---

## ⚙️ Configuration

`spacetimedb-tui` reads its configuration from `~/.config/stdb-tui/config.toml` by default. You can override this path with `--config <PATH>`.

### Full Example `config.toml`

```toml
# ~/.config/stdb-tui/config.toml

# ── Connection ─────────────────────────────────────────────────────────────────
[connection]
host     = "localhost"
port     = 3000
database = ""           # leave empty to show the database browser on startup
token    = ""           # or set env var STDB_TOKEN

# ── UI ─────────────────────────────────────────────────────────────────────────
[ui]
theme          = "dark"          # "dark" | "light" | "solarized"
starting_panel = "browser"       # "browser" | "sql" | "logs" | "metrics" | "modules"
mouse_support  = false           # enable experimental mouse support
show_statusbar = true
show_borders   = true
date_format    = "%Y-%m-%d %H:%M:%S"

# ── Table Viewer ───────────────────────────────────────────────────────────────
[table_viewer]
page_size      = 50              # rows per page
live_updates   = true            # enable WebSocket subscription by default
highlight_new  = true            # flash new rows green
highlight_del  = true            # flash deleted rows red
max_cell_width = 40              # truncate cell display width

# ── SQL Console ────────────────────────────────────────────────────────────────
[sql]
history_file   = "~/.local/share/stdb-tui/sql_history"
history_limit  = 500
auto_complete  = true
editor         = ""              # leave empty to use $EDITOR

# ── Metrics ────────────────────────────────────────────────────────────────────
[metrics]
refresh_interval_ms = 2000       # poll interval in milliseconds
sparkline_window    = 60         # seconds of history shown in sparklines

# ── Logs ───────────────────────────────────────────────────────────────────────
[logs]
default_level  = "info"          # "debug" | "info" | "warn" | "error"
max_buffer     = 10000           # maximum log lines held in memory
wrap_lines     = false

# ── Key Bindings (override defaults) ───────────────────────────────────────────
[keybindings]
quit           = "q"
help           = "?"
refresh        = "F5"
focus_next     = "Tab"
focus_prev     = "BackTab"
```

### Environment Variables

| Variable | Description |
|---|---|
| `STDB_TOKEN` | SpacetimeDB auth token (overrides `config.toml`) |
| `STDB_HOST` | Server host (overrides `config.toml`) |
| `STDB_PORT` | Server port (overrides `config.toml`) |
| `STDB_DATABASE` | Default database (overrides `config.toml`) |
| `STDB_TUI_CONFIG` | Path to config file (overrides default location) |
| `EDITOR` | Editor used by the SQL console `Ctrl+e` command |
| `NO_COLOR` | Set to any value to disable all terminal colours |

---

## 🤝 Contributing

Contributions are warmly welcomed! Whether it's a bug fix, a new feature, improved documentation, or a new theme — we'd love to have your help.

### Getting Started

```bash
# Fork and clone the repository
git clone https://github.com/<your-username>/spacetimedb-tui.git
cd spacetimedb-tui

# Install development dependencies (just, cargo-nextest recommended)
cargo install just cargo-nextest

# Run the test suite
cargo nextest run

# Run with debug logging enabled
RUST_LOG=debug cargo run -- --host localhost --port 3000

# Check formatting and lints before committing
cargo fmt --check
cargo clippy -- -D warnings
```

### Development Workflow

1. **Fork** the repository on GitHub.
2. **Create a branch** from `main` with a descriptive name:
   ```bash
   git checkout -b feat/module-inspector-search
   git checkout -b fix/sql-history-overflow
   ```
3. **Write your code.** Keep commits small and focused. Follow the existing code style.
4. **Add or update tests** for any changed behaviour.
5. **Run the full check suite:**
   ```bash
   just ci   # equivalent to: fmt check → clippy → nextest run
   ```
6. **Open a Pull Request** against the `main` branch. Fill in the PR template and link any related issues.
7. A maintainer will review your PR, leave feedback, and merge when ready. 🎉

### Contribution Guidelines

- **Code style:** `rustfmt` defaults. Run `cargo fmt` before every commit.
- **Lints:** All `cargo clippy` warnings are treated as errors in CI.
- **Commit messages:** Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, `chore:`, etc.).
- **Tests:** New features should include unit or integration tests. UI behaviour can be tested via snapshot tests using `insta`.
- **Breaking changes:** Discuss in an issue before implementing. Prefix commit with `feat!:` or `fix!:` and update `CHANGELOG.md`.
- **Dependencies:** Prefer adding to `[dev-dependencies]` when a crate is only needed for tests/tooling. Minimise production dependency surface.

### Reporting Bugs

Please [open an issue](https://github.com/alice-ai/spacetimedb-tui/issues/new?template=bug_report.md) and include:
- Your OS and terminal emulator
- `stdb-tui --version` output
- SpacetimeDB server version (`spacetime version`)
- Steps to reproduce
- Expected vs actual behaviour
- Any relevant logs (`RUST_LOG=debug stdb-tui 2>debug.log`)

### Requesting Features

[Open a feature request](https://github.com/alice-ai/spacetimedb-tui/issues/new?template=feature_request.md) describing the use case and the proposed UX. We triage feature requests weekly.

### Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](https://www.contributor-covenant.org/version/2/1/code_of_conduct/). By participating, you agree to uphold a welcoming and respectful environment for everyone.

---

## 📄 License

`spacetimedb-tui` is open source software, released under the **MIT License**.

Copyright © 2024 **Alice AI & Beyond Horizons Industries**

See the [LICENSE](./LICENSE) file for the full license text.

---

<div align="center">

Made with ❤️ and ☕ by [Alice AI & Beyond Horizons Industries](https://github.com/alice-ai)

*Exploring the SpacetimeDB universe, one terminal at a time.*

</div>

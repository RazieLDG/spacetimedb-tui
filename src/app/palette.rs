//! Command palette input handling and command dispatch.

use super::*;

impl super::App {
    // ── Command palette (Faz 6.3) ────────────────────────────────────────

    /// Route a key event into the active command palette overlay.
    /// Mirrors `handle_modal_key`'s "take, mutate, put back" pattern
    /// so we never hold two borrows on `state` at once.
    pub(crate) async fn handle_palette_key(&mut self, key: KeyEvent) {
        let Some(mut palette) = self.state.palette.take() else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                // Cancel — drop the palette entirely.
                return;
            }
            KeyCode::Enter => {
                if let Some(cmd) = palette.current() {
                    self.dispatch_command(cmd).await;
                }
                return;
            }
            KeyCode::Down | KeyCode::Tab => {
                let len = palette.filter().len();
                palette.next(len);
            }
            KeyCode::Up | KeyCode::BackTab => {
                palette.prev();
            }
            KeyCode::Backspace => {
                palette.query.backspace();
                palette.selected = 0;
            }
            KeyCode::Char(ch) => {
                palette.query.insert(ch);
                palette.selected = 0;
            }
            _ => {}
        }

        self.state.palette = Some(palette);
    }

    /// Run the action behind a [`Command`].
    pub(crate) async fn dispatch_command(&mut self, cmd: crate::state::palette::Command) {
        use crate::state::palette::Command as C;
        match cmd {
            C::GotoTables => self.state.current_tab = Tab::Tables,
            C::GotoSql => self.state.current_tab = Tab::Sql,
            C::GotoLogs => self.state.current_tab = Tab::Logs,
            C::GotoMetrics => self.state.current_tab = Tab::Metrics,
            C::GotoModule => self.state.current_tab = Tab::Module,
            C::GotoLive => self.state.current_tab = Tab::Live,
            C::RefreshCurrentView => self.refresh_current_view().await,
            C::ReconnectWebSocket => {
                self.connect_ws().await;
                self.state
                    .set_notification("Reconnecting WebSocket…".to_string());
            }
            C::ToggleHelp => {
                self.state.show_help = !self.state.show_help;
                self.state.help_scroll = 0;
            }
            C::ExportCsv => {
                self.export_current_result(crate::ui::export::ExportFormat::Csv);
            }
            C::ExportJson => {
                self.export_current_result(crate::ui::export::ExportFormat::Json);
            }
            C::CopyCell => self.copy_selected_cell(),
            C::CopyRow => self.copy_selected_row(),
            C::Quit => self.state.should_quit = true,
        }
    }
}

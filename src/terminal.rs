//! Terminal setup/teardown owned by a RAII guard.
//!
//! [`TerminalGuard`] acquires raw mode, the alternate screen, mouse
//! capture, and a hidden cursor in order, recording each completed
//! step. If any step fails, only the steps that already succeeded are
//! rolled back. [`TerminalGuard::restore`] is idempotent, and `Drop`
//! calls it, so the terminal is restored on normal return, error
//! return, and unwind alike — without restoring twice.
//!
//! [`TerminalOps`] abstracts the underlying terminal operations so the
//! guard's partial/idempotent logic is unit-testable without touching a
//! real terminal. [`CrosstermTerminalOps`] is the production adapter.

/// Abstract terminal operations so [`TerminalGuard`] can be tested
/// without a real terminal.
pub trait TerminalOps {
    type Error;
    fn enable_raw_mode(&mut self) -> Result<(), Self::Error>;
    fn enter_alternate_screen(&mut self) -> Result<(), Self::Error>;
    fn enable_mouse_capture(&mut self) -> Result<(), Self::Error>;
    fn hide_cursor(&mut self) -> Result<(), Self::Error>;
    fn show_cursor(&mut self) -> Result<(), Self::Error>;
    fn disable_mouse_capture(&mut self) -> Result<(), Self::Error>;
    fn leave_alternate_screen(&mut self) -> Result<(), Self::Error>;
    fn disable_raw_mode(&mut self) -> Result<(), Self::Error>;
}

/// Production [`TerminalOps`] backed by `crossterm`.
///
/// Stateless: each call reacquires `std::io::stdout()`, so the guard
/// never holds the stdout handle that the Ratatui backend later owns.
#[derive(Debug, Default, Clone, Copy)]
pub struct CrosstermTerminalOps;

impl CrosstermTerminalOps {
    pub fn new() -> Self {
        Self
    }
}

impl TerminalOps for CrosstermTerminalOps {
    type Error = std::io::Error;

    fn enable_raw_mode(&mut self) -> Result<(), Self::Error> {
        crossterm::terminal::enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
    }

    fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)
    }

    fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
    }

    fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)
    }

    fn disable_raw_mode(&mut self) -> Result<(), Self::Error> {
        crossterm::terminal::disable_raw_mode()
    }
}

/// Owns the terminal's altered state and restores it exactly once.
///
/// Tracks which setup steps completed so a failure partway through
/// [`Self::enter`] rolls back only what succeeded, and so
/// [`Self::restore`] (also run from `Drop`) never restores twice.
pub struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw: bool,
    alternate: bool,
    mouse: bool,
    cursor_hidden: bool,
    restored: bool,
}

impl<O: TerminalOps> TerminalGuard<O> {
    /// Acquire raw mode, alternate screen, mouse capture, and a hidden
    /// cursor. On any failure, roll back the steps already completed
    /// and return the error.
    pub fn enter(ops: O) -> Result<Self, O::Error> {
        let mut guard = Self {
            ops,
            raw: false,
            alternate: false,
            mouse: false,
            cursor_hidden: false,
            restored: false,
        };
        guard.ops.enable_raw_mode()?;
        guard.raw = true;
        if let Err(error) = guard.ops.enter_alternate_screen() {
            guard.restore();
            return Err(error);
        }
        guard.alternate = true;
        if let Err(error) = guard.ops.enable_mouse_capture() {
            guard.restore();
            return Err(error);
        }
        guard.mouse = true;
        if let Err(error) = guard.ops.hide_cursor() {
            guard.restore();
            return Err(error);
        }
        guard.cursor_hidden = true;
        Ok(guard)
    }

    /// Restore the terminal, undoing only the steps that completed.
    /// Idempotent: a second call (or the `Drop` call) is a no-op.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        if self.cursor_hidden {
            let _ = self.ops.show_cursor();
            self.cursor_hidden = false;
        }
        if self.mouse {
            let _ = self.ops.disable_mouse_capture();
            self.mouse = false;
        }
        if self.alternate {
            let _ = self.ops.leave_alternate_screen();
            self.alternate = false;
        }
        if self.raw {
            let _ = self.ops.disable_raw_mode();
            self.raw = false;
        }
        self.restored = true;
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod terminal_guard_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default, Clone)]
    struct FakeOps {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail_after_raw: bool,
    }

    impl TerminalOps for FakeOps {
        type Error = &'static str;
        fn enable_raw_mode(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("enable_raw");
            Ok(())
        }
        fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("enter_alt");
            if self.fail_after_raw {
                Err("alt failed")
            } else {
                Ok(())
            }
        }
        fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("enable_mouse");
            Ok(())
        }
        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("hide_cursor");
            Ok(())
        }
        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("show_cursor");
            Ok(())
        }
        fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("disable_mouse");
            Ok(())
        }
        fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("leave_alt");
            Ok(())
        }
        fn disable_raw_mode(&mut self) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push("disable_raw");
            Ok(())
        }
    }

    #[test]
    fn partial_setup_restores_only_completed_steps() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let ops = FakeOps {
            calls: calls.clone(),
            fail_after_raw: true,
        };

        let result = TerminalGuard::enter(ops);

        assert!(result.is_err());
        assert_eq!(
            &*calls.borrow(),
            &["enable_raw", "enter_alt", "disable_raw"]
        );
    }

    #[test]
    fn restore_is_idempotent() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let ops = FakeOps {
            calls: calls.clone(),
            fail_after_raw: false,
        };
        let mut guard = TerminalGuard::enter(ops).unwrap();

        guard.restore();
        guard.restore();
        drop(guard);

        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == "disable_raw")
                .count(),
            1
        );
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == "leave_alt")
                .count(),
            1
        );
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == "disable_mouse")
                .count(),
            1
        );
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|call| **call == "show_cursor")
                .count(),
            1
        );
    }
}

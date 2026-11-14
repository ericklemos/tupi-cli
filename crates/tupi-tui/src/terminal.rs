use std::io::{self, Stdout};

use crossterm::cursor;
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            cursor::Hide,
            crossterm::event::EnableMouseCapture,
            crossterm::event::EnableBracketedPaste
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }

        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(
                    stdout,
                    LeaveAlternateScreen,
                    cursor::Show,
                    crossterm::event::DisableMouseCapture,
                    crossterm::event::DisableBracketedPaste
                );
                let _ = terminal::disable_raw_mode();
                return Err(error);
            }
        };

        Ok(Self { terminal })
    }

    pub fn draw<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(render).map(|_| ())
    }

    pub fn copy(&mut self, text: &str) -> io::Result<()> {
        use std::io::Write;

        let sequence = crate::clipboard::osc52(text);
        let escaped = self
            .terminal
            .backend_mut()
            .write_all(sequence.as_bytes())
            .and_then(|()| self.terminal.backend_mut().flush())
            .is_ok();

        // OSC 52 é ignorado por vários terminais; o helper do sistema cobre
        // esses casos. Se ao menos um caminho funcionou, a cópia valeu.
        if crate::clipboard::copy_to_system(text) || escaped {
            return Ok(());
        }

        Err(io::Error::other("clipboard indisponível"))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            cursor::Show,
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste
        );
        let _ = self.terminal.show_cursor();
    }
}

use {
    crate::{
        defer::Deferer,
        report::Result,
    },
    ::ratatui::{
        Terminal as RatatuiTermianl,
        backend::CrosstermBackend,
        crossterm::{
            execute,
            terminal::{
                EnterAlternateScreen,
                LeaveAlternateScreen,
                disable_raw_mode,
                enable_raw_mode,
            },
        },
    },
    ::std::{
        io::{
            StdoutLock,
            stdout,
        },
        mem::forget,
    },
};

pub struct Renderer {
    terminal: Terminal,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout().lock()))?;
        enable_raw_mode()?;

        let defer = Deferer::new(|| {
            disable_raw_mode()?;
            Result::Ok(())
        });

        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        forget(defer);

        Result::Ok(Self {
            terminal,
        })
    }

    pub fn terminal(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    fn deinit(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Result::Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if let Result::Err(error) = self.deinit() {
            error.report();
        }
    }
}

pub type Terminal<B = CrosstermBackend<StdoutLock<'static>>> = RatatuiTermianl<B>;

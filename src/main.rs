use {
    ::ratatui::{
        Terminal,
        backend::CrosstermBackend,
        buffer::Buffer,
        crossterm::{
            event::{
                Event,
                KeyCode,
                KeyEvent,
                KeyEventKind,
                read,
            },
            execute,
            terminal::{
                EnterAlternateScreen,
                LeaveAlternateScreen,
                disable_raw_mode,
                enable_raw_mode,
            },
        },
        layout::Rect,
        style::{
            Color,
            Modifier,
        },
        symbols::border::THICK,
        text::{
            Line,
            Span,
            Text,
        },
        widgets::{
            Block,
            Paragraph,
            StatefulWidget,
            Widget,
        },
    },
    ::std::{
        backtrace::Backtrace,
        error::Error,
        fmt::{
            Display,
            Formatter,
            Result as FmtResult,
        },
        fs::File,
        io::{
            BufWriter,
            stdout,
        },
        num::Saturating,
        panic::{
            set_hook,
            take_hook,
        },
        process::ExitCode,
        result::Result as StdResult,
    },
    ::tracing::{
        debug,
        error,
        field::display,
        info,
        instrument,
        level_filters::LevelFilter,
        trace,
    },
    ::tracing_appender::non_blocking::{
        NonBlocking,
        WorkerGuard,
    },
    ::tracing_error::{
        ErrorLayer,
        SpanTrace,
    },
    ::tracing_subscriber::{
        filter::EnvFilter,
        fmt::{
            Layer,
            format::FmtSpan,
        },
        layer::SubscriberExt as _,
        registry::Registry,
        util::SubscriberInitExt as _,
    },
};

fn main() -> ExitCode {
    match init() {
        Result::Ok(_guard) => match run() {
            Result::Ok(()) => ExitCode::SUCCESS,
            Result::Err(error) => {
                error.report();
                ExitCode::FAILURE
            },
        },
        Result::Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}

fn init() -> Result<WorkerGuard> {
    let (writer, guard) = NonBlocking::new(BufWriter::new(File::create("tracing.log")?));

    Registry::default()
        .with(
            EnvFilter::builder()
                .with_env_var("TRACING_LOG")
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()?,
        )
        .with(
            Layer::new()
                .with_writer(writer)
                .with_thread_ids(true)
                .with_thread_names(true)
                .with_file(true)
                .with_line_number(true)
                .with_span_events(FmtSpan::FULL)
                .with_ansi(false),
        )
        .with(ErrorLayer::default())
        .try_init()?;

    Result::Ok(guard)
}

#[instrument]
fn run() -> Result<()> {
    debug!("take panic hook");
    let hook = take_hook();
    debug!("set panic hook");

    set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or(info.payload().downcast_ref::<String>().map(String::as_str));

        let location = info.location().map(display);
        let backtrace = Backtrace::force_capture();
        let spantrace = SpanTrace::capture();
        error!(payload, location, %backtrace, %spantrace, "we panic");
        hook(info);
    }));

    info!("enable raw mode");
    enable_raw_mode()?;

    let deferrer = Deferrer::new(|| {
        info!("disable raw mode");
        disable_raw_mode()?;
        Result::Ok(())
    });

    let mut lock = stdout().lock();
    info!("enter alternate screen");
    execute!(lock, EnterAlternateScreen)?;

    let _deferrer = Deferrer::new(|| {
        drop(deferrer);
        info!("leave alternate screen");
        execute!(stdout(), LeaveAlternateScreen)?;
        Result::Ok(())
    });

    let mut terminal = Terminal::new(CrosstermBackend::new(lock))?;
    trace!(?terminal);
    let mut counter = Saturating(0);
    let mut exit = false;

    while !exit {
        terminal.draw(|frame| {
            frame.render_stateful_widget(Counter, frame.area(), &mut counter.0);
        })?;

        if let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = read()?
        {
            match code {
                KeyCode::Char('q') => {
                    info!("exit loop");
                    exit = true
                },
                KeyCode::Left => counter -= 1,
                KeyCode::Right => counter += 1,
                _ => (),
            }
        }
    }

    Result::Ok(())
}

struct Deferrer<F: FnOnce() -> Result<()>> {
    f: Option<F>,
}

impl<F: FnOnce() -> Result<()>> Deferrer<F> {
    fn new(f: F) -> Self {
        Self {
            f: Option::Some(f),
        }
    }
}

impl<F: FnOnce() -> Result<()>> Drop for Deferrer<F> {
    #[instrument(level = "debug", skip(self))]
    fn drop(&mut self) {
        if let Option::Some(f) = self.f.take() {
            if let Result::Err(error) = f() {
                error.report();
            }
        }
    }
}

struct Counter;

impl StatefulWidget for Counter {
    type State = u32;

    #[instrument(level = "trace", skip(self, buf))]
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let title = Line::styled(" Counter App Tutorial ", Modifier::BOLD);

        let instructions = Line::from(vec![
            Span::raw(" Decrement "),
            Span::styled("<Left>", (Color::Blue, Modifier::BOLD)),
            Span::raw(" Increment "),
            Span::styled("<Right>", (Color::Blue, Modifier::BOLD)),
            Span::raw(" Quit "),
            Span::styled("<Q> ", (Color::Blue, Modifier::BOLD)),
        ]);

        let block = Block::bordered()
            .title(title.centered())
            .title_bottom(instructions.centered())
            .border_set(THICK);

        let counter_text = Text::from(vec![Line::from(vec![
            Span::raw("Value: "),
            Span::styled(state.to_string(), Color::Yellow),
        ])]);

        Paragraph::new(counter_text)
            .centered()
            .block(block)
            .render(area, buf);
    }
}

struct Report {
    error: Box<dyn Send + Error>,
    backtrace: Backtrace,
    spantrace: SpanTrace,
}

impl Display for Report {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        self.error.fmt(f)?;
        f.write_str("\n\n")?;
        self.backtrace.fmt(f)?;
        f.write_str("\n\n")?;
        self.spantrace.fmt(f)?;
        Result::Ok(())
    }
}

impl Report {
    fn report(&self) {
        error!(error = self.error, backtrace = %self.backtrace, spantrace = %self.spantrace);
    }
}

impl<E: 'static + Send + Error> From<E> for Report {
    fn from(value: E) -> Self {
        Self {
            error: Box::new(value),
            backtrace: Backtrace::force_capture(),
            spantrace: SpanTrace::capture(),
        }
    }
}

type Result<T, E = Report> = StdResult<T, E>;

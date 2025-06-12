mod opus;

use {
    ::iroh::{
        NodeId,
        SecretKey,
    },
    ::rand::rngs::OsRng,
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
        layout::{
            Constraint,
            Rect,
        },
        style::Color,
        widgets::{
            Row,
            StatefulWidget,
            Table,
            TableState,
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
    let mut exit = false;
    let mut state = HomeState::new();

    while !exit {
        terminal.draw(|frame| {
            frame.render_stateful_widget(Home, frame.area(), &mut state);
        })?;

        if let Event::Key(KeyEvent {
            code: KeyCode::Char(char),
            kind: KeyEventKind::Press,
            ..
        }) = read()?
        {
            match char {
                'q' => {
                    info!("exit loop");
                    exit = true
                },
                'j' => state.table_state.select_next(),
                'k' => state.table_state.select_previous(),
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

struct Home;

struct HomeState {
    mode: Option<Mode>,
    secret: SecretKey,
    node_id: Option<NodeId>,
    table_state: TableState,
}

impl HomeState {
    fn new() -> Self {
        let mut state = Self {
            mode: Option::None,
            secret: SecretKey::from_bytes(&Default::default()),
            node_id: Option::None,
            table_state: TableState::new().with_selected_cell((0, 1)),
        };

        state.generate_secret();
        state
    }

    fn generate_secret(&mut self) {
        self.secret = SecretKey::generate(OsRng);
    }
}

enum Mode {
    Host,
    Join,
}

impl StatefulWidget for Home {
    type State = HomeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        StatefulWidget::render(
            Table::new(
                [
                    Row::new(["A", "B"]),
                    Row::new(["C", "D"]),
                    Row::new(["E", "F"]),
                ],
                [1, 2].map(Constraint::Fill),
            )
            .cell_highlight_style((Color::White, Color::Black)),
            area,
            buf,
            &mut state.table_state,
        );
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

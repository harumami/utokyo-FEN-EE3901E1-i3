use {
    ::color_eyre::{
        eyre::Result,
        install as install_eyre,
    },
    ::ratatui::crossterm::terminal::enable_raw_mode,
    ::std::{
        io::stderr,
        process::ExitCode,
    },
    ::tokio::{
        runtime::Runtime,
        signal::ctrl_c,
        task::JoinHandle,
    },
    ::tracing::{
        error,
        level_filters::LevelFilter,
        trace,
    },
    ::tracing_appender::non_blocking::{
        NonBlocking,
        WorkerGuard,
    },
    ::tracing_subscriber::{
        filter::EnvFilter,
        fmt::Layer,
        layer::SubscriberExt as _,
        registry::Registry,
        util::SubscriberInitExt as _,
    },
    ::webrtc::{
        api::APIBuilder,
        ice_transport::ice_server::RTCIceServer,
        peer_connection::configuration::RTCConfiguration,
    },
    ratatui::{
        Terminal,
        crossterm::{
            execute,
            terminal::{
                EnterAlternateScreen,
                LeaveAlternateScreen,
                disable_raw_mode,
            },
        },
        prelude::CrosstermBackend,
    },
    std::io::{
        StdoutLock,
        stdout,
    },
};

fn main() -> ExitCode {
    match init() {
        Result::Ok(_guard) => match run() {
            Result::Ok(_handle) => ExitCode::SUCCESS,
            Result::Err(error) => {
                error!(error = &*error);
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
    install_eyre()?;
    let (writer, guard) = NonBlocking::new(stderr());

    Registry::default()
        .with(Layer::new().with_writer(writer))
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()?,
        )
        .try_init()?;

    Result::Ok(guard)
}

fn run() -> Result<()> {
    // let renderer =
    let handle = Runtime::new()?.spawn(task());
    Result::Ok(())
}

async fn task() -> Result<()> {
    let connection = APIBuilder::new()
        .build()
        .new_peer_connection(RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .await?;

    trace!(%connection);
    ctrl_c().await?;
    connection.close().await?;
    Result::Ok(())
}

struct Renderer {
    terminal: Terminal<CrosstermBackend<StdoutLock<'static>>>,
}

impl Renderer {
    fn new() -> Result<Self> {
        let mut renderer = Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout().lock()))?,
        };

        enable_raw_mode()?;
        execute!(renderer.backend_mut(), EnterAlternateScreen)?;
        Result::Ok(renderer)
    }

    fn backend_mut(&mut self) -> &mut CrosstermBackend<StdoutLock<'static>> {
        self.terminal.backend_mut()
    }

    fn kill(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.backend_mut(), LeaveAlternateScreen)?;
        Result::Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if let Result::Err(error) = self.kill() {
            error!(error = &*error);
        }
    }
}

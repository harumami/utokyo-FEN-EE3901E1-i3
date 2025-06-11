mod defer;
mod render;
mod report;
mod widget;

use {
    crate::{
        render::Renderer,
        report::Result,
    },
    ::std::{
        fs::File,
        io::BufWriter,
        process::ExitCode,
    },
    ::tokio::{
        runtime::Runtime,
        signal::ctrl_c,
    },
    ::tracing::{
        level_filters::LevelFilter,
        trace,
    },
    ::tracing_appender::non_blocking::{
        NonBlocking,
        WorkerGuard,
    },
    ::tracing_error::ErrorLayer,
    ::tracing_subscriber::{
        filter::EnvFilter,
        fmt::{
            Layer,
            format::DefaultFields,
        },
        layer::SubscriberExt as _,
        registry::Registry,
        util::SubscriberInitExt as _,
    },
    ::webrtc::{
        api::APIBuilder,
        ice_transport::ice_server::RTCIceServer,
        peer_connection::configuration::RTCConfiguration,
    },
    ratatui::Frame,
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
        .with(Layer::new().with_writer(writer))
        .with(ErrorLayer::new(DefaultFields::new()))
        .try_init()?;

    Result::Ok(guard)
}

fn run() -> Result<()> {
    let mut renderer = Renderer::new()?;
    let handle = Runtime::new()?.spawn(task());

    renderer.terminal().draw(|frame| {
        if let Result::Err(error) = draw(frame) {
            error.report();
        }
    })?;

    Result::Ok(())
}

fn draw(frame: &mut Frame<'_>) -> Result<()> {
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

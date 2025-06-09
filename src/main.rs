use {
    ::color_eyre::{
        eyre::Result,
        install as install_eyre,
    },
    ::std::{
        io::stderr,
        process::ExitCode,
    },
    ::tracing::{
        error,
        level_filters::LevelFilter,
    },
    ::tracing_appender::non_blocking::{
        NonBlocking,
        WorkerGuard,
    },
    ::tracing_subscriber::{
        filter::EnvFilter,
        fmt::Layer as FmtLayer,
        layer::SubscriberExt as _,
        registry::Registry as SpanRegistry,
        util::SubscriberInitExt as _,
    },
    ::webrtc::api::APIBuilder,
    tokio::signal::ctrl_c,
    tracing::trace,
    webrtc::{
        ice_transport::ice_server::RTCIceServer,
        peer_connection::configuration::RTCConfiguration,
    },
};

#[tokio::main]
async fn main() -> ExitCode {
    match Result::Ok(()).and_then(|()| install_eyre()).and_then(|()| {
        let (writer, guard) = NonBlocking::new(stderr());

        SpanRegistry::default()
            .with(FmtLayer::new().with_writer(writer))
            .with(
                EnvFilter::builder()
                    .with_default_directive(LevelFilter::INFO.into())
                    .from_env()?,
            )
            .try_init()?;

        Result::Ok(guard)
    }) {
        Result::Ok(_guard) => {},
        Result::Err(error) => eprintln!("{error}"),
    }

    match run().await {
        Result::Ok(_guard) => ExitCode::SUCCESS,
        Result::Err(error) => {
            error!(error = &*error);
            ExitCode::FAILURE
        },
    }
}

async fn run() -> Result<WorkerGuard> {
    install_eyre()?;
    let (writer, guard) = NonBlocking::new(stderr());

    SpanRegistry::default()
        .with(FmtLayer::new().with_writer(writer))
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()?,
        )
        .try_init()?;

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
    Result::Ok(guard)
}

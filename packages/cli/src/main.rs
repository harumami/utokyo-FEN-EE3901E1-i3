use {
    ::clap::{
        Parser,
        Subcommand,
    },
    ::iroh::NodeId,
    ::phone::{
        Instance,
        Secret,
    },
    ::rancor::{
        BoxedError,
        ResultExt as _,
    },
    ::std::{
        error::Error,
        io::{
            BufWriter,
            stderr,
        },
        process::ExitCode,
    },
    ::tokio::{
        runtime::Runtime,
        signal::ctrl_c,
    },
    ::tracing::{
        debug,
        error,
        info,
        instrument,
        level_filters::LevelFilter,
        warn,
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
};

fn main() -> ExitCode {
    match init() {
        Result::Ok(result) => match result {
            Result::Ok((command, _guard)) => match run(command) {
                Result::Ok(()) => ExitCode::SUCCESS,
                Result::Err(error) => {
                    error!(error = &error as &dyn Error);
                    ExitCode::FAILURE
                },
            },
            Result::Err(code) => code,
        },
        Result::Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}

fn init() -> Result<Result<(Command, WorkerGuard), ExitCode>, BoxedError> {
    let Args {
        command,
        tracing,
    } = match Args::try_parse() {
        Result::Ok(args) => args,
        Result::Err(error) => {
            error.print().into_error()?;

            return Result::Ok(Result::Err(match error.exit_code() {
                0 => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }));
        },
    };

    let (writer, guard) = NonBlocking::new(BufWriter::new(stderr()));

    Registry::default()
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::ERROR.into())
                .parse(tracing.as_deref().unwrap_or(""))
                .into_error()?,
        )
        .with(Layer::new().with_writer(writer))
        .try_init()
        .into_error()?;

    Result::Ok(Result::Ok((command, guard)))
}

#[instrument(skip(command))]
fn run(command: Command) -> Result<(), BoxedError> {
    let (runtime, instance, connection) = match command {
        Command::Generate => {
            let secret = Secret::generate();
            println!("Your SECRET: {secret}");
            println!("Your Node ID: {}", secret.node_id());
            return Result::Ok(());
        },
        Command::Host {
            secret,
        } => {
            let runtime = Runtime::new().into_error()?;

            let (instance, connection) = runtime.block_on(async {
                let instance = Instance::bind(secret).await?;

                println!(
                    "Your Node ID: {}",
                    instance.endpoint().secret_key().public()
                );

                let connection = instance.accept::<BoxedError, _>().await?;
                Result::Ok((instance, connection))
            })?;

            (runtime, instance, connection)
        },
        Command::Join {
            node_id,
        } => {
            let runtime = Runtime::new().into_error()?;

            let (instance, connection) = runtime.block_on(async {
                let instance = Instance::bind(Option::None).await?;
                let connection = instance.connect::<BoxedError, _>(node_id).await?;
                Result::Ok((instance, connection))
            })?;

            (runtime, instance, connection)
        },
    };

    let recorder = connection.record::<BoxedError>();
    let player = connection.play::<BoxedError>();
    let close_handle = connection.close_handle().clone();

    runtime.spawn(async move {
        if let Result::Err(error) = ctrl_c().await {
            error!(error = &error as &dyn Error);
        }

        debug!("close stream");
        close_handle.close().await;
    });

    println!("Let's talk!");
    runtime.block_on(connection.join())?;
    println!("Bye.");

    if let Result::Err(error) = recorder {
        info!(error = &error as &dyn Error);
    }

    if let Result::Err(error) = player {
        info!(error = &error as &dyn Error);
    }

    runtime.block_on(instance.close());
    Result::Ok(())
}

#[derive(Parser)]
struct Args {
    #[clap(subcommand)]
    command: Command,
    #[clap(long)]
    tracing: Option<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    Generate,
    Host {
        #[clap(long)]
        secret: Option<Secret>,
    },
    Join {
        node_id: NodeId,
    },
}

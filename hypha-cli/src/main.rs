use {
    ::clap::{
        Parser,
        Subcommand,
    },
    ::hypha_core::{
        Instance,
        Secret,
    },
    ::iroh::NodeId,
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
        sync::Arc,
    },
    ::tokio::{
        io::{
            AsyncBufReadExt as _,
            BufReader,
            stdin,
        },
        runtime::Runtime,
        signal::ctrl_c,
        select,
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

    let connection = Arc::new(connection);

    let _recorder = match connection.record::<BoxedError, BoxedError>() {
        Result::Ok(recorder) => Option::Some(recorder),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let _player = match connection.play::<BoxedError, BoxedError>() {
        Result::Ok(player) => Option::Some(player),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let close_handle = connection.close_handle().clone();
    let mute_handle = connection.mute_handle();

    runtime.spawn(async move {
        if let Result::Err(error) = ctrl_c().await {
            error!(error = &error as &dyn Error);
        }

        debug!("close stream");
        close_handle.close();
    });

    let input_connection = connection.clone();
    let input_close_handle = connection.close_handle().clone();

    runtime.spawn(async move {
        let mut reader = BufReader::new(stdin());
        let mut line = String::new();

        loop {
            select! {
                _ = input_close_handle.wait() => break,
                result = reader.read_line(&mut line) => match result {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.trim() == "m" {
                            mute_handle.toggle();

                            if mute_handle.is_muted() {
                                println!("\n[ MUTED ]");
                            } else {
                                println!("\n[ UNMUTED ]");
                            }
                        }

                        line.clear();
                    }
                    Err(error) => {
                        warn!(error = &error as &dyn Error);
                        break;
                    },
                },
            }
        }
    });

    println!("Let's talk! (press 'm' and enter to mute, ctrl+c to exit)");
    runtime.block_on(connection.close_handle().wait());
    println!("\nBye.");
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

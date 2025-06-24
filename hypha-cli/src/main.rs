use {
    ::clap::{
        Parser,
        Subcommand,
    },
    ::hypha_core::{
        Address,
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
        io::{
            AsyncBufReadExt as _,
            BufReader,
            stdin,
        },
        runtime::Runtime,
    },
    ::tracing::{
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
    tracing::debug,
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
            address,
        } => {
            let runtime = Runtime::new().into_error()?;

            let (instance, connection) = runtime.block_on(async {
                let instance = Instance::bind(Option::None).await?;
                let connection = instance.connect::<BoxedError, _>(address).await?;
                Result::Ok((instance, connection))
            })?;

            (runtime, instance, connection)
        },
    };

    let mut directs_watch = instance.endpoint().direct_addresses();

    runtime.spawn(async move {
        let directs = match directs_watch.get() {
            Result::Ok(directs) => match directs {
                Option::Some(directs) => directs,
                Option::None => loop {
                    match directs_watch.updated().await {
                        Result::Ok(directs) => match directs {
                            Option::Some(directs) => break directs,
                            Option::None => {
                                debug!("directs are not found");
                                continue;
                            },
                        },
                        Result::Err(error) => {
                            error!(error = &error as &dyn Error);
                            return;
                        },
                    }
                },
            },
            Result::Err(error) => {
                error!(error = &error as &dyn Error);
                return;
            },
        };

        for direct in directs {
            println!("Your Direct: {}", direct.addr);
        }
    });

    let _recorder = match connection.record::<BoxedError>() {
        Result::Ok(recorder) => Option::Some(recorder),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let _player = match connection.play::<BoxedError>() {
        Result::Ok(player) => Option::Some(player),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let mute_handle = connection.mute_handle().clone();
    let close_handle = connection.close_handle().clone();

    runtime.spawn(async move {
        let mut reader = BufReader::new(stdin());
        let mut line = String::new();

        loop {
            match reader.read_line(&mut line).await {
                Result::Ok(0) => break,
                Result::Ok(_) => (),
                Result::Err(error) => {
                    warn!(error = &error as &dyn Error);
                    break;
                },
            }

            match line.trim() {
                "M" => {
                    mute_handle.toggle();

                    if mute_handle.is_muted() {
                        println!("[ MUTED ]");
                    } else {
                        println!("[ UNMUTED ]");
                    }
                },
                "Q" => {
                    close_handle.close();
                    break;
                },
                _ => (),
            }

            line.clear();
        }
    });

    println!("Let's talk! ('M' to mute, 'Q' to exit)");
    runtime.block_on(connection.join())?;
    println!("Bye.");
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
        address: Address,
    },
}

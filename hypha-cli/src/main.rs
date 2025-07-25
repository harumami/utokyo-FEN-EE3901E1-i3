use {
    ::clap::{
        Parser,
        Subcommand,
    },
    ::hypha_core::{
        Instance,
        PublicId,
        SecretId,
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
    let (secret, method) = match command {
        Command::Generate => {
            let secret = SecretId::generate();
            println!("Your SECRET ID: {secret}");
            println!("Your public ID: {}", secret.public());
            return Result::Ok(());
        },
        Command::Host {
            secret,
        } => (secret, Method::Host),
        Command::Join {
            id,
        } => (Option::None, Method::Join(id)),
    };

    let runtime = Runtime::new().into_error()?;
    let instance = runtime.block_on(Instance::bind(secret))?;

    println!(
        "Your public ID: {}",
        instance.endpoint().secret_key().public()
    );

    let (peer, data_stream) = runtime.block_on(async {
        Result::Ok(match method {
            Method::Host => {
                let peer = instance.accept().await?;
                let data_stream = peer.accept_stream::<BoxedError, _>().await?;
                (peer, data_stream)
            },
            Method::Join(id) => {
                let peer = instance.connect(id).await?;
                let data_stream = peer.open_stream::<BoxedError, _>().await?;
                (peer, data_stream)
            },
        })
    })?;

    let _rec_stream = match peer.record_stream::<BoxedError>() {
        Result::Ok(recorder) => Option::Some(recorder),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let _play_stream = match peer.play_stream::<BoxedError>() {
        Result::Ok(player) => Option::Some(player),
        Result::Err(error) => {
            info!(error = &error as &dyn Error);
            Option::None
        },
    };

    let mute_handle = peer.mute_handle().clone();
    let deafen_handle = peer.deafen_handle().clone();
    let close_handle = peer.close_handle().clone();

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

                    if mute_handle.is_on() {
                        println!("[ MUTED ]");
                    } else {
                        println!("[ UNMUTED ]");
                    }
                },
                "D" => {
                    deafen_handle.toggle();

                    if deafen_handle.is_on() {
                        println!("[ DEAFENED ]");
                    } else {
                        println!("[ UNDEAFENED ]");
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

    println!("Let's talk! ('M' to mute, 'D' to deafen, 'Q' to exit)");
    runtime.block_on(data_stream.join())?;
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
        secret: Option<SecretId>,
    },
    Join {
        id: PublicId,
    },
}

enum Method {
    Host,
    Join(PublicId),
}

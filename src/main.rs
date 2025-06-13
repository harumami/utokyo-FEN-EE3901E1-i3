mod opus;

use {
    crate::opus::{
        OPUS_APPLICATION_VOIP,
        OPUS_OK,
        OpusEncoder,
        opus_encode,
        opus_encoder_create,
        opus_encoder_destroy,
    },
    ::clap::{
        Parser,
        Subcommand,
    },
    ::color_eyre::{
        eyre::{
            OptionExt as _,
            Result,
            bail,
            ensure,
            eyre,
        },
        install,
    },
    ::cpal::{
        Data,
        SampleFormat,
        SizedSample,
        platform::{
            Host,
            default_host,
        },
        traits::{
            DeviceTrait,
            HostTrait,
            StreamTrait,
        },
    },
    ::crossbeam::queue::ArrayQueue,
    ::dasp::{
        sample::{
            Sample,
            ToSample,
            types::I24,
        },
        signal::{
            Signal,
            from_interleaved_samples_iter,
            from_iter,
        },
    },
    ::iroh::{
        NodeId,
        SecretKey,
        endpoint::{
            Endpoint,
            RecvStream,
            SendStream,
        },
    },
    ::rand::rngs::OsRng,
    ::std::{
        cmp::Reverse,
        error::Error,
        io::{
            BufWriter,
            stderr,
        },
        iter::from_fn,
        process::ExitCode,
        sync::Arc,
    },
    ::tokio::{
        sync::Notify,
        task::spawn_blocking,
    },
    ::tracing::{
        debug,
        error,
        info,
        instrument,
        trace,
        warn,
    },
    ::tracing_appender::non_blocking::{
        NonBlocking,
        WorkerGuard,
    },
    ::tracing_error::ErrorLayer,
    ::tracing_subscriber::{
        filter::EnvFilter,
        fmt::Layer,
        layer::SubscriberExt as _,
        registry::Registry,
        util::SubscriberInitExt as _,
    },
    cpal::{
        available_hosts,
        host_from_id,
    },
    dasp::interpolate::linear::Linear,
    std::collections::VecDeque,
};

#[tokio::main]
async fn main() -> ExitCode {
    match init() {
        Result::Ok(result) => match result {
            Result::Ok((command, _guard)) => match run(command).await {
                Result::Ok(()) => ExitCode::SUCCESS,
                Result::Err(error) => {
                    eprintln!("{error}");
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

fn init() -> Result<Result<(Command, WorkerGuard), ExitCode>> {
    install()?;

    let Args {
        command,
        tracing,
    } = match Args::try_parse() {
        Result::Ok(args) => args,
        Result::Err(error) => {
            error.print()?;

            return Result::Ok(Result::Err(match error.exit_code() {
                0 => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }));
        },
    };

    let (writer, guard) = NonBlocking::new(BufWriter::new(stderr()));

    Registry::default()
        .with(EnvFilter::try_new(tracing.as_deref().unwrap_or(""))?)
        .with(Layer::new().with_writer(writer))
        .with(ErrorLayer::default())
        .try_init()?;

    Result::Ok(Result::Ok((command, guard)))
}

#[instrument(skip(command))]
async fn run(command: Command) -> Result<()> {
    let alpn = b"/eeic-i3/31";

    match command {
        Command::Generate => {
            println!("{}", generate());

            for host in available_hosts() {
                println!("Host: {}", host.name());
                let host = host_from_id(host)?;

                for device in host.devices()? {
                    println!("Device: {}", device.name()?);

                    for input_config in device.supported_input_configs()? {
                        println!("Input Config: {input_config:?}");
                    }

                    for output_config in device.supported_output_configs()? {
                        println!("Output Config: {output_config:?}");
                    }

                    if device.supports_input() {
                        println!("Default Input Config: {:?}", device.default_input_config()?);
                    }

                    if device.supports_output() {
                        println!(
                            "Default Output Config: {:?}",
                            device.default_output_config()?
                        );
                    }
                }
            }
        },
        Command::Host {
            secret,
        } => {
            let endpoint = bind(secret, alpn).await?;
            debug!("accept connection");

            let connection = loop {
                match endpoint.accept().await.ok_or_eyre("no incoming")?.accept() {
                    Result::Ok(connecting) => break connecting.await?,
                    Result::Err(error) => {
                        debug!(error = &error as &dyn Error);
                        continue;
                    },
                }
            };

            trace!(?connection);
            info!("accepted connection");
            debug!("accept bi stream");
            let (send_stream, recv_stream) = connection.accept_bi().await?;
            commute(send_stream, recv_stream).await?;
            debug!("close endpoint");
            endpoint.close().await;
        },
        Command::Join {
            node_id,
        } => {
            let endpoint = bind(Option::None, alpn).await?;
            info!(%node_id, "connect to peer");

            let connection = endpoint
                .connect(node_id, alpn)
                .await
                .map_err(|error| eyre!(error.into_boxed_dyn_error()))?;

            trace!(?connection);
            info!("connected to peer");
            debug!("open bi stream");
            let (send_stream, recv_stream) = connection.open_bi().await?;
            commute(send_stream, recv_stream).await?;
            debug!("close endpoint");
            endpoint.close().await;
        },
    }

    Result::Ok(())
}

#[instrument]
fn generate() -> SecretKey {
    let secret = SecretKey::generate(OsRng);
    info!(%secret, "generated new secret key");
    secret
}

#[instrument]
async fn bind(secret: Option<SecretKey>, alpn: &[u8]) -> Result<Endpoint> {
    let secret = match secret {
        Option::Some(secret) => secret,
        Option::None => generate(),
    };

    info!(%secret, "use secret key");
    let node_id = secret.public();
    info!(%node_id, "bind to node id");
    println!("{node_id}");
    debug!("open endpoint");

    let endpoint = Endpoint::builder()
        .secret_key(secret)
        .alpns(vec![alpn.to_vec()])
        .discovery_n0()
        .bind()
        .await
        .map_err(|error| eyre!(error.into_boxed_dyn_error()))?;

    trace!(?endpoint);
    Result::Ok(endpoint)
}

#[instrument]
async fn commute(send_stream: SendStream, recv_stream: RecvStream) -> Result<()> {
    let host = default_host();
    trace!(host = ?host.id());
    let sample_rate = 48000;
    let channels = 2;

    if let Option::Some(output_device) = host.default_output_device() {}

    host.default_input_device();
    info!("start communication");
    Result::Ok(())
}

#[instrument(skip(host))]
async fn record(send_stream: SendStream, host: Host) -> Result<()> {
    const SAMPLE_RATE: usize = 48000;
    const CHANNELS: usize = 2;
    const FRAME_SIZE: usize = 960;
    const CAPACITY: usize = 16 * FRAME_SIZE;
    const MAX_PACKET_SIZE: usize = 4000;
    let device = host.default_input_device().ok_or_eyre("no input device")?;
    trace!(device = device.name()?);

    let mut configs = device.supported_input_configs()?.collect::<Vec<_>>();

    configs.sort_by_key(|config| {
        Reverse((
            match config.channels() {
                1 => 1,
                2 => 2,
                _ => 0,
            },
            config.max_sample_rate(),
        ))
    });

    let config = configs
        .first()
        .ok_or_eyre("no input configuration")?
        .with_max_sample_rate();

    trace!(?config);

    let stereo = match config.channels() {
        1 => false,
        2 => true,
        _ => bail!("no input configuration which is stereo or mono"),
    };

    let sample_rate = config.sample_rate().0;
    let buffer0 = Arc::new(RingBuffer::new(CAPACITY));
    let buffer1 = buffer0.clone();

    let input_stream = spawn_blocking(move || {
        device.build_input_stream_raw(
            &config.config(),
            config.sample_format(),
            move |data, _| match data.sample_format() {
                SampleFormat::I8 => buffer0.extend(data_to_frames::<i8>(data, stereo)),
                SampleFormat::I16 => buffer0.extend(data_to_frames::<i16>(data, stereo)),
                SampleFormat::I24 => buffer0.extend(data_to_frames::<I24>(data, stereo)),
                SampleFormat::I32 => buffer0.extend(data_to_frames::<i32>(data, stereo)),
                SampleFormat::I64 => buffer0.extend(data_to_frames::<i64>(data, stereo)),
                SampleFormat::U8 => buffer0.extend(data_to_frames::<u8>(data, stereo)),
                SampleFormat::U16 => buffer0.extend(data_to_frames::<u16>(data, stereo)),
                SampleFormat::U32 => buffer0.extend(data_to_frames::<u32>(data, stereo)),
                SampleFormat::U64 => buffer0.extend(data_to_frames::<u64>(data, stereo)),
                SampleFormat::F32 => buffer0.extend(data_to_frames::<f32>(data, stereo)),
                SampleFormat::F64 => buffer0.extend(data_to_frames::<f64>(data, stereo)),
                _ => (),
            },
            |error| error!(error = &error as &dyn Error),
            Option::None,
        )
    })
    .await??;

    input_stream.play()?;
    let mut encoder = Encoder::new(SAMPLE_RATE, CHANNELS)?;

    loop {
        buffer1.wait().await;

        encoder.input().extend(
            from_iter(buffer1.drain())
                .from_hz_to_hz(
                    Linear::new([0; 2], [0; 2]),
                    sample_rate as _,
                    SAMPLE_RATE as _,
                )
                .until_exhausted()
                .flatten(),
        );

        while encoder.ready(FRAME_SIZE) {
            encoder.encode(FRAME_SIZE, MAX_PACKET_SIZE)?;
        }
    }
}

fn data_to_frames<S: 'static + SizedSample + ToSample<i16>>(
    data: &Data,
    stereo: bool,
) -> impl Iterator<Item = [i16; 2]> {
    let frames = match data.as_slice::<S>() {
        Option::Some(data) => data,
        Option::None => {
            warn!("invalid type for given data");
            &[]
        },
    }
    .iter()
    .copied()
    .map(Sample::to_sample);

    match stereo {
        true => Option::Some(from_interleaved_samples_iter(frames).until_exhausted())
            .into_iter()
            .flatten()
            .chain(Option::None.into_iter().flatten()),
        false => Option::None.into_iter().flatten().chain(
            Option::Some(frames.map(|sample| [sample, sample]))
                .into_iter()
                .flatten(),
        ),
    }
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
        secret: Option<SecretKey>,
    },
    Join {
        node_id: NodeId,
    },
}

struct RingBuffer {
    queue: ArrayQueue<[i16; 2]>,
    notify: Notify,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            notify: Notify::new(),
        }
    }

    fn extend(&self, values: impl Iterator<Item = [i16; 2]>) {
        for value in values {
            self.queue.force_push(value);
        }

        self.notify.notify_one();
    }

    fn drain(&self) -> impl Iterator<Item = [i16; 2]> {
        from_fn(|| self.queue.pop())
    }

    async fn wait(&self) {
        self.notify.notified().await;
    }
}

struct Encoder {
    opus: *mut OpusEncoder,
    channels: usize,
    input: Vec<i16>,
    output: Vec<u8>,
}

impl Encoder {
    fn new(sample_rate: usize, channels: usize) -> Result<Self> {
        let mut error = 0;

        let opus = unsafe {
            opus_encoder_create(
                sample_rate as _,
                channels as _,
                OPUS_APPLICATION_VOIP as _,
                &mut error,
            )
        };

        ensure!(error == OPUS_OK as _, "failed to create opus encoder");

        Result::Ok(Self {
            opus,
            channels,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    fn input(&mut self) -> &mut Vec<i16> {
        &mut self.input
    }

    fn output(&self) -> &[u8] {
        &self.output
    }

    fn ready(&self, frame_size: usize) -> bool {
        self.input.len() >= self.channels * frame_size
    }

    fn encode(&mut self, frame_size: usize, max_packet_size: usize) -> Result<()> {
        ensure!(self.ready(frame_size), "not enough samples");
        self.output.resize(max_packet_size, 0);

        let n = unsafe {
            opus_encode(
                self.opus,
                self.input.as_ptr(),
                frame_size as _,
                self.output.as_mut_ptr(),
                self.output.len() as _,
            )
        };

        ensure!(n >= 0, "failed to encode opus");
        self.input.drain(0..self.channels * frame_size);
        self.output.truncate(n as _);
        Result::Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.opus) };
    }
}

mod opus;
mod speex;

use {
    crate::{
        opus::{
            OPUS_APPLICATION_VOIP,
            OpusDecoder,
            OpusEncoder,
            opus_decode_float,
            opus_decoder_create,
            opus_decoder_destroy,
            opus_encode_float,
            opus_encoder_create,
            opus_encoder_destroy,
            opus_strerror,
        },
        speex::{
            RESAMPLER_ERR_SUCCESS,
            SpeexResamplerState,
            speex_resampler_destroy,
            speex_resampler_init,
            speex_resampler_process_interleaved_float,
            speex_resampler_strerror,
        },
    },
    ::clap::{
        Parser,
        Subcommand,
    },
    ::cpal::{
        Data,
        FromSample,
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
        frame::Frame,
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
    ::rancor::{
        BoxedError,
        OptionExt as _,
        ResultExt as _,
        Source,
        fail,
    },
    ::rand::rngs::OsRng,
    ::std::{
        cmp::Reverse,
        error::Error,
        ffi::{
            CStr,
            c_int,
        },
        fmt::{
            Display,
            Formatter,
            Result as FmtResult,
        },
        io::{
            BufWriter,
            stderr,
        },
        iter::from_fn,
        process::ExitCode,
        sync::Arc,
    },
    ::tokio::{
        signal::ctrl_c,
        sync::{
            Notify,
            RwLock,
        },
        task::{
            block_in_place,
            spawn,
        },
    },
    ::tracing::{
        debug,
        error,
        info,
        instrument,
        level_filters::LevelFilter,
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
};

#[tokio::main]
async fn main() -> ExitCode {
    match init::<BoxedError>() {
        Result::Ok(result) => match result {
            Result::Ok((command, _guard)) => match run::<BoxedError>(command).await {
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

fn init<E: Source>() -> Result<Result<(Command, WorkerGuard), ExitCode>, E> {
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
        .with(ErrorLayer::default())
        .try_init()
        .into_error()?;

    Result::Ok(Result::Ok((command, guard)))
}

#[instrument(skip(command))]
async fn run<E: Source>(command: Command) -> Result<(), E> {
    let alpn = b"/eeic-i3/31";

    match command {
        Command::Generate => {
            println!("Your SECRET: {}", generate());
        },
        Command::Host {
            secret,
        } => {
            let endpoint = bind(secret, alpn).await?;
            debug!("accept connection");

            let connection = loop {
                match endpoint.accept().await.into_error()?.accept() {
                    Result::Ok(connecting) => break connecting.await.into_error()?,
                    Result::Err(error) => {
                        debug!(error = &error as &dyn Error);
                        continue;
                    },
                }
            };

            trace!(?connection);
            info!("accepted connection");
            debug!("accept bi stream");
            let (send_stream, recv_stream) = connection.accept_bi().await.into_error()?;
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
                .map_err(|error| AppError::Boxed(error.into_boxed_dyn_error()))
                .into_error()?;

            trace!(?connection);
            info!("connected to peer");
            debug!("open bi stream");
            let (send_stream, recv_stream) = connection.open_bi().await.into_error()?;
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
async fn bind<E: Source>(secret: Option<SecretKey>, alpn: &[u8]) -> Result<Endpoint, E> {
    let secret = match secret {
        Option::Some(secret) => secret,
        Option::None => generate(),
    };

    info!(%secret, "use secret key");
    let node_id = secret.public();
    info!(%node_id, "bind to node id");
    println!("Your Node ID: {node_id}");
    debug!("open endpoint");

    let endpoint = Endpoint::builder()
        .secret_key(secret)
        .alpns(vec![alpn.to_vec()])
        .discovery_n0()
        .bind()
        .await
        .map_err(|error| AppError::Boxed(error.into_boxed_dyn_error()))
        .into_error()?;

    trace!(?endpoint);
    Result::Ok(endpoint)
}

#[instrument]
async fn commute<E: Source>(send_stream: SendStream, recv_stream: RecvStream) -> Result<(), E> {
    debug!("start communication");
    let host = Arc::new(default_host());
    trace!(host = ?host.id());
    let sample_rate = 48000;
    let channels = 2;
    let frame_size = sample_rate as usize * 20 / 1000;
    let max_frame_size = sample_rate as usize * 120 / 1000;
    let max_packet_size = 4000;
    let quality = 7;
    let exit = Arc::new(RwLock::new(false));

    let play_handle = spawn(play::<E>(
        recv_stream,
        host.clone(),
        sample_rate,
        channels,
        max_frame_size,
        max_packet_size,
        quality,
        exit.clone(),
    ));

    let record_handle = spawn(record::<E>(
        send_stream,
        host,
        sample_rate,
        channels,
        frame_size,
        max_packet_size,
        quality,
        exit.clone(),
    ));

    spawn(async move {
        if let Result::Err(error) = ctrl_c().await {
            error!(error = &error as &dyn Error);
        }

        *exit.write().await = true;
    });

    println!("Let's talk!");

    if let Result::Err(error) = record_handle.await.into_error()? {
        warn!(error = &error as &dyn Error);
    }

    if let Result::Err(error) = play_handle.await.into_error()? {
        warn!(error = &error as &dyn Error);
    }

    println!("Bye.");
    Result::Ok(())
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip(send_stream, host))]
async fn record<E: Source>(
    mut send_stream: SendStream,
    host: Arc<Host>,
    sample_rate: u32,
    channels: u32,
    frame_size: usize,
    max_packet_size: usize,
    quality: u32,
    exit: Arc<RwLock<bool>>,
) -> Result<(), E> {
    debug!("get input device");
    let device = host.default_input_device().into_error()?;
    trace!(device = device.name().into_error()?);

    let mut configs = device
        .supported_input_configs()
        .into_error()?
        .collect::<Vec<_>>();

    configs.sort_by_key(|config| {
        Reverse((
            match config.channels() {
                1 => 1,
                2 => 2,
                _ => 0,
            },
            config.max_sample_rate(),
            config.sample_format().sample_size(),
            config.sample_format().is_float(),
        ))
    });

    let config = configs.first().into_error()?.with_max_sample_rate();
    trace!(?config);

    let stereo = match config.channels() {
        1 => false,
        2 => true,
        _ => fail!(AppError::Raw(
            "no input configuration which is stereo or mono"
        )),
    };

    trace!(stereo);
    let raw_sample_rate = config.sample_rate().0;
    trace!(raw_sample_rate);
    let buffer0 = Arc::new(RingBuffer::new(raw_sample_rate as usize / 10));
    let buffer1 = buffer0.clone();
    debug!("build input stream");

    let _input_stream = block_in_place(move || {
        let input_stream = device
            .build_input_stream_raw(
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
            .into_error()?;

        input_stream.play().into_error()?;

        Result::<_, E>::Ok('block: {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            break 'block ();

            #[cfg(not(all(target_os = "macos", target_os = "ios")))]
            break 'block input_stream;
        })
    })
    .into_error()?;

    let mut buffer2 = Vec::new();
    let mut resampler = Resampler::new(channels, raw_sample_rate, sample_rate, quality)?;
    let mut encoder = Encoder::new(sample_rate, channels)?;
    debug!("start record loop");

    while !*exit.read().await {
        buffer1.wait().await;
        debug!(used = buffer1.used());
        buffer2.extend(buffer1.drain().flatten());
        let (n, buffer3) = resampler.resample(&buffer2)?;
        buffer2.drain(0..(n * channels) as _);
        encoder.input().extend(buffer3);

        while encoder.ready(frame_size) {
            encoder.encode(frame_size, max_packet_size)?;

            if !encoder.output.is_empty() {
                debug!("send opus packet");
                let len = (encoder.output().len() as u32).to_le_bytes();
                send_stream.write_all(&len).await.into_error()?;
                send_stream.write_all(encoder.output()).await.into_error()?;
                send_stream.write_all(&[0; 4]).await.into_error()?;
            }
        }
    }

    debug!("exit record loop");
    send_stream.finish().into_error()?;
    send_stream.stopped().await.into_error()?;
    Result::Ok(())
}

#[instrument]
fn data_to_frames<S: 'static + SizedSample + ToSample<f32>>(
    data: &Data,
    stereo: bool,
) -> impl Iterator<Item = [f32; 2]> {
    let frames = match data.as_slice::<S>() {
        Option::Some(data) => {
            debug!(data_len = data.len());
            data
        },
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

#[allow(clippy::too_many_arguments)]
#[instrument(skip(recv_stream, host))]
async fn play<E: Source>(
    mut recv_stream: RecvStream,
    host: Arc<Host>,
    sample_rate: u32,
    channels: u32,
    max_frame_size: usize,
    max_packet_size: usize,
    quality: u32,
    exit: Arc<RwLock<bool>>,
) -> Result<(), E> {
    debug!("get output device");
    let device = host.default_output_device().into_error()?;
    trace!(device = device.name().into_error()?);

    let mut configs = device
        .supported_output_configs()
        .into_error()?
        .collect::<Vec<_>>();

    configs.sort_by_key(|config| {
        Reverse((
            match config.channels() {
                1 => 1,
                2 => 2,
                _ => 0,
            },
            config.max_sample_rate(),
            config.sample_format().sample_size(),
            config.sample_format().is_float(),
        ))
    });

    let config = configs.first().into_error()?.with_max_sample_rate();
    trace!(?config);

    let stereo = match config.channels() {
        1 => false,
        2 => true,
        _ => fail!(AppError::Raw(
            "no output configuration which is stereo or mono"
        )),
    };

    trace!(stereo);
    let raw_sample_rate = config.sample_rate().0;
    trace!(raw_sample_rate);
    let buffer0 = Arc::new(RingBuffer::new(raw_sample_rate as usize / 10));
    let buffer1 = buffer0.clone();
    debug!("build output stream");

    let _output_stream = block_in_place(move || {
        let output_stream = device
            .build_output_stream_raw(
                &config.config(),
                config.sample_format(),
                move |data, _| match data.sample_format() {
                    SampleFormat::I8 => frames_to_data::<i8>(data, buffer0.drain(), stereo),
                    SampleFormat::I16 => frames_to_data::<i16>(data, buffer0.drain(), stereo),
                    SampleFormat::I24 => frames_to_data::<I24>(data, buffer0.drain(), stereo),
                    SampleFormat::I32 => frames_to_data::<i32>(data, buffer0.drain(), stereo),
                    SampleFormat::I64 => frames_to_data::<i64>(data, buffer0.drain(), stereo),
                    SampleFormat::U8 => frames_to_data::<u8>(data, buffer0.drain(), stereo),
                    SampleFormat::U16 => frames_to_data::<u16>(data, buffer0.drain(), stereo),
                    SampleFormat::U32 => frames_to_data::<u32>(data, buffer0.drain(), stereo),
                    SampleFormat::U64 => frames_to_data::<u64>(data, buffer0.drain(), stereo),
                    SampleFormat::F32 => frames_to_data::<f32>(data, buffer0.drain(), stereo),
                    SampleFormat::F64 => frames_to_data::<f64>(data, buffer0.drain(), stereo),
                    _ => (),
                },
                |error| error!(error = &error as &dyn Error),
                Option::None,
            )
            .into_error()?;

        output_stream.play().into_error()?;

        Result::<_, E>::Ok('block: {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            break 'block ();

            #[cfg(not(all(target_os = "macos", target_os = "ios")))]
            break 'block output_stream;
        })
    })?;

    let mut buffer2 = Vec::new();
    let mut decoder = Decoder::new(sample_rate, channels)?;
    let mut resampler = Resampler::new(channels, sample_rate, raw_sample_rate, quality)?;
    debug!("start play loop");

    while !*exit.read().await {
        debug!("receive opus packed");
        let mut buffer = [0; 4];
        recv_stream.read_exact(&mut buffer).await.into_error()?;
        decoder.input().resize(u32::from_le_bytes(buffer) as _, 0);
        recv_stream.read_exact(decoder.input()).await.into_error()?;
        recv_stream.read_exact(&mut buffer).await.into_error()?;

        if buffer != [0; 4] {
            fail!(AppError::Raw("broken data"));
        }

        decoder.decode(max_frame_size)?;
        buffer2.extend_from_slice(decoder.output());
        let (n, buffer3) = resampler.resample(&buffer2)?;
        buffer2.drain(0..(n * channels) as _);
        buffer1.extend(buffer3.chunks(2).map(|frame| [frame[0], frame[1]]));
        debug!(used = buffer1.used());
    }

    debug!("exit play loop");
    Result::Ok(())
}

#[instrument(skip(frames))]
fn frames_to_data<S: 'static + SizedSample + Frame + FromSample<f32>>(
    data: &mut Data,
    frames: impl Iterator<Item = [f32; 2]>,
    stereo: bool,
) {
    let samples = match stereo {
        true => &mut frames.flatten() as &mut dyn Iterator<Item = _>,
        false => &mut frames.map(|[sample0, sample1]| (sample0 + sample1) / 2.0)
            as &mut dyn Iterator<Item = _>,
    };

    let mut signal = from_iter(samples).map(Sample::from_sample);

    #[allow(unused_mut)]
    for mut sample in match data.as_slice_mut::<S>() {
        Option::Some(data) => {
            debug!(data_len = data.len());
            data
        },
        Option::None => {
            warn!("invalid type for given data");
            &mut []
        },
    } {
        *sample = signal.next();
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
    queue: ArrayQueue<[f32; 2]>,
    notify: Notify,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            notify: Notify::new(),
        }
    }

    fn used(&self) -> f32 {
        self.queue.len() as f32 / self.queue.capacity() as f32
    }

    fn extend(&self, values: impl Iterator<Item = [f32; 2]>) {
        for value in values {
            self.queue.force_push(value);
        }

        self.notify.notify_one();
    }

    fn drain(&self) -> impl Iterator<Item = [f32; 2]> {
        from_fn(|| self.queue.pop())
    }

    async fn wait(&self) {
        self.notify.notified().await;
    }
}

struct Resampler {
    raw: *mut SpeexResamplerState,
    channels: u32,
    in_rate: u32,
    out_rate: u32,
    out_buffer: Vec<f32>,
}

impl Resampler {
    #[instrument]
    fn new<E: Source>(channels: u32, in_rate: u32, out_rate: u32, quality: u32) -> Result<Self, E> {
        let mut error = 0;

        let raw =
            unsafe { speex_resampler_init(channels, in_rate, out_rate, quality as _, &mut error) };

        handle_speex_error(error)?;

        Result::Ok(Self {
            raw,
            channels,
            in_rate,
            out_rate,
            out_buffer: Vec::new(),
        })
    }

    #[instrument(skip(self))]
    fn resample<E: Source>(&mut self, in_buffer: &[f32]) -> Result<(u32, &[f32]), E> {
        self.out_buffer.resize(
            in_buffer.len() * self.out_rate as usize / self.in_rate as usize + 1,
            0.0,
        );

        let mut in_len = in_buffer.len() as u32 / self.channels;
        let mut out_len = self.out_buffer.len() as u32 / self.channels;

        let error = unsafe {
            speex_resampler_process_interleaved_float(
                self.raw,
                in_buffer.as_ptr(),
                &mut in_len,
                self.out_buffer.as_mut_ptr(),
                &mut out_len,
            )
        };

        handle_speex_error(error)?;
        Result::Ok((in_len, &self.out_buffer[0..(self.channels * out_len) as _]))
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        unsafe { speex_resampler_destroy(self.raw) };
    }
}

unsafe impl Send for Resampler {}

struct Encoder {
    raw: *mut OpusEncoder,
    channels: u32,
    input: Vec<f32>,
    output: Vec<u8>,
}

impl Encoder {
    #[instrument]
    fn new<E: Source>(sample_rate: u32, channels: u32) -> Result<Self, E> {
        let mut error = 0;

        let raw = unsafe {
            opus_encoder_create(
                sample_rate as _,
                channels as _,
                OPUS_APPLICATION_VOIP as _,
                &mut error,
            )
        };

        handle_opus_error(error)?;

        Result::Ok(Self {
            raw,
            channels,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    fn input(&mut self) -> &mut Vec<f32> {
        &mut self.input
    }

    fn output(&self) -> &[u8] {
        &self.output
    }

    fn ready(&self, frame_size: usize) -> bool {
        self.input.len() >= self.channels as usize * frame_size
    }

    #[instrument(skip(self))]
    fn encode<E: Source>(&mut self, frame_size: usize, max_packet_size: usize) -> Result<(), E> {
        if !self.ready(frame_size) {
            fail!(AppError::Raw("not enough samples"));
        }

        self.output.resize(max_packet_size, 0);

        let n = unsafe {
            opus_encode_float(
                self.raw,
                self.input.as_ptr(),
                frame_size as _,
                self.output.as_mut_ptr(),
                self.output.len() as _,
            )
        };

        handle_opus_error(n)?;
        self.input.drain(0..self.channels as usize * frame_size);
        self.output.truncate(n as _);
        Result::Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.raw) };
    }
}

unsafe impl Send for Encoder {}

struct Decoder {
    raw: *mut OpusDecoder,
    channels: u32,
    input: Vec<u8>,
    output: Vec<f32>,
}

impl Decoder {
    #[instrument]
    fn new<E: Source>(sample_rate: u32, channels: u32) -> Result<Self, E> {
        let mut error = 0;
        let raw = unsafe { opus_decoder_create(sample_rate as _, channels as _, &mut error) };
        handle_opus_error(error)?;

        Result::Ok(Self {
            raw,
            channels,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    fn input(&mut self) -> &mut Vec<u8> {
        &mut self.input
    }

    fn output(&self) -> &[f32] {
        &self.output
    }

    #[instrument(skip(self))]
    fn decode<E: Source>(&mut self, max_frame_size: usize) -> Result<(), E> {
        self.output.resize(max_frame_size, 0.0);

        let frames = unsafe {
            opus_decode_float(
                self.raw,
                self.input.as_ptr(),
                self.input.len() as _,
                self.output.as_mut_ptr(),
                max_frame_size as _,
                0,
            )
        };

        handle_opus_error(frames)?;

        self.output
            .truncate(self.channels as usize * frames as usize);

        Result::Ok(())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.raw) };
    }
}

unsafe impl Send for Decoder {}

#[instrument]
fn handle_opus_error<E: Source>(error: c_int) -> Result<(), E> {
    if error < 0 {
        let error = unsafe { CStr::from_ptr(opus_strerror(error)) }
            .to_str()
            .into_error()?;

        fail!(AppError::Opus(error));
    }

    Result::Ok(())
}

#[instrument]
fn handle_speex_error<E: Source>(error: c_int) -> Result<(), E> {
    if error != RESAMPLER_ERR_SUCCESS as _ {
        let error = unsafe { CStr::from_ptr(speex_resampler_strerror(error)) }
            .to_str()
            .into_error()?;

        fail!(AppError::Speex(error));
    }

    Result::Ok(())
}

#[derive(Debug)]
enum AppError {
    Raw(&'static str),
    Boxed(Box<dyn Error + Send + Sync + 'static>),
    Opus(&'static str),
    Speex(&'static str),
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match self {
            Self::Raw(message) => write!(f, "{message}"),
            Self::Boxed(error) => write!(f, "{error}"),
            Self::Opus(error) => write!(f, "opus error: {error}"),
            Self::Speex(error) => write!(f, "speex error: {error}"),
        }
    }
}

impl Error for AppError {}

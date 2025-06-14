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
    ::rand::rngs::OsRng,
    ::std::{
        cmp::Reverse,
        error::Error,
        ffi::{
            CStr,
            c_int,
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
            spawn,
            spawn_blocking,
        },
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
    let host = Arc::new(default_host());
    trace!(host = ?host.id());
    let sample_rate = 48000;
    let channels = 2;
    let frame_size = sample_rate as usize * 20 / 1000;
    let max_frame_size = sample_rate as usize * 120 / 1000;
    let max_packet_size = 4000;
    let quality = 5;
    let exit = Arc::new(RwLock::new(false));

    let record_handle = spawn(record(
        send_stream,
        host.clone(),
        sample_rate,
        channels,
        frame_size,
        max_packet_size,
        quality,
        exit.clone(),
    ));

    let play_handle = spawn(play(
        recv_stream,
        host,
        sample_rate,
        channels,
        max_frame_size,
        max_packet_size,
        quality,
        exit.clone(),
    ));

    ctrl_c().await?;
    *exit.write().await = true;

    if let Result::Err(error) = record_handle.await? {
        eprintln!("{error}");
    }

    if let Result::Err(error) = play_handle.await? {
        eprintln!("{error}");
    }

    Result::Ok(())
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip(host))]
async fn record(
    mut send_stream: SendStream,
    host: Arc<Host>,
    sample_rate: u32,
    channels: u32,
    frame_size: usize,
    max_packet_size: usize,
    quality: u32,
    exit: Arc<RwLock<bool>>,
) -> Result<()> {
    debug!("get input device");
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

    trace!(stereo);
    let raw_sample_rate = config.sample_rate().0;
    let buffer0 = Arc::new(RingBuffer::new(16 * frame_size));
    let buffer1 = buffer0.clone();
    debug!("build input stream");

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
    let mut buffer2 = Vec::new();
    let mut resampler = Resampler::new(channels, raw_sample_rate, sample_rate, quality)?;
    let mut encoder = Encoder::new(sample_rate, channels)?;
    debug!("start record loop");

    while !*exit.read().await {
        buffer1.wait().await;
        buffer2.extend(buffer1.drain().flatten());
        let (n, buffer3) = resampler.resample(&buffer2)?;
        buffer2.drain(0..(n * channels) as usize);
        encoder.input().extend(buffer3);

        while encoder.ready(frame_size) {
            encoder.encode(frame_size, max_packet_size)?;

            if !encoder.output.is_empty() {
                debug!("send opus packet");
                let len = (encoder.output().len() as u32).to_le_bytes();
                send_stream.write_all(&len).await?;
                send_stream.write_all(encoder.output()).await?;
            }
        }
    }

    debug!("exit record loop");
    send_stream.finish()?;
    send_stream.stopped().await?;
    Result::Ok(())
}

fn data_to_frames<S: 'static + SizedSample + ToSample<f32>>(
    data: &Data,
    stereo: bool,
) -> impl Iterator<Item = [f32; 2]> {
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

#[allow(clippy::too_many_arguments)]
#[instrument(skip(host))]
async fn play(
    mut recv_stream: RecvStream,
    host: Arc<Host>,
    sample_rate: u32,
    channels: u32,
    max_frame_size: usize,
    max_packet_size: usize,
    quality: u32,
    exit: Arc<RwLock<bool>>,
) -> Result<()> {
    debug!("get output device");

    let device = host
        .default_output_device()
        .ok_or_eyre("no output device")?;

    trace!(device = device.name()?);
    let mut configs = device.supported_output_configs()?.collect::<Vec<_>>();

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
        .ok_or_eyre("no output configuration")?
        .with_max_sample_rate();

    trace!(?config);

    let stereo = match config.channels() {
        1 => false,
        2 => true,
        _ => bail!("no output configuration which is stereo or mono"),
    };

    trace!(stereo);
    let raw_sample_rate = config.sample_rate().0;
    let buffer0 = Arc::new(RingBuffer::new(16 * max_frame_size));
    let buffer1 = buffer0.clone();
    debug!("build output stream");

    let output_stream = spawn_blocking(move || {
        device.build_output_stream_raw(
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
    })
    .await??;

    output_stream.play()?;
    let mut buffer2 = Vec::new();
    let mut decoder = Decoder::new(sample_rate, channels)?;
    let mut resampler = Resampler::new(channels, sample_rate, raw_sample_rate, quality)?;
    debug!("start play loop");

    while !*exit.read().await {
        debug!("receive opus packed");
        let mut buffer = [0; 4];
        recv_stream.read_exact(&mut buffer).await?;
        decoder.input().resize(u32::from_le_bytes(buffer) as _, 0);
        recv_stream.read_exact(decoder.input()).await?;
        decoder.decode(max_frame_size)?;
        buffer2.extend_from_slice(decoder.output());
        let (n, buffer3) = resampler.resample(&buffer2)?;
        buffer2.drain(0..n as usize);
        buffer1.extend(buffer3.chunks(2).map(|frame| [frame[0], frame[1]]));
    }

    debug!("exit play loop");
    Result::Ok(())
}

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
        Option::Some(data) => data,
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
    fn new(channels: u32, in_rate: u32, out_rate: u32, quality: u32) -> Result<Self> {
        let mut error = 0;

        let raw =
            unsafe { speex_resampler_init(channels, in_rate, out_rate, quality as _, &mut error) };

        Self::handle_error(error)?;

        Result::Ok(Self {
            raw,
            channels,
            in_rate,
            out_rate,
            out_buffer: Vec::new(),
        })
    }

    fn resample(&mut self, in_buffer: &[f32]) -> Result<(u32, &[f32])> {
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

        Self::handle_error(error)?;
        Result::Ok((in_len, &self.out_buffer[0..(self.channels * out_len) as _]))
    }

    fn handle_error(error: c_int) -> Result<()> {
        if error != RESAMPLER_ERR_SUCCESS {
            let error = unsafe { CStr::from_ptr(speex_resampler_strerror(error)) }.to_str()?;
            bail!("speex error: {error}");
        }

        Result::Ok(())
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
    fn new(sample_rate: u32, channels: u32) -> Result<Self> {
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

    fn encode(&mut self, frame_size: usize, max_packet_size: usize) -> Result<()> {
        ensure!(self.ready(frame_size), "not enough samples");
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
    fn new(sample_rate: u32, channels: u32) -> Result<Self> {
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

    fn decode(&mut self, max_frame_size: usize) -> Result<()> {
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

fn handle_opus_error(error: c_int) -> Result<()> {
    if error < 0 {
        let error = unsafe { CStr::from_ptr(opus_strerror(error)) }.to_str()?;
        bail!("opus error: {error}");
    }

    Result::Ok(())
}

use {
    ::cpal::{
        SampleFormat,
        SizedSample,
        StreamConfig,
        platform::{
            Device,
            Host,
            Stream,
            default_host,
        },
        traits::{
            DeviceTrait as _,
            HostTrait as _,
            StreamTrait as _,
        },
    },
    ::crossbeam::queue::ArrayQueue,
    ::dasp::sample::{
        FromSample,
        I24,
        ToSample,
    },
    ::iroh::{
        KeyParsingError,
        NodeId,
        SecretKey,
        endpoint::{
            Endpoint,
            RecvStream,
            SendStream,
        },
    },
    ::log::{
        debug,
        error,
        trace,
        warn,
    },
    ::opus_sys::{
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
    ::rancor::{
        OptionExt as _,
        ResultExt as _,
        Source,
        fail,
    },
    ::rand::rngs::OsRng,
    ::speex_sys::{
        RESAMPLER_ERR_SUCCESS,
        SpeexResamplerState,
        speex_resampler_destroy,
        speex_resampler_init,
        speex_resampler_process_interleaved_float,
        speex_resampler_strerror,
    },
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
        iter::from_fn,
        marker::PhantomData,
        ops::Deref,
        str::FromStr,
        sync::Arc,
    },
    ::tokio::{
        sync::Notify,
        task::{
            JoinHandle,
            spawn,
        },
    },
    tokio::select,
};

#[derive(Clone, Debug)]
pub struct Secret {
    key: SecretKey,
}

impl Secret {
    pub fn generate() -> Self {
        Self {
            key: SecretKey::generate(OsRng),
        }
    }

    pub fn node_id(&self) -> NodeId {
        self.key.public()
    }

    pub fn key(&self) -> &SecretKey {
        &self.key
    }
}

impl Display for Secret {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        self.key.fmt(f)
    }
}

impl FromStr for Secret {
    type Err = KeyParsingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Result::Ok(Self {
            key: SecretKey::from_str(s)?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Instance {
    endpoint: Endpoint,
}

impl Instance {
    const ALPN: &[u8] = b"/eeic-i3/31";
    const CHANNELS: u32 = 2;
    const FRAME_SIZE: usize = Self::SAMPLE_RATE as usize * 20 / 1000;
    const MAX_FRAME_SIZE: usize = Self::SAMPLE_RATE as usize * 120 / 1000;
    const MAX_PACKET_SIZE: usize = 4000;
    const RESAMPLE_QUALITY: u32 = 7;
    const RING_SIZE: usize = Self::SAMPLE_RATE as usize * 100 / 1000;
    const SAMPLE_RATE: u32 = 48000;

    pub async fn bind<E: Source>(secret: Option<Secret>) -> Result<Self, E> {
        let secret = match secret {
            Option::Some(secret) => secret,
            Option::None => Secret::generate(),
        };

        trace!("use {secret} as secret");
        let node_id = secret.node_id();
        trace!("use {node_id} as node id");
        debug!("open endpoint");

        let endpoint = Endpoint::builder()
            .secret_key(secret.key().clone())
            .alpns(vec![Self::ALPN.to_vec()])
            .discovery_n0()
            .bind()
            .await
            .map_err(|error| AnyError(error.into_boxed_dyn_error()))
            .into_error()?;

        Result::Ok(Self {
            endpoint,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn accept<E0: Source, E1: Source>(&self) -> Result<Connection<E0>, E1> {
        debug!("accept connection");

        let connection = loop {
            match self.endpoint.accept().await.into_error()?.accept() {
                Result::Ok(connecting) => break connecting.await.into_error()?,
                Result::Err(error) => {
                    debug!("{error}");
                    continue;
                },
            }
        };

        debug!("accept bi stream");
        let (send_stream, mut recv_stream) = connection.accept_bi().await.into_error()?;
        let mut buffer = [0];
        recv_stream.read_exact(&mut buffer).await.into_error()?;

        if buffer != [0] {
            fail!(AnyError("connection is broken".into()));
        }

        let connection = Connection::new(send_stream, recv_stream)?;
        Result::Ok(connection)
    }

    pub async fn connect<E0: Source, E1: Source>(
        &self,
        node_id: NodeId,
    ) -> Result<Connection<E0>, E1> {
        debug!("connect to {node_id}");

        let connection = self
            .endpoint
            .connect(node_id, Self::ALPN)
            .await
            .map_err(|error| AnyError(error.into_boxed_dyn_error()))
            .into_error()?;

        debug!("open bi stream");
        let (mut send_stream, recv_stream) = connection.open_bi().await.into_error()?;
        send_stream.write_all(&[0]).await.into_error()?;
        let connection = Connection::new(send_stream, recv_stream)?;
        Result::Ok(connection)
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}

#[derive(Debug)]
pub struct Connection<E> {
    rec_ring: Arc<RingBuffer<[f32; 2]>>,
    play_ring: Arc<RingBuffer<[f32; 2]>>,
    close_handle: Arc<CloseHandle>,
    send_handle: JoinHandle<Result<(), E>>,
    recv_handle: JoinHandle<Result<(), E>>,
}

impl<E0> Connection<E0> {
    pub fn record<E1: Source, E2: Source>(&self) -> Result<Recorder, E1> {
        Recorder::new::<_, E2>(self.rec_ring.clone())
    }

    pub fn play<E1: Source, E2: Source>(&self) -> Result<Player, E1> {
        Player::new::<_, E2>(self.play_ring.clone())
    }

    pub fn close_handle(&self) -> &Arc<CloseHandle> {
        &self.close_handle
    }
}

impl<E0: Source> Connection<E0> {
    fn new<E1: Source>(
        mut send_stream: SendStream,
        mut recv_stream: RecvStream,
    ) -> Result<Self, E1> {
        let rec_ring = Arc::new(RingBuffer::new(Instance::RING_SIZE));
        let play_ring = Arc::new(RingBuffer::new(Instance::RING_SIZE));
        let close_handle = Arc::new(CloseHandle::new());

        let send_handle = {
            let rec_ring = rec_ring.clone();
            let close_handle = close_handle.clone();

            spawn(async move {
                let mut encoder = Encoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS)?;
                debug!("start record loop");

                let rec_loop = async {
                    loop {
                        rec_ring.wait().await;

                        debug!(
                            "input ring: {} of {} used: {}%",
                            rec_ring.len(),
                            rec_ring.capacity(),
                            100. * rec_ring.len() as f32 / rec_ring.capacity() as f32,
                        );

                        encoder.input().extend(rec_ring.drain().flatten());

                        while encoder.ready(Instance::FRAME_SIZE) {
                            encoder.encode(Instance::FRAME_SIZE, Instance::MAX_PACKET_SIZE)?;

                            if !encoder.output.is_empty() {
                                debug!("send opus packet");
                                let len = (encoder.output().len() as u32).to_le_bytes();
                                send_stream.write_all(&len).await.into_error()?;
                                send_stream.write_all(encoder.output()).await.into_error()?;
                            }
                        }
                    }

                    #[allow(unreachable_code)]
                    Result::<_, E0>::Ok(())
                };

                select! {
                    _ = rec_loop => (),
                    _ = close_handle.wait() => (),
                }

                debug!("exit record loop");
                send_stream.finish().into_error()?;
                send_stream.stopped().await.into_error()?;
                Result::<_, E0>::Ok(())
            })
        };

        let recv_handle = {
            let play_ring = play_ring.clone();
            let close_handle = close_handle.clone();

            spawn(async move {
                let mut decoder = Decoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS)?;

                debug!("start play loop");

                let play_loop = async {
                    loop {
                        debug!("receive opus packed");
                        let mut len = [0; 4];
                        recv_stream.read_exact(&mut len).await.into_error()?;
                        decoder.input().resize(u32::from_le_bytes(len) as _, 0);
                        recv_stream.read_exact(decoder.input()).await.into_error()?;
                        decoder.decode(Instance::MAX_FRAME_SIZE)?;

                        play_ring
                            .extend(decoder.output().chunks(2).map(|frame| [frame[0], frame[1]]));

                        debug!(
                            "play ring: {} of {} used: {}%",
                            play_ring.len(),
                            play_ring.capacity(),
                            100. * play_ring.len() as f32 / play_ring.capacity() as f32,
                        );
                    }

                    #[allow(unreachable_code)]
                    Result::<_, E0>::Ok(())
                };

                select! {
                    _ = play_loop => (),
                    _ = close_handle.wait() => (),
                }

                debug!("exit play loop");
                Result::<_, E0>::Ok(())
            })
        };

        Result::Ok(Self {
            rec_ring,
            play_ring,
            close_handle,
            send_handle,
            recv_handle,
        })
    }

    pub async fn join<E1: Source>(self) -> Result<(), E1> {
        debug!("join send handle");

        if let Result::Err(error) = self.send_handle.await.into_error()? {
            warn!("{error}");
        }

        debug!("join recv handle");

        if let Result::Err(error) = self.recv_handle.await.into_error()? {
            warn!("{error}");
        }

        Result::Ok(())
    }
}

pub struct Recorder {
    host: Host,
    device: Device,
    stream: ThreadBound<Stream>,
}

impl Recorder {
    fn new<E0: Source, E1: Source>(ring: Arc<RingBuffer<[f32; 2]>>) -> Result<Self, E0> {
        let host = default_host();
        debug!("audio host: {:?}", host.id());
        let device = host.default_input_device().into_error()?;
        debug!("input audio device: {}", device.name().into_error()?);

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
        debug!("input audio config: {config:?}");

        let stream = match config.sample_format() {
            SampleFormat::I8 => Self::build_stream::<i8, _, E1>(&device, &config.config(), ring),
            SampleFormat::I16 => Self::build_stream::<i16, _, E1>(&device, &config.config(), ring),
            SampleFormat::I24 => Self::build_stream::<I24, _, E1>(&device, &config.config(), ring),
            SampleFormat::I32 => Self::build_stream::<i32, _, E1>(&device, &config.config(), ring),
            SampleFormat::I64 => Self::build_stream::<i64, _, E1>(&device, &config.config(), ring),
            SampleFormat::U8 => Self::build_stream::<u8, _, E1>(&device, &config.config(), ring),
            SampleFormat::U16 => Self::build_stream::<u16, _, E1>(&device, &config.config(), ring),
            SampleFormat::U32 => Self::build_stream::<u32, _, E1>(&device, &config.config(), ring),
            SampleFormat::U64 => Self::build_stream::<u64, _, E1>(&device, &config.config(), ring),
            SampleFormat::F32 => Self::build_stream::<f32, _, E1>(&device, &config.config(), ring),
            SampleFormat::F64 => Self::build_stream::<f64, _, E1>(&device, &config.config(), ring),
            format => fail!(AnyError(format!("unknown format: {format}").into())),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_stream<T: SizedSample + ToSample<f32>, E0: Source, E1: Source>(
        device: &Device,
        config: &StreamConfig,
        ring: Arc<RingBuffer<[f32; 2]>>,
    ) -> Result<ThreadBound<Stream>, E0> {
        debug!("build input audio stream");

        let stereo = match config.channels {
            1 => false,
            2 => true,
            _ => fail!(AnyError("no input config which is stereo or mono".into())),
        };

        debug!("input is stereo: {stereo}");
        debug!("input sample rate: {}", config.sample_rate.0);

        let mut resampler = Resampler::new(
            config.channels as _,
            config.sample_rate.0,
            Instance::SAMPLE_RATE,
            Instance::RESAMPLE_QUALITY,
        )?;

        let stream = device
            .build_input_stream::<T, _, _>(
                config,
                move |data, _| {
                    debug!("rec input frames: {}", data.len());
                    resampler
                        .input()
                        .extend(data.iter().copied().map(T::to_sample));

                    if let Result::Err(error) = resampler.resample::<E1>() {
                        error!("{error}");
                        return;
                    }

                    let frames = match stereo {
                        true => &mut resampler
                            .output()
                            .chunks(2)
                            .map(|frame| [frame[0], frame[1]])
                            as &mut dyn Iterator<Item = _>,
                        false => &mut resampler
                            .output()
                            .iter()
                            .copied()
                            .map(|sample| [sample, sample])
                            as &mut dyn Iterator<Item = _>,
                    };

                    ring.extend(frames);
                },
                |error| error!("{error}"),
                Option::None,
            )
            .into_error()?;

        Result::Ok(ThreadBound::new(stream))
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }
}

pub struct Player {
    host: Host,
    device: Device,
    stream: ThreadBound<Stream>,
}

impl Player {
    fn new<E0: Source, E1: Source>(ring: Arc<RingBuffer<[f32; 2]>>) -> Result<Self, E0> {
        let host = default_host();
        debug!("audio host: {:?}", host.id());
        let device = host.default_output_device().into_error()?;
        debug!("output audio device: {}", device.name().into_error()?);

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
        debug!("output audio config: {config:?}");

        let stream = match config.sample_format() {
            SampleFormat::I8 => Self::build_stream::<i8, _, E1>(&device, &config.config(), ring),
            SampleFormat::I16 => Self::build_stream::<i16, _, E1>(&device, &config.config(), ring),
            SampleFormat::I24 => Self::build_stream::<I24, _, E1>(&device, &config.config(), ring),
            SampleFormat::I32 => Self::build_stream::<i32, _, E1>(&device, &config.config(), ring),
            SampleFormat::I64 => Self::build_stream::<i64, _, E1>(&device, &config.config(), ring),
            SampleFormat::U8 => Self::build_stream::<u8, _, E1>(&device, &config.config(), ring),
            SampleFormat::U16 => Self::build_stream::<u16, _, E1>(&device, &config.config(), ring),
            SampleFormat::U32 => Self::build_stream::<u32, _, E1>(&device, &config.config(), ring),
            SampleFormat::U64 => Self::build_stream::<u64, _, E1>(&device, &config.config(), ring),
            SampleFormat::F32 => Self::build_stream::<f32, _, E1>(&device, &config.config(), ring),
            SampleFormat::F64 => Self::build_stream::<f64, _, E1>(&device, &config.config(), ring),
            format => fail!(AnyError(format!("unknown format: {format}").into())),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_stream<T: SizedSample + FromSample<f32>, E0: Source, E1: Source>(
        device: &Device,
        config: &StreamConfig,
        ring: Arc<RingBuffer<[f32; 2]>>,
    ) -> Result<ThreadBound<Stream>, E0> {
        debug!("build output audio stream");
        let channels = config.channels;

        let stereo = match channels {
            1 => false,
            2 => true,
            _ => fail!(AnyError("no output config which is stereo or mono".into())),
        };

        debug!("input is stereo: {stereo}");
        let sample_rate = config.sample_rate.0;
        debug!("input sample rate: {sample_rate}");

        let mut resampler = Resampler::new(
            channels as _,
            Instance::SAMPLE_RATE,
            sample_rate,
            Instance::RESAMPLE_QUALITY,
        )?;

        let mut cursor = 0;

        let stream = device
            .build_output_stream::<T, _, _>(
                config,
                move |data, _| {
                    debug!("play output frames: {}", data.len());
                    let mut data = data.iter_mut();

                    loop {
                        while cursor < resampler.output().len() {
                            let Option::Some(data) = data.next() else {
                                return;
                            };

                            *data = T::from_sample(resampler.output()[cursor]);
                            cursor += 1;
                        }

                        if ring.len() == 0 {
                            debug!("fill by equilibrium");

                            for data in data {
                                *data = T::EQUILIBRIUM;
                            }

                            return;
                        }

                        let frames = ring.drain().take(
                            data.len() * Instance::SAMPLE_RATE as usize
                                / sample_rate as usize
                                / channels as usize
                                + 32,
                        );

                        let samples = match stereo {
                            true => &mut frames.flatten() as &mut dyn Iterator<Item = _>,
                            false => &mut frames.map(|frame| (frame[0] + frame[1]) / 2.),
                        };

                        resampler.input().extend(samples);

                        match resampler.resample::<E1>() {
                            Result::Ok(()) => cursor = 0,
                            Result::Err(error) => error!("{error}"),
                        }
                    }
                },
                |error| error!("{error}"),
                Option::None,
            )
            .into_error()?;

        Result::Ok(ThreadBound::new(stream))
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }
}

#[derive(Debug)]
struct RingBuffer<T> {
    queue: ArrayQueue<T>,
    notify: Notify,
}

impl<T> RingBuffer<T> {
    fn new(capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            notify: Notify::new(),
        }
    }

    fn capacity(&self) -> usize {
        self.queue.capacity()
    }

    fn len(&self) -> usize {
        self.queue.len()
    }

    fn extend(&self, values: impl Iterator<Item = T>) {
        for value in values {
            self.queue.force_push(value);
        }

        self.notify.notify_waiters();
    }

    fn drain(&self) -> impl Iterator<Item = T> {
        from_fn(|| self.queue.pop())
    }

    async fn wait(&self) {
        self.notify.notified().await;
    }
}

#[derive(Debug)]
pub struct CloseHandle {
    notify: Notify,
}

impl CloseHandle {
    fn new() -> Self {
        Self {
            notify: Notify::new(),
        }
    }

    pub fn close(&self) {
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        self.notify.notified().await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
struct ThreadBound<T> {
    value: T,
    _phantom: PhantomData<*const ()>,
}

impl<T> ThreadBound<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            _phantom: PhantomData,
        }
    }
}

impl<T> Deref for ThreadBound<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[derive(Debug)]
struct Encoder {
    raw: *mut OpusEncoder,
    channels: u32,
    input: Vec<f32>,
    output: Vec<u8>,
}

impl Encoder {
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

        OpusError::new(error).into_error()?;

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

    fn encode<E: Source>(&mut self, frame_size: usize, max_packet_size: usize) -> Result<(), E> {
        self.output.resize(max_packet_size, 0);

        let size = unsafe {
            opus_encode_float(
                self.raw,
                self.input.as_ptr(),
                frame_size as _,
                self.output.as_mut_ptr(),
                self.output.len() as _,
            )
        };

        OpusError::new(size).into_error()?;
        self.input.drain(0..self.channels as usize * frame_size);
        self.output.truncate(size as _);
        Result::Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.raw) };
    }
}

unsafe impl Send for Encoder {}

#[derive(Debug)]
struct Decoder {
    raw: *mut OpusDecoder,
    channels: u32,
    input: Vec<u8>,
    output: Vec<f32>,
}

impl Decoder {
    fn new<E: Source>(sample_rate: u32, channels: u32) -> Result<Self, E> {
        let mut error = 0;
        let raw = unsafe { opus_decoder_create(sample_rate as _, channels as _, &mut error) };
        OpusError::new(error).into_error()?;

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

        OpusError::new(frames).into_error()?;

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

#[derive(Debug)]
struct OpusError(c_int);

impl OpusError {
    fn new(error: c_int) -> Result<(), Self> {
        match error {
            0.. => Result::Ok(()),
            ..0 => Result::Err(Self(error)),
        }
    }
}

impl Display for OpusError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match unsafe { CStr::from_ptr(opus_strerror(self.0)) }.to_str() {
            Result::Ok(error) => write!(f, "opus: {}", error),
            Result::Err(error) => write!(f, "opus: {}", error),
        }
    }
}

impl Error for OpusError {}

#[derive(Debug)]
struct Resampler {
    raw: *mut SpeexResamplerState,
    channels: u32,
    in_rate: u32,
    out_rate: u32,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Resampler {
    fn new<E: Source>(channels: u32, in_rate: u32, out_rate: u32, quality: u32) -> Result<Self, E> {
        let mut error = 0;

        let raw =
            unsafe { speex_resampler_init(channels, in_rate, out_rate, quality as _, &mut error) };

        SpeexError::new(error).into_error()?;

        Result::Ok(Self {
            raw,
            channels,
            in_rate,
            out_rate,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    fn input(&mut self) -> &mut Vec<f32> {
        &mut self.input
    }

    fn output(&self) -> &[f32] {
        &self.output
    }

    fn resample<E: Source>(&mut self) -> Result<(), E> {
        self.output.resize(
            self.input.len() * self.out_rate as usize / self.in_rate as usize + 1,
            0.0,
        );

        let mut in_len = self.input.len() as u32 / self.channels;
        let mut out_len = self.output.len() as u32 / self.channels;

        let error = unsafe {
            speex_resampler_process_interleaved_float(
                self.raw,
                self.input.as_ptr(),
                &mut in_len,
                self.output.as_mut_ptr(),
                &mut out_len,
            )
        };

        SpeexError::new(error).into_error()?;
        self.input.drain(..(self.channels * in_len) as usize);
        self.output.truncate((self.channels * out_len) as _);
        Result::Ok(())
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        unsafe { speex_resampler_destroy(self.raw) };
    }
}

unsafe impl Send for Resampler {}

#[derive(Debug)]
struct SpeexError(c_int);

impl SpeexError {
    fn new(error: c_int) -> Result<(), Self> {
        match error == RESAMPLER_ERR_SUCCESS as _ {
            true => Result::Ok(()),
            false => Result::Err(Self(error)),
        }
    }
}

impl Display for SpeexError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match unsafe { CStr::from_ptr(speex_resampler_strerror(self.0)) }.to_str() {
            Result::Ok(error) => write!(f, "speex: {}", error),
            Result::Err(error) => write!(f, "speex: {}", error),
        }
    }
}

impl Error for SpeexError {}

#[derive(Debug)]
struct AnyError(Box<dyn Error + Send + Sync + 'static>);

impl Display for AnyError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        self.0.fmt(f)
    }
}

impl Error for AnyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

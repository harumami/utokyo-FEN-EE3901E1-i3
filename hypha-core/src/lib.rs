use {
    ::cpal::{
        I24,
        SampleFormat,
        SampleRate,
        SizedSample,
        StreamConfig,
        SupportedStreamConfigRange,
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
    ::dasp::sample::{
        FromSample,
        ToSample,
    },
    ::hypha_ring::{
        Consumer,
        Producer,
        Ring,
    },
    ::iroh::{
        KeyParsingError,
        PublicKey,
        SecretKey,
        endpoint::{
            Connection,
            Endpoint,
            RecvStream,
            SendStream,
        },
    },
    ::opus_rs::{
        Decoder,
        Encoder,
    },
    ::rancor::{
        OptionExt as _,
        ResultExt as _,
        Source,
        fail,
    },
    ::rand::rngs::OsRng,
    ::speex_rs::Resampler,
    ::std::{
        cmp::Reverse,
        error::Error,
        fmt::{
            Debug,
            Display,
            Formatter,
            Result as FmtResult,
        },
        marker::PhantomData,
        ops::Deref,
        str::FromStr,
        sync::{
            Arc,
            atomic::{
                AtomicBool,
                Ordering,
            },
        },
    },
    ::tokio::{
        select,
        sync::Notify,
        task::{
            JoinHandle,
            spawn,
        },
    },
    ::tracing::{
        debug,
        error,
        trace,
        warn,
    },
};

#[derive(Clone, Debug)]
pub struct SecretId {
    key: SecretKey,
}

impl SecretId {
    pub fn new(key: SecretKey) -> Self {
        Self {
            key,
        }
    }

    pub fn generate() -> Self {
        Self::new(SecretKey::generate(OsRng))
    }

    pub fn public(&self) -> PublicId {
        PublicId::new(self.key.public())
    }

    pub fn key(&self) -> &SecretKey {
        &self.key
    }
}

impl Display for SecretId {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Debug::fmt(&self.key, f)
    }
}

impl FromStr for SecretId {
    type Err = KeyParsingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SecretKey::from_str(s).map(Self::new)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PublicId {
    key: PublicKey,
}

impl PublicId {
    fn new(key: PublicKey) -> Self {
        Self {
            key,
        }
    }

    pub fn key(&self) -> &PublicKey {
        &self.key
    }
}

impl Display for PublicId {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.key, f)
    }
}

impl FromStr for PublicId {
    type Err = KeyParsingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PublicKey::from_str(s).map(Self::new)
    }
}

#[derive(Clone, Debug)]
pub struct Instance {
    endpoint: Endpoint,
}

impl Instance {
    const ALPN: &[u8] = b"/harumami/hypha";
    const CHANNELS: u32 = 2;
    const FRAME_SIZE: usize = Self::SAMPLE_RATE as usize * 20 / 1000;
    const MAX_FRAME_SIZE: usize = Self::SAMPLE_RATE as usize * 120 / 1000;
    const MAX_PACKET_SIZE: usize = 4000;
    const RESAMPLE_QUALITY: u32 = 7;
    const RING_SIZE: usize = Self::SAMPLE_RATE as usize * 200 / 1000;
    const RING_THRESHOLD: usize = Self::RING_SIZE / 2;
    const SAMPLE_RATE: u32 = 48000;

    pub async fn bind<E: Source>(secret: Option<SecretId>) -> Result<Self, E> {
        let secret = match secret {
            Option::Some(secret) => secret,
            Option::None => SecretId::generate(),
        };

        let public = secret.public();
        trace!(%secret, %public);
        debug!("open endpoint");

        let endpoint = Endpoint::builder()
            .secret_key(secret.key().clone())
            .alpns(vec![Self::ALPN.to_vec()])
            .discovery_n0()
            .bind()
            .await
            .into_error()?;

        Result::Ok(Self {
            endpoint,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn accept<E: Source>(&self) -> Result<Peer, E> {
        debug!("accept connection");

        let connection = loop {
            match self.endpoint.accept().await.into_error()?.accept() {
                Result::Ok(connecting) => break connecting.await.into_error()?,
                Result::Err(error) => {
                    debug!(error = &error as &dyn Error);
                    continue;
                },
            }
        };

        Result::Ok(Peer::new(connection))
    }

    pub async fn connect<E: Source>(&self, id: PublicId) -> Result<Peer, E> {
        debug!(%id, "connect to");

        let connection = self
            .endpoint
            .connect(*id.key(), Self::ALPN)
            .await
            .into_error()?;

        Result::Ok(Peer::new(connection))
    }

    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}

#[derive(Debug)]
pub struct Peer {
    connection: Connection,
    rec_ring: Ring<[f32; 2], { Instance::RING_SIZE }>,
    play_ring: Ring<[f32; 2], { Instance::RING_SIZE }>,
    mute_handle: Arc<ToggleHandle>,
    deafen_handle: Arc<ToggleHandle>,
    close_handle: Arc<CloseHandle>,
}

impl Peer {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            rec_ring: Ring::new(),
            play_ring: Ring::new(),
            mute_handle: Arc::new(ToggleHandle::new()),
            deafen_handle: Arc::new(ToggleHandle::new()),
            close_handle: Arc::new(CloseHandle::new()),
        }
    }

    pub async fn accept_stream<E0: Source, E1: Source>(&self) -> Result<DataStream<E0>, E1> {
        debug!("accept bi stream");
        let (send_stream, mut recv_stream) = self.connection.accept_bi().await.into_error()?;

        let mut buffer = [0];
        recv_stream.read_exact(&mut buffer).await.into_error()?;

        if buffer != [0] {
            fail!(AnyError("connection is broken"));
        }

        let rec_consumer = self.rec_ring.consumer().into_error()?;
        let play_producer = self.play_ring.producer().into_error()?;

        let stream = DataStream::new(
            send_stream,
            recv_stream,
            rec_consumer,
            play_producer,
            &self.close_handle,
        )?;

        Result::Ok(stream)
    }

    pub async fn open_stream<E0: Source, E1: Source>(&self) -> Result<DataStream<E0>, E1> {
        debug!("open bi stream");
        let (mut send_stream, recv_stream) = self.connection.open_bi().await.into_error()?;
        send_stream.write_all(&[0]).await.into_error()?;
        let rec_consumer = self.rec_ring.consumer().into_error()?;
        let play_producer = self.play_ring.producer().into_error()?;

        let stream = DataStream::new(
            send_stream,
            recv_stream,
            rec_consumer,
            play_producer,
            &self.close_handle,
        )?;

        Result::Ok(stream)
    }

    pub fn record_stream<E: Source>(&self) -> Result<AudioStream, E> {
        AudioStream::record::<_>(&self.rec_ring, &self.mute_handle)
    }

    pub fn play_stream<E: Source>(&self) -> Result<AudioStream, E> {
        AudioStream::play::<_>(&self.play_ring, &self.deafen_handle)
    }

    pub fn mute_handle(&self) -> &Arc<ToggleHandle> {
        &self.mute_handle
    }

    pub fn deafen_handle(&self) -> &Arc<ToggleHandle> {
        &self.deafen_handle
    }

    pub fn close_handle(&self) -> &Arc<CloseHandle> {
        &self.close_handle
    }
}

pub struct DataStream<E> {
    send_handle: JoinHandle<Result<(), E>>,
    recv_handle: JoinHandle<Result<(), E>>,
}

impl<E0: Source> DataStream<E0> {
    fn new<E1: Source>(
        mut send_stream: SendStream,
        mut recv_stream: RecvStream,
        mut rec_consumer: Consumer<[f32; 2], { Instance::RING_SIZE }>,
        play_producer: Producer<[f32; 2], { Instance::RING_SIZE }>,
        close_handle: &Arc<CloseHandle>,
    ) -> Result<Self, E1> {
        let send_handle = {
            let close_handle = close_handle.clone();

            spawn(async move {
                let mut encoder =
                    Encoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS).into_error()?;

                debug!("start record loop");

                let rec_loop = {
                    let send_stream = &mut send_stream;

                    async move {
                        loop {
                            rec_consumer.notified().await;
                            debug!(rec_ring.len = rec_consumer.len());

                            encoder.input().extend(rec_consumer.drain().flatten());

                            while encoder.ready(Instance::FRAME_SIZE) {
                                encoder
                                    .encode(Instance::FRAME_SIZE, Instance::MAX_PACKET_SIZE)
                                    .into_error()?;

                                if !encoder.output().is_empty() {
                                    debug!("send opus packet");
                                    let len = (encoder.output().len() as u32).to_le_bytes();
                                    send_stream.write_all(&len).await.into_error()?;
                                    send_stream.write_all(encoder.output()).await.into_error()?;
                                }
                            }
                        }

                        #[allow(unreachable_code)]
                        Result::<_, E0>::Ok(())
                    }
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
            let close_handle = close_handle.clone();

            spawn(async move {
                let mut decoder =
                    Decoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS).into_error()?;

                debug!("start play loop");

                let play_loop = async move {
                    loop {
                        debug!("receive opus packed");
                        let mut len = [0; 4];
                        recv_stream.read_exact(&mut len).await.into_error()?;
                        decoder.input().resize(u32::from_le_bytes(len) as _, 0);
                        recv_stream.read_exact(decoder.input()).await.into_error()?;
                        decoder.decode(Instance::MAX_FRAME_SIZE).into_error()?;

                        if play_producer.len() > Instance::RING_THRESHOLD {
                            debug!(
                                len = play_producer.len(),
                                threshold = Instance::RING_THRESHOLD,
                                "play_ring level exceeded threshold"
                            );

                            continue;
                        }

                        if let Result::Err(samples) = play_producer
                            .extend(decoder.output().chunks(2).map(|frame| [frame[0], frame[1]]))
                        {
                            warn!(count = samples.count(), "drop samples from network");
                        }

                        debug!(play_ring.len = play_producer.len());
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
            send_handle,
            recv_handle,
        })
    }

    pub async fn join<E1: Source>(self) -> Result<(), E1> {
        debug!("join send handle");

        if let Result::Err(error) = self.send_handle.await.into_error()? {
            warn!(error = &error as &dyn Error);
        }

        debug!("join recv handle");

        if let Result::Err(error) = self.recv_handle.await.into_error()? {
            warn!(error = &error as &dyn Error);
        }

        Result::Ok(())
    }
}

pub struct AudioStream {
    host: Host,
    device: Device,
    stream: ThreadBound<Stream>,
}

impl AudioStream {
    fn record<E: Source>(
        ring: &Ring<[f32; 2], { Instance::RING_SIZE }>,
        mute_handle: &Arc<ToggleHandle>,
    ) -> Result<Self, E> {
        let producer = ring.producer().into_error()?;
        let host = default_host();
        debug!(host = host.id().name(), "audio host");
        let device = host.default_input_device().into_error()?;
        debug!(device = device.name().into_error()?, "input audio device");

        let mut configs = device
            .supported_input_configs()
            .into_error()?
            .collect::<Vec<_>>();

        configs.sort_by_key(Self::config_sort_key);
        let config = configs.first().into_error()?.with_max_sample_rate();
        debug!(?config, "input audio config");
        let mute_handle = mute_handle.clone();

        let stream = match config.sample_format() {
            SampleFormat::I8 => {
                Self::build_input::<i8, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::I16 => {
                Self::build_input::<i16, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::I24 => {
                Self::build_input::<I24, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::I32 => {
                Self::build_input::<i32, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::I64 => {
                Self::build_input::<i64, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::U8 => {
                Self::build_input::<u8, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::U16 => {
                Self::build_input::<u16, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::U32 => {
                Self::build_input::<u32, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::U64 => {
                Self::build_input::<u64, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::F32 => {
                Self::build_input::<f32, _>(&device, &config.config(), producer, mute_handle)
            },
            SampleFormat::F64 => {
                Self::build_input::<f64, _>(&device, &config.config(), producer, mute_handle)
            },
            format => fail!(UnknownFormatError(format)),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_input<T: SizedSample + ToSample<f32>, E: Source>(
        device: &Device,
        config: &StreamConfig,
        producer: Producer<[f32; 2], { Instance::RING_SIZE }>,
        mute_handle: Arc<ToggleHandle>,
    ) -> Result<ThreadBound<Stream>, E> {
        debug!("build input audio stream");

        let stereo = match config.channels {
            1 => false,
            2 => true,
            _ => fail!(AnyError("no input config which is stereo or mono")),
        };

        debug!(stereo, "input is stereo or not");
        debug!(sample_rate = config.sample_rate.0, "input sample rate");

        let mut resampler = Resampler::new(
            config.channels as _,
            config.sample_rate.0,
            Instance::SAMPLE_RATE,
            Instance::RESAMPLE_QUALITY,
        )
        .into_error()?;

        let stream = device
            .build_input_stream::<T, _, _>(
                config,
                move |data, _| {
                    debug!(len = data.len(), "rec input frames");
                    resampler
                        .input()
                        .extend(data.iter().copied().map(T::to_sample));

                    if let Result::Err(error) = resampler.resample() {
                        error!(error = &error as &dyn Error);
                        return;
                    }

                    if mute_handle.is_on() {
                        debug!("stream is muted");
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

                    if let Result::Err(samples) = producer.extend(frames) {
                        warn!(count = samples.count(), "drop samples from device");
                    }
                },
                |error| error!(error = &error as &dyn Error),
                Option::None,
            )
            .into_error()?;

        Result::Ok(ThreadBound::new(stream))
    }

    fn play<E: Source>(
        ring: &Ring<[f32; 2], { Instance::RING_SIZE }>,
        deafen_handle: &Arc<ToggleHandle>,
    ) -> Result<Self, E> {
        let consumer = ring.consumer().into_error()?;
        let host = default_host();
        debug!(host = host.id().name(), "audio host");
        let device = host.default_output_device().into_error()?;
        debug!(device = device.name().into_error()?, "output audio device");

        let mut configs = device
            .supported_output_configs()
            .into_error()?
            .collect::<Vec<_>>();

        configs.sort_by_key(Self::config_sort_key);
        let config = configs.first().into_error()?.with_max_sample_rate();
        debug!(?config, "output audio config");
        let deafen_handle = deafen_handle.clone();

        let stream = match config.sample_format() {
            SampleFormat::I8 => {
                Self::build_output::<i8, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::I16 => {
                Self::build_output::<i16, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::I24 => {
                Self::build_output::<I24, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::I32 => {
                Self::build_output::<i32, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::I64 => {
                Self::build_output::<i64, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::U8 => {
                Self::build_output::<u8, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::U16 => {
                Self::build_output::<u16, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::U32 => {
                Self::build_output::<u32, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::U64 => {
                Self::build_output::<u64, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::F32 => {
                Self::build_output::<f32, _>(&device, &config.config(), consumer, deafen_handle)
            },
            SampleFormat::F64 => {
                Self::build_output::<f64, _>(&device, &config.config(), consumer, deafen_handle)
            },
            format => fail!(UnknownFormatError(format)),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_output<T: SizedSample + FromSample<f32>, E: Source>(
        device: &Device,
        config: &StreamConfig,
        consumer: Consumer<[f32; 2], { Instance::RING_SIZE }>,
        deafen_handle: Arc<ToggleHandle>,
    ) -> Result<ThreadBound<Stream>, E> {
        debug!("build output audio stream");
        let channels = config.channels;

        let stereo = match channels {
            1 => false,
            2 => true,
            _ => fail!(AnyError("no output config which is stereo or mono")),
        };

        debug!(stereo, "output is stereo or not");
        let sample_rate = config.sample_rate.0;
        debug!(sample_rate, "output sample rate");

        let mut resampler = Resampler::new(
            channels as _,
            Instance::SAMPLE_RATE,
            sample_rate,
            Instance::RESAMPLE_QUALITY,
        )
        .into_error()?;

        let mut cursor = 0;

        let stream = device
            .build_output_stream::<T, _, _>(
                config,
                move |data, _| {
                    debug!(len = data.len(), "play output frames");
                    let mut data = data.iter_mut();

                    if deafen_handle.is_on() {
                        debug!("stream is deafened");

                        for data in data {
                            *data = T::EQUILIBRIUM;
                        }

                        return;
                    }

                    loop {
                        while cursor < resampler.output().len() {
                            let Option::Some(data) = data.next() else {
                                return;
                            };

                            *data = T::from_sample(resampler.output()[cursor]);
                            cursor += 1;
                        }

                        if consumer.is_empty() {
                            debug!("fill by equilibrium");

                            for data in data {
                                *data = T::EQUILIBRIUM;
                            }

                            return;
                        }

                        let frames = consumer.drain().take(
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

                        match resampler.resample() {
                            Result::Ok(()) => cursor = 0,
                            Result::Err(error) => error!(error = &error as &dyn Error),
                        }
                    }
                },
                |error| error!(error = &error as &dyn Error),
                Option::None,
            )
            .into_error()?;

        Result::Ok(ThreadBound::new(stream))
    }

    fn config_sort_key(
        config: &SupportedStreamConfigRange,
    ) -> Reverse<(i32, SampleRate, usize, bool)> {
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
pub struct ToggleHandle {
    on: AtomicBool,
}

impl ToggleHandle {
    fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
        }
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    pub fn toggle(&self) {
        let on = !self.on.fetch_xor(true, Ordering::AcqRel);
        debug!(on, "toggled");
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Debug)]
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
struct AnyError(&'static str);

impl Display for AnyError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        Display::fmt(&self.0, f)
    }
}

impl Error for AnyError {}

#[derive(Debug)]
struct UnknownFormatError(SampleFormat);

impl Display for UnknownFormatError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "unknown format: {}", self.0)
    }
}

impl Error for UnknownFormatError {}

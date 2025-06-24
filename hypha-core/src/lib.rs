use {
    ::cpal::{
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
    ::opus_rs::{
        Decoder,
        Encoder,
    },
    ::rancor::{
        BoxedError,
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
            Display,
            Formatter,
            Result as FmtResult,
        },
        iter::from_fn,
        marker::PhantomData,
        net::SocketAddr,
        ops::Deref,
        str::FromStr,
        sync::{
            Arc,
            atomic::{
                AtomicBool,
                Ordering,
            },
        },
        time::Duration,
    },
    ::tokio::{
        select,
        sync::Notify,
        task::{
            JoinHandle,
            spawn,
        },
        time::sleep,
    },
    ::tracing::{
        debug,
        error,
        trace,
        warn,
    },
};

#[derive(Clone, Debug)]
pub struct Secret {
    key: SecretKey,
}

impl Secret {
    pub fn new(key: SecretKey) -> Self {
        Self {
            key,
        }
    }

    pub fn generate() -> Self {
        Self::new(SecretKey::generate(OsRng))
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
    const ALPN: &[u8] = b"/harumami/hypha";
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
            .discovery_local_network()
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
                    debug!(error = &error as &dyn Error);
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
        address: Address,
    ) -> Result<Connection<E0>, E1> {
        debug!(%address, "connect to");

        let node_id = match address {
            Address::Id(id) => id,
            Address::Direct(direct) => 'label: {
                debug!(%direct, "search remote direct");

                for _ in 0..10 {
                    for remote in self.endpoint.remote_info_iter() {
                        let id = remote.node_id;
                        debug!(%id, "find remote");

                        for addr in &remote.addrs {
                            debug!(remote_addr = %addr.addr, "find remote address");

                            if addr.addr == direct {
                                break 'label id;
                            }
                        }
                    }

                    sleep(Duration::from_secs(1)).await;
                }

                fail!(AnyError("unknown address".into()));
            },
        };

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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Address {
    Id(NodeId),
    Direct(SocketAddr),
}

impl Display for Address {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Id(id) => id.fmt(f),
            Self::Direct(direct) => direct.fmt(f),
        }
    }
}

impl FromStr for Address {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match FromStr::from_str(s) {
            Result::Ok(id) => Result::Ok(Self::Id(id)),
            Result::Err(error0) => match FromStr::from_str(s) {
                Result::Ok(direct) => Result::Ok(Self::Direct(direct)),
                Result::Err(error1) => Result::Err(AnyError("invalid address".into()))
                    .into_error()
                    .trace(error0)
                    .trace(error1),
            },
        }
    }
}

#[derive(Debug)]
pub struct Connection<E> {
    rec_ring: Arc<RingBuffer<[f32; 2]>>,
    play_ring: Arc<RingBuffer<[f32; 2]>>,
    mute_handle: Arc<MuteHandle>,
    close_handle: Arc<CloseHandle>,
    send_handle: JoinHandle<Result<(), E>>,
    recv_handle: JoinHandle<Result<(), E>>,
}

impl<E0> Connection<E0> {
    pub fn record<E1: Source>(&self) -> Result<AudioStream, E1> {
        AudioStream::record::<_>(self.rec_ring.clone(), self.mute_handle.clone())
    }

    pub fn play<E1: Source>(&self) -> Result<AudioStream, E1> {
        AudioStream::play::<_>(self.play_ring.clone())
    }

    pub fn mute_handle(&self) -> &Arc<MuteHandle> {
        &self.mute_handle
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
        let mute_handle = Arc::new(MuteHandle::new());
        let close_handle = Arc::new(CloseHandle::new());

        let send_handle = {
            let rec_ring = rec_ring.clone();
            let close_handle = close_handle.clone();

            spawn(async move {
                let mut encoder =
                    Encoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS).into_error()?;

                debug!("start record loop");

                let rec_loop = async {
                    loop {
                        rec_ring.wait().await;

                        debug!(
                            len = rec_ring.len(),
                            cap = rec_ring.capacity(),
                            per = 100. * rec_ring.len() as f32 / rec_ring.capacity() as f32,
                            "input ring",
                        );

                        encoder.input().extend(rec_ring.drain().flatten());

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
                let mut decoder =
                    Decoder::new(Instance::SAMPLE_RATE, Instance::CHANNELS).into_error()?;

                debug!("start play loop");

                let play_loop = async {
                    loop {
                        debug!("receive opus packed");
                        let mut len = [0; 4];
                        recv_stream.read_exact(&mut len).await.into_error()?;
                        decoder.input().resize(u32::from_le_bytes(len) as _, 0);
                        recv_stream.read_exact(decoder.input()).await.into_error()?;
                        decoder.decode(Instance::MAX_FRAME_SIZE).into_error()?;

                        play_ring
                            .extend(decoder.output().chunks(2).map(|frame| [frame[0], frame[1]]));

                        debug!(
                            len = play_ring.len(),
                            cap = play_ring.capacity(),
                            per = 100. * play_ring.len() as f32 / play_ring.capacity() as f32,
                            "play ring",
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
            mute_handle,
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

#[derive(Debug)]
pub struct MuteHandle {
    muted: AtomicBool,
}

impl MuteHandle {
    fn new() -> Self {
        Self {
            muted: AtomicBool::new(false),
        }
    }

    pub fn toggle(&self) {
        let old = self.muted.fetch_xor(true, Ordering::AcqRel);
        debug!(muted = !old, "toggled mute");
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
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

pub struct AudioStream {
    host: Host,
    device: Device,
    stream: ThreadBound<Stream>,
}

impl AudioStream {
    fn record<E0: Source>(
        ring: Arc<RingBuffer<[f32; 2]>>,
        mute_handle: Arc<MuteHandle>,
    ) -> Result<Self, E0> {
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

        let stream = match config.sample_format() {
            SampleFormat::I8 => {
                Self::build_input::<i8, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::I16 => {
                Self::build_input::<i16, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::I24 => {
                Self::build_input::<I24, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::I32 => {
                Self::build_input::<i32, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::I64 => {
                Self::build_input::<i64, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::U8 => {
                Self::build_input::<u8, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::U16 => {
                Self::build_input::<u16, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::U32 => {
                Self::build_input::<u32, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::U64 => {
                Self::build_input::<u64, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::F32 => {
                Self::build_input::<f32, _>(&device, &config.config(), ring, mute_handle)
            },
            SampleFormat::F64 => {
                Self::build_input::<f64, _>(&device, &config.config(), ring, mute_handle)
            },
            format => fail!(AnyError(format!("unknown format: {format}").into())),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_input<T: SizedSample + ToSample<f32>, E0: Source>(
        device: &Device,
        config: &StreamConfig,
        ring: Arc<RingBuffer<[f32; 2]>>,
        mute_handle: Arc<MuteHandle>,
    ) -> Result<ThreadBound<Stream>, E0> {
        debug!("build input audio stream");

        let stereo = match config.channels {
            1 => false,
            2 => true,
            _ => fail!(AnyError("no input config which is stereo or mono".into())),
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

                    if mute_handle.is_muted() {
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

                    ring.extend(frames);
                },
                |error| error!(error = &error as &dyn Error),
                Option::None,
            )
            .into_error()?;

        Result::Ok(ThreadBound::new(stream))
    }

    fn play<E0: Source>(ring: Arc<RingBuffer<[f32; 2]>>) -> Result<Self, E0> {
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

        let stream = match config.sample_format() {
            SampleFormat::I8 => Self::build_output::<i8, _>(&device, &config.config(), ring),
            SampleFormat::I16 => Self::build_output::<i16, _>(&device, &config.config(), ring),
            SampleFormat::I24 => Self::build_output::<I24, _>(&device, &config.config(), ring),
            SampleFormat::I32 => Self::build_output::<i32, _>(&device, &config.config(), ring),
            SampleFormat::I64 => Self::build_output::<i64, _>(&device, &config.config(), ring),
            SampleFormat::U8 => Self::build_output::<u8, _>(&device, &config.config(), ring),
            SampleFormat::U16 => Self::build_output::<u16, _>(&device, &config.config(), ring),
            SampleFormat::U32 => Self::build_output::<u32, _>(&device, &config.config(), ring),
            SampleFormat::U64 => Self::build_output::<u64, _>(&device, &config.config(), ring),
            SampleFormat::F32 => Self::build_output::<f32, _>(&device, &config.config(), ring),
            SampleFormat::F64 => Self::build_output::<f64, _>(&device, &config.config(), ring),
            format => fail!(AnyError(format!("unknown format: {format}").into())),
        }?;

        stream.play().into_error()?;

        Result::Ok(Self {
            host,
            device,
            stream,
        })
    }

    fn build_output<T: SizedSample + FromSample<f32>, E0: Source>(
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

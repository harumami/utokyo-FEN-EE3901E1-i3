use {
    ::opus_sys::{
        OPUS_APPLICATION_VOIP,
        OpusEncoder,
        opus_encoder_create,
    },
    opus_sys::{
        OpusDecoder,
        opus_decode_float,
        opus_decoder_create,
        opus_decoder_destroy,
        opus_encode_float,
        opus_encoder_destroy,
        opus_strerror,
    },
    std::{
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
    },
};

#[derive(Debug)]
pub struct Encoder {
    raw: *mut OpusEncoder,
    channels: u32,
    input: Vec<f32>,
    output: Vec<u8>,
}

impl Encoder {
    pub fn new(sample_rate: u32, channels: u32) -> Result<Self, OpusError> {
        let mut error = 0;

        let raw = unsafe {
            opus_encoder_create(
                sample_rate as _,
                channels as _,
                OPUS_APPLICATION_VOIP as _,
                &mut error,
            )
        };

        OpusError::new(error)?;

        Result::Ok(Self {
            raw,
            channels,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    pub fn input(&mut self) -> &mut Vec<f32> {
        &mut self.input
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }

    pub fn ready(&self, frame_size: usize) -> bool {
        self.input.len() >= self.channels as usize * frame_size
    }

    pub fn encode(&mut self, frame_size: usize, max_packet_size: usize) -> Result<(), OpusError> {
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

        OpusError::new(size)?;
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
pub struct Decoder {
    raw: *mut OpusDecoder,
    channels: u32,
    input: Vec<u8>,
    output: Vec<f32>,
}

impl Decoder {
    pub fn new(sample_rate: u32, channels: u32) -> Result<Self, OpusError> {
        let mut error = 0;
        let raw = unsafe { opus_decoder_create(sample_rate as _, channels as _, &mut error) };
        OpusError::new(error)?;

        Result::Ok(Self {
            raw,
            channels,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    pub fn input(&mut self) -> &mut Vec<u8> {
        &mut self.input
    }

    pub fn output(&self) -> &[f32] {
        &self.output
    }

    pub fn decode(&mut self, max_frame_size: usize) -> Result<(), OpusError> {
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

        OpusError::new(frames)?;

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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OpusError(c_int);

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
            Result::Ok(error) => write!(f, "opus: {error}"),
            Result::Err(error) => write!(f, "opus: {error}"),
        }
    }
}

impl Error for OpusError {}

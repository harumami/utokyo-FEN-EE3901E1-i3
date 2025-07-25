use {
    speex_sys::{
        RESAMPLER_ERR_SUCCESS,
        SpeexResamplerState,
        speex_resampler_destroy,
        speex_resampler_init,
        speex_resampler_process_interleaved_float,
        speex_resampler_strerror,
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
pub struct Resampler {
    raw: *mut SpeexResamplerState,
    channels: u32,
    in_rate: u32,
    out_rate: u32,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Resampler {
    pub fn new(
        channels: u32,
        in_rate: u32,
        out_rate: u32,
        quality: u32,
    ) -> Result<Self, SpeexError> {
        let mut error = 0;

        let raw =
            unsafe { speex_resampler_init(channels, in_rate, out_rate, quality as _, &mut error) };

        SpeexError::new(error)?;

        Result::Ok(Self {
            raw,
            channels,
            in_rate,
            out_rate,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    pub fn input(&mut self) -> &mut Vec<f32> {
        &mut self.input
    }

    pub fn output(&self) -> &[f32] {
        &self.output
    }

    pub fn resample(&mut self) -> Result<(), SpeexError> {
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

        SpeexError::new(error)?;
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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpeexError(c_int);

impl SpeexError {
    pub fn new(error: c_int) -> Result<(), Self> {
        match error == RESAMPLER_ERR_SUCCESS as _ {
            true => Result::Ok(()),
            false => Result::Err(Self(error)),
        }
    }
}

impl Display for SpeexError {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        match unsafe { CStr::from_ptr(speex_resampler_strerror(self.0)) }.to_str() {
            Result::Ok(error) => write!(f, "speex: {error}"),
            Result::Err(error) => write!(f, "speex: {error}"),
        }
    }
}

impl Error for SpeexError {}

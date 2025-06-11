use {
    ::std::{
        backtrace::Backtrace,
        error::Error,
        fmt::{
            Display,
            Formatter,
            Result as FmtResult,
        },
        result::Result as StdResult,
    },
    ::tracing::error,
    ::tracing_error::SpanTrace,
};

#[derive(Debug)]
pub struct Report {
    error: Box<dyn Send + Error>,
    backtrace: Backtrace,
    spantrace: SpanTrace,
}

impl Display for Report {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        self.error.fmt(f)?;
        f.write_str("\n\n")?;
        self.backtrace.fmt(f)?;
        f.write_str("\n\n")?;
        self.spantrace.fmt(f)?;
        Result::Ok(())
    }
}

impl Report {
    pub fn report(&self) {
        error!(error = self.error, backtrace = %self.backtrace, spantrace = %self.spantrace);
    }
}

impl<E: 'static + Send + Error> From<E> for Report {
    fn from(value: E) -> Self {
        Self {
            error: Box::new(value),
            backtrace: Backtrace::force_capture(),
            spantrace: SpanTrace::capture(),
        }
    }
}

pub type Result<T, E = Report> = StdResult<T, E>;

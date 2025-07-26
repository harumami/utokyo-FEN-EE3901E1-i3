use {
    ::hypha_core::{
        CloseHandle as RsCloseHandle,
        DataStream as RsDataStream,
        Instance as RsInstance,
        Peer as RsPeer,
        PublicId as RsPublicId,
        SecretId as RsSecretId,
        ToggleHandle as RsToggleHandle,
    },
    ::pyo3::{
        Bound,
        PyErr,
        PyResult,
        exceptions::PyRuntimeError,
        marker::Python,
        pyclass,
        pymethods,
        pymodule,
        types::{
            PyAny,
            PyModule,
            PyModuleMethods as _,
        },
    },
    ::pyo3_async_runtimes::tokio::future_into_py,
    ::pyo3_stub_gen::derive::{
        gen_stub_pyclass,
        gen_stub_pymethods,
    },
    ::rancor::{
        BoxedError,
        ResultExt as _,
        Source,
        Trace,
    },
    ::std::{
        error::Error,
        fmt::{
            Debug,
            Display,
            Formatter,
            Result as FmtResult,
        },
        str::FromStr as _,
        sync::Arc,
        thread::sleep,
        time::Duration,
    },
    ::tokio::{
        sync::Mutex,
        task::{
            JoinHandle,
            spawn_blocking,
        },
    },
    ::tracing::error,
};

#[pymodule]
fn hypha_py(module: &Bound<PyModule>) -> PyResult<()> {
    module.add_class::<SecretId>()?;
    module.add_class::<PublicId>()?;
    module.add_class::<Instance>()?;
    module.add_class::<Peer>()?;
    module.add_class::<DataStream>()?;
    module.add_class::<AudioStream>()?;
    module.add_class::<ToggleHandle>()?;
    module.add_class::<CloseHandle>()?;
    Result::Ok(())
}

#[gen_stub_pyclass]
#[pyclass(frozen, str = "{0}")]
#[derive(Clone, Debug)]
struct SecretId(RsSecretId);

#[gen_stub_pymethods]
#[pymethods]
impl SecretId {
    #[new]
    #[pyo3(signature = (s = Option::None))]
    fn new(s: Option<&str>) -> Result<Self, RuntimeError> {
        Result::Ok(Self(match s {
            Option::Some(s) => RsSecretId::from_str(s).into_error()?,
            Option::None => RsSecretId::generate(),
        }))
    }

    fn public(&self) -> PublicId {
        PublicId(self.0.public())
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen, str = "{0}")]
#[derive(Clone, Hash, Debug)]
struct PublicId(RsPublicId);

#[gen_stub_pymethods]
#[pymethods]
impl PublicId {
    #[new]
    fn new(s: &str) -> Result<Self, RuntimeError> {
        Result::Ok(Self(RsPublicId::from_str(s).into_error()?))
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
#[derive(Clone, Debug)]
struct Instance(RsInstance);

#[gen_stub_pymethods]
#[pymethods]
impl Instance {
    #[staticmethod]
    fn bind(py: Python, secret: Option<SecretId>) -> PyResult<Bound<PyAny>> {
        future_into_py(py, async {
            let rs = RsInstance::bind::<RuntimeError>(secret.map(|secret| secret.0)).await?;
            Result::Ok(Self(rs))
        })
    }

    fn secret(&self) -> SecretId {
        SecretId(RsSecretId::new(self.0.endpoint().secret_key().clone()))
    }

    fn accept<'py>(this: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(Peer(this.get().0.accept::<RuntimeError>().await?))
        })
    }

    fn connect<'py>(
        this: Bound<'py, Self>,
        py: Python<'py>,
        id: PublicId,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(Peer(this.get().0.connect::<RuntimeError>(id.0).await?))
        })
    }

    fn close<'py>(this: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            this.get().0.close().await;
            Result::Ok(())
        })
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
#[derive(Debug)]
struct Peer(RsPeer);

#[gen_stub_pymethods]
#[pymethods]
impl Peer {
    fn accept_stream<'py>(this: Bound<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(DataStream::new(
                this.get()
                    .0
                    .accept_stream::<RuntimeError, RuntimeError>()
                    .await?,
            ))
        })
    }

    fn open_stream<'py>(this: Bound<Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(DataStream::new(
                this.get()
                    .0
                    .open_stream::<RuntimeError, RuntimeError>()
                    .await?,
            ))
        })
    }

    fn record_stream(this: Bound<Self>) -> AudioStream {
        let this = this.unbind();

        AudioStream(spawn_blocking(move || {
            let _stream = match this.get().0.record_stream::<RuntimeError>() {
                Result::Ok(stream) => Option::Some(stream),
                Result::Err(error) => {
                    error!(error = &error as &dyn Error);
                    Option::None
                },
            };

            loop {
                sleep(Duration::from_secs(1024));
            }
        }))
    }

    fn play_stream(this: Bound<Self>) -> AudioStream {
        let this = this.unbind();

        AudioStream(spawn_blocking(move || {
            let _stream = match this.get().0.play_stream::<RuntimeError>() {
                Result::Ok(stream) => Option::Some(stream),
                Result::Err(error) => {
                    error!(error = &error as &dyn Error);
                    Option::None
                },
            };

            loop {
                sleep(Duration::from_secs(1024));
            }
        }))
    }

    fn mute_handle(&self) -> ToggleHandle {
        ToggleHandle(self.0.mute_handle().clone())
    }

    fn deafen_handle(&self) -> ToggleHandle {
        ToggleHandle(self.0.deafen_handle().clone())
    }

    fn close_handle(&self) -> CloseHandle {
        CloseHandle(self.0.close_handle().clone())
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
struct DataStream(Arc<Mutex<Option<RsDataStream<RuntimeError>>>>);

impl DataStream {
    fn new(stream: RsDataStream<RuntimeError>) -> Self {
        Self(Arc::new(Mutex::new(Option::Some(stream))))
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl DataStream {
    fn join<'py>(this: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            if let Option::Some(stream) = this.get().0.lock().await.take() {
                stream.join::<RuntimeError>().await?;
            }

            Result::Ok(())
        })
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
struct AudioStream(#[allow(dead_code)] JoinHandle<()>);

#[gen_stub_pyclass]
#[pyclass(frozen)]
#[derive(Debug)]
struct ToggleHandle(Arc<RsToggleHandle>);

#[gen_stub_pymethods]
#[pymethods]
impl ToggleHandle {
    fn is_on(&self) -> bool {
        self.0.is_on()
    }

    fn toggle(&self) {
        self.0.toggle();
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
#[derive(Debug)]
struct CloseHandle(Arc<RsCloseHandle>);

#[gen_stub_pymethods]
#[pymethods]
impl CloseHandle {
    fn close(&self) {
        self.0.close();
    }
}

#[derive(Debug)]
struct RuntimeError(BoxedError);

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.0, f)
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

impl Trace for RuntimeError {
    fn trace<R: Debug + Display + Send + Sync + 'static>(self, trace: R) -> Self {
        Self(self.0.trace(trace))
    }
}

impl Source for RuntimeError {
    fn new<T: Error + Send + Sync + 'static>(source: T) -> Self {
        Self(BoxedError::new(source))
    }
}

impl From<RuntimeError> for PyErr {
    fn from(value: RuntimeError) -> Self {
        PyRuntimeError::new_err(format!("{value}"))
    }
}

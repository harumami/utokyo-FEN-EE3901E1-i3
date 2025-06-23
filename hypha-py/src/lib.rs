use {
    ::hypha_core::{
        CloseHandle as RsCloseHandle,
        Connection as RsConnection,
        Instance as RsInstance,
        Secret as RsSecret,
    },
    ::iroh::NodeId as RsNodeId,
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
    ::tokio::task::{
        JoinHandle,
        spawn_blocking,
    },
    ::tracing::error,
};

#[pymodule]
fn hypha_py(module: &Bound<PyModule>) -> PyResult<()> {
    module.add_class::<Secret>()?;
    module.add_class::<NodeId>()?;
    module.add_class::<Instance>()?;
    module.add_class::<Connection>()?;
    module.add_class::<AudioStream>()?;
    module.add_class::<CloseHandle>()?;
    Result::Ok(())
}

#[gen_stub_pyclass]
#[pyclass(frozen, str = "{0}")]
#[derive(Clone, Debug)]
struct Secret(RsSecret);

#[gen_stub_pymethods]
#[pymethods]
impl Secret {
    #[new]
    #[pyo3(signature = (s = Option::None))]
    fn new(s: Option<&str>) -> Result<Self, RuntimeError> {
        Result::Ok(Self(match s {
            Option::Some(s) => RsSecret::from_str(s).into_error()?,
            Option::None => RsSecret::generate(),
        }))
    }

    fn node_id(&self) -> NodeId {
        NodeId(self.0.node_id())
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen, str = "{0}")]
#[derive(Clone, Hash, Debug)]
struct NodeId(RsNodeId);

#[gen_stub_pymethods]
#[pymethods]
impl NodeId {
    #[new]
    fn new(s: &str) -> Result<Self, RuntimeError> {
        Result::Ok(NodeId(RsNodeId::from_str(s).into_error()?))
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
    fn bind(py: Python, secret: Option<Secret>) -> PyResult<Bound<PyAny>> {
        future_into_py(py, async {
            let rs = RsInstance::bind::<RuntimeError>(secret.map(|secret| secret.0)).await?;
            Result::Ok(Self(rs))
        })
    }

    fn secret(&self) -> Secret {
        Secret(RsSecret::new(self.0.endpoint().secret_key().clone()))
    }

    fn accept<'py>(this: Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(Connection(
                this.get().0.accept::<RuntimeError, RuntimeError>().await?,
            ))
        })
    }

    fn connect<'py>(
        this: Bound<'py, Self>,
        py: Python<'py>,
        node_id: NodeId,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = this.unbind();

        future_into_py(py, async move {
            Result::Ok(Connection(
                this.get()
                    .0
                    .connect::<RuntimeError, RuntimeError>(node_id.0)
                    .await?,
            ))
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
struct Connection(RsConnection<RuntimeError>);

#[gen_stub_pymethods]
#[pymethods]
impl Connection {
    fn record(this: Bound<Self>) -> AudioStream {
        let this = this.unbind();

        AudioStream(spawn_blocking(move || {
            let _stream = match this.get().0.record::<RuntimeError, RuntimeError>() {
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

    fn play(this: Bound<Self>) -> AudioStream {
        let this = this.unbind();

        AudioStream(spawn_blocking(move || {
            let _stream = match this.get().0.play::<RuntimeError, RuntimeError>() {
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

    fn close_handle(&self) -> CloseHandle {
        CloseHandle(self.0.close_handle().clone())
    }
}

#[gen_stub_pyclass]
#[pyclass(frozen)]
struct AudioStream(#[allow(dead_code)] JoinHandle<()>);

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

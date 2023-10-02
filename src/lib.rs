use std::ffi::{c_int, CStr};
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyRuntimeError;
use pyo3::types::{PyDict, PyModule};
use pyo3::{
    ffi, pyclass, pyfunction, pymethods, pymodule, wrap_pyfunction, PyErr, PyResult, Python,
};

#[pyfunction]
#[pyo3(signature = (allow_fork = false, allow_exec = false, allow_threads = true, allow_daemon_threads = false))]
/// Creates a new Python interpreter with it's own isolated GIL.
///
/// This method takes the following optional arguments:
/// - `allow_fork` (bool) - Defaults to `false`.
/// - `allow_exec` (bool) - Defaults to `false`.
/// - `allow_threads` (bool) - Defaults to `false`.
/// - `allow_daemon_threads` (bool) - Defaults to `false`.
///
/// Some of these configs may cause issues, use at your own risk.
fn create_interpreter(
    allow_fork: bool,
    allow_exec: bool,
    allow_threads: bool,
    allow_daemon_threads: bool,
) -> PyResult<SubInterpreter> {
    let config = InterpreterConfig {
        allow_fork,
        allow_exec,
        allow_threads,
        allow_daemon_threads,
    };

    let interpreter = Interpreter::create(config)?;

    Ok(SubInterpreter(Arc::new(Mutex::new(interpreter))))
}

#[pymodule]
/// Wraps the new Python 3.12 subinterpreters API.
fn subinterpreters(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(create_interpreter, m)?)?;
    m.add_class::<SubInterpreter>()?;
    Ok(())
}

#[pyclass]
pub struct SubInterpreter(Arc<Mutex<Interpreter>>);

#[pymethods]
impl SubInterpreter {
    /// Run a Python script within the sub-interpreter.
    fn run_code(
        &self,
        code: String,
        globals: Option<&PyDict>,
        locals: Option<&PyDict>,
    ) -> PyResult<()> {
        use unindent::unindent;
        let code = unindent(&code);

        let lock = self.0.lock().unwrap();

        if !lock.is_valid() {
            return Err(PyRuntimeError::new_err("Interpreter has shutdown."));
        }

        lock.scope(|| Python::with_gil(|py| py.run(&code, globals, locals)))
    }

    /// Shuts down the interpreter.
    ///
    /// Once shutdown, the interpreter cannot be used anymore.
    fn shutdown(&self) {
        let lock = self.0.lock().unwrap();
        lock.shutdown();
    }
}

#[derive(Debug, Copy, Clone)]
/// The config for creating a new sub interpreter.
pub struct InterpreterConfig {
    /// If this is `false` then the runtime will not support forking the process in any thread where
    /// the sub-interpreter is currently active. Otherwise fork is unrestricted.
    ///
    /// Note that the subprocess module still works when fork is disallowed.
    ///
    /// NOTE:
    /// It is probably not a good idea to enable this, the affects of forking are largely unknown
    /// around the behaviour of the sub-interpreters and if that causes effectively an exploding
    /// amount of threads.
    ///
    /// TL;DR: You probably do not want this.
    allow_fork: bool,
    /// If this is `false` then the runtime will not support replacing the current process via exec
    /// (e.g. os.execv()) in any thread where the sub-interpreter is currently active.
    /// Otherwise exec is unrestricted.
    ///
    /// Note that the subprocess module still works when exec is disallowed.
    ///
    /// NOTE:
    /// Like `allow_fork` you are probably asking for trouble, if you enable this; do so at your
    /// own risk, the consequences of replacing the current process is unknown.
    allow_exec: bool,
    /// If this is `false` then the sub-interpreter’s threading module won’t create threads.
    /// Otherwise threads are allowed.
    ///
    /// *This is enabled by default.*
    allow_threads: bool,
    /// If this is `false` then the sub-interpreter’s threading module won’t create daemon threads.
    /// Otherwise daemon threads are allowed (as long as allow_threads is also enabled).
    ///
    /// *This is enabled by default.*
    allow_daemon_threads: bool,
}

#[derive(Debug, thiserror::Error)]
/// A error which occurred while creating the interpreter.
pub enum CreateInterpreterError {
    #[error("daemon threads cannot be enabled if `allow_threads` is `false`.")]
    ConfigError,
    #[error("a Python interpreter has not yet been initialised and or is not running.")]
    InitialisationError,
    #[error("no GIL is currently setup within the the current thread.")]
    MissingGil,
    #[error("{0}")]
    Other(String),
}

impl From<CreateInterpreterError> for PyErr {
    fn from(value: CreateInterpreterError) -> Self {
        PyRuntimeError::new_err(value.to_string())
    }
}

/// A wrapper around a currently active sub-interpreter.
///
/// Once this is dropped, the interpreter will be shutdown.
struct Interpreter {
    inner: *mut ffi::PyThreadState,
}

impl Interpreter {
    /// Creates a new sub-interpreter using the given config.
    ///
    /// Returns an error if the interpreter config is invalid or
    /// Python failed to create the interpreter.
    fn create(config: InterpreterConfig) -> Result<Self, CreateInterpreterError> {
        if !config.allow_threads && config.allow_daemon_threads {
            return Err(CreateInterpreterError::ConfigError);
        }

        let config = ffi::PyInterpreterConfig {
            use_main_obmalloc: 0,
            allow_fork: config.allow_fork as c_int,
            allow_exec: config.allow_exec as c_int,
            allow_threads: config.allow_threads as c_int,
            allow_daemon_threads: config.allow_daemon_threads as c_int,
            check_multi_interp_extensions: 1,
            gil: ffi::PyInterpreterConfig_OWN_GIL,
        };

        // SAFETY:
        // This method simply wraps the internal calls to cpython, and should handle
        // all operations correctly.
        unsafe { Self::create_internal(config) }
    }

    fn is_valid(&self) -> bool {
        !self.inner.is_null()
    }

    fn shutdown(&self) {
        if self.inner.is_null() {
            return;
        }

        // Temporarily set the thread state to the `inner` state
        // so we can shutdown the interpreter.
        unsafe {
            let tmp_state = ffi::PyThreadState_Get();
            ffi::PyThreadState_Swap(self.inner);
            ffi::Py_EndInterpreter(self.inner);
            ffi::PyThreadState_Swap(tmp_state);
        }
    }

    fn scope<'a, F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T + 'a,
    {
        assert!(!self.inner.is_null());

        unsafe {
            let old = ffi::PyThreadState_Get();
            ffi::PyThreadState_Swap(self.inner);

            let res = f();

            ffi::PyThreadState_Swap(old);

            res
        }
    }

    unsafe fn create_internal(
        config: ffi::PyInterpreterConfig,
    ) -> Result<Self, CreateInterpreterError> {
        if ffi::Py_IsInitialized() == 0 {
            return Err(CreateInterpreterError::InitialisationError);
        }

        // Get the current GIL thread state.
        let existing_state = ffi::PyThreadState_Get();
        if existing_state.is_null() {
            return Err(CreateInterpreterError::MissingGil);
        }

        let mut state: *mut ffi::PyThreadState = std::ptr::null_mut();

        // The `Py_NewInterpreterFromConfig` method replaces/swaps the current thread state.
        // And also set the passed `state` to be the new thread state.
        let status = ffi::Py_NewInterpreterFromConfig(&mut state as *mut _, &config as *const _);

        // To avoid this behaviour as mentioned above, we will swap the old state back.
        // This means any operations in this thread stay on the original state.
        ffi::PyThreadState_Swap(existing_state);

        if ffi::PyStatus_Exception(status) != 0 {
            let msg = CStr::from_ptr(status.err_msg).to_str().unwrap_or("Unknown");
            return Err(CreateInterpreterError::Other(msg.to_string()));
        }

        assert!(
            !state.is_null(),
            "thread state was none after Python returned successful response, something is very wrong.",
        );

        Ok(Self { inner: state })
    }
}

unsafe impl Send for Interpreter {}

impl Drop for Interpreter {
    fn drop(&mut self) {
        self.shutdown()
    }
}

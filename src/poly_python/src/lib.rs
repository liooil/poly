/// Embedded RustPython backend for the Polyglot Bun fork.
///
/// # Architecture
///
/// ## Module Registry (`ModuleRegistry`)
///
/// Instead of calling `runpy.run_path` from a fresh scope each time, every
/// Python module is loaded exactly once by its normalized absolute path. The
/// registry keyed by canonical path stores the module's namespace reference
/// within the persistent interpreter. Subsequent calls to `call`, `describe`,
/// or `import` re-use the same module state.
///
/// ## Request Protocol
///
/// Four operations tunneled through a single JSON `<->` JSON bridge function:
///
/// - `load`    — Load a `.py` file into the registry if not already loaded.
///   Returns the module's public name list and JSON-constant exports.
/// - `describe`— Like `load` but always returns full export metadata (used by
///   Bun's transpiler to generate synthetic ESM source).
/// - `call`    — Call a named function in an already-loaded module with JSON
///   arguments. Returns the JSON result.
/// - `run_file`— Run a `.py` file as a top-level script with `sys.argv`.
///   Only for `poly app.py` entry.
///
/// ## Python `js` package
///
/// An embedded Python `js` package provides `import_module()` which routes
/// the specifier to Bun's resolver through a thread-local host callback. The
/// callback is installed by `bun_runtime` during JSC VM init and unset on
/// shutdown.
///
/// ## Reentrancy Guard
///
/// A `POLY_CALL_DEPTH` thread-local counter permits `JS -> Python -> JS`
/// (depth 2) but rejects a second `-> Python` recurrence (depth 3+) with
/// `ERR_POLY_REENTRANT_PYTHON_CALL`.
///
/// ## Value Domain
///
/// v1 passes only JSON-compatible values: `None/null`, `bool`, finite
/// 64-bit-safe int/float, UTF-8 string, list, and string-keyed dict.  Values
/// outside this domain (bytes, BigInt, NaN/Infinity, undefined, cyclic
/// structures, custom objects) are rejected with a typed error.
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rustpython::vm::AsObject;
use rustpython::vm::builtins::PyBaseExceptionRef;
use rustpython::vm::function::FuncArgs;
use rustpython::vm::scope::Scope;
use rustpython::vm::{Settings, VirtualMachine};
use rustpython::{Interpreter, InterpreterBuilder, InterpreterBuilderExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

fn trace(message: &str) {
    if std::env::var_os("POLY_TRACE").is_some() {
        eprintln!("[poly trace] {message}");
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PythonError {
    message: String,
    kind: PythonErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonErrorKind {
    Import,
    TypeError,
    ReentrantCall,
    NoHostContext,
    JavaScript,
    Unknown(std::string::String),
}

impl PythonError {
    fn new(message: impl Into<String>) -> Self {
        let m = message.into();
        Self {
            kind: PythonErrorKind::Unknown(m.clone()),
            message: m,
        }
    }

    fn new_kind(message: impl Into<String>, kind: PythonErrorKind) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    pub fn kind(&self) -> &PythonErrorKind {
        &self.kind
    }

    pub fn into_message(self) -> String {
        self.message
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PythonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PythonError {}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Opaque request sent across the JSON bridge.
#[derive(Debug, Deserialize, Serialize)]
pub struct BridgeRequest {
    pub kind: String, // "load" | "describe" | "call" | "run_file" | "py_get_var" | "py_call_attr"
    #[serde(default)]
    pub module: String,
    #[serde(default)]
    pub handle: u64, // cross-runtime handle for call_py_handle
    #[serde(default)]
    pub function: String,
    #[serde(default)]
    pub args: Vec<Value>,
    #[serde(default)]
    pub script_args: Vec<String>,
    #[serde(default)]
    pub referrer: String,
    /// Attribute name for `py_get_var` / `py_call_attr`.
    #[serde(default)]
    pub property: String,
    /// Source code for `py_eval`.
    #[serde(default)]
    pub code: String,
}

/// One variable exported from the Python REPL scope after evaluation.
/// Functions and arbitrary objects are exported by name (the name is the
/// cross-runtime handle, mirroring v1 module interop); other
/// JSON-serializable values by value.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplExport {
    pub name: String,
    /// "function", "value" or "object".
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// Result of one REPL input evaluated against the persistent REPL scope.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReplEvalResult {
    /// Input is syntactically incomplete; keep collecting lines.
    pub incomplete: bool,
    /// `repr()` of the evaluated expression (None for statements).
    pub value: Option<String>,
    /// Error message / traceback, if evaluation failed.
    pub error: Option<String>,
    /// Variables in the REPL scope after evaluation (functions by handle,
    /// JSON values by value). Empty when `incomplete` — nothing ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<ReplExport>,
}

/// A shared (cross-language) variable injected into the Python REPL scope
/// before evaluation: a JSON value or a JavaScript function (by name).
#[derive(Debug, Clone)]
pub struct SharedExport {
    pub name: String,
    pub kind: String,
    pub value: Option<Value>,
}

/// Opaque response returned across the JSON bridge.
#[derive(Debug, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports: Option<ModuleExports>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// `py_get_var`: the attribute value is callable (the JS side should
    /// expose a function proxy backed by `py_call_attr`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
}

/// Module export metadata returned by `describe` or `load`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleExports {
    /// Exported names (in `__all__` order if present, otherwise sorted).
    pub names: Vec<String>,
    /// JSON-serializable constant values for names that are literals.
    pub constants: HashMap<String, Value>,
    /// Names that are callable functions.
    pub callables: Vec<String>,
}

// ---------------------------------------------------------------------------
// Module registry
// ---------------------------------------------------------------------------

/// Persistent module state within the RustPython interpreter.
struct ModuleState {
    /// The scope containing the module's globals.
    scope: Scope,
}

/// The thread-safe module registry, guarded by a mutex.
struct ModuleRegistry {
    modules: HashMap<PathBuf, ModuleState>,
}

impl ModuleRegistry {
    fn new() -> Self {
        Self {
            modules: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Python `js` package source
// ---------------------------------------------------------------------------

/// Embedded Python source for the `js` package that provides
/// `import_module()` and the `js.x.y` dotted access.
const JS_PACKAGE_SOURCE: &str = r#"
import sys
import json

class _JSVariable:
    """A live proxy for a JavaScript global, identified by its name. Calls
    resolve the function by name on every call; attribute/item access reads
    through to the JS object. The name is the handle (mirrors v1 module
    interop) — no snapshots, both namespaces stay fused."""

    # Marker so REPL export skips proxies injected from JavaScript.
    _poly_js_var = True

    def __init__(self, name):
        self._name = name

    def __call__(self, *args):
        _check_host()
        request = json.dumps({
            'kind': 'js_call_var',
            'name': self._name,
            'args': list(args),
        })
        response_json = globals()['__poly_js_host_call'](request)
        response = json.loads(response_json)
        if not response.get('ok', False):
            raise _js_error(response)
        return response.get('value') if 'value' in response else None

    def __getattr__(self, name):
        # Internal attributes (dunders, _name) never proxy.
        if name.startswith('_'):
            raise AttributeError(name)
        return self._read(name)

    def __getitem__(self, key):
        return self._read(key)

    def _read(self, property_name):
        _check_host()
        request = json.dumps({
            'kind': 'js_get_var',
            'name': self._name,
            'property': str(property_name),
        })
        response_json = globals()['__poly_js_host_call'](request)
        response = json.loads(response_json)
        if not response.get('ok', False):
            raise _js_error(response)
        return response.get('value') if 'value' in response else None

    def __repr__(self):
        return f"<js variable {self._name}>"


def _make_js_variable(name):
    return _JSVariable(name)


class _JSFunction:
    """A callable wrapper around a named JS module export (v1: no handles,
    resolved by module specifier + name on every call)."""

    def __init__(self, module, name, referrer):
        self._module = module
        self._name = name
        self._referrer = referrer

    def __call__(self, *args):
        _check_host()
        request = json.dumps({
            'kind': 'call_js',
            'module': self._module,
            'function': self._name,
            'referrer': self._referrer,
            'args': list(args),
        })
        response_json = globals()['__poly_js_host_call'](request)
        response = json.loads(response_json)
        if not response.get('ok', False):
            raise _js_error(response)
        return response.get('value') if 'value' in response else None

    def __repr__(self):
        return f"<js function {self._module}.{self._name}>"


class _ModuleWrapper:
    """Wraps a JS module namespace for attribute access."""

    def __init__(self, exports, module, referrer):
        self._module = module
        self._referrer = referrer
        values = exports.get('values', {}) if isinstance(exports, dict) else {}
        functions = exports.get('functions', []) if isinstance(exports, dict) else []
        self._exports = values
        # Names from the module become direct attributes
        for name, value in values.items():
            if value is not None:
                super().__setattr__(name, value)
        # Callable exports become callable wrappers
        for name in functions:
            super().__setattr__(name, _JSFunction(self._module, name, self._referrer))

    def __getattr__(self, name):
        # Delegate to the underlying exports (for names with reserved fn name collisions)
        exports = super().__getattribute__('_exports')
        if name in exports:
            return exports[name]
        raise AttributeError(f"module has no attribute {name!r}")


class _JSRoot:
    """Root 'js' module object used for 'js.pkg.name' dotted access."""

    def __init__(self):
        self._cache = {}

    def import_module(self, specifier):
        _check_host()
        # Relative specifiers are resolved relative to the caller's file.
        caller_frame = sys._getframe(1)
        caller_file = caller_frame.f_globals.get('__file__', None)
        referrer = caller_file if caller_file else ''
        request = json.dumps({
            'kind': 'import_js',
            'module': specifier,
            'referrer': referrer,
        })
        response_json = globals()['__poly_js_host_call'](request)
        response = json.loads(response_json)
        if not response.get('ok', False):
            raise _js_error(response)
        exports = response['exports'] if 'exports' in response else {}
        return _ModuleWrapper(exports, specifier, referrer)

    def __getattr__(self, name):
        # js.x.y -> import_module('x/y')
        if name.startswith('_'):
            raise AttributeError(name)
        return self.import_module(name)


def _js_error(response):
    error = response.get('error', 'Unknown error')
    js_name = response.get('js_name', 'Error')
    js_message = response.get('js_message', str(error))
    js_stack = response.get('js_stack', '')
    msg = f'{js_name}: {js_message}'
    if js_stack:
        msg += f'\n  JS stack:\n{js_stack}'
    return JavaScriptError(msg)


class JavaScriptError(Exception):
    """Raised when a JavaScript operation fails."""

    def __init__(self, message):
        self.js_name = 'Error'
        self.js_message = str(message)
        self.js_stack = ''
        super().__init__(str(message))


def _check_host():
    """Ensure we are inside a JSC/Bun runtime with a host callback."""
    if '__poly_js_host_call' not in globals():
        raise RuntimeError(
            "js.import_module() can only be called from within the Poly/Bun runtime. "
            "No JSC host context is available."
        )


# Install the root module
js = _JSRoot()
sys.modules['js'] = js


# ---------------------------------------------------------------------------
# poly — cross-language helpers for the secondary languages (C / SQL / Shell).
# v1: thin wrappers over globalThis bridge functions exposed by the REPL:
#   poly.c("add", 2, 3)   -> call a C function from the C REPL session
#   poly.sql("SELECT ...") -> query the SQL REPL session database
#   poly.sqlexec("...")    -> execute a SQL script (DDL / migrations)
# Each call resolves the JS side by name at call time (no handles).
# ---------------------------------------------------------------------------

def _poly_call_js(name, args):
    _check_host()
    request = json.dumps({'kind': 'js_call_var', 'name': name, 'args': list(args)})
    response_json = globals()['__poly_js_host_call'](request)
    response = json.loads(response_json)
    if not response.get('ok', False):
        raise _js_error(response)
    return response.get('value') if 'value' in response else None


def c(name, *args):
    """Call a C function defined in the C REPL session (via globalThis.__polyC)."""
    return _poly_call_js('__polyCCall', [name, *args])


def sql(query):
    """Run a query against the SQL REPL session; returns rows as a list of dicts."""
    return _poly_call_js('__polySqlQuery', [query])


def sqlexec(script):
    """Execute a SQL script (multi-statement) against the SQL REPL session."""
    return _poly_call_js('__polySqlExec', [script])


poly = sys.modules['poly'] = type(sys)('poly')
poly.c = c
poly.sql = sql
poly.sqlexec = sqlexec
"#;

// ---------------------------------------------------------------------------
// Reentrancy guard
// ---------------------------------------------------------------------------

thread_local! {
    static POLY_CALL_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Guard that increments call depth on enter and decrements on drop.
struct ReentrancyGuard;

impl ReentrancyGuard {
    fn enter() -> Result<Self, PythonError> {
        let depth = POLY_CALL_DEPTH.get();
        if depth >= 2 {
            return Err(PythonError::new_kind(
                "ERR_POLY_REENTRANT_PYTHON_CALL: re-entered Python from a JS callback. \
                 JS -> Python -> JS -> Python is not allowed in the synchronous v1 interop."
                    .to_string(),
                PythonErrorKind::ReentrantCall,
            ));
        }
        POLY_CALL_DEPTH.set(depth + 1);
        Ok(Self)
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        let depth = POLY_CALL_DEPTH.get();
        POLY_CALL_DEPTH.set(depth.saturating_sub(1));
    }
}

// ---------------------------------------------------------------------------
// Host callback (installed by bun_runtime)
// ---------------------------------------------------------------------------

/// Thread-local pointer to a host function that handles `import_js` requests.
type HostCallback = fn(&str) -> Result<String, String>;

std::thread_local! {
    static HOST_CALLBACK: std::cell::Cell<Option<HostCallback>> = const { std::cell::Cell::new(None) };
}

/// Set the host callback for Python → JS module importing.
/// Must be called from the JS thread.
pub fn set_host_callback(callback: HostCallback) {
    HOST_CALLBACK.set(Some(callback));
}

/// Clear the host callback (typically on VM shutdown).
pub fn clear_host_callback() {
    HOST_CALLBACK.set(None);
}

// ---------------------------------------------------------------------------
// Persistent Python runtime
// ---------------------------------------------------------------------------

struct PythonRuntime {
    interpreter: Interpreter,
    registry: Mutex<ModuleRegistry>,
    /// Persistent REPL scope: variables assigned in `poly repl` (py mode)
    /// survive across inputs. Created on first `repl_eval`.
    repl_scope: parking_lot::Mutex<Option<Scope>>,
}

thread_local! {
    /// The VM is created lazily after JSC has entered the Bun runtime and stays
    /// alive for the process lifetime. Avoiding TLS destruction also prevents
    /// RustPython cleanup from racing JSC's process-exit teardown.
    static RUNTIME: &'static PythonRuntime =
        Box::leak(Box::new(PythonRuntime::new()));
}

impl PythonRuntime {
    fn new() -> Self {
        trace("initializing RustPython runtime");

        let mut settings = Settings::default();
        settings.argv = vec!["<poly-python>".to_owned()];
        settings.write_bytecode = false;
        settings.install_signal_handlers = false;

        let interpreter = InterpreterBuilder::new()
            .settings(settings)
            .init_stdlib()
            .interpreter();

        // Bootstrap the `js` package in the interpreter. This scope is also
        // the REPL scope, so Python REPL code sees the js package (including
        // the _make_js_variable proxy factory and __poly_js_host_call) and
        // its globals() stays fused with globalThis.
        let repl_scope = interpreter.enter(|vm| {
            let scope = vm.new_scope_with_main().unwrap();
            // Register the host-call dispatch function as a builtin.
            let dispatch_fn = vm.new_function("__poly_js_host_call", poly_js_host_call);
            scope
                .globals
                .set_item("__poly_js_host_call", dispatch_fn.into(), vm)
                .unwrap();
            // Run the js package source
            vm.run_string(
                scope.clone(),
                JS_PACKAGE_SOURCE,
                "<poly-js-package>".to_owned(),
            )
            .expect("js package bootstrap failed");
            scope
        });

        trace("RustPython runtime initialized");
        Self {
            interpreter,
            registry: Mutex::new(ModuleRegistry::new()),
            repl_scope: parking_lot::Mutex::new(Some(repl_scope)),
        }
    }

    /// Handle a bridge request: dispatch to load/describe/call/run_file/
    /// import_js.
    fn handle_request(&self, request: BridgeRequest) -> Result<BridgeResponse, PythonError> {
        let _guard = ReentrancyGuard::enter()?;

        match request.kind.as_str() {
            "load" => self.load_module(&request),
            "describe" => self.describe_module(&request),
            "call" => self.call_function(&request),
            "run_file" => self.run_file_inner(&request),
            "import_js" => self.import_js(&request),
            "py_call_var" => Ok(self.py_call_var(&request.function, &request.args)),
            "py_get_var" => Ok(self.py_get_var(&request.function, &request.property)),
            "py_call_attr" => {
                Ok(self.py_call_attr(&request.function, &request.property, &request.args))
            }
            "py_eval" => Ok(self.py_eval(&request.code)),
            other => Err(PythonError::new(format!("unknown request kind: {other}"))),
        }
    }

    /// Load a Python module by path into the persistent registry.
    fn load_module(&self, request: &BridgeRequest) -> Result<BridgeResponse, PythonError> {
        let path = canonicalize_path(&request.module)?;
        let (scope, _module_loaded) = self.load_or_share(&path, &request.module)?;

        let exports = self.extract_exports(&scope)?;
        Ok(BridgeResponse {
            ok: true,
            exports: Some(exports),
            value: None,
            callable: None,
            error: None,
            error_kind: None,
            traceback: None,
        })
    }

    /// Describe a module — like load but returns full export metadata.
    fn describe_module(&self, request: &BridgeRequest) -> Result<BridgeResponse, PythonError> {
        // Same as `load` for v1
        self.load_module(request)
    }

    /// Call a named function in an already-loaded module.
    fn call_function(&self, request: &BridgeRequest) -> Result<BridgeResponse, PythonError> {
        let path = canonicalize_path(&request.module)?;
        let registry = self.registry.lock().unwrap();
        let state = registry
            .modules
            .get(&path)
            .ok_or_else(|| PythonError::new(format!("module not loaded: {}", path.display())))?;

        let result = self.interpreter.enter(|vm| {
            // Get the function from globals
            let callable = match state.scope.globals.get_item(&request.function, vm) {
                Ok(val) => val,
                Err(exception) => {
                    return BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("function not found: {}", request.function)),
                        error_kind: Some("ImportError".to_string()),
                        traceback: Some(render_exception(vm, &exception)),
                    };
                }
            };

            // Convert JS args to Python values
            let py_args: Vec<_> = match request
                .args
                .iter()
                .map(|v| json_value_to_py(vm, v))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(args) => args,
                Err(e) => {
                    return BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("argument conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    };
                }
            };

            // Build FuncArgs and call
            let func_args = FuncArgs::from(py_args);
            match callable.call(func_args, vm) {
                Ok(result_value) => match py_value_to_json(vm, &result_value) {
                    Ok(json_val) => BridgeResponse {
                        ok: true,
                        exports: None,
                        value: Some(json_val),
                        callable: None,
                        error: None,
                        error_kind: None,
                        traceback: None,
                    },
                    Err(e) => BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("value conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    },
                },
                Err(exception) => {
                    let msg = render_exception(vm, &exception);
                    let kind = if msg.contains("ERR_POLY_REENTRANT_PYTHON_CALL") {
                        "ReentrantCall"
                    } else {
                        "PythonError"
                    };
                    BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(msg.clone()),
                        error_kind: Some(kind.to_string()),
                        traceback: Some(msg),
                    }
                }
            }
        });

        Ok(result)
    }

    /// Run a file as a standalone script (for `poly app.py` entry).
    fn run_file_inner(&self, request: &BridgeRequest) -> Result<BridgeResponse, PythonError> {
        let path = canonicalize_path(&request.module)?;
        let source = fs::read_to_string(&path)
            .map_err(|e| PythonError::new(format!("cannot read {}: {e}", path.display())))?;
        let display_path = path.to_string_lossy().into_owned();
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();

        let mut settings = Settings::default();
        settings.argv = std::iter::once(display_path.clone())
            .chain(request.script_args.iter().cloned())
            .collect();
        settings.path_list.push(parent);
        settings.write_bytecode = false;
        settings.install_signal_handlers = false;

        let interpreter = InterpreterBuilder::new()
            .settings(settings)
            .init_stdlib()
            .interpreter();

        let exit_code = interpreter.run(|vm| {
            // Also register the __poly_js_host_call function in this one-shot interpreter
            let scope = vm.new_scope_with_main()?;
            let dispatch_fn = vm.new_function("__poly_js_host_call", poly_js_host_call);
            scope
                .globals
                .set_item("__poly_js_host_call", dispatch_fn.into(), vm)?;
            // Bootstrap js package
            vm.run_string(
                scope.clone(),
                JS_PACKAGE_SOURCE,
                "<poly-js-package>".to_owned(),
            )?;
            // Make `__file__` available so `js.import_module()` can resolve
            // relative specifiers against the script's directory.
            scope
                .globals
                .set_item("__file__", vm.new_pyobj(display_path.clone()), vm)?;
            // Run the user script
            vm.run_string(scope, &source, display_path).map(drop)
        });

        Ok(BridgeResponse {
            ok: true,
            exports: None,
            value: Some(Value::Number((exit_code as i64).into())),
            callable: None,
            error: None,
            error_kind: None,
            traceback: None,
        })
    }

    /// Import a JS module from Python. Dispatches through the host callback.
    fn import_js(&self, request: &BridgeRequest) -> Result<BridgeResponse, PythonError> {
        let host_cb = HOST_CALLBACK.with(|cb| cb.get()).ok_or_else(|| {
            PythonError::new_kind(
                "js.import_module() requires an active JSC/Bun runtime context".to_string(),
                PythonErrorKind::NoHostContext,
            )
        })?;

        let inner_request = serde_json::json!({
            "kind": "import_js",
            "module": request.module,
            "referrer": request.referrer,
        });

        let response_json = host_cb(&inner_request.to_string()).map_err(PythonError::new)?;

        let response: BridgeResponse = serde_json::from_str(&response_json)
            .map_err(|e| PythonError::new(format!("invalid host response: {e}")))?;

        Ok(response)
    }

    /// Evaluate one REPL input against the persistent REPL scope. Tries the
    /// input as an expression first (printing `repr` unless the result is
    /// None), then as a statement. Syntactically incomplete input (open
    /// brackets, trailing block colon, backslash continuation) is reported
    /// so the REPL can collect more lines.
    fn repl_eval(&self, code: &str, shared: &[SharedExport]) -> ReplEvalResult {
        use rustpython::vm::compiler::Mode;

        self.interpreter.enter(|vm| {
            // Persistent scope: create on first use, reuse forever after.
            let scope = {
                let mut guard = self.repl_scope.lock();
                if guard.is_none() {
                    *guard = Some(vm.new_scope_with_main().unwrap());
                }
                guard.as_ref().unwrap().clone()
            };

            // Inject shared (cross-language) variables before evaluating.
            // JavaScript functions and objects become live `_JSVariable`
            // proxies (resolved by name on every access); only JSON
            // primitives are copied by value.
            for export in shared {
                let py_value = match export.kind.as_str() {
                    "function" | "object" => match make_js_variable(vm, &scope, &export.name) {
                        Some(v) => v,
                        None => continue,
                    },
                    _ => match export
                        .value
                        .as_ref()
                        .and_then(|v| json_value_to_py(vm, v).ok())
                    {
                        Some(v) => v,
                        None => continue,
                    },
                };
                if scope
                    .globals
                    .set_item(export.name.as_str(), py_value, vm)
                    .is_err()
                {
                    return ReplEvalResult {
                        incomplete: false,
                        value: None,
                        error: Some(format!("cannot set shared variable {:?}", export.name)),
                        exports: Vec::new(),
                    };
                }
            }

            // 1. Expression.
            let result = match vm.compile(code, Mode::Eval, "<repl>".to_owned()) {
                Ok(code_obj) => match vm.run_code_obj(code_obj, scope.clone()) {
                    Ok(value) => {
                        // CPython REPL: print repr unless the result is None.
                        if vm.is_none(&value) {
                            ReplEvalResult {
                                incomplete: false,
                                value: None,
                                error: None,
                                exports: Vec::new(),
                            }
                        } else {
                            match value.repr_utf8(vm) {
                                Ok(s) => ReplEvalResult {
                                    incomplete: false,
                                    value: Some(s.to_string()),
                                    error: None,
                                    exports: Vec::new(),
                                },
                                Err(e) => ReplEvalResult {
                                    incomplete: false,
                                    value: None,
                                    error: Some(render_exception(vm, &e)),
                                    exports: Vec::new(),
                                },
                            }
                        }
                    }
                    Err(e) => ReplEvalResult {
                        incomplete: false,
                        value: None,
                        error: Some(render_exception(vm, &e)),
                        exports: Vec::new(),
                    },
                },
                Err(_) => {
                    // 2. Statement.
                    match vm.compile(code, Mode::Exec, "<repl>".to_owned()) {
                        Ok(code_obj) => match vm.run_code_obj(code_obj, scope.clone()) {
                            Ok(_) => ReplEvalResult {
                                incomplete: false,
                                value: None,
                                error: None,
                                exports: Vec::new(),
                            },
                            Err(e) => ReplEvalResult {
                                incomplete: false,
                                value: None,
                                error: Some(render_exception(vm, &e)),
                                exports: Vec::new(),
                            },
                        },
                        Err(compile_err) => {
                            if is_incomplete_python(code) {
                                ReplEvalResult {
                                    incomplete: true,
                                    value: None,
                                    error: None,
                                    exports: Vec::new(),
                                }
                            } else {
                                ReplEvalResult {
                                    incomplete: false,
                                    value: None,
                                    error: Some(format!("SyntaxError: {compile_err}")),
                                    exports: Vec::new(),
                                }
                            }
                        }
                    }
                }
            };

            // Export the REPL globals for cross-language sharing: functions
            // by name (the name is the handle), JSON-serializable values by
            // value. Skip underscore-prefixed names (dunders and injected
            // helpers like __poly_js_host_call) and proxies injected from
            // JavaScript (marked with _poly_js_var) — exporting those back
            // would create a call loop.
            let exports = if result.incomplete {
                Vec::new()
            } else {
                let mut exports = Vec::new();
                for (key, value) in scope.globals.items_vec() {
                    let name = match key.str(vm) {
                        Ok(s) => s.to_string(),
                        Err(_) => continue,
                    };
                    if name.starts_with('_') {
                        continue;
                    }
                    // Modules leaked into the REPL scope by the embedded
                    // `js` package bootstrap (`sys`, `json`, `js`, ...) are
                    // implementation detail, not user variables.
                    if &*value.class().name() == "module" {
                        continue;
                    }
                    if value.is_callable() {
                        // Skip JS proxies injected into this scope.
                        if value.get_attr("_poly_js_var", vm).is_ok() {
                            continue;
                        }
                        exports.push(ReplExport {
                            name,
                            kind: "function".to_string(),
                            value: None,
                        });
                    } else if let Ok(json) = py_value_to_json(vm, &value) {
                        exports.push(ReplExport {
                            name,
                            kind: "value".to_string(),
                            value: Some(json),
                        });
                    } else {
                        // Arbitrary Python object (instance, module, ...):
                        // export by name as a live proxy handle, mirroring
                        // the function case — the JS side reads attributes
                        // through `py_get_var` / `py_call_attr`.
                        exports.push(ReplExport {
                            name,
                            kind: "object".to_string(),
                            value: None,
                        });
                    }
                }
                exports
            };

            ReplEvalResult {
                incomplete: result.incomplete,
                value: result.value,
                error: result.error,
                exports,
            }
        })
    }

    /// Call a Python function from the REPL scope by name with JSON
    /// arguments (JS -> Python). The name is resolved on every call, so it
    /// always refers to the current binding.
    fn py_call_var(&self, name: &str, args: &[Value]) -> BridgeResponse {
        self.interpreter.enter(|vm| {
            let func = {
                let scope = self.repl_scope.lock();
                match scope
                    .as_ref()
                    .and_then(|s| s.globals.get_item(name, vm).ok())
                {
                    Some(f) => f,
                    None => {
                        return BridgeResponse {
                            ok: false,
                            exports: None,
                            value: None,
                            callable: None,
                            error: Some(format!("unknown Python variable: {name}")),
                            error_kind: Some("NameError".to_string()),
                            traceback: None,
                        };
                    }
                }
            };

            let py_args: Vec<_> = match args
                .iter()
                .map(|v| json_value_to_py(vm, v))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(args) => args,
                Err(e) => {
                    return BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("argument conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    };
                }
            };

            let func_args = FuncArgs::from(py_args);
            match func.call(func_args, vm) {
                Ok(result_value) => match py_value_to_json(vm, &result_value) {
                    Ok(json_val) => BridgeResponse {
                        ok: true,
                        exports: None,
                        value: Some(json_val),
                        callable: None,
                        error: None,
                        error_kind: None,
                        traceback: None,
                    },
                    Err(e) => BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("value conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    },
                },
                Err(exception) => BridgeResponse {
                    ok: false,
                    exports: None,
                    value: None,
                    callable: None,
                    error: Some(render_exception(vm, &exception)),
                    error_kind: Some("PythonError".to_string()),
                    traceback: Some(render_exception(vm, &exception)),
                },
            }
        })
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Read an attribute of a Python REPL-scope variable by name (JS ->
    /// Python object proxy). Attribute values that are callable come back
    /// with `callable: true` (the JS side exposes a function proxy backed by
    /// `py_call_attr`); JSON-serializable values by value; anything else by
    /// its `repr()` string.
    fn py_get_var(&self, name: &str, property: &str) -> BridgeResponse {
        self.interpreter.enter(|vm| {
            let value = {
                let scope = self.repl_scope.lock();
                match scope
                    .as_ref()
                    .and_then(|s| s.globals.get_item(name, vm).ok())
                {
                    Some(v) => v,
                    None => {
                        return BridgeResponse {
                            ok: false,
                            exports: None,
                            value: None,
                            callable: None,
                            error: Some(format!("unknown Python variable: {name}")),
                            error_kind: Some("NameError".to_string()),
                            traceback: None,
                        };
                    }
                }
            };

            match value.get_attr(&vm.ctx.new_str(property.to_string()), vm) {
                Ok(attr) => {
                    if attr.is_callable() {
                        BridgeResponse {
                            ok: true,
                            exports: None,
                            value: None,
                            callable: Some(true),
                            error: None,
                            error_kind: None,
                            traceback: None,
                        }
                    } else {
                        match py_value_to_json(vm, &attr) {
                            Ok(json_val) => BridgeResponse {
                                ok: true,
                                exports: None,
                                value: Some(json_val),
                                callable: None,
                                error: None,
                                error_kind: None,
                                traceback: None,
                            },
                            Err(_) => {
                                // Not JSON-serializable: fall back to repr().
                                let repr = attr
                                    .repr(vm)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|_| "<unprintable>".to_string());
                                BridgeResponse {
                                    ok: true,
                                    exports: None,
                                    value: Some(Value::String(repr)),
                                    callable: None,
                                    error: None,
                                    error_kind: None,
                                    traceback: None,
                                }
                            }
                        }
                    }
                }
                Err(exception) => BridgeResponse {
                    ok: false,
                    exports: None,
                    value: None,
                    callable: None,
                    error: Some(render_exception(vm, &exception)),
                    error_kind: Some("AttributeError".to_string()),
                    traceback: Some(render_exception(vm, &exception)),
                },
            }
        })
    }

    /// Call a method of a Python REPL-scope variable by name (JS -> Python
    /// object proxy). Resolves `getattr(scope[name], property)` then calls it
    /// with JSON arguments — mirrors `py_call_var` for attributes.
    fn py_call_attr(&self, name: &str, property: &str, args: &[Value]) -> BridgeResponse {
        self.interpreter.enter(|vm| {
            let value = {
                let scope = self.repl_scope.lock();
                match scope
                    .as_ref()
                    .and_then(|s| s.globals.get_item(name, vm).ok())
                {
                    Some(v) => v,
                    None => {
                        return BridgeResponse {
                            ok: false,
                            exports: None,
                            value: None,
                            callable: None,
                            error: Some(format!("unknown Python variable: {name}")),
                            error_kind: Some("NameError".to_string()),
                            traceback: None,
                        };
                    }
                }
            };

            let attr = match value.get_attr(&vm.ctx.new_str(property.to_string()), vm) {
                Ok(attr) => attr,
                Err(exception) => {
                    return BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(render_exception(vm, &exception)),
                        error_kind: Some("AttributeError".to_string()),
                        traceback: Some(render_exception(vm, &exception)),
                    };
                }
            };

            let py_args: Vec<_> = match args
                .iter()
                .map(|v| json_value_to_py(vm, v))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(args) => args,
                Err(e) => {
                    return BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("argument conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    };
                }
            };

            match attr.call(FuncArgs::from(py_args), vm) {
                Ok(result_value) => match py_value_to_json(vm, &result_value) {
                    Ok(json_val) => BridgeResponse {
                        ok: true,
                        exports: None,
                        value: Some(json_val),
                        callable: None,
                        error: None,
                        error_kind: None,
                        traceback: None,
                    },
                    Err(e) => BridgeResponse {
                        ok: false,
                        exports: None,
                        value: None,
                        callable: None,
                        error: Some(format!("value conversion error: {e}")),
                        error_kind: Some("TypeError".to_string()),
                        traceback: None,
                    },
                },
                Err(exception) => BridgeResponse {
                    ok: false,
                    exports: None,
                    value: None,
                    callable: None,
                    error: Some(render_exception(vm, &exception)),
                    error_kind: Some("PythonError".to_string()),
                    traceback: Some(render_exception(vm, &exception)),
                },
            }
        })
    }

    /// Evaluate a snippet against the persistent REPL scope (Shell mode's
    /// `poly python <expr>`). Returns the `repr()` of the expression result
    /// (None for statements / None results). Exports are not synced here —
    /// entering Python mode syncs the full scope.
    fn py_eval(&self, code: &str) -> BridgeResponse {
        let result = self.repl_eval(code, &[]);
        if let Some(error) = result.error {
            return BridgeResponse {
                ok: false,
                exports: None,
                value: None,
                callable: None,
                error: Some(error),
                error_kind: Some("PythonError".to_string()),
                traceback: None,
            };
        }
        BridgeResponse {
            ok: true,
            exports: None,
            value: result.value.map(Value::String),
            callable: None,
            error: None,
            error_kind: None,
            traceback: None,
        }
    }

    /// Load a module by path, sharing the scope if already loaded.
    fn load_or_share(
        &self,
        path: &Path,
        _original_specifier: &str,
    ) -> Result<(Scope, bool), PythonError> {
        let mut registry = self.registry.lock().unwrap();

        if let Some(state) = registry.modules.get(path) {
            trace(&format!("reusing already-loaded module {}", path.display()));
            return Ok((state.scope.clone(), false));
        }

        let source = fs::read_to_string(path)
            .map_err(|e| PythonError::new(format!("cannot read {}: {e}", path.display())))?;
        let display_path = path.to_string_lossy().into_owned();

        // Load the module inside the persistent interpreter.
        let scope = self.interpreter.enter(|vm| {
            let scope = vm.new_scope_with_main().unwrap();

            // Inject the module path as __file__
            scope
                .globals
                .set_item("__file__", vm.new_pyobj(display_path.clone()), vm)
                .unwrap();

            // Run the module source
            vm.run_string(scope.clone(), &source, display_path.clone())
                .map_err(|e| {
                    PythonError::new(format!(
                        "failed to load module {}: {}",
                        display_path,
                        render_exception(vm, &e)
                    ))
                })?;

            Ok(scope)
        })?;

        registry.modules.insert(
            path.to_path_buf(),
            ModuleState {
                scope: scope.clone(),
            },
        );

        trace(&format!("loaded module {}", path.display()));
        Ok((scope, true))
    }

    /// Extract module exports according to the v1 rules.
    fn extract_exports(&self, scope: &Scope) -> Result<ModuleExports, PythonError> {
        let mut constants: HashMap<String, Value> = HashMap::new();
        let mut callables: Vec<String> = Vec::new();

        let items = scope.globals.items_vec();

        for (key, value) in items {
            // Get the string key through the interpreter
            let key_name: String = self
                .interpreter
                .enter(|vm| key.str(vm).map(|s| s.to_string()).unwrap_or_default());

            // Skip dunder names
            if key_name.starts_with('_') {
                continue;
            }

            // Check if callable
            let is_callable = value.is_callable();

            if is_callable {
                callables.push(key_name);
                continue;
            }

            // Try to convert to JSON
            match self.interpreter.enter(|vm| py_value_to_json(vm, &value)) {
                Ok(json_val) => {
                    constants.insert(key_name, json_val);
                }
                Err(_) => {
                    // Not JSON-serializable, skip silently
                    continue;
                }
            }
        }

        // Sort names for deterministic output
        let mut names: Vec<String> = constants.keys().cloned().collect();
        names.extend(callables.clone());
        names.sort();
        names.dedup();

        Ok(ModuleExports {
            names,
            constants,
            callables,
        })
    }
}

// ---------------------------------------------------------------------------
// JSON ↔ Python value conversion
// ---------------------------------------------------------------------------

/// Convert a `serde_json::Value` to a RustPython object.
fn json_value_to_py(
    vm: &VirtualMachine,
    value: &Value,
) -> Result<rustpython::vm::PyObjectRef, PythonError> {
    match value {
        Value::Null => Ok(vm.ctx.none()),
        Value::Bool(b) => Ok(vm.new_pyobj(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(vm.new_pyobj(i))
            } else if let Some(f) = n.as_f64() {
                if !f.is_finite() {
                    return Err(PythonError::new_kind(
                        "NaN/Infinity values not supported in v1 interop".to_string(),
                        PythonErrorKind::TypeError,
                    ));
                }
                Ok(vm.new_pyobj(f))
            } else {
                Err(PythonError::new("unsupported JSON number"))
            }
        }
        Value::String(s) => Ok(vm.new_pyobj(s.clone())),
        Value::Array(arr) => {
            let items: Result<Vec<_>, _> = arr.iter().map(|v| json_value_to_py(vm, v)).collect();
            Ok(vm.ctx.new_list(items?).into())
        }
        Value::Object(obj) => {
            let dict = vm.ctx.new_dict();
            for (k, v) in obj {
                let py_v = json_value_to_py(vm, v)?;
                dict.set_item(k.as_str(), py_v, vm).map_err(|e| {
                    PythonError::new(format!("dict set error: {}", render_exception(vm, &e)))
                })?;
            }
            Ok(dict.into())
        }
    }
}

/// Convert a RustPython object to `serde_json::Value`.
fn py_value_to_json(
    vm: &VirtualMachine,
    obj: &rustpython::vm::PyObjectRef,
) -> Result<Value, PythonError> {
    // None
    if obj.is(&vm.ctx.none) {
        return Ok(Value::Null);
    }

    // Check for bool (must come before int since bool is subclass of int)
    if obj
        .downcast_ref::<rustpython::vm::builtins::PyBool>()
        .is_some()
    {
        // Compare with true_value
        let is_true = obj.is(&vm.ctx.true_value);
        return Ok(Value::Bool(is_true));
    }

    // int
    if let Some(int_obj) = obj.downcast_ref::<rustpython::vm::builtins::PyInt>() {
        return match int_obj.try_to_primitive::<i64>(vm) {
            Ok(i) => Ok(Value::Number(i.into())),
            Err(_) => Err(PythonError::new_kind(
                "integer outside i64 range for v1 interop".to_string(),
                PythonErrorKind::TypeError,
            )),
        };
    }

    // float
    if let Some(float_obj) = obj.downcast_ref::<rustpython::vm::builtins::PyFloat>() {
        let val = float_obj.to_f64();
        if !val.is_finite() {
            return Err(PythonError::new_kind(
                "NaN/Infinity float values not supported in v1 interop".to_string(),
                PythonErrorKind::TypeError,
            ));
        }
        if let Some(n) = serde_json::Number::from_f64(val) {
            return Ok(Value::Number(n));
        }
        return Ok(Value::Number(serde_json::Number::from_f64(0.0).unwrap()));
    }

    // str
    if let Some(str_ref) = obj.downcast_ref::<rustpython::vm::builtins::PyStr>() {
        return Ok(Value::String(str_ref.to_string()));
    }

    // list
    if let Some(list_ref) = obj.downcast_ref::<rustpython::vm::builtins::PyList>() {
        let elements = list_ref.borrow_vec();
        let items: Result<Vec<_>, _> = elements
            .iter()
            .map(|item| py_value_to_json(vm, item))
            .collect();
        return Ok(Value::Array(items?));
    }

    // dict
    if let Some(dict_ref) = obj.downcast_ref::<rustpython::vm::builtins::PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict_ref.items_vec() {
            let key = k.str(vm).map(|s| s.to_string()).unwrap_or_default();
            let val = py_value_to_json(vm, &v)?;
            map.insert(key, val);
        }
        return Ok(Value::Object(map));
    }

    // tuple → JSON array
    if let Some(tuple_ref) = obj.downcast_ref::<rustpython::vm::builtins::PyTuple>() {
        let items: Result<Vec<_>, _> = tuple_ref
            .iter()
            .map(|item| py_value_to_json(vm, item))
            .collect();
        return Ok(Value::Array(items?));
    }

    Err(PythonError::new_kind(
        format!(
            "unsupported Python type for v1 interop: {}",
            obj.class().name()
        ),
        PythonErrorKind::TypeError,
    ))
}

// ---------------------------------------------------------------------------
// Host callback dispatcher (registered as Python builtin)
// ---------------------------------------------------------------------------

fn poly_js_host_call(request_str: String, vm: &VirtualMachine) -> rustpython::vm::PyResult {
    let host_cb = HOST_CALLBACK.with(|cb| cb.get());

    match host_cb {
        Some(callback) => match callback(&request_str) {
            Ok(response) => Ok(vm.new_pyobj(response)),
            Err(e) => Err(vm.new_value_error(format!("host callback failed: {e}"))),
        },
        None => Err(vm.new_runtime_error(
            "__poly_js_host_call requires an active JSC/Bun runtime context".to_owned(),
        )),
    }
}

/// Heuristic: does this input need more lines? Open brackets, a trailing
/// block colon, or a backslash continuation all mean the REPL should keep
/// collecting instead of reporting a SyntaxError.
fn is_incomplete_python(code: &str) -> bool {
    let trimmed = code.trim_end();
    if trimmed.ends_with('\\') {
        return true;
    }
    let mut depth: i32 = 0;
    for c in code.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    if depth > 0 {
        return true;
    }
    // Trailing colon starts a block. (A dict literal ends with `}`, so this
    // only fires on real block headers like `def f():` / `if x:`.)
    trimmed.ends_with(':')
}

/// Create a Python `_JSVariable` proxy for a JavaScript function (by global
/// name), via the factory installed by the embedded `js` package.
fn make_js_variable(
    vm: &VirtualMachine,
    scope: &Scope,
    name: &str,
) -> Option<rustpython::vm::PyObjectRef> {
    let factory = scope.globals.get_item("_make_js_variable", vm).ok()?;
    let args = FuncArgs::from(vec![vm.new_pyobj(name.to_string())]);
    factory.call(args, vm).ok()
}

/// Evaluate one REPL input against the persistent Python REPL scope.
/// `shared` exports are injected first (JSON values directly, JS function
/// handles as callable proxies); the response exports the scope's functions
/// by handle and JSON values by value for cross-language sharing. Returns a
/// JSON `ReplEvalResult` ({incomplete, value, error, exports}).
pub fn repl_eval(code: &str, shared: &[SharedExport]) -> Result<String, PythonError> {
    let result = RUNTIME.with(|runtime| runtime.repl_eval(code, shared));
    serde_json::to_string(&result)
        .map_err(|e| PythonError::new(format!("cannot encode repl result: {e}")))
}

// ---------------------------------------------------------------------------
// Path normalization
// ---------------------------------------------------------------------------

fn canonicalize_path(raw: &str) -> Result<PathBuf, PythonError> {
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| PythonError::new(format!("cannot resolve cwd: {e}")))?
            .join(path)
    };

    // Resolve .. and . components lexically
    let mut components: Vec<std::path::Component> = Vec::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }

    Ok(components.iter().collect())
}

// ---------------------------------------------------------------------------
// Exception rendering
// ---------------------------------------------------------------------------

fn render_exception(vm: &VirtualMachine, exception: &PyBaseExceptionRef) -> String {
    let mut rendered = String::new();
    if vm.write_exception(&mut rendered, exception).is_err() || rendered.is_empty() {
        "RustPython execution failed".to_owned()
    } else {
        rendered
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Handle a bridge request (JSON in, JSON out).
pub fn handle_bridge_request(request_json: &str) -> Result<String, PythonError> {
    trace("handling bridge request");

    let request: BridgeRequest = serde_json::from_str(request_json)
        .map_err(|e| PythonError::new(format!("invalid bridge request: {e}")))?;

    if request.module.is_empty()
        && request.kind != "import_js"
        && request.kind != "py_call_var"
        && request.kind != "py_get_var"
        && request.kind != "py_call_attr"
        && request.kind != "py_eval"
    {
        return Err(PythonError::new("module path cannot be empty"));
    }

    let result = RUNTIME.with(|runtime| runtime.handle_request(request));

    match result {
        Ok(response) => serde_json::to_string(&response)
            .map_err(|e| PythonError::new(format!("cannot encode response: {e}"))),
        Err(e) => {
            let err_response = BridgeResponse {
                ok: false,
                exports: None,
                value: None,
                callable: None,
                error: Some(e.message().to_string()),
                error_kind: Some(format!("{:?}", e.kind())),
                traceback: None,
            };
            serde_json::to_string(&err_response).map_err(|_| e)
        }
    }
}

/// Run a Python file as a standalone script with args (for `poly app.py`).
pub fn run_file(path: &Path, args: &[String]) -> Result<u32, PythonError> {
    let request = BridgeRequest {
        kind: "run_file".to_string(),
        module: path.to_string_lossy().into_owned(),
        handle: 0,
        function: String::new(),
        args: Vec::new(),
        script_args: args.to_vec(),
        referrer: String::new(),
        property: String::new(),
        code: String::new(),
    };

    let response = RUNTIME.with(|runtime| runtime.run_file_inner(&request))?;

    if response.ok {
        Ok(response
            .value
            .as_ref()
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32)
    } else {
        Err(PythonError::new(
            response
                .error
                .unwrap_or_else(|| "unknown error".to_string()),
        ))
    }
}

/// Legacy compatibility: the old `call_json` that the JSC bridge uses.
pub fn call_json(request_json: &str) -> Result<String, PythonError> {
    handle_bridge_request(request_json)
}

/// Load a Python module by path, returning its export metadata as JSON.
pub fn describe_module(module_path: &str) -> Result<String, PythonError> {
    let request = BridgeRequest {
        kind: "describe".to_string(),
        module: module_path.to_string(),
        handle: 0,
        function: String::new(),
        args: Vec::new(),
        script_args: Vec::new(),
        referrer: String::new(),
        property: String::new(),
        code: String::new(),
    };

    let response = RUNTIME.with(|runtime| runtime.describe_module(&request))?;

    serde_json::to_string(&response)
        .map_err(|e| PythonError::new(format!("cannot encode response: {e}")))
}

// ---------------------------------------------------------------------------
// Settings helper (used by tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn embedded_settings() -> Settings {
    let mut settings = Settings::default();
    settings.argv = vec!["<poly-python>".to_owned()];
    settings.write_bytecode = false;
    settings.install_signal_handlers = false;
    settings
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn test_fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// RustPython needs more stack than the test harness default; run
    /// interpreter-touching tests on a dedicated big-stack thread.
    fn run_with_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .name("poly-test-thread".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    /// Helper: load then call a function in a fixture module on one
    /// big-stack thread (the runtime registry is thread-local, so load and
    /// call must share the thread).
    fn call_test_module(function: &str, args: Vec<Value>) -> BridgeResponse {
        let function = function.to_owned();
        run_with_big_stack(move || {
            let fixture = test_fixture_path("math_tools.py");
            let path_str = fixture.to_string_lossy().into_owned();
            let load_req = BridgeRequest {
                kind: "load".to_string(),
                module: path_str.clone(),
                handle: 0,
                function: String::new(),
                args: Vec::new(),
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            RUNTIME.with(|r| r.handle_request(load_req)).unwrap();

            let call_req = BridgeRequest {
                kind: "call".to_string(),
                module: path_str,
                handle: 0,
                function,
                args,
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            RUNTIME.with(|r| r.handle_request(call_req)).unwrap()
        })
    }

    /// Helper: load a module and return its exports.
    fn load_test_module(name: &str) -> ModuleExports {
        let name = name.to_owned();
        run_with_big_stack(move || {
            let fixture = test_fixture_path(&name);
            let request = BridgeRequest {
                kind: "load".to_string(),
                module: fixture.to_string_lossy().into_owned(),
                handle: 0,
                function: String::new(),
                args: Vec::new(),
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            let response = RUNTIME.with(|r| r.handle_request(request)).unwrap();
            response.exports.expect("expected exports")
        })
    }

    // =======================================================================
    // Basic module loading
    // =======================================================================

    #[test]
    fn module_loads_and_reuses_state() {
        run_with_big_stack(|| {
            let fixture = test_fixture_path("math_tools.py");
            let path_str = fixture.to_string_lossy().into_owned();

            // Load once
            let req1 = BridgeRequest {
                kind: "load".to_string(),
                module: path_str.clone(),
                handle: 0,
                function: String::new(),
                args: Vec::new(),
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            let _resp1 = RUNTIME.with(|r| r.handle_request(req1)).unwrap();

            // Load twice — should reuse cached module
            let req2 = BridgeRequest {
                kind: "call".to_string(),
                module: path_str,
                handle: 0,
                function: "get_call_count".to_string(),
                args: vec![],
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            let resp2 = RUNTIME.with(|r| r.handle_request(req2)).unwrap();
            assert!(resp2.ok, "second call should succeed: {:?}", resp2.error);
        })
    }

    #[test]
    fn module_exports_callables() {
        let exports = load_test_module("math_tools.py");
        assert!(
            exports.callables.contains(&"add".to_string()),
            "add should be a callable"
        );
    }

    #[test]
    fn module_exports_constants() {
        let exports = load_test_module("math_tools.py");
        if let Some(pi) = exports.constants.get("PI") {
            // The fixture deliberately defines PI = 3.1416, not the constant.
            #[allow(clippy::approx_constant)]
            let expected = Value::Number(serde_json::Number::from_f64(3.1416).unwrap());
            assert_eq!(pi, &expected);
        }
    }

    #[test]
    fn module_default_exports_excludes_underscore() {
        let exports = load_test_module("math_tools.py");
        // _internal_helper should not be in names or callables
        assert!(!exports.names.contains(&"_internal_helper".to_string()));
    }

    // =======================================================================
    // Function calls
    // =======================================================================

    #[test]
    fn calls_python_function_synchronously() {
        let response = call_test_module("add", vec![Value::from(20), Value::from(22)]);
        assert!(response.ok, "call should succeed: {:?}", response.error);
        assert_eq!(response.value, Some(Value::from(42)));
    }

    #[test]
    fn calls_function_with_string_args() {
        let response = call_test_module("greet", vec![Value::from("world")]);
        assert!(response.ok);
        assert_eq!(response.value, Some(Value::from("hello world")));
    }

    #[test]
    fn calls_function_with_list_arg() {
        let response = call_test_module(
            "sum_list",
            vec![Value::Array(vec![
                Value::from(1),
                Value::from(2),
                Value::from(3),
            ])],
        );
        assert!(response.ok);
        assert_eq!(response.value, Some(Value::from(6)));
    }

    // =======================================================================
    // Error handling
    // =======================================================================

    #[test]
    fn python_exception_maps_to_error_response() {
        // Call a non-existent function
        let response = call_test_module("does_not_exist", vec![]);
        assert!(!response.ok);
        assert!(response.error.is_some());
    }

    #[test]
    fn type_error_for_unsupported_value() {
        let response = call_test_module("make_object", vec![]);
        assert!(!response.ok);
    }

    #[test]
    fn module_not_found_error() {
        run_with_big_stack(|| {
            let request = BridgeRequest {
                kind: "load".to_string(),
                module: "/nonexistent/module.py".to_string(),
                handle: 0,
                function: String::new(),
                args: Vec::new(),
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };

            let response = RUNTIME.with(|r| r.handle_request(request));
            assert!(response.is_err());
        })
    }

    // =======================================================================
    // Validation
    // =======================================================================

    #[test]
    fn empty_module_path_rejected() {
        let request_json = r#"{"kind":"load","module":"","function":"test"}"#;
        let response = handle_bridge_request(request_json);
        assert!(response.is_err());
    }

    #[test]
    fn embedded_runtime_preserves_host_signal_handlers() {
        let settings = embedded_settings();
        assert!(!settings.install_signal_handlers);
    }

    // =======================================================================
    // Reentrancy guard
    // =======================================================================

    #[test]
    fn reentrancy_guard_depth_one_allowed() {
        let g = ReentrancyGuard::enter();
        assert!(g.is_ok());
        // Dropping the guard decrements
        drop(g);
        let g2 = ReentrancyGuard::enter();
        assert!(g2.is_ok());
    }

    #[test]
    fn reentrancy_guard_depth_three_rejected() {
        let g1 = ReentrancyGuard::enter();
        assert!(g1.is_ok());
        // Depth 2 allowed, depth 3+ should fail
        let g2 = ReentrancyGuard::enter();
        assert!(g2.is_ok());
        let g3 = ReentrancyGuard::enter();
        assert!(g3.is_err());
    }

    // =======================================================================
    // describe_module public API
    // =======================================================================

    #[test]
    fn describe_module_returns_export_metadata() {
        run_with_big_stack(|| {
            let fixture = test_fixture_path("math_tools.py");
            let path_str = fixture.to_string_lossy().into_owned();
            let json = describe_module(&path_str).unwrap();
            let response: BridgeResponse = serde_json::from_str(&json).unwrap();
            assert!(response.ok);
            let exports = response.exports.unwrap();
            assert!(exports.callables.contains(&"add".to_string()));
        })
    }
}

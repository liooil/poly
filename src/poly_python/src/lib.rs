use std::fmt;
use std::fs;
use std::path::Path;

use rustpython::vm::builtins::{PyBaseExceptionRef, PyStrRef};
use rustpython::vm::{Settings, VirtualMachine};
use rustpython::{Interpreter, InterpreterBuilder, InterpreterBuilderExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn trace(message: &str) {
    if std::env::var_os("POLY_TRACE").is_some() {
        eprintln!("[poly trace] {message}");
    }
}

const CALL_BRIDGE: &str = r#"
import contextlib
import io
import json
import os
import runpy
import sys
import traceback

_request = json.loads(__poly_request_json)
_captured = io.StringIO()
_module = os.path.abspath(_request["module"])
_module_dir = os.path.dirname(_module)
_added_path = _module_dir not in sys.path

try:
    if _added_path:
        sys.path.insert(0, _module_dir)
    with contextlib.redirect_stdout(_captured):
        _namespace = runpy.run_path(_module)
        _target = _namespace[_request["function"]]
        _value = _target(*_request.get("args", []))
    __poly_response_json = json.dumps(
        {
            "ok": True,
            "value": _value,
            "stdout": _captured.getvalue(),
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
except BaseException as _error:
    __poly_response_json = json.dumps(
        {
            "ok": False,
            "error": f"{type(_error).__name__}: {_error}",
            "traceback": traceback.format_exc(),
            "stdout": _captured.getvalue(),
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
finally:
    if _added_path:
        sys.path.remove(_module_dir)
"#;

#[derive(Debug)]
pub struct PythonError {
    message: String,
}

impl PythonError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PythonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PythonError {}

#[derive(Debug, Deserialize, Serialize)]
pub struct PythonCallRequest {
    pub module: String,
    pub function: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

struct PythonRuntime {
    interpreter: Interpreter,
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
        let settings = embedded_settings();

        let interpreter = InterpreterBuilder::new()
            .settings(settings)
            .init_stdlib()
            .interpreter();

        trace("RustPython runtime initialized");
        Self { interpreter }
    }

    fn call_json(&self, request_json: &str) -> Result<String, PythonError> {
        trace("decoding JS to Python request");
        let request: PythonCallRequest = serde_json::from_str(request_json)
            .map_err(|error| PythonError::new(format!("invalid call request: {error}")))?;
        validate_request(&request)?;
        let normalized = normalize_request(request)?;
        let normalized_json = serde_json::to_string(&normalized)
            .map_err(|error| PythonError::new(format!("cannot encode call request: {error}")))?;

        trace("entering RustPython interpreter");
        let result = self.interpreter.enter(|vm| {
            let result = (|| {
                let scope = vm.new_scope_with_main()?;
                scope
                    .globals
                    .set_item("__poly_request_json", vm.new_pyobj(normalized_json), vm)?;
                vm.run_string(scope.clone(), CALL_BRIDGE, "<poly-bridge>".to_owned())?;
                let response = scope.globals.get_item("__poly_response_json", vm)?;
                let response: PyStrRef = response.downcast().map_err(|_| {
                    vm.new_type_error("bridge response must be a string".to_owned())
                })?;
                Ok(response.expect_str().to_owned())
            })();

            result.map_err(|exception| PythonError::new(render_exception(vm, &exception)))
        });
        trace("left RustPython interpreter");
        result
    }
}

fn embedded_settings() -> Settings {
    let mut settings = Settings::default();
    settings.argv = vec!["<poly-python>".to_owned()];
    settings.write_bytecode = false;
    // RustPython defaults to installing process-wide C signal handlers. Poly
    // is an embedding host, so Bun/JSC must remain the owner of the process
    // signal and exception state that it configures during runtime startup.
    settings.install_signal_handlers = false;
    settings
}

pub fn call_json(request_json: &str) -> Result<String, PythonError> {
    trace("dispatching JS to Python call");
    let result = RUNTIME.with(|runtime| runtime.call_json(request_json));
    trace("completed JS to Python call");
    result
}

pub fn run_file(path: &Path, args: &[String]) -> Result<u32, PythonError> {
    let source = fs::read_to_string(path).map_err(|error| {
        PythonError::new(format!(
            "cannot read Python file {}: {error}",
            path.display()
        ))
    })?;
    let display_path = path.to_string_lossy().into_owned();
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .into_owned();

    let mut settings = Settings::default();
    settings.argv = std::iter::once(display_path.clone())
        .chain(args.iter().cloned())
        .collect();
    settings.path_list.push(parent);
    settings.write_bytecode = false;

    let interpreter = InterpreterBuilder::new()
        .settings(settings)
        .init_stdlib()
        .interpreter();
    let exit_code = interpreter.run(|vm| {
        let scope = vm.new_scope_with_main()?;
        vm.run_string(scope, &source, display_path).map(drop)
    });

    Ok(exit_code)
}

fn validate_request(request: &PythonCallRequest) -> Result<(), PythonError> {
    if request.module.is_empty() {
        return Err(PythonError::new("Python module path cannot be empty"));
    }
    if request.function.is_empty() {
        return Err(PythonError::new("Python function name cannot be empty"));
    }
    if !Path::new(&request.module).is_file() {
        return Err(PythonError::new(format!(
            "Python module does not exist: {}",
            request.module
        )));
    }
    Ok(())
}

fn normalize_request(mut request: PythonCallRequest) -> Result<PythonCallRequest, PythonError> {
    let module = Path::new(&request.module);
    // On Unix, `Path::canonicalize` obtains a `realpath(NULL)` buffer from
    // libc. The final Bun executable routes Rust allocations through mimalloc,
    // so freeing that libc-owned buffer crosses allocators and can crash.
    // A lexical absolute path is sufficient because the bridge normalizes it
    // again with Python's `os.path.abspath`.
    let absolute = if module.is_absolute() {
        module.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                PythonError::new(format!("cannot resolve the current directory: {error}"))
            })?
            .join(module)
    };
    request.module = absolute.to_string_lossy().into_owned();
    Ok(request)
}

fn render_exception(vm: &VirtualMachine, exception: &PyBaseExceptionRef) -> String {
    let mut rendered = String::new();
    if vm.write_exception(&mut rendered, exception).is_err() || rendered.is_empty() {
        "RustPython execution failed".to_owned()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{PythonCallRequest, embedded_settings, normalize_request};

    #[test]
    fn embedded_runtime_preserves_host_signal_handlers() {
        assert!(!embedded_settings().install_signal_handlers);
    }

    #[test]
    fn request_normalization_avoids_allocator_crossing_canonicalization() {
        let request = PythonCallRequest {
            module: Path::new("relative")
                .join("module.py")
                .to_string_lossy()
                .into_owned(),
            function: "main".to_owned(),
            args: Vec::new(),
        };

        let normalized = normalize_request(request).unwrap();
        let module = Path::new(&normalized.module);
        assert!(module.is_absolute());
        assert!(module.ends_with(Path::new("relative").join("module.py")));
    }
}

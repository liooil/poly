use std::path::PathBuf;

use poly_python::{PythonCallRequest, call_json, initialize};
use serde_json::{Value, json};

#[test]
fn calls_python_and_returns_structured_json() {
    // Rust's test harness uses a smaller stack than Bun's main runtime thread.
    // The call itself remains synchronous and on this same spawned test thread.
    std::thread::Builder::new()
        .name("poly-same-thread-test".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let caller_thread = std::thread::current().id();
            initialize();
            initialize();
            let module = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/math_tools.py")
                .canonicalize()
                .unwrap();
            let request = PythonCallRequest {
                module: module.to_string_lossy().into_owned(),
                function: "add".to_owned(),
                args: vec![Value::from(20), Value::from(22)],
            };

            let response = call_json(&serde_json::to_string(&request).unwrap()).unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();

            assert_eq!(std::thread::current().id(), caller_thread);
            assert_eq!(response["ok"], json!(true));
            assert_eq!(response["value"], json!(42));
            assert_eq!(response["stdout"], json!("[python] adding 20 and 22\n"));
        })
        .unwrap()
        .join()
        .unwrap();
}

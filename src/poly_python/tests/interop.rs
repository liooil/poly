use std::path::PathBuf;

use poly_python::{BridgeRequest, call_json};
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
            let module = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/math_tools.py")
                .canonicalize()
                .unwrap();
            let module = module.to_string_lossy().into_owned();

            // The runtime registry is thread-local: load first, then call on
            // the same thread.
            let load_request = BridgeRequest {
                kind: "load".to_owned(),
                module: module.clone(),
                handle: 0,
                function: String::new(),
                args: Vec::new(),
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };
            call_json(&serde_json::to_string(&load_request).unwrap()).unwrap();

            let request = BridgeRequest {
                kind: "call".to_owned(),
                module,
                handle: 0,
                function: "add".to_owned(),
                args: vec![Value::from(20), Value::from(22)],
                script_args: Vec::new(),
                referrer: String::new(),
                property: String::new(),
                code: String::new(),
            };

            let response = call_json(&serde_json::to_string(&request).unwrap()).unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();

            assert_eq!(std::thread::current().id(), caller_thread);
            assert_eq!(response["ok"], json!(true));
            assert_eq!(response["value"], json!(42));
        })
        .unwrap()
        .join()
        .unwrap();
}

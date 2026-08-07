//! Integration tests for the Poly Rust interpreter: full pipeline
//! (parse → lower → typecheck → interpret) against small source programs.

use poly_rust::diagnostics::DiagKind;
use poly_rust::{RunOutcome, run_source};

/// Run source, capturing interpreter stdout into a buffer.
fn run(src: &str) -> (u32, Vec<String>, Vec<(DiagKind, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let out: Box<dyn std::io::Write + '_> = Box::new(&mut buf);
    let outcome = run_source(src, "test.rs", &[], Some(out));
    let stdout = String::from_utf8_lossy(&buf).to_string();
    let diags = outcome
        .diagnostics
        .iter()
        .map(|d| (d.kind, d.message.clone()))
        .collect();
    (outcome.exit_code, stdout.lines().map(|s| s.to_string()).collect(), diags)
}

fn exit_code(outcome: &RunOutcome) -> u32 {
    outcome.exit_code
}

#[test]
fn hello_world() {
    let (code, out, diags) = run(r#"
fn main() {
    println!("hello");
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["hello"]);
    assert!(diags.is_empty());
}

#[test]
fn fib_recursion() {
    let (code, out, diags) = run(r#"
fn fib(n: i64) -> i64 {
    if n <= 1 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    println!("{}", fib(10));
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["55"]);
    assert!(diags.is_empty());
}

#[test]
fn let_bindings_and_arithmetic() {
    let (code, out, diags) = run(r#"
fn main() {
    let a = 10;
    let b: i64 = 20;
    let c = a + b * 2;
    println!("{}", c);
    let s = "poly";
    println!("{}", s);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["50", "poly"]);
}

#[test]
fn mutable_assignment() {
    let (code, out, diags) = run(r#"
fn main() {
    let mut x = 1;
    x = x + 2;
    x += 3;
    println!("{}", x);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["6"]);
}

#[test]
fn while_loop() {
    let (code, out, diags) = run(r#"
fn main() {
    let mut i = 0;
    let mut sum = 0;
    while i < 10 {
        sum = sum + i;
        i = i + 1;
    }
    println!("{}", sum);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["45"]);
}

#[test]
fn loop_infinite_safety_valve() {
    // `loop {}` without break can never exit; the interpreter's safety valve
    // must fire rather than hang the test.
    let (code, _out, diags) = run(r#"
fn main() {
    loop {
    }
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Runtime));
}

#[test]
fn return_early() {
    let (code, out, diags) = run(r#"
fn f(x: i64) -> i64 {
    if x > 0 {
        return 1;
    }
    return 2;
}

fn main() {
    println!("{} {}", f(5), f(-5));
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["1 2"]);
}

#[test]
fn divide_by_zero_runtime_error() {
    let (code, _out, diags) = run(r#"
fn main() {
    let x = 1 / 0;
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Runtime));
}

#[test]
fn syntax_error_detected() {
    let (code, _out, diags) = run(r#"
fn main() {
    let = 5;
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Syntax));
}

#[test]
fn unsupported_trait_detected_with_span() {
    let (code, _out, diags) = run(r#"
trait Greet {
    fn greet(&self);
}

fn main() {
    println!("hi");
}
"#);
    assert_eq!(code, 1);
    let (kind, msg) = diags.first().unwrap();
    assert_eq!(*kind, DiagKind::Unsupported);
    assert!(msg.contains("trait"), "message was: {msg}");
    assert!(msg.contains("not supported"), "message was: {msg}");
}

#[test]
fn unsupported_struct_detected() {
    let (code, _out, diags) = run(r#"
struct Point {
    x: i64,
}

fn main() {}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Unsupported));
}

#[test]
fn type_error_immutable_assign() {
    let (code, _out, diags) = run(r#"
fn main() {
    let x = 5;
    x = 6;
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Type));
    assert!(diags.first().unwrap().1.contains("immutable"));
}

#[test]
fn type_error_mismatch() {
    let (code, _out, diags) = run(r#"
fn main() {
    let x: i64 = true;
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Type));
    assert!(diags.first().unwrap().1.contains("mismatched types"));
}

#[test]
fn unknown_function_runtime_error() {
    let (code, _out, diags) = run(r#"
fn main() {
    nope();
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Type));
    assert!(diags.first().unwrap().1.contains("cannot find function"));
}

#[test]
fn bool_and_short_circuit() {
    let (code, out, diags) = run(r#"
fn main() {
    let a = true && false;
    let b = true || false;
    let c = !a && b;
    println!("{} {} {}", a, b, c);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["false true true"]);
}

#[test]
fn string_escaping() {
    let (code, out, diags) = run(r#"
fn main() {
    println!("a\nb\tc");
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["a", "b\tc"]);
}

#[test]
fn float_arithmetic() {
    let (code, out, diags) = run(r#"
fn main() {
    let x = 1.5 + 2.25;
    println!("{}", x);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["3.75"]);
}

#[test]
fn no_main_runtime_error() {
    let (code, _out, diags) = run(r#"
fn helper() {}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Runtime));
    assert!(diags.first().unwrap().1.contains("main"));
}

#[test]
fn nested_blocks() {
    let (code, out, diags) = run(r#"
fn main() {
    let x = {
        let y = 3;
        y * 2
    };
    println!("{}", x);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["6"]);
}

#[test]
fn else_if_chain() {
    let (code, out, diags) = run(r#"
fn main() {
    let x = 2;
    if x == 1 {
        println!("one");
    } else if x == 2 {
        println!("two");
    } else {
        println!("other");
    }
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["two"]);
}

#[test]
fn recursion_depth_guard() {
    let (code, _out, diags) = run(r#"
fn f(n: i64) -> i64 {
    f(n + 1)
}

fn main() {
    f(0);
}
"#);
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Runtime));
    assert!(diags.first().unwrap().1.contains("stack overflow"));
}

#[test]
fn int_overflow_runtime_error() {
    let (code, _out, diags) = run(r#"
fn main() {
    let big: i128 = 170141183460469231731687303715884105727;
    let x = big * 2;
    println!("{}", x);
}
"#);
    // i128::MAX * 2 overflows i128 — should be a runtime error, not silent
    // wraparound.
    assert_eq!(code, 1);
    assert_eq!(diags.first().map(|d| d.0), Some(DiagKind::Runtime));
    assert!(diags.first().unwrap().1.contains("overflow"));
}

#[test]
fn cast_expression() {
    let (code, out, diags) = run(r#"
fn main() {
    let x = 7;
    let y = x as f64;
    println!("{}", y);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["7"]);
}

#[test]
fn print_with_debug() {
    let (code, out, diags) = run(r#"
fn main() {
    let s = "quoted";
    println!("{:?}", s);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["\"quoted\""]);
}

#[test]
fn shadowing() {
    let (code, out, diags) = run(r#"
fn main() {
    let x = 1;
    let x = x + 1;
    println!("{}", x);
}
"#);
    assert_eq!(code, 0, "diagnostics: {diags:?}");
    assert_eq!(out, vec!["2"]);
}

// ---------------------------------------------------------------------------
// exit_code helper is used above; keep it referenced so the import is used.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _assert_exit(outcome: &RunOutcome, expected: u32) {
    assert_eq!(exit_code(outcome), expected);
}

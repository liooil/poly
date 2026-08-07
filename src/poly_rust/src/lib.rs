//! Experimental Rust source interpreter for the Polyglot Bun fork.
//!
//! `poly run a.rs` parses Rust source with rust-analyzer's parser
//! (`ra_ap_syntax`), lowers it to a small HIR, structurally type-checks it,
//! and interprets it in-process. No rustc, Cargo, Miri, LLVM, or generated
//! executables are involved.
//!
//! Pipeline:
//!
//! ```text
//! source ── ra_ap_syntax parse ── lower(HIR) ── typecheck ── interpret
//!    │            │                    │             │            │
//!    └─ syntax errors          unsupported     type errors   runtime errors
//! ```
//!
//! All four error classes are reported as [`Diagnostic`]s with source spans.

pub mod diagnostics;
pub mod hir;
pub mod interpreter;
pub mod syntax;
pub mod typecheck;
pub mod value;

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use diagnostics::line_starts;
use diagnostics::Diagnostic;
use diagnostics::render;

/// Result of running a `.rs` file: exit code on success, diagnostics on
/// failure.
#[derive(Debug)]
pub struct RunOutcome {
    pub exit_code: u32,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse, lower, typecheck, and interpret a Rust source string.
///
/// `source_path` is used for diagnostics. `args` are exposed as `env::args`
/// (currently unused by the mini subset; kept for future compatibility).
/// Interpreter output goes to `out` (`None` = process stdout).
pub fn run_source(
    source: &str,
    _source_path: &str,
    _args: &[String],
    out: Option<Box<dyn std::io::Write + '_>>,
) -> RunOutcome {
    // 1. Parse.
    let parsed = syntax::parse(source);
    if !parsed.errors.is_empty() {
        return RunOutcome {
            exit_code: 1,
            diagnostics: parsed.errors,
        };
    }

    // 2. Lower to HIR (reports unsupported features).
    let program = hir::lower(&parsed.file);
    if !program.diagnostics.is_empty() {
        return RunOutcome {
            exit_code: 1,
            diagnostics: program.diagnostics,
        };
    }

    // 3. Type-check (reports type errors).
    let type_diags = typecheck::TypeChecker::check(&program);
    if !type_diags.is_empty() {
        return RunOutcome {
            exit_code: 1,
            diagnostics: type_diags,
        };
    }

    // 4. Interpret (reports runtime errors).
    let mut interp = interpreter::Interpreter::new(out);
    match interp.run(&program) {
        Ok(()) => RunOutcome {
            exit_code: 0,
            diagnostics: interp.diagnostics().to_vec(),
        },
        Err(()) => RunOutcome {
            exit_code: 1,
            diagnostics: interp.diagnostics().to_vec(),
        },
    }
}

/// Read a `.rs` file and run it. Returns the exit code; diagnostics are
/// printed to stderr in rustc-style format.
pub fn run_file(path: &Path, args: &[String]) -> u32 {
    let display_path = path.display().to_string();
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {display_path}: {e}");
            return 1;
        }
    };

    let outcome = run_source(&source, &display_path, args, None);

    // Print diagnostics to stderr.
    let starts = line_starts(&source);
    let mut rendered = String::new();
    for diag in &outcome.diagnostics {
        let _ = writeln!(rendered, "{}", render(diag, &display_path, &source, &starts));
    }
    if !rendered.is_empty() {
        let _ = std::io::stderr().write_all(rendered.as_bytes());
    }

    // Ensure stdout is flushed before we hand control back.
    let _ = std::io::stdout().flush();
    outcome.exit_code
}

/// Convenience for the bridge: run a source string and return a JSON-shaped
/// summary. Kept minimal — the CLI path uses [`run_file`].
pub fn describe_unsupported(source: &str) -> Vec<Diagnostic> {
    let parsed = syntax::parse(source);
    if !parsed.errors.is_empty() {
        return parsed.errors;
    }
    hir::lower(&parsed.file).diagnostics
}

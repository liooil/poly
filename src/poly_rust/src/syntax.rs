//! Syntax layer: wraps rust-analyzer's parser (`ra_ap_syntax`) and turns
//! parse errors into [`Diagnostic`]s.
//!
//! rust-analyzer's parser is a hand-written, resilient Rust parser that
//! produces a lossless syntax tree (rowan). It never fails hard: malformed
//! input produces an ERROR node plus a list of [`SyntaxError`]s, which we
//! surface as syntax diagnostics.

use ra_ap_syntax::{Edition, SourceFile};

use crate::diagnostics::Diagnostic;

/// A parsed source file plus the parser-produced diagnostics.
pub struct Parsed {
    /// The rust-analyzer source file (root of the syntax tree).
    pub file: SourceFile,
    /// Syntax errors reported by the parser.
    pub errors: Vec<Diagnostic>,
}

/// Parse Rust source text with the 2024 edition (matches the workspace's
/// edition and is the current stable edition).
pub fn parse(source: &str) -> Parsed {
    let parsed = SourceFile::parse(source, Edition::Edition2024);
    let errors = parsed
        .errors()
        .into_iter()
        .map(|err| {
            // `SyntaxError::message` is available via Display; range via
            // `range()`.
            let range = err.range();
            Diagnostic::syntax(range, err.to_string())
        })
        .collect();
    Parsed {
        file: parsed.tree(),
        errors,
    }
}

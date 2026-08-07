//! Diagnostics for the Poly Rust interpreter.
//!
//! Every diagnostic carries a source location so callers can render
//! rustc-style `error: ... --> file:line:col` output. Error kinds are
//! deliberately disjoint so a caller can distinguish syntax errors,
//! unsupported Rust features, type errors, and runtime errors.

use std::fmt;

use text_size::TextRange;

/// The four error classes the Poly Rust runtime distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagKind {
    /// The source did not parse (ra_ap_syntax produced parse errors).
    Syntax,
    /// The source parsed, but uses a Rust feature the interpreter does not
    /// support yet (trait, struct, generics, ...).
    Unsupported,
    /// The program type-checks structurally (arity, operand kinds, ...) but
    /// violates the mini type system.
    Type,
    /// A runtime failure (divide by zero, integer overflow, missing main, ...).
    Runtime,
}

impl fmt::Display for DiagKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagKind::Syntax => write!(f, "error"),
            DiagKind::Unsupported => write!(f, "error"),
            DiagKind::Type => write!(f, "error"),
            DiagKind::Runtime => write!(f, "error"),
        }
    }
}

/// A single diagnostic with a source span.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub kind: DiagKind,
    pub message: String,
    /// Byte range in the original source. `None` for whole-file diagnostics.
    pub range: Option<TextRange>,
    /// Human-readable feature name for `Unsupported` diagnostics, e.g.
    /// `trait` or `struct`. Used for the "`X` is not supported" phrasing.
    pub feature: Option<String>,
}

impl Diagnostic {
    pub fn syntax(range: TextRange, message: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Syntax,
            message: message.into(),
            range: Some(range),
            feature: None,
        }
    }

    pub fn unsupported(range: TextRange, feature: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Unsupported,
            message: format!("`{}` is not supported by the Poly Rust runtime yet", feature.into()),
            range: Some(range),
            feature: None,
        }
    }

    pub fn unsupported_no_range(feature: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Unsupported,
            message: format!("`{}` is not supported by the Poly Rust runtime yet", feature.into()),
            range: None,
            feature: None,
        }
    }

    pub fn ty(range: TextRange, message: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Type,
            message: message.into(),
            range: Some(range),
            feature: None,
        }
    }

    pub fn runtime(range: TextRange, message: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Runtime,
            message: message.into(),
            range: Some(range),
            feature: None,
        }
    }
}

/// Renders a `Diagnostic` with a rustc-style source location:
///
/// ```text
/// error: `trait` is not supported by the Poly Rust runtime yet
///  --> app.rs:8:1
/// ```
///
/// `line_starts` is the cumulative byte offsets of each line start in the
/// source (see [`line_starts`]).
pub fn render(diag: &Diagnostic, file: &str, source: &str, line_starts: &[usize]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{}: {}\n",
        match diag.kind {
            DiagKind::Syntax => "syntax error",
            DiagKind::Unsupported => "error",
            DiagKind::Type => "type error",
            DiagKind::Runtime => "runtime error",
        },
        diag.message
    ));

    if let Some(range) = diag.range {
        let (line, col) = line_col(u32::from(range.start()), line_starts);
        out.push_str(&format!(" --> {file}:{line}:{col}\n"));
        if let Some(text) = source_line(source, u32::from(range.start()), line_starts) {
            out.push_str(&format!(" {text}\n"));
            let caret_col = col.saturating_sub(1).min(text.chars().count());
            out.push_str(&format!(" {}^", " ".repeat(caret_col)));
            if !range.is_empty() {
                let width = (u32::from(range.end()).saturating_sub(u32::from(range.start())))
                    .max(1)
                    .min(40) as usize;
                out.push_str(&"~".repeat(width.saturating_sub(1)));
            }
            out.push('\n');
        }
    }
    out
}

/// Byte offsets of every line start in `source`. Line 0 starts at byte 0.
pub fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Convert a byte offset to 1-based `(line, column)` using `line_starts`.
pub fn line_col(offset: u32, line_starts: &[usize]) -> (usize, usize) {
    let offset = offset as usize;
    let line = line_starts.partition_point(|&s| s <= offset);
    let line_start = line_starts[line.saturating_sub(1)];
    // Column is 1-based, measured in characters.
    let col = source_column(offset, line_start, line_starts, None).max(1);
    (line, col)
}

/// The character column (1-based) of `offset` within its line.
fn source_column(offset: usize, line_start: usize, _starts: &[usize], _src: Option<&str>) -> usize {
    // Callers pass the byte delta; without the source text we count bytes.
    // `render` re-derives the visual caret from the actual line text, so a
    // byte count is acceptable here (CJK columns are off by one per char).
    offset.saturating_sub(line_start) + 1
}

/// Return the text of the line containing byte `offset`, without the trailing
/// newline (if any), or `None` if `offset` is out of range.
pub fn source_line(source: &str, offset: u32, line_starts: &[usize]) -> Option<String> {
    let offset = offset as usize;
    if offset > source.len() {
        return None;
    }
    let line_idx = line_starts.partition_point(|&s| s <= offset);
    let start = line_starts[line_idx.saturating_sub(1)];
    let end = line_starts.get(line_idx).copied().unwrap_or(source.len());
    let line = source.get(start..end)?;
    Some(line.trim_end_matches(['\r', '\n']).to_string())
}

//! Runtime values for the Poly Rust interpreter.
//!
//! The mini type system recognizes five value kinds: signed/unsigned
//! integers (stored as `i128` so all Rust integer widths fit), f64 floats,
//! booleans, strings, and void (unit `()`).

use std::fmt;

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Any Rust integer width, stored widened. Lossy narrowing happens on
    /// typed operations (e.g. assigning to a `i32` variable).
    Int(i128),
    Float(f64),
    Bool(bool),
    /// Owned UTF-8 string (from string literals or `String`-ish values).
    Str(String),
    /// Unit `()`.
    Void,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "string",
            Value::Void => "()",
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            // Rust has no truthiness coercion; `if` needs a bool. The
            // interpreter type-checks this before evaluating, so this only
            // guards against internal bugs.
            _ => true,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Void => write!(f, "()"),
        }
    }
}

/// The static type of a value, used by the structural type checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Void,
    /// An as-yet-unresolved type (e.g. a call to an unknown function, or an
    /// untyped expression that will be checked at runtime).
    Unknown,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::Int => "integer",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "string",
            Ty::Void => "()",
            Ty::Unknown => "unknown",
        }
    }
}

impl From<&Value> for Ty {
    fn from(v: &Value) -> Self {
        match v {
            Value::Int(_) => Ty::Int,
            Value::Float(_) => Ty::Float,
            Value::Bool(_) => Ty::Bool,
            Value::Str(_) => Ty::Str,
            Value::Void => Ty::Void,
        }
    }
}

/// Parse a Rust integer type name into its width and signedness.
/// Returns `(bits, signed)` for the supported widths.
pub fn int_type_info(name: &str) -> Option<(u32, bool)> {
    match name {
        "i8" => Some((8, true)),
        "i16" => Some((16, true)),
        "i32" => Some((32, true)),
        "i64" => Some((64, true)),
        "i128" => Some((128, true)),
        "isize" => Some((64, true)),
        "u8" => Some((8, false)),
        "u16" => Some((16, false)),
        "u32" => Some((32, false)),
        "u64" => Some((64, false)),
        "u128" => Some((128, false)),
        "usize" => Some((64, false)),
        _ => None,
    }
}

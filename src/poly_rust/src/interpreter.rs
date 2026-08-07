//! The Poly Rust interpreter.
//!
//! Walks the lowered HIR with an environment of mutable bindings. Function
//! calls push a new scope; `return` unwinds via a control-flow error. Runtime
//! failures (divide by zero, overflow, unknown variable/function, stack
//! overflow) surface as `DiagKind::Runtime` diagnostics — distinct from
//! syntax, unsupported-feature, and type errors.

use std::collections::HashMap;
use std::io::Write;

use text_size::TextRange;

use crate::diagnostics::{DiagKind, Diagnostic};
use crate::hir::{BinOp, Block, Expr, FnDef, Program, Stmt, UnOp};
use crate::value::Value;

/// Max call depth before we report a stack overflow (recursion guard).
// Deliberately conservative: each interpreter call consumes multiple native
// frames (eval → invoke → exec_block → eval ...), so a deep recursion can
// exhaust the native stack before 512 logical frames. 100 keeps the guard
// well clear of STATUS_STACK_OVERFLOW on default 1-8 MiB thread stacks.
const MAX_CALL_DEPTH: usize = 100;

/// Max loop iterations before reporting runaway loops (safety valve; the
/// interpreter has no break so infinite loops would hang forever).
const MAX_LOOP_ITERATIONS: u64 = 1_000_000;

/// Error type for expression evaluation. `Return` propagates a `return`
/// statement out of nested blocks and loops to the enclosing function call.
enum EvalError {
    /// A runtime error — already recorded in `self.diagnostics`.
    Runtime,
    /// `return <value>` unwinding to the function boundary.
    Return(Value),
}

impl<E> From<E> for EvalError
where
    E: Into<Box<dyn std::error::Error>>,
{
    fn from(_: E) -> Self {
        EvalError::Runtime
    }
}

type EvalResult<T> = Result<T, EvalError>;

pub struct Interpreter<'a> {
    /// All functions by name.
    fns: HashMap<String, FnDef>,
    /// Diagnostics produced at runtime.
    diagnostics: Vec<Diagnostic>,
    call_depth: usize,
    /// Where interpreter output goes. `None` = process stdout.
    out: Option<Box<dyn Write + 'a>>,
}

#[derive(Debug, Clone)]
struct Binding {
    value: Value,
    mutable: bool,
}

impl<'a> Interpreter<'a> {
    /// Create an interpreter writing interpreter output (`println!`) to
    /// `out`. `None` writes to process stdout.
    pub fn new(out: Option<Box<dyn Write + 'a>>) -> Self {
        Interpreter {
            fns: HashMap::new(),
            diagnostics: Vec::new(),
            call_depth: 0,
            out,
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn error(&mut self, span: TextRange, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::runtime(span, message));
    }

    /// Run the program: resolve `main`, call it, return its exit value.
    pub fn run(&mut self, program: &Program) -> Result<(), ()> {
        for item in &program.items {
            match item {
                crate::hir::Item::Fn(f) => {
                    self.fns.insert(f.name.clone(), f.clone());
                }
            }
        }
        let Some(main) = self.fns.get("main").cloned() else {
            self.diagnostics.push(Diagnostic {
                kind: DiagKind::Runtime,
                message: "main function not found".to_string(),
                range: None,
                feature: None,
            });
            return Err(());
        };
        if !main.params.is_empty() {
            self.diagnostics.push(Diagnostic {
                kind: DiagKind::Runtime,
                message: "main function takes no arguments".to_string(),
                range: Some(main.span),
                feature: None,
            });
            return Err(());
        }

        let mut env = HashMap::new();
        self.call_depth = 1;
        match self.exec_block(&main.body, &mut env) {
            Ok(_) => Ok(()),
            Err(EvalError::Return(_)) | Err(EvalError::Runtime) => {
                if self.diagnostics.is_empty() {
                    self.diagnostics.push(Diagnostic {
                        kind: DiagKind::Runtime,
                        message: "program terminated abnormally".to_string(),
                        range: None,
                        feature: None,
                    });
                }
                Err(())
            }
        }
    }

    /// Execute a block with a fresh child scope, returning the block's tail
    /// value (or `Value::Void`).
    ///
    /// Rust block semantics: `let` bindings are block-scoped (they do not
    /// escape), but assignment to an *outer* binding writes through. We model
    /// this by cloning the parent env, executing, then copying every binding
    /// that already existed in the parent back into it (assignments landed on
    /// those slots; fresh `let`s stay inside the block).
    fn exec_block(
        &mut self,
        block: &Block,
        parent: &mut HashMap<String, Binding>,
    ) -> EvalResult<Value> {
        let mut env = parent.clone();
        let result = self.exec_block_in(&block, &mut env);
        // Write back assignments to pre-existing bindings.
        for (name, binding) in &env {
            if parent.contains_key(name) {
                parent.insert(name.clone(), binding.clone());
            }
        }
        result
    }

    /// Execute a block against a caller-provided env (the write-back
    /// bookkeeping lives in [`Self::exec_block`]).
    fn exec_block_in(
        &mut self,
        block: &Block,
        env: &mut HashMap<String, Binding>,
    ) -> EvalResult<Value> {
        for stmt in &block.stmts {
            self.exec_stmt(stmt, env)?;
        }
        match &block.tail {
            Some(tail) => self.eval(tail, env),
            None => Ok(Value::Void),
        }
    }

    fn exec_stmt(&mut self, stmt: &Stmt, env: &mut HashMap<String, Binding>) -> EvalResult<()> {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                ty,
                init,
                span,
            } => {
                let mut value = self.eval(init, env)?;
                // Narrow to the declared integer width.
                if let (Some(declared), Value::Int(v)) = (ty, &value) {
                    if let Some((bits, signed)) = int_info_for_ty(*declared) {
                        if let Some(narrowed) = narrow_to_width(*v, bits, signed) {
                            value = Value::Int(narrowed);
                        } else {
                            self.error(
                                *span,
                                format!("literal out of range for `{}`", ty_name(*declared)),
                            );
                            return Err(EvalError::Runtime);
                        }
                    }
                }
                env.insert(
                    name.clone(),
                    Binding {
                        value,
                        mutable: *mutable,
                    },
                );
                Ok(())
            }
            Stmt::Expr(e) => {
                self.eval(e, env)?;
                Ok(())
            }
        }
    }

    fn eval(&mut self, e: &Expr, env: &mut HashMap<String, Binding>) -> EvalResult<Value> {
        match e {
            Expr::Int(v, _) => Ok(Value::Int(*v)),
            Expr::Float(v, _) => Ok(Value::Float(*v)),
            Expr::Bool(v, _) => Ok(Value::Bool(*v)),
            Expr::Str(s, _) => Ok(Value::Str(s.clone())),
            Expr::Unit(_) => Ok(Value::Void),
            Expr::Path(name, span) => match env.get(name) {
                Some(b) => Ok(b.value.clone()),
                None => {
                    self.error(*span, format!("cannot find value `{name}` in this scope"));
                    Err(EvalError::Runtime)
                }
            },
            Expr::Binary { op, lhs, rhs, span } => {
                // Short-circuit `&&` / `||`.
                if *op == BinOp::And {
                    let l = self.eval(lhs, env)?;
                    if !bool_value(&l) {
                        return Ok(Value::Bool(false));
                    }
                    let r = self.eval(rhs, env)?;
                    return Ok(Value::Bool(bool_value(&r)));
                }
                if *op == BinOp::Or {
                    let l = self.eval(lhs, env)?;
                    if bool_value(&l) {
                        return Ok(Value::Bool(true));
                    }
                    let r = self.eval(rhs, env)?;
                    return Ok(Value::Bool(bool_value(&r)));
                }
                let l = self.eval(lhs, env)?;
                let r = self.eval(rhs, env)?;
                self.eval_binary(*op, l, r, *span)
            }
            Expr::Unary { op, operand, span } => {
                let v = self.eval(operand, env)?;
                match op {
                    UnOp::Neg => match v {
                        Value::Int(i) => i.checked_neg().map(Value::Int).ok_or_else(|| {
                            self.error(*span, "integer overflow in negation");
                            EvalError::Runtime
                        }),
                        Value::Float(f) => Ok(Value::Float(-f)),
                        other => {
                            self.error(
                                *span,
                                format!(
                                    "cannot negate a value of type {}",
                                    other.type_name()
                                ),
                            );
                            Err(EvalError::Runtime)
                        }
                    },
                    UnOp::Not => match v {
                        Value::Bool(b) => Ok(Value::Bool(!b)),
                        other => {
                            self.error(
                                *span,
                                format!(
                                    "cannot apply `!` to a value of type {}",
                                    other.type_name()
                                ),
                            );
                            Err(EvalError::Runtime)
                        }
                    },
                }
            }
            Expr::Call { callee, args, span } => {
                if callee == "println!" || callee == "print!" {
                    return self.eval_print(callee == "println!", args, env, *span);
                }
                // User function call.
                let Some(f) = self.fns.get(callee).cloned() else {
                    self.error(
                        *span,
                        format!("cannot find function `{callee}` in this scope"),
                    );
                    return Err(EvalError::Runtime);
                };
                if f.params.len() != args.len() {
                    self.error(
                        *span,
                        format!(
                            "this function takes {} arguments but {} were supplied",
                            f.params.len(),
                            args.len()
                        ),
                    );
                    return Err(EvalError::Runtime);
                }
                // Evaluate args in the caller's env first (Rust semantics:
                // all args are evaluated before the call).
                let mut arg_values = Vec::with_capacity(args.len());
                for a in args {
                    arg_values.push(self.eval(a, env)?);
                }
                if self.call_depth >= MAX_CALL_DEPTH {
                    self.error(*span, "stack overflow (call depth exceeded)");
                    return Err(EvalError::Runtime);
                }
                self.call_depth += 1;
                let result = self.invoke(&f, arg_values, span);
                self.call_depth -= 1;
                result
            }
            Expr::If {
                cond,
                then,
                els,
                span,
            } => {
                let c = self.eval(cond, env)?;
                if !matches!(c, Value::Bool(_)) {
                    self.error(
                        *span,
                        format!(
                            "expected `bool` as if condition, found {}",
                            c.type_name()
                        ),
                    );
                    return Err(EvalError::Runtime);
                }
                if c.truthy() {
                    self.exec_block(then, env)
                } else if let Some(else_expr) = els {
                    self.eval(else_expr, env)
                } else {
                    Ok(Value::Void)
                }
            }
            Expr::Loop { body, span } => {
                let mut iterations = 0u64;
                loop {
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        self.error(
                            *span,
                            "loop exceeded iteration limit (no break in supported subset)",
                        );
                        return Err(EvalError::Runtime);
                    }
                    self.exec_block(body, env)?;
                }
            }
            Expr::While { cond, body, span } => {
                let mut iterations = 0u64;
                loop {
                    let c = self.eval(cond, env)?;
                    if !matches!(c, Value::Bool(_)) {
                        self.error(
                            *span,
                            format!(
                                "expected `bool` as while condition, found {}",
                                c.type_name()
                            ),
                        );
                        return Err(EvalError::Runtime);
                    }
                    if !c.truthy() {
                        return Ok(Value::Void);
                    }
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        self.error(
                            *span,
                            "loop exceeded iteration limit (no break in supported subset)",
                        );
                        return Err(EvalError::Runtime);
                    }
                    self.exec_block(body, env)?;
                }
            }
            Expr::Return(value, _) => {
                let v = match value {
                    Some(v) => self.eval(v, env)?,
                    None => Value::Void,
                };
                Err(EvalError::Return(v))
            }
            Expr::Block(b) => self.exec_block(b, env),
            Expr::Assign {
                name,
                op,
                value,
                span,
            } => {
                let rhs = self.eval(value, env)?;
                let Some(binding) = env.get_mut(name) else {
                    self.error(*span, format!("cannot find value `{name}` in this scope"));
                    return Err(EvalError::Runtime);
                };
                if !binding.mutable {
                    self.error(
                        *span,
                        format!("cannot assign to immutable variable `{name}`"),
                    );
                    return Err(EvalError::Runtime);
                }
                if let Some(op) = op {
                    let combined =
                        self.eval_binary(*op, binding.value.clone(), rhs, *span)?;
                    binding.value = combined;
                } else {
                    binding.value = rhs;
                }
                Ok(binding.value.clone())
            }
            Expr::Cast {
                operand,
                target,
                span,
            } => {
                let v = self.eval(operand, env)?;
                self.apply_cast(v, *target, *span)
            }
        }
    }

    /// Evaluate a binary op on concrete values.
    fn eval_binary(
        &mut self,
        op: BinOp,
        l: Value,
        r: Value,
        span: TextRange,
    ) -> EvalResult<Value> {
        use BinOp::*;
        match op {
            Add | Sub | Mul | Div | Rem => match (&l, &r) {
                (Value::Int(a), Value::Int(b)) => {
                    let result = match op {
                        Add => a.checked_add(*b),
                        Sub => a.checked_sub(*b),
                        Mul => a.checked_mul(*b),
                        Div => {
                            if *b == 0 {
                                self.error(span, "attempt to divide by zero");
                                return Err(EvalError::Runtime);
                            }
                            a.checked_div(*b)
                        }
                        Rem => {
                            if *b == 0 {
                                self.error(
                                    span,
                                    "attempt to calculate the remainder with a divisor of zero",
                                );
                                return Err(EvalError::Runtime);
                            }
                            a.checked_rem(*b)
                        }
                        _ => unreachable!(),
                    };
                    result.map(Value::Int).ok_or_else(|| {
                        self.error(span, "integer overflow");
                        EvalError::Runtime
                    })
                }
                (Value::Float(a), Value::Float(b)) => {
                    let result = match op {
                        Add => a + b,
                        Sub => a - b,
                        Mul => a * b,
                        Div => {
                            if *b == 0.0 {
                                self.error(span, "attempt to divide by zero");
                                return Err(EvalError::Runtime);
                            }
                            a / b
                        }
                        Rem => a % b,
                        _ => unreachable!(),
                    };
                    Ok(Value::Float(result))
                }
                // Mixed int/float: the mini interpreter promotes for convenience.
                (Value::Int(a), Value::Float(b)) => self
                    .eval_binary(op, Value::Float(*a as f64), Value::Float(*b), span),
                (Value::Float(a), Value::Int(b)) => self
                    .eval_binary(op, Value::Float(*a), Value::Float(*b as f64), span),
                _ => {
                    self.error(
                        span,
                        format!(
                            "arithmetic requires numeric operands, found {} and {}",
                            l.type_name(),
                            r.type_name()
                        ),
                    );
                    Err(EvalError::Runtime)
                }
            },
            Eq | NotEq => {
                let eq = match (&l, &r) {
                    (Value::Int(a), Value::Int(b)) => a == b,
                    (Value::Float(a), Value::Float(b)) => a == b,
                    (Value::Bool(a), Value::Bool(b)) => a == b,
                    (Value::Str(a), Value::Str(b)) => a == b,
                    (Value::Void, Value::Void) => true,
                    _ => {
                        self.error(
                            span,
                            format!(
                                "cannot compare {} with {}",
                                l.type_name(),
                                r.type_name()
                            ),
                        );
                        return Err(EvalError::Runtime);
                    }
                };
                Ok(Value::Bool(if op == Eq { eq } else { !eq }))
            }
            Lt | Le | Gt | Ge => {
                let ord = match (&l, &r) {
                    (Value::Int(a), Value::Int(b)) => a.cmp(b),
                    (Value::Float(a), Value::Float(b)) => {
                        a.partial_cmp(b).ok_or_else(|| {
                            self.error(span, "cannot order NaN");
                            EvalError::Runtime
                        })?
                    }
                    (Value::Int(a), Value::Float(b)) => {
                        (*a as f64).partial_cmp(b).ok_or_else(|| {
                            self.error(span, "cannot order NaN");
                            EvalError::Runtime
                        })?
                    }
                    (Value::Float(a), Value::Int(b)) => {
                        a.partial_cmp(&(*b as f64)).ok_or_else(|| {
                            self.error(span, "cannot order NaN");
                            EvalError::Runtime
                        })?
                    }
                    _ => {
                        self.error(
                            span,
                            format!(
                                "ordering requires numeric operands, found {} and {}",
                                l.type_name(),
                                r.type_name()
                            ),
                        );
                        return Err(EvalError::Runtime);
                    }
                };
                use std::cmp::Ordering;
                let result = match op {
                    Lt => ord == Ordering::Less,
                    Le => ord != Ordering::Greater,
                    Gt => ord == Ordering::Greater,
                    Ge => ord != Ordering::Less,
                    _ => unreachable!(),
                };
                Ok(Value::Bool(result))
            }
            And | Or => {
                // Short-circuit handled by the caller; both values are bools.
                Ok(Value::Bool(bool_value(&l) && bool_value(&r)))
            }
        }
    }

    /// `println!("...", args...)` / `print!(...)` — format string with `{}`
    /// placeholders.
    fn eval_print(
        &mut self,
        newline: bool,
        args: &[Expr],
        env: &mut HashMap<String, Binding>,
        span: TextRange,
    ) -> EvalResult<Value> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(self.eval(a, env)?);
        }
        // First argument must be a format string literal.
        let fmt = match args.first() {
            Some(Expr::Str(s, _)) => s.clone(),
            Some(_) => {
                self.error(
                    span,
                    "first argument to println! must be a string literal",
                );
                return Err(EvalError::Runtime);
            }
            None => String::new(),
        };
        let rendered = format_with(&fmt, &values[1..], |name| {
            env.get(name).map(|b| b.value.clone())
        })
        .map_err(|msg| {
            self.error(span, msg);
            EvalError::Runtime
        })?;
        let mut output: Vec<u8> = rendered.into_bytes();
        if newline {
            output.push(b'\n');
        }
        match &mut self.out {
            Some(w) => {
                let _ = w.write_all(&output);
            }
            None => {
                let _ = std::io::stdout().write_all(&output);
            }
        }
        Ok(Value::Void)
    }

    /// Invoke a function with evaluated arguments. Catches `return`.
    fn invoke(
        &mut self,
        f: &FnDef,
        args: Vec<Value>,
        _call_span: &TextRange,
    ) -> EvalResult<Value> {
        let mut env = HashMap::new();
        for (param, value) in f.params.iter().zip(args) {
            let mut value = value;
            // Narrow args to declared widths.
            if let Some(declared) = param.ty {
                if let (Value::Int(v), Some((bits, signed))) =
                    (value.clone(), int_info_for_ty(declared))
                {
                    if let Some(n) = narrow_to_width(v, bits, signed) {
                        value = Value::Int(n);
                    }
                }
            }
            env.insert(
                param.name.clone(),
                Binding {
                    value,
                    mutable: param.mutable,
                },
            );
        }
        match self.exec_block(&f.body, &mut env) {
            Ok(v) => Ok(v),
            Err(EvalError::Return(v)) => Ok(v),
            Err(EvalError::Runtime) => Err(EvalError::Runtime),
        }
    }

    fn apply_cast(
        &mut self,
        v: Value,
        target: crate::value::Ty,
        span: TextRange,
    ) -> EvalResult<Value> {
        use crate::value::Ty;
        match (v, target) {
            (Value::Int(i), Ty::Int) => Ok(Value::Int(i)),
            (Value::Int(i), Ty::Float) => Ok(Value::Float(i as f64)),
            (Value::Float(f), Ty::Float) => Ok(Value::Float(f)),
            (Value::Float(f), Ty::Int) => {
                if f.is_finite() {
                    Ok(Value::Int(f as i128))
                } else {
                    self.error(span, "cannot cast non-finite float to integer");
                    Err(EvalError::Runtime)
                }
            }
            (other, target) => {
                self.error(
                    span,
                    format!("cannot cast {} to {}", other.type_name(), target.name()),
                );
                Err(EvalError::Runtime)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bool_value(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}

/// Map a `Ty::Int` to its width info (the mini system stores all ints as
/// i128; narrowing happens on declaration/assignment).
fn int_info_for_ty(ty: crate::value::Ty) -> Option<(u32, bool)> {
    match ty {
        crate::value::Ty::Int => Some((128, true)),
        _ => None,
    }
}

fn narrow_to_width(v: i128, bits: u32, signed: bool) -> Option<i128> {
    // 128-bit is the full i128 range (the mini type system stores all
    // integers as i128); avoid the shift that would overflow at bits = 128.
    if bits >= 128 {
        return Some(v);
    }
    if signed {
        let min = -(1i128 << (bits - 1));
        let max = (1i128 << (bits - 1)) - 1;
        (min..=max).contains(&v).then_some(v)
    } else {
        let max = (1i128 << bits) - 1;
        (0..=max).contains(&v).then_some(v)
    }
}

fn ty_name(ty: crate::value::Ty) -> &'static str {
    ty.name()
}

/// Simple `{}` / `{name}` / `{:?}` format-string renderer.
///
/// Positional `{}` and `{:?}` consume `values` in order; `{name}` (Rust 2021
/// inline-capture style) looks up `name` through `lookup`.
fn format_with(
    fmt: &str,
    values: &[Value],
    lookup: impl Fn(&str) -> Option<Value>,
) -> Result<String, String> {
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    let mut arg_idx = 0usize;
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                out.push('{');
                continue;
            }
            let mut spec = String::new();
            while let Some(&n) = chars.peek() {
                if n == '}' {
                    break;
                }
                spec.push(n);
                chars.next();
            }
            if chars.next() != Some('}') {
                return Err("unterminated `{` in format string".to_string());
            }
            if spec.is_empty() {
                let v = values
                    .get(arg_idx)
                    .ok_or_else(|| "not enough arguments in format string".to_string())?;
                out.push_str(&v.to_string());
                arg_idx += 1;
            } else if spec == ":?" {
                let v = values
                    .get(arg_idx)
                    .ok_or_else(|| "not enough arguments in format string".to_string())?;
                match v {
                    Value::Str(s) => {
                        out.push('"');
                        out.push_str(s);
                        out.push('"');
                    }
                    other => out.push_str(&other.to_string()),
                }
                arg_idx += 1;
            } else if let Some(v) = spec.strip_prefix(':') {
                // `{:?}` handled above; other format specifiers unsupported.
                return Err(format!(
                    "invalid format specifier `{{{spec}}}` (only `{{}}`, `{{:?}}`, and `{{name}}` are supported)"
                ));
            } else {
                // `{name}` — inline capture from the environment.
                let v = lookup(&spec).ok_or_else(|| {
                    format!("cannot capture `{spec}`: no such variable in scope")
                })?;
                out.push_str(&v.to_string());
            }
        } else if c == '}' {
            if chars.peek() == Some(&'}') {
                chars.next();
                out.push('}');
            } else {
                return Err("unmatched `}` in format string".to_string());
            }
        } else {
            out.push(c);
        }
    }
    if arg_idx < values.len() {
        return Err(format!(
            "{} arguments were supplied but only {} were used",
            values.len(),
            arg_idx
        ));
    }
    Ok(out)
}

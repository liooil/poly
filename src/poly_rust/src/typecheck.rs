//! Structural type checker for the mini Rust subset.
//!
//! This is a deliberately simple, one-pass checker over the HIR. It
//! distinguishes *type errors* (reported as `DiagKind::Type`) from
//! *unsupported features* (reported during lowering) and *runtime errors*
//! (reported by the interpreter). It does not attempt full Rust inference:
//! untyped variables are treated as `Unknown` and checked dynamically.

use std::collections::HashMap;

use text_size::TextRange;

use crate::diagnostics::Diagnostic;
use crate::hir::{BinOp, Block, Expr, FnDef, Item, Program, Stmt, UnOp};
use crate::value::{Ty, int_type_info};

/// A binding's static type, tracking mutability so assignments can be
/// rejected.
#[derive(Debug, Clone, Copy)]
struct Binding {
    ty: Ty,
    mutable: bool,
}

#[derive(Debug, Default)]
pub struct TypeChecker {
    /// Functions by name -> return type. Used to type call expressions.
    fns: HashMap<String, Ty>,
    diagnostics: Vec<Diagnostic>,
}

impl TypeChecker {
    pub fn check(program: &Program) -> Vec<Diagnostic> {
        let mut checker = TypeChecker {
            fns: HashMap::new(),
            diagnostics: Vec::new(),
        };
        for item in &program.items {
            match item {
                Item::Fn(f) => {
                    checker
                        .fns
                        .insert(f.name.clone(), f.ret_ty.unwrap_or(Ty::Void));
                }
            }
        }
        for item in &program.items {
            match item {
                Item::Fn(f) => checker.check_fn(f),
            }
        }
        checker.diagnostics
    }

    fn check_fn(&mut self, f: &FnDef) {
        let mut env = HashMap::new();
        for p in &f.params {
            let ty = p.ty.unwrap_or(Ty::Unknown);
            env.insert(p.name.clone(), Binding { ty, mutable: false });
        }
        self.check_block(&f.body, &mut env);
        // Return type check: if the fn declares a return type, the tail expr
        // (if any) must be compatible. `return` expressions are checked
        // recursively during block checking.
        if let Some(expected) = f.ret_ty {
            if let Some(tail) = &f.body.tail {
                let actual = self.expr_ty(tail, &mut env);
                self.expect_assignable(actual, expected, tail.span(), "return value");
            }
        }
    }

    fn check_block(&mut self, block: &Block, env: &mut HashMap<String, Binding>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let {
                    name,
                    mutable,
                    ty,
                    init,
                    span,
                } => {
                    let init_ty = self.expr_ty(init, env);
                    if let Some(declared) = ty {
                        self.expect_assignable(init_ty, *declared, *span, "initializer");
                    }
                    env.insert(
                        name.clone(),
                        Binding {
                            ty: ty.unwrap_or(init_ty),
                            mutable: *mutable,
                        },
                    );
                }
                Stmt::Expr(e) => {
                    self.expr_ty(e, env);
                }
            }
        }
        if let Some(tail) = &block.tail {
            self.expr_ty(tail, env);
        }
    }

    /// Infer the type of an expression, checking it recursively. Mutates
    /// `env` for assignments (checks mutability, records the value type).
    fn expr_ty(&mut self, e: &Expr, env: &mut HashMap<String, Binding>) -> Ty {
        match e {
            Expr::Int(_, _) => Ty::Int,
            Expr::Float(_, _) => Ty::Float,
            Expr::Bool(_, _) => Ty::Bool,
            Expr::Str(_, _) => Ty::Str,
            Expr::Unit(_) => Ty::Void,
            Expr::Path(name, span) => match env.get(name) {
                Some(b) => b.ty,
                None => {
                    self.error(*span, format!("cannot find value `{name}` in this scope"));
                    Ty::Unknown
                }
            },
            Expr::Binary { op, lhs, rhs, span } => {
                let lt = self.expr_ty(lhs, env);
                let rt = self.expr_ty(rhs, env);
                self.check_binary(*op, lt, rt, *span)
            }
            Expr::Unary { op, operand, span } => {
                let t = self.expr_ty(operand, env);
                match op {
                    UnOp::Neg => {
                        if !matches!(t, Ty::Int | Ty::Float | Ty::Unknown) {
                            self.error(
                                *span,
                                format!(
                                    "cannot negate a value of type {}",
                                    t.name()
                                ),
                            );
                        }
                        t
                    }
                    UnOp::Not => {
                        if !matches!(t, Ty::Bool | Ty::Unknown) {
                            self.error(
                                *span,
                                format!("cannot apply `!` to a value of type {}", t.name()),
                            );
                        }
                        Ty::Bool
                    }
                }
            }
            Expr::Call { callee, args, span } => {
                for a in args {
                    self.expr_ty(a, env);
                }
                if let Some(ret) = self.fns.get(callee) {
                    *ret
                } else if callee == "println!" || callee == "print!" {
                    Ty::Void
                } else {
                    self.error(
                        *span,
                        format!("cannot find function `{callee}` in this scope"),
                    );
                    Ty::Unknown
                }
            }
            Expr::If {
                cond,
                then,
                els,
                span,
            } => {
                let ct = self.expr_ty(cond, env);
                if !matches!(ct, Ty::Bool | Ty::Unknown) {
                    self.error(
                        *span,
                        format!(
                            "expected `bool` as if condition, found {}",
                            ct.name()
                        ),
                    );
                }
                self.check_block(then, env);
                let tt = then
                    .tail
                    .as_ref()
                    .map(|t| self.expr_ty(t, env))
                    .unwrap_or(Ty::Void);
                match els {
                    Some(e) => {
                        let et = self.expr_ty(e, env);
                        if !self.compatible(tt, et) && tt != Ty::Unknown && et != Ty::Unknown {
                            self.error(
                                *span,
                                format!(
                                    "if and else have incompatible types: {} and {}",
                                    tt.name(),
                                    et.name()
                                ),
                            );
                        }
                        tt
                    }
                    None => {
                        if tt != Ty::Void && tt != Ty::Unknown {
                            self.error(
                                *span,
                                format!(
                                    "if without else yields {}, but the value is used",
                                    tt.name()
                                ),
                            );
                        }
                        Ty::Void
                    }
                }
            }
            Expr::Loop { body, span } => {
                self.check_block(body, env);
                // `loop` never yields (it never terminates in the supported
                // subset — no break). Treat as Void.
                let _ = span;
                Ty::Void
            }
            Expr::While { cond, body, span } => {
                let ct = self.expr_ty(cond, env);
                if !matches!(ct, Ty::Bool | Ty::Unknown) {
                    self.error(
                        *span,
                        format!("expected `bool` as while condition, found {}", ct.name()),
                    );
                }
                self.check_block(body, env);
                Ty::Void
            }
            Expr::Return(value, span) => {
                let t = value
                    .as_ref()
                    .map(|v| self.expr_ty(v, env))
                    .unwrap_or(Ty::Void);
                // The function's return type was recorded; but we don't know
                // which fn we're in here. The interpreter validates at
                // runtime; the static check for `return` compatibility is
                // done in check_fn via the tail. Skip per-return checks.
                let _ = (t, span);
                Ty::Void
            }
            Expr::Block(b) => {
                self.check_block(b, env);
                b.tail
                    .as_ref()
                    .map(|t| self.expr_ty(t, env))
                    .unwrap_or(Ty::Void)
            }
            Expr::Assign {
                name,
                op,
                value,
                span,
            } => {
                let vt = self.expr_ty(value, env);
                match env.get(name) {
                    Some(b) => {
                        if !b.mutable {
                            self.error(
                                *span,
                                format!("cannot assign to immutable variable `{name}`"),
                            );
                        }
                        if op.is_some() {
                            if !matches!(b.ty, Ty::Int | Ty::Float | Ty::Unknown)
                                || !matches!(vt, Ty::Int | Ty::Float | Ty::Unknown)
                            {
                                self.error(
                                    *span,
                                    format!(
                                        "compound assignment requires numeric operands, found {} and {}",
                                        b.ty.name(),
                                        vt.name()
                                    ),
                                );
                            }
                        }
                        self.expect_assignable(vt, b.ty, *span, "assignment");
                        b.ty
                    }
                    None => {
                        self.error(
                            *span,
                            format!("cannot find value `{name}` in this scope"),
                        );
                        Ty::Unknown
                    }
                }
            }
            Expr::Cast { operand, target, span } => {
                let ot = self.expr_ty(operand, env);
                let ok = match (ot, target) {
                    (Ty::Int | Ty::Float | Ty::Unknown, Ty::Int | Ty::Float) => true,
                    (_, _) => {
                        self.error(
                            *span,
                            format!(
                                "cannot cast {} to {}",
                                ot.name(),
                                target.name()
                            ),
                        );
                        false
                    }
                };
                if ok {
                    *target
                } else {
                    Ty::Unknown
                }
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, lt: Ty, rt: Ty, span: TextRange) -> Ty {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let numeric = |t: Ty| matches!(t, Ty::Int | Ty::Float | Ty::Unknown);
                if !numeric(lt) || !numeric(rt) {
                    self.error(
                        span,
                        format!(
                            "arithmetic requires numeric operands, found {} and {}",
                            lt.name(),
                            rt.name()
                        ),
                    );
                    Ty::Unknown
                } else if lt == Ty::Float || rt == Ty::Float {
                    Ty::Float
                } else {
                    Ty::Int
                }
            }
            BinOp::Eq | BinOp::NotEq => {
                // Equality across any two compatible types.
                if !self.compatible(lt, rt) && lt != Ty::Unknown && rt != Ty::Unknown {
                    self.error(
                        span,
                        format!(
                            "cannot compare {} with {}",
                            lt.name(),
                            rt.name()
                        ),
                    );
                }
                Ty::Bool
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let numeric = |t: Ty| matches!(t, Ty::Int | Ty::Float | Ty::Unknown);
                if !numeric(lt) || !numeric(rt) {
                    self.error(
                        span,
                        format!(
                            "ordering requires numeric operands, found {} and {}",
                            lt.name(),
                            rt.name()
                        ),
                    );
                }
                Ty::Bool
            }
            BinOp::And | BinOp::Or => {
                if !matches!(lt, Ty::Bool | Ty::Unknown)
                    || !matches!(rt, Ty::Bool | Ty::Unknown)
                {
                    self.error(
                        span,
                        format!(
                            "logical operator requires bool operands, found {} and {}",
                            lt.name(),
                            rt.name()
                        ),
                    );
                }
                Ty::Bool
            }
        }
    }

    /// Check that `actual` can be assigned to a slot of type `expected`.
    fn expect_assignable(&mut self, actual: Ty, expected: Ty, span: TextRange, what: &str) {
        if actual == Ty::Unknown || expected == Ty::Unknown {
            return;
        }
        if !self.compatible(actual, expected) {
            self.error(
                span,
                format!(
                    "mismatched types: {what} has type {}, expected {}",
                    actual.name(),
                    expected.name()
                ),
            );
        }
    }

    /// Are two types mutually assignable under the mini type system?
    fn compatible(&self, a: Ty, b: Ty) -> bool {
        a == b
    }
}

impl TypeChecker {
    fn error(&mut self, range: TextRange, message: String) {
        self.diagnostics.push(Diagnostic::ty(range, message));
    }
}

/// Convenience: parse a type name to `(bits, signed)` — used by the
/// interpreter for narrowing. Kept here so the checker and interpreter share
/// the same type vocabulary.
pub fn int_width(name: &str) -> Option<(u32, bool)> {
    int_type_info(name)
}

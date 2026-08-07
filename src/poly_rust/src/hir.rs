//! HIR: lowering from rust-analyzer's syntax tree to a small, interpreter-friendly
//! intermediate representation.
//!
//! The interpreter never touches the rowan syntax tree: lowering happens once,
//! unsupported features are reported as [`Diagnostic`]s at their source spans,
//! and the resulting [`Program`] is what the interpreter walks. Keeping the
//! HIR tiny is deliberate — growing the supported Rust subset means growing
//! these types, not teaching the interpreter about syntax.

use ra_ap_syntax::ast::{
    self, AstNode, HasArgList, HasGenericArgs, HasGenericParams, HasLoopBody, HasModuleItem,
    HasName,
};
use text_size::TextRange;

use crate::diagnostics::Diagnostic;
use crate::value::Ty;

/// A lowered source file: a list of items plus the diagnostics produced
/// during lowering (unsupported features, structural problems).
#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
    /// Diagnostics produced during lowering (unsupported features). The
    /// interpreter refuses to run if any error-level diagnostic exists.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDef),
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<Param>,
    pub ret_ty: Option<Ty>,
    pub body: Block,
    pub span: TextRange,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Option<Ty>,
    pub mutable: bool,
    pub span: TextRange,
}

/// A statement list plus an optional tail expression (the block's value).
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: TextRange,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let [mut] name [: ty] = expr;`
    Let {
        name: String,
        mutable: bool,
        ty: Option<Ty>,
        init: Expr,
        span: TextRange,
    },
    /// A bare expression statement; the value is discarded.
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer literal. Suffixes (`10i32`) are stripped during lowering.
    Int(i128, TextRange),
    Float(f64, TextRange),
    Bool(bool, TextRange),
    Str(String, TextRange),
    /// Unit `()`.
    Unit(TextRange),
    /// Name reference (variable).
    Path(String, TextRange),
    /// `a op b`
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: TextRange,
    },
    /// `!x`, `-x`
    Unary {
        op: UnOp,
        operand: Box<Expr>,
        span: TextRange,
    },
    /// `name(args...)` — calls to user functions or builtins like `println!`.
    Call {
        callee: String,
        args: Vec<Expr>,
        span: TextRange,
    },
    If {
        cond: Box<Expr>,
        then: Block,
        els: Option<Box<Expr>>,
        span: TextRange,
    },
    /// `loop { ... }` — infinite loop. `break`/`continue` unsupported.
    Loop {
        body: Block,
        span: TextRange,
    },
    While {
        cond: Box<Expr>,
        body: Block,
        span: TextRange,
    },
    /// `return [expr]`
    Return(Option<Box<Expr>>, TextRange),
    /// Block expression `{ stmts; tail }`.
    Block(Block),
    /// `name = expr` (assignment to a mutable variable) and compound
    /// assignments (`+=`, `-=`, ...).
    Assign {
        name: String,
        op: Option<BinOp>,
        value: Box<Expr>,
        span: TextRange,
    },
    /// `expr as Type` — casts between numeric types.
    Cast {
        operand: Box<Expr>,
        target: Ty,
        span: TextRange,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

impl Expr {
    /// The source span of this expression.
    pub fn span(&self) -> TextRange {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Bool(_, s)
            | Expr::Str(_, s)
            | Expr::Unit(s)
            | Expr::Path(_, s)
            | Expr::Unary { span: s, .. }
            | Expr::Call { span: s, .. }
            | Expr::If { span: s, .. }
            | Expr::Loop { span: s, .. }
            | Expr::While { span: s, .. }
            | Expr::Return(_, s)
            | Expr::Assign { span: s, .. }
            | Expr::Cast { span: s, .. } => *s,
            Expr::Binary { span: s, .. } => *s,
            Expr::Block(b) => b.span,
        }
    }
}

/// Lower a parsed source file into a [`Program`]. Unsupported constructs are
/// reported as `Unsupported` diagnostics (with their source span) and not
/// lowered further.
pub fn lower(file: &ast::SourceFile) -> Program {
    let mut ctx = LowerCtx {
        diagnostics: Vec::new(),
    };
    let mut items = Vec::new();
    for item in file.items() {
        match item {
            ast::Item::Fn(f) => items.push(Item::Fn(ctx.lower_fn(&f))),
            ast::Item::MacroCall(m) => {
                let span = m.syntax().text_range();
                let name = m
                    .path()
                    .and_then(|p| p.segment())
                    .and_then(|s| s.name_ref())
                    .map(|n| n.text().to_string())
                    .unwrap_or_else(|| "macro".to_string());
                ctx.diagnostics.push(Diagnostic::unsupported(
                    span,
                    format!("top-level `{name}!`"),
                ));
            }
            other => {
                let span = other.syntax().text_range();
                let feature = match other {
                    ast::Item::Struct(_) => "struct",
                    ast::Item::Enum(_) => "enum",
                    ast::Item::Trait(_) => "trait",
                    ast::Item::Impl(_) => "impl",
                    ast::Item::Use(_) => "use",
                    ast::Item::Module(_) => "mod",
                    ast::Item::Const(_) => "const",
                    ast::Item::Static(_) => "static",
                    ast::Item::TypeAlias(_) => "type alias",
                    ast::Item::Union(_) => "union",
                    ast::Item::ExternBlock(_) => "extern block",
                    ast::Item::ExternCrate(_) => "extern crate",
                    ast::Item::MacroDef(_) => "macro definition",
                    ast::Item::MacroRules(_) => "macro_rules!",
                    ast::Item::AsmExpr(_) => "asm",
                    _ => "item",
                };
                ctx.diagnostics.push(Diagnostic::unsupported(span, feature));
            }
        }
    }
    Program {
        items,
        diagnostics: ctx.diagnostics,
    }
}

struct LowerCtx {
    diagnostics: Vec<Diagnostic>,
}

impl LowerCtx {
    fn unsupported(&mut self, span: TextRange, feature: impl Into<String>) {
        self.diagnostics.push(Diagnostic::unsupported(span, feature));
    }

    fn lower_fn(&mut self, f: &ast::Fn) -> FnDef {
        let span = f.syntax().text_range();
        let name = f.name().map(|n| n.text().to_string()).unwrap_or_default();

        if f.unsafe_token().is_some() {
            self.unsupported(span, "unsafe fn");
        }
        if f.async_token().is_some() {
            self.unsupported(span, "async fn");
        }
        if f.const_token().is_some() {
            self.unsupported(span, "const fn");
        }
        if f.abi().is_some() {
            self.unsupported(span, "extern fn");
        }
        if f.generic_param_list().is_some() {
            self.unsupported(span, "generic parameters");
        }

        let params = match f.param_list() {
            Some(pl) => pl.params().map(|p| self.lower_param(&p)).collect(),
            None => Vec::new(),
        };

        let ret_ty = match f.ret_type() {
            Some(rt) => rt.ty().and_then(|t| self.lower_type(&t)),
            None => None,
        };

        let body = match f.body() {
            Some(b) => self.lower_block(&b),
            None => {
                self.unsupported(span, "function without a body");
                Block {
                    stmts: Vec::new(),
                    tail: None,
                    span,
                }
            }
        };

        FnDef {
            name,
            params,
            ret_ty,
            body,
            span,
        }
    }

    fn lower_param(&mut self, p: &ast::Param) -> Param {
        let span = p.syntax().text_range();
        let (name, mutable) = match p.pat() {
            Some(ast::Pat::IdentPat(id)) => (
                id.name()
                    .map(|n| n.text().to_string())
                    .unwrap_or_default(),
                id.mut_token().is_some(),
            ),
            _ => {
                self.unsupported(span, "pattern parameters");
                (String::new(), false)
            }
        };
        let ty = p.ty().and_then(|t| self.lower_type(&t));
        Param {
            name,
            ty,
            mutable,
            span,
        }
    }

    /// Lower a type reference. Returns `None` for unsupported types (after
    /// reporting a diagnostic) — the parameter/let is treated as untyped.
    fn lower_type(&mut self, t: &ast::Type) -> Option<Ty> {
        match t {
            ast::Type::PathType(p) => {
                let name = p
                    .path()
                    .and_then(|path| path.segment())
                    .and_then(|s| s.name_ref())
                    .map(|n| n.text().to_string());
                match name.as_deref() {
                    Some(
                        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16"
                        | "u32" | "u64" | "u128" | "usize",
                    ) => Some(Ty::Int),
                    Some("f32" | "f64") => Some(Ty::Float),
                    Some("bool") => Some(Ty::Bool),
                    Some("String") => Some(Ty::Str),
                    Some(other) => {
                        self.unsupported(t.syntax().text_range(), format!("type `{other}`"));
                        None
                    }
                    None => {
                        self.unsupported(t.syntax().text_range(), "path type");
                        None
                    }
                }
            }
            ast::Type::RefType(r) => {
                // `&str` is the string form we support; `&String` too.
                match r.ty() {
                    Some(ast::Type::PathType(p)) => {
                        let name = p
                            .path()
                            .and_then(|path| path.segment())
                            .and_then(|s| s.name_ref())
                            .map(|n| n.text().to_string());
                        match name.as_deref() {
                            Some("str" | "String") => Some(Ty::Str),
                            _ => {
                                self.unsupported(t.syntax().text_range(), "reference type");
                                None
                            }
                        }
                    }
                    _ => {
                        self.unsupported(t.syntax().text_range(), "reference type");
                        None
                    }
                }
            }
            ast::Type::InferType(_) => None, // `_` — leave untyped
            ast::Type::TupleType(tup) => {
                let is_unit = tup.fields().next().is_none();
                if is_unit {
                    Some(Ty::Void)
                } else {
                    self.unsupported(t.syntax().text_range(), "tuple type");
                    None
                }
            }
            other => {
                let feature = match other {
                    ast::Type::ArrayType(_) => "array type",
                    ast::Type::SliceType(_) => "slice type",
                    ast::Type::DynTraitType(_) => "dyn trait type",
                    ast::Type::ImplTraitType(_) => "impl trait type",
                    ast::Type::FnPtrType(_) => "function pointer type",
                    ast::Type::NeverType(_) => "never type",
                    ast::Type::ParenType(_) => "parenthesized type",
                    ast::Type::MacroType(_) => "macro type",
                    ast::Type::ForType(_) => "higher-ranked type",
                    ast::Type::PatternType(_) => "pattern type",
                    _ => "type",
                };
                self.unsupported(t.syntax().text_range(), feature);
                None
            }
        }
    }

    fn lower_block(&mut self, b: &ast::BlockExpr) -> Block {
        let span = b.syntax().text_range();
        let mut stmts = Vec::new();
        // The tail expression (`{ stmt; tail }`) is a direct child of the
        // STMT_LIST node, but is *not* an `ast::Stmt` (no EXPR_STMT wrapper,
        // no semicolon). Walk the STMT_LIST's children; the last one that
        // casts to an Expr but not a Stmt is the tail.
        let mut tail = None;
        if let Some(list) = b.stmt_list() {
            for child in list.syntax().children() {
                if let Some(stmt) = ast::Stmt::cast(child.clone()) {
                    self.lower_stmt(&stmt, &mut stmts);
                } else if let Some(expr) = ast::Expr::cast(child) {
                    tail = self.lower_expr(&expr).map(Box::new);
                }
            }
        }
        Block { stmts, tail, span }
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt, out: &mut Vec<Stmt>) {
        match stmt {
            ast::Stmt::LetStmt(l) => out.push(self.lower_let(l)),
            ast::Stmt::ExprStmt(e) => {
                if let Some(expr) = e.expr() {
                    // Macro calls in statement position (`println!(...)`)
                    // are handled by `lower_expr`'s `MacroExpr` arm.
                    if let Some(lowered) = self.lower_expr(&expr) {
                        out.push(Stmt::Expr(lowered));
                    }
                }
            }
            ast::Stmt::Item(item) => {
                let span = item.syntax().text_range();
                self.unsupported(span, "nested item");
            }
        }
    }

    fn lower_let(&mut self, l: &ast::LetStmt) -> Stmt {
        let span = l.syntax().text_range();
        let (name, mutable) = match l.pat() {
            Some(ast::Pat::IdentPat(id)) => (
                id.name()
                    .map(|n| n.text().to_string())
                    .unwrap_or_default(),
                id.mut_token().is_some(),
            ),
            _ => {
                self.unsupported(span, "pattern in let");
                (String::new(), false)
            }
        };
        let ty = l.ty().and_then(|t| self.lower_type(&t));
        let init = match l.initializer() {
            Some(e) => self.lower_expr(&e).unwrap_or(Expr::Unit(span)),
            None => {
                self.unsupported(span, "let without initializer");
                Expr::Unit(span)
            }
        };
        Stmt::Let {
            name,
            mutable,
            ty,
            init,
            span,
        }
    }

    fn lower_expr(&mut self, e: &ast::Expr) -> Option<Expr> {
        let span = e.syntax().text_range();
        Some(match e {
            ast::Expr::Literal(lit) => self.lower_literal(&lit, span)?,
            ast::Expr::PathExpr(p) => {
                let path = p.path()?;
                let mut segs = path.segments();
                let seg = segs.next()?;
                if segs.next().is_some() {
                    self.unsupported(span, "multi-segment path");
                    return None;
                }
                if seg.generic_arg_list().is_some() {
                    self.unsupported(span, "generic arguments");
                    return None;
                }
                Expr::Path(seg.name_ref()?.text().to_string(), span)
            }
            ast::Expr::TupleExpr(t) => {
                let mut fields = t.fields();
                if fields.next().is_none() {
                    Expr::Unit(span)
                } else {
                    self.unsupported(span, "tuple expression");
                    return None;
                }
            }
            ast::Expr::ParenExpr(p) => {
                let inner = p.expr()?;
                self.lower_expr(&inner)?
            }
            ast::Expr::BinExpr(b) => {
                // Assignment and compound-assignment are BinExpr nodes with
                // `BinaryOp::Assignment`.
                use ra_ap_syntax::ast::BinaryOp as RAOp;
                let op = b.op_kind()?;
                if let RAOp::Assignment { op: assign_op } = op {
                    let name = match b.lhs() {
                        Some(ast::Expr::PathExpr(p)) => {
                            let path = p.path()?;
                            let mut segs = path.segments();
                            let seg = segs.next()?;
                            if segs.next().is_some() {
                                self.unsupported(span, "multi-segment assignment target");
                                return None;
                            }
                            seg.name_ref()?.text().to_string()
                        }
                        _ => {
                            self.unsupported(span, "assignment target");
                            return None;
                        }
                    };
                    let value = self.lower_expr(&b.rhs()?)?;
                    let compound = assign_op.and_then(|ao| {
                        use ra_ap_syntax::ast::ArithOp;
                        match ao {
                            ArithOp::Add => Some(BinOp::Add),
                            ArithOp::Sub => Some(BinOp::Sub),
                            ArithOp::Mul => Some(BinOp::Mul),
                            ArithOp::Div => Some(BinOp::Div),
                            ArithOp::Rem => Some(BinOp::Rem),
                            _ => None,
                        }
                    });
                    if assign_op.is_some() && compound.is_none() {
                        self.unsupported(span, "compound assignment");
                        return None;
                    }
                    Expr::Assign {
                        name,
                        op: compound,
                        value: Box::new(value),
                        span,
                    }
                } else {
                    let lhs = self.lower_expr(&b.lhs()?)?;
                    let rhs = self.lower_expr(&b.rhs()?)?;
                    let op = self.lower_binop(op)?;
                    Expr::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        span,
                    }
                }
            }
            ast::Expr::PrefixExpr(p) => {
                let op = match p.op_kind()? {
                    ra_ap_syntax::ast::UnaryOp::Neg => UnOp::Neg,
                    ra_ap_syntax::ast::UnaryOp::Not => UnOp::Not,
                    ra_ap_syntax::ast::UnaryOp::Deref => {
                        self.unsupported(span, "dereference");
                        return None;
                    }
                };
                let operand = self.lower_expr(&p.expr()?)?;
                Expr::Unary {
                    op,
                    operand: Box::new(operand),
                    span,
                }
            }
            ast::Expr::CallExpr(c) => {
                // `name(args)` — callee must be a plain path. Method calls
                // and field calls are unsupported. (Macro calls in call
                // position are handled by the `MacroExpr` arm above.)
                let callee = match c.expr() {
                    Some(ast::Expr::PathExpr(p)) => {
                        let path = p.path()?;
                        let mut segs = path.segments();
                        let seg = segs.next()?;
                        if segs.next().is_some() {
                            self.unsupported(span, "multi-segment callee");
                            return None;
                        }
                        seg.name_ref()?.text().to_string()
                    }
                    _ => {
                        self.unsupported(span, "method call or field call");
                        return None;
                    }
                };
                let args = c
                    .arg_list()
                    .map(|al| al.args().filter_map(|a| self.lower_expr(&a)).collect())
                    .unwrap_or_default();
                Expr::Call { callee, args, span }
            }
            ast::Expr::IfExpr(if_expr) => {
                let cond = self.lower_expr(&if_expr.condition()?)?;
                let then = self.lower_block(&if_expr.then_branch()?);
                let els = match if_expr.else_branch() {
                    Some(ast::ElseBranch::Block(b)) => {
                        Some(Box::new(Expr::Block(self.lower_block(&b))))
                    }
                    Some(ast::ElseBranch::IfExpr(inner)) => {
                        let inner_expr = self.lower_expr(&ast::Expr::IfExpr(inner))?;
                        Some(Box::new(inner_expr))
                    }
                    None => None,
                };
                Expr::If {
                    cond: Box::new(cond),
                    then,
                    els,
                    span,
                }
            }
            ast::Expr::BlockExpr(b) => Expr::Block(self.lower_block(&b)),
            ast::Expr::LoopExpr(l) => {
                if l.label().is_some() {
                    self.unsupported(span, "loop label");
                    return None;
                }
                let body = l.loop_body()?;
                Expr::Loop {
                    body: self.lower_block(&body),
                    span,
                }
            }
            ast::Expr::WhileExpr(w) => {
                if w.label().is_some() {
                    self.unsupported(span, "loop label");
                    return None;
                }
                let cond = self.lower_expr(&w.condition()?)?;
                let body = w.loop_body()?;
                Expr::While {
                    cond: Box::new(cond),
                    body: self.lower_block(&body),
                    span,
                }
            }
            ast::Expr::ReturnExpr(r) => {
                let value = match r.expr() {
                    Some(e) => Some(Box::new(self.lower_expr(&e)?)),
                    None => None,
                };
                Expr::Return(value, span)
            }
            ast::Expr::CastExpr(c) => {
                let operand = self.lower_expr(&c.expr()?)?;
                let target = match c.ty() {
                    Some(t) => self.lower_type(&t)?,
                    None => return None,
                };
                Expr::Cast {
                    operand: Box::new(operand),
                    target,
                    span,
                }
            }
            ast::Expr::MacroExpr(m) => {
                // A macro call in expression position: `println!(...)` /
                // `print!(...)` are lowered to a Call; anything else is
                // unsupported.
                let call = m.macro_call()?;
                let path = call.path()?;
                let mut segs = path.segments();
                let seg = segs.next()?;
                if segs.next().is_some() {
                    self.unsupported(span, "multi-segment macro path");
                    return None;
                }
                let name = format!("{}!", seg.name_ref()?.text());
                if name != "println!" && name != "print!" {
                    self.unsupported(span, format!("macro `{name}`"));
                    return None;
                }
                let args = call
                    .token_tree()
                    .map(|tt| {
                        // Extract the comma-separated expressions inside the
                        // token tree. Simplest correct approach: parse the
                        // token tree text as a parenthesized expression list
                        // via ra_ap_syntax's expression parser. We use the
                        // raw text between parens and split on top-level
                        // commas.
                        split_tt_args(&tt, self)
                    })
                    .unwrap_or_default();
                Expr::Call { callee: name, args, span }
            }
            other => {
                let feature: String = match other {
                    ast::Expr::ClosureExpr(_) => "closure".to_string(),
                    ast::Expr::MatchExpr(_) => "match".to_string(),
                    ast::Expr::ForExpr(_) => "for loop".to_string(),
                    ast::Expr::BreakExpr(_) => "break".to_string(),
                    ast::Expr::ContinueExpr(_) => "continue".to_string(),
                    ast::Expr::ArrayExpr(_) => "array expression".to_string(),
                    ast::Expr::IndexExpr(_) => "index expression".to_string(),
                    ast::Expr::FieldExpr(_) => "field access".to_string(),
                    ast::Expr::MethodCallExpr(_) => "method call".to_string(),
                    ast::Expr::RefExpr(_) => "reference expression".to_string(),
                    ast::Expr::RangeExpr(_) => "range expression".to_string(),
                    ast::Expr::TryExpr(_) => "try expression".to_string(),
                    ast::Expr::AwaitExpr(_) => "await expression".to_string(),
                    ast::Expr::YieldExpr(_) => "yield expression".to_string(),
                    ast::Expr::YeetExpr(_) => "yeet expression".to_string(),
                    ast::Expr::FormatArgsExpr(_) => "format string expression".to_string(),
                    ast::Expr::RecordExpr(_) => "struct expression".to_string(),
                    ast::Expr::MacroExpr(m) => {
                        let name = m
                            .macro_call()
                            .and_then(|c| c.path())
                            .and_then(|p| p.segment())
                            .and_then(|s| s.name_ref())
                            .map(|n| n.text().to_string())
                            .unwrap_or_else(|| "macro".to_string());
                        format!("macro `{name}!`")
                    }
                    ast::Expr::UnderscoreExpr(_) => "underscore expression".to_string(),
                    ast::Expr::LetExpr(_) => "let expression".to_string(),
                    ast::Expr::IncludeBytesExpr(_) => "include_bytes!".to_string(),
                    ast::Expr::OffsetOfExpr(_) => "offset_of!".to_string(),
                    ast::Expr::AsmExpr(_) => "asm".to_string(),
                    ast::Expr::BecomeExpr(_) => "become".to_string(),
                    _ => "expression".to_string(),
                };
                self.unsupported(span, feature);
                return None;
            }
        })
    }

    fn lower_literal(&mut self, lit: &ast::Literal, span: TextRange) -> Option<Expr> {
        use ra_ap_syntax::SyntaxKind;
        // The LITERAL node's kind is just `LITERAL`; the actual literal kind
        // is the first token inside it (INT_NUMBER, STRING, TRUE_KW, ...).
        let kind = lit
            .syntax()
            .first_token()
            .map(|t| t.kind())
            .unwrap_or(SyntaxKind::ERROR);
        let text = lit.syntax().text().to_string();
        match kind {
            SyntaxKind::INT_NUMBER => {
                let parsed = parse_int_literal(&strip_int_suffix(&text));
                match parsed {
                    Some(v) => Some(Expr::Int(v, span)),
                    None => {
                        self.unsupported(span, format!("integer literal `{text}`"));
                        None
                    }
                }
            }
            SyntaxKind::FLOAT_NUMBER => {
                match strip_float_suffix(&text).parse::<f64>() {
                    Ok(v) if v.is_finite() => Some(Expr::Float(v, span)),
                    _ => {
                        self.unsupported(span, format!("float literal `{text}`"));
                        None
                    }
                }
            }
            SyntaxKind::STRING => Some(Expr::Str(unescape_string(&text), span)),
            SyntaxKind::TRUE_KW => Some(Expr::Bool(true, span)),
            SyntaxKind::FALSE_KW => Some(Expr::Bool(false, span)),
            SyntaxKind::CHAR => {
                self.unsupported(span, "char literal");
                None
            }
            SyntaxKind::BYTE_STRING | SyntaxKind::C_STRING => {
                self.unsupported(span, "byte/raw string literal");
                None
            }
            _ => {
                self.unsupported(span, "literal");
                None
            }
        }
    }

    fn lower_binop(&mut self, op: ra_ap_syntax::ast::BinaryOp) -> Option<BinOp> {
        use ra_ap_syntax::ast::{ArithOp, BinaryOp as RAOp, CmpOp, LogicOp, Ordering};
        match op {
            RAOp::ArithOp(ArithOp::Add) => Some(BinOp::Add),
            RAOp::ArithOp(ArithOp::Sub) => Some(BinOp::Sub),
            RAOp::ArithOp(ArithOp::Mul) => Some(BinOp::Mul),
            RAOp::ArithOp(ArithOp::Div) => Some(BinOp::Div),
            RAOp::ArithOp(ArithOp::Rem) => Some(BinOp::Rem),
            RAOp::ArithOp(
                ArithOp::Shl | ArithOp::Shr | ArithOp::BitXor | ArithOp::BitOr | ArithOp::BitAnd,
            ) => {
                self.unsupported(TextRange::default(), "bitwise operator");
                None
            }
            RAOp::CmpOp(CmpOp::Eq { negated: false }) => Some(BinOp::Eq),
            RAOp::CmpOp(CmpOp::Eq { negated: true }) => Some(BinOp::NotEq),
            RAOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Less,
                strict: true,
            }) => Some(BinOp::Lt),
            RAOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Less,
                strict: false,
            }) => Some(BinOp::Le),
            RAOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Greater,
                strict: true,
            }) => Some(BinOp::Gt),
            RAOp::CmpOp(CmpOp::Ord {
                ordering: Ordering::Greater,
                strict: false,
            }) => Some(BinOp::Ge),
            RAOp::LogicOp(LogicOp::And) => Some(BinOp::And),
            RAOp::LogicOp(LogicOp::Or) => Some(BinOp::Or),
            RAOp::Assignment { .. } => unreachable!("handled by caller"),
        }
    }
}

// ---------------------------------------------------------------------------
// Macro argument parsing helpers
// ---------------------------------------------------------------------------

/// Split a macro token tree's contents (`("{}", x)`) into a list of
/// expressions. The token tree is walked at the top level; each comma-
/// separated run is re-parsed as an `ast::Expr` (the token tree itself has no
/// expression structure).
fn split_tt_args(
    tt: &ast::TokenTree,
    ctx: &mut LowerCtx,
) -> Vec<Expr> {
    use ra_ap_syntax::ast::TokenTreeChildren;
    let mut out = Vec::new();
    let mut current = String::new();
    for item in TokenTreeChildren::new(tt) {
        match item {
            ra_ap_syntax::NodeOrToken::Token(t) => {
                let text = t.text().to_string();
                if text == "," {
                    push_tt_expr(&mut current, ctx, &mut out);
                } else {
                    current.push_str(&text);
                }
            }
            ra_ap_syntax::NodeOrToken::Node(n) => {
                current.push_str(&n.syntax().text().to_string());
            }
        }
    }
    push_tt_expr(&mut current, ctx, &mut out);
    out
}

fn push_tt_expr(current: &mut String, ctx: &mut LowerCtx, out: &mut Vec<Expr>) {
    let text = current.trim().to_string();
    current.clear();
    if text.is_empty() {
        return;
    }
    // Parse the run as an expression and lower it. `ok()` fails on parse
    // errors (e.g. an unterminated literal), which we skip silently — the
    // source file parse already reported any real syntax errors.
    let parse = ast::Expr::parse(&text, ra_ap_syntax::Edition::Edition2024);
    if let Ok(tree) = parse.ok() {
        if let Some(lowered) = ctx.lower_expr(&tree) {
            out.push(lowered);
        }
    }
}

// ---------------------------------------------------------------------------
// Literal parsing helpers
// ---------------------------------------------------------------------------

/// Strip a Rust integer suffix (`10i32` → `10`), handling `_` separators.
fn strip_int_suffix(text: &str) -> String {
    let mut end = text.len();
    while end > 0 {
        let candidate = &text[..end];
        if parse_int_literal(candidate).is_some() {
            return candidate.replace('_', "");
        }
        end -= 1;
    }
    text.replace('_', "")
}

fn parse_int_literal(text: &str) -> Option<i128> {
    let (neg, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let body = body.replace('_', "");
    let value = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i128::from_str_radix(hex, 16).ok()
    } else if let Some(oct) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) {
        i128::from_str_radix(oct, 8).ok()
    } else if let Some(bin) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) {
        i128::from_str_radix(bin, 2).ok()
    } else {
        body.parse::<i128>().ok()
    };
    value.map(|v| if neg { -v } else { v })
}

/// Strip a float suffix (`1.5f32` → `1.5`).
fn strip_float_suffix(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut end = bytes.len();
    while end > 0 && !bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    text[..end].to_string()
}

/// Unescape a Rust string literal body (without the surrounding quotes):
/// handles `\\`, `\"`, `\n`, `\r`, `\t`, `\0`, `\'`, and `\u{...}`.
fn unescape_string(text: &str) -> String {
    let inner = text
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(text);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('u') => {
                if chars.next() == Some('{') {
                    let mut hex = String::new();
                    for h in chars.by_ref() {
                        if h == '}' {
                            break;
                        }
                        hex.push(h);
                    }
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                    }
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

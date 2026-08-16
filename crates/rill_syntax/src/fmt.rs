//! `rill fmt` — the canonical formatter.
//!
//! Output is defined by the AST, not by the input text: one canonical form
//! (two-space indent, spaced operators, one blank line between top-level
//! declarations), so formatting is idempotent and there is exactly one way to
//! write any program.
//!
//! Two things need care because the parser desugars them:
//! - `a |> f(b)` becomes an ordinary call, so `Expr::Call` records that it was
//!   written as a pipeline and the printer restores it;
//! - `&&`, `||` and `!` become `if`, so the printer recognizes the shapes they
//!   desugar to. A hand-written `if c then true else d` therefore comes back
//!   as `c || d` — same meaning, canonical spelling.
//!
//! Comments are carried across by line anchoring: own-line comments attach to
//! the declaration, statement or match arm that follows them; a trailing
//! comment stays on the line of the construct it followed.

use std::collections::BTreeMap;

use crate::ast::*;
use crate::lexer::{lex_with_comments, Comment};
use crate::parser::parse;
use crate::SyntaxError;

/// Line width above which a single-line function body moves to its own line.
const WIDTH: usize = 100;

pub fn format_source(src: &str) -> Result<String, SyntaxError> {
    let (_, comments) = lex_with_comments(src)?;
    let module = parse(src)?;
    let mut f = Fmt { out: String::new(), comments: Comments::new(comments), last_line: 0 };
    f.module(&module);
    Ok(f.out)
}

// ---- statement boundaries -------------------------------------------------

fn stmt_expr(s: &Stmt) -> &Expr {
    match s {
        Stmt::Bind { value, .. } => value,
        Stmt::Expr(e) => e,
    }
}

// ---- comment bookkeeping ---------------------------------------------------

struct Comments {
    /// Own-line comments, sorted by line.
    own: Vec<(usize, String)>,
    next: usize,
    trailing: BTreeMap<usize, String>,
}

impl Comments {
    fn new(list: Vec<Comment>) -> Comments {
        let mut own = Vec::new();
        let mut trailing = BTreeMap::new();
        for c in list {
            if c.own_line {
                own.push((c.line, c.text));
            } else {
                trailing.insert(c.line, c.text);
            }
        }
        own.sort_by_key(|(l, _)| *l);
        Comments { own, next: 0, trailing }
    }

    fn before(&mut self, line: usize) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        while self.next < self.own.len() && self.own[self.next].0 < line {
            out.push(self.own[self.next].clone());
            self.next += 1;
        }
        out
    }

    fn rest(&mut self) -> Vec<String> {
        let out: Vec<String> = self.own[self.next..].iter().map(|(_, t)| t.clone()).collect();
        self.next = self.own.len();
        out
    }

    fn on(&mut self, line: usize) -> Option<String> {
        self.trailing.remove(&line)
    }
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

// ---- printer ---------------------------------------------------------------

/// A top-level declaration, kept in source order.
enum Decl<'a> {
    Import(&'a ImportDecl),
    Type(&'a TypeDef),
    Trait(&'a TraitDef),
    Impl(&'a ImplDef),
    Extern(&'a ExternFn),
    Fn(&'a FnDef),
}

impl Decl<'_> {
    fn line(&self) -> usize {
        match self {
            Decl::Import(d) => d.line,
            Decl::Type(d) => d.line,
            Decl::Trait(d) => d.line,
            Decl::Impl(d) => d.line,
            Decl::Extern(d) => d.line,
            Decl::Fn(d) => d.line,
        }
    }

    fn is_import(&self) -> bool {
        matches!(self, Decl::Import(_))
    }
}

struct Fmt {
    out: String,
    comments: Comments,
    /// Highest source line already printed; used to tell whether the author
    /// left a blank line between two items (one blank is then preserved).
    last_line: usize,
}

impl Fmt {
    fn push(&mut self, indent: usize, text: &str) {
        self.out.push_str(&pad(indent));
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn blank(&mut self) {
        self.out.push('\n');
    }

    fn mark(&mut self, src_line: usize) {
        self.last_line = self.last_line.max(src_line);
    }

    /// True when the author left at least one blank line before `line`.
    fn had_gap(&self, line: usize) -> bool {
        self.last_line > 0 && line > self.last_line + 1
    }

    /// Emits a line plus the trailing comment that sat on `src_line`.
    fn push_at(&mut self, indent: usize, text: &str, src_line: usize) {
        self.mark(src_line);
        match self.comments.on(src_line) {
            Some(c) => self.push(indent, &format!("{text}  {c}")),
            None => self.push(indent, text),
        }
    }

    /// Everything that precedes the construct at `line`: one blank line
    /// wherever the author left one, plus the own-line comments attached to
    /// it. Idempotent, so callers may nest freely.
    fn lead(&mut self, line: usize, indent: usize) {
        for (cl, text) in self.comments.before(line) {
            if self.had_gap(cl) {
                self.blank();
            }
            self.mark(cl);
            self.push(indent, &text);
        }
        if self.had_gap(line) {
            self.blank();
        }
        self.last_line = self.last_line.max(line.saturating_sub(1));
    }

    fn module(&mut self, m: &Module) {
        let mut decls: Vec<Decl> = Vec::new();
        decls.extend(m.imports.iter().map(Decl::Import));
        decls.extend(m.types.iter().map(Decl::Type));
        decls.extend(m.traits.iter().map(Decl::Trait));
        decls.extend(m.impls.iter().map(Decl::Impl));
        decls.extend(m.externs.iter().map(Decl::Extern));
        decls.extend(m.functions.iter().map(Decl::Fn));
        decls.sort_by_key(|d| d.line());

        let mut prev_import = false;
        for (i, d) in decls.iter().enumerate() {
            // Where this declaration's text really starts: at its own line, or
            // at the first comment attached to it.
            let start = self.comments.own[self.comments.next..]
                .first()
                .map(|(l, _)| *l)
                .filter(|l| *l < d.line())
                .unwrap_or_else(|| d.line());
            let has_lead = start < d.line();
            // One blank line between declarations; consecutive imports the
            // author kept together stay together.
            let keep_together =
                prev_import && d.is_import() && !has_lead && !self.had_gap(d.line());
            if i > 0 && !keep_together {
                self.blank();
                // The separator consumed the source gap; don't double it.
                self.last_line = self.last_line.max(start.saturating_sub(1));
            }
            self.lead(d.line(), 0);
            match d {
                Decl::Import(im) => {
                    let text = match &im.alias {
                        Some(a) => format!("import \"{}\" as {a}", im.path),
                        None => format!("import \"{}\"", im.path),
                    };
                    self.push_at(0, &text, im.line)
                }
                Decl::Type(t) => self.typedef(t),
                Decl::Trait(t) => self.traitdef(t),
                Decl::Impl(im) => self.impldef(im),
                Decl::Extern(e) => {
                    let params: Vec<String> =
                        e.params.iter().map(|(n, t)| format!("{n}: {}", type_ref(t))).collect();
                    let lib = match &e.lib {
                        Some(l) => format!(" \"{l}\""),
                        None => String::new(),
                    };
                    let ret = match &e.ret {
                        Some(t) => format!(" -> {}", type_ref(t)),
                        None => String::new(),
                    };
                    self.push_at(
                        0,
                        &format!("extern{lib} fn {}({}){ret}", e.name, params.join(", ")),
                        e.line,
                    );
                }
                Decl::Fn(f) => self.fndef(f, 0),
            }
            prev_import = d.is_import();
        }
        for c in self.comments.rest() {
            self.out.push('\n');
            self.out.push_str(&c);
            self.out.push('\n');
        }
    }

    fn typedef(&mut self, t: &TypeDef) {
        let params =
            if t.params.is_empty() { String::new() } else { format!("({})", t.params.join(", ")) };
        self.push_at(0, &format!("type {}{}", t.name, params), t.line);
        for c in &t.ctors {
            self.lead(c.line, 1);
            let fields: Vec<String> = c
                .fields
                .iter()
                .map(|(name, ty)| match name {
                    Some(n) => format!("{n}: {}", type_ref(ty)),
                    None => type_ref(ty),
                })
                .collect();
            let text = if fields.is_empty() {
                c.name.clone()
            } else {
                format!("{}({})", c.name, fields.join(", "))
            };
            self.push_at(1, &text, c.line);
        }
    }

    fn traitdef(&mut self, t: &TraitDef) {
        self.push_at(0, &format!("trait {}({})", t.name, t.param), t.line);
        for m in &t.methods {
            self.lead(m.line, 1);
            let params: Vec<String> = m.params.iter().map(type_ref).collect();
            self.push_at(
                1,
                &format!("{}({}) -> {}", m.name, params.join(", "), type_ref(&m.ret)),
                m.line,
            );
        }
    }

    fn impldef(&mut self, im: &ImplDef) {
        self.push_at(0, &format!("impl {}({})", im.trait_name, type_ref(&im.target)), im.line);
        for f in &im.fns {
            self.fndef(f, 1);
        }
    }

    fn fndef(&mut self, f: &FnDef, indent: usize) {
        self.lead(f.line, indent);
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| match &p.ann {
                Some(t) => format!("{}: {}", p.name, type_ref(t)),
                None => p.name.clone(),
            })
            .collect();
        let ret = match &f.ret_ann {
            Some(t) => format!(" -> {}", type_ref(t)),
            None => String::new(),
        };
        let kw = if f.exported { "export fn" } else { "fn" };
        let header = format!("{kw} {}({}){} =", f.name, params.join(", "), ret);

        match &f.body {
            Expr::Block(stmts) => {
                self.push_at(indent, &header, f.line);
                self.stmts(stmts, indent + 1);
            }
            body => {
                let rendered = self.render(body, indent + 1);
                let one_line = format!("{header} {rendered}");
                if rendered.contains('\n')
                    || pad(indent).len() + one_line.len() <= WIDTH
                {
                    self.push_at(indent, &one_line, f.line);
                } else {
                    // Too long for one line: body moves to its own block.
                    self.push_at(indent, &header, f.line);
                    let rendered = self.render(body, indent + 2);
                    self.push(indent + 1, &rendered);
                }
            }
        }
    }

    fn stmts(&mut self, stmts: &[Stmt], indent: usize) {
        for s in stmts {
            let line = stmt_line(s);
            self.lead(line, indent);
            let text = match s {
                Stmt::Bind { name, value, .. } => {
                    format!("{name} = {}", self.render(value, indent + 1))
                }
                Stmt::Expr(e) => self.render(e, indent + 1),
            };
            self.push_at(indent, &text, line);
        }
    }

    /// The statements of a block body, one per line at `ind`, own-line
    /// comments woven in and trailing comments kept on their lines. The
    /// string starts with a newline, ready to follow `->`, `then` or `else`.
    fn block_lines(&mut self, stmts: &[Stmt], ind: usize) -> String {
        let mut s = String::new();
        for st in stmts {
            let line = stmt_line(st);
            for (cl, text) in self.comments.before(line) {
                self.mark(cl);
                s.push_str(&format!("\n{}{}", pad(ind), text));
            }
            self.mark(line);
            let text = match st {
                Stmt::Bind { name, value, .. } => {
                    format!("{name} = {}", self.render(value, ind + 1))
                }
                Stmt::Expr(e) => self.render(e, ind + 1),
            };
            s.push_str(&format!("\n{}{}", pad(ind), text));
            if let Some(c) = self.comments.on(line) {
                s.push_str(&format!("  {c}"));
            }
        }
        s
    }

    /// Whether an `else` branch forces the layout form: it is a block, or an
    /// `else if` chain that ends in one.
    fn else_needs_layout(e: &Expr) -> bool {
        match e {
            Expr::Block(_) => true,
            Expr::If { then, els, .. } => {
                matches!(then.as_ref(), Expr::Block(_)) || Self::else_needs_layout(els)
            }
            _ => false,
        }
    }

    /// Renders an expression. Continuation lines (match arms, block
    /// statements) are indented at `cont`; the first line carries no indent.
    fn render(&mut self, expr: &Expr, cont: usize) -> String {
        match expr {
            Expr::Match { scrutinee, arms, .. } => {
                let mut s = format!("match {}", self.render(scrutinee, cont));
                for arm in arms {
                    for (cl, text) in self.comments.before(arm.line) {
                        self.mark(cl);
                        s.push_str(&format!("\n{}{}", pad(cont), text));
                    }
                    self.mark(arm.line);
                    if let Expr::Block(stmts) = &arm.body {
                        s.push_str(&format!("\n{}{} ->", pad(cont), pattern(&arm.pattern)));
                        if let Some(c) = self.comments.on(arm.line) {
                            s.push_str(&format!("  {c}"));
                        }
                        s.push_str(&self.block_lines(stmts, cont + 1));
                    } else {
                        let body = self.render(&arm.body, cont + 1);
                        s.push_str(&format!("\n{}{} -> {}", pad(cont), pattern(&arm.pattern), body));
                        if let Some(c) = self.comments.on(arm.line) {
                            s.push_str(&format!("  {c}"));
                        }
                    }
                }
                s
            }
            Expr::Block(stmts) => {
                let mut parts = Vec::new();
                for s in stmts {
                    let text = match s {
                        Stmt::Bind { name, value, .. } => {
                            format!("{name} = {}", self.render(value, cont + 1))
                        }
                        Stmt::Expr(e) => self.render(e, cont + 1),
                    };
                    parts.push(format!("{}{}", pad(cont), text));
                }
                parts.join("\n").trim_start().to_string()
            }
            // `if` and the prefix forms can carry a multi-line branch (a
            // `match`), which has to stay on its own lines.
            Expr::If { cond, then, els } => {
                let then_blk = matches!(then.as_ref(), Expr::Block(_));
                if then_blk || Self::else_needs_layout(els) {
                    let kw = cont.saturating_sub(1);
                    let mut s = format!("if {} then", inline(cond, 0));
                    match then.as_ref() {
                        Expr::Block(stmts) => s.push_str(&self.block_lines(stmts, cont)),
                        other => {
                            s.push(' ');
                            s.push_str(&self.render(other, cont));
                        }
                    }
                    s.push_str(&format!("\n{}else", pad(kw)));
                    match els.as_ref() {
                        Expr::Block(stmts) => s.push_str(&self.block_lines(stmts, cont)),
                        other => {
                            s.push(' ');
                            s.push_str(&self.render(other, cont));
                        }
                    }
                    return s;
                }
                let t = self.render(then, cont);
                let e = self.render(els, cont);
                if t.contains('\n') || e.contains('\n') {
                    format!("if {} then {t} else {e}", inline(cond, 0))
                } else {
                    inline(expr, 0)
                }
            }
            Expr::Lambda { params, body, .. } => {
                let b = self.render(body, cont);
                if b.contains('\n') {
                    format!("\\{} -> {b}", params.join(", "))
                } else {
                    inline(expr, 0)
                }
            }
            Expr::Spawn { body, .. } => {
                let b = self.render(body, cont);
                if b.contains('\n') {
                    format!("spawn {b}")
                } else {
                    inline(expr, 0)
                }
            }
            Expr::Select { arms, default, .. } => {
                let mut s = "select".to_string();
                for arm in arms {
                    for (cl, text) in self.comments.before(arm.line) {
                        self.mark(cl);
                        s.push_str(&format!("\n{}{}", pad(cont), text));
                    }
                    self.mark(arm.line);
                    let op = match &arm.op {
                        SelectOp::Recv { chan, bind } => {
                            let call = format!("recv({})", inline(chan, 0));
                            match bind {
                                Some(n) => format!("{n} = {call}"),
                                None => call,
                            }
                        }
                        SelectOp::Send { chan, value } => {
                            format!("send({}, {})", inline(chan, 0), inline(value, 0))
                        }
                    };
                    if let Expr::Block(stmts) = &arm.body {
                        s.push_str(&format!("\n{}{op} ->", pad(cont)));
                        s.push_str(&self.block_lines(stmts, cont + 1));
                    } else {
                        let body = self.render(&arm.body, cont + 1);
                        s.push_str(&format!("\n{}{op} -> {body}", pad(cont)));
                    }
                }
                if let Some(d) = default {
                    // A literal default body carries no line of its own; it
                    // sits on the line after the last arm.
                    let dl = expr_line(d);
                    let after_last = arms.last().map(|a| a.line + 1).unwrap_or(0);
                    self.mark(if dl == 0 { after_last } else { dl });
                    if let Expr::Block(stmts) = d.as_ref() {
                        s.push_str(&format!("\n{}default ->", pad(cont)));
                        s.push_str(&self.block_lines(stmts, cont + 1));
                    } else {
                        let body = self.render(d, cont + 1);
                        s.push_str(&format!("\n{}default -> {body}", pad(cont)));
                    }
                }
                s
            }
            other => inline(other, 0),
        }
    }
}

fn stmt_line(s: &Stmt) -> usize {
    match s {
        Stmt::Bind { line, .. } => *line,
        Stmt::Expr(e) => expr_line(e),
    }
}

fn expr_line(e: &Expr) -> usize {
    match e {
        Expr::Var(_, l) => *l,
        Expr::Call { line, .. } => *line,
        Expr::Binary { line, .. } => *line,
        Expr::Match { line, .. } => *line,
        Expr::Lambda { line, .. } => *line,
        Expr::Spawn { line, .. } => *line,
        Expr::Select { line, .. } => *line,
        Expr::Index { line, .. } => *line,
        Expr::IndexSet { line, .. } => *line,
        Expr::Unary { expr, .. } => expr_line(expr),
        Expr::If { cond, .. } => expr_line(cond),
        Expr::Block(stmts) => stmts.first().map(stmt_line).unwrap_or(0),
        _ => 0,
    }
}

// ---- inline expressions ----------------------------------------------------

const P_PIPE: u8 = 1;
const P_OR: u8 = 2;
const P_AND: u8 = 3;
const P_CMP: u8 = 4;
const P_ADD: u8 = 5;
const P_MUL: u8 = 6;
const P_UNARY: u8 = 7;
/// `b[i]` binds tighter than any operator, so its base is printed at a level
/// that parenthesizes anything looser.
const P_INDEX: u8 = 8;

fn binop(op: BinOp) -> (&'static str, u8) {
    match op {
        BinOp::Add => ("+", P_ADD),
        BinOp::Sub => ("-", P_ADD),
        BinOp::Mul => ("*", P_MUL),
        BinOp::Div => ("/", P_MUL),
        BinOp::Mod => ("%", P_MUL),
        BinOp::Eq => ("==", P_CMP),
        BinOp::Ne => ("!=", P_CMP),
        BinOp::Lt => ("<", P_CMP),
        BinOp::Le => ("<=", P_CMP),
        BinOp::Gt => (">", P_CMP),
        BinOp::Ge => (">=", P_CMP),
    }
}

fn wrap(need: bool, s: String) -> String {
    if need {
        format!("({s})")
    } else {
        s
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Formats a float the way the lexer accepts it back (always a decimal point).
fn float_lit(v: f64) -> String {
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

fn inline(e: &Expr, prec: u8) -> String {
    match e {
        Expr::Int(v) => v.to_string(),
        Expr::Float(v) => float_lit(*v),
        Expr::Str(s) => format!("\"{}\"", escape(s)),
        Expr::Bool(b) => b.to_string(),
        Expr::Var(n, _) => n.clone(),
        Expr::Call { name, args, piped, .. } => {
            if *piped && !args.is_empty() {
                let head = inline(&args[0], P_PIPE);
                let rest: Vec<String> = args[1..].iter().map(|a| inline(a, 0)).collect();
                let call = if rest.is_empty() {
                    name.clone()
                } else {
                    format!("{name}({})", rest.join(", "))
                };
                wrap(prec > P_PIPE, format!("{head} |> {call}"))
            } else {
                let a: Vec<String> = args.iter().map(|a| inline(a, 0)).collect();
                format!("{name}({})", a.join(", "))
            }
        }
        Expr::Index { base, index, .. } => {
            format!("{}[{}]", inline(base, P_INDEX), inline(index, 0))
        }
        Expr::IndexSet { base, index, value, .. } => {
            format!("{}[{}] = {}", inline(base, P_INDEX), inline(index, 0), inline(value, 0))
        }
        Expr::Unary { op: UnOp::Neg, expr } => {
            wrap(prec > P_UNARY, format!("-{}", inline(expr, P_UNARY)))
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let (sym, p) = binop(*op);
            // Comparisons don't chain, so both sides bind tighter.
            let rp = if p == P_CMP { p } else { p + 1 };
            wrap(prec > p, format!("{} {sym} {}", inline(lhs, p), inline(rhs, rp)))
        }
        Expr::If { cond, then, els } => {
            // Restore the operators these shapes desugar from.
            match (then.as_ref(), els.as_ref()) {
                (Expr::Bool(false), Expr::Bool(true)) => {
                    wrap(prec > P_UNARY, format!("!{}", inline(cond, P_UNARY)))
                }
                // Both operators are left-associative: the left operand sits
                // at the same level, the right one binds tighter.
                (Expr::Bool(true), rhs) => wrap(
                    prec > P_OR,
                    format!("{} || {}", inline(cond, P_OR), inline(rhs, P_OR + 1)),
                ),
                (lhs, Expr::Bool(false)) => wrap(
                    prec > P_AND,
                    format!("{} && {}", inline(cond, P_AND), inline(lhs, P_AND + 1)),
                ),
                _ => wrap(
                    prec > 0,
                    format!(
                        "if {} then {} else {}",
                        inline(cond, 0),
                        inline(then, 0),
                        inline(els, 0)
                    ),
                ),
            }
        }
        Expr::Lambda { params, body, .. } => {
            wrap(prec > 0, format!("\\{} -> {}", params.join(", "), inline(body, 0)))
        }
        Expr::Spawn { body, .. } => wrap(prec > 0, format!("spawn {}", inline(body, 0))),
        // Inside parentheses there is no layout, so the arms are printed on
        // one line — which is exactly the form that parses back there.
        Expr::Match { scrutinee, arms, .. } => {
            let arms: Vec<String> = arms
                .iter()
                .map(|a| format!("{} -> {}", pattern(&a.pattern), inline(&a.body, 0)))
                .collect();
            wrap(prec > 0, format!("match {} {}", inline(scrutinee, 0), arms.join(" ")))
        }
        Expr::Select { arms, default, .. } => {
            let mut parts: Vec<String> = arms
                .iter()
                .map(|a| {
                    let op = match &a.op {
                        SelectOp::Recv { chan, bind } => {
                            let call = format!("recv({})", inline(chan, 0));
                            match bind {
                                Some(n) => format!("{n} = {call}"),
                                None => call,
                            }
                        }
                        SelectOp::Send { chan, value } => {
                            format!("send({}, {})", inline(chan, 0), inline(value, 0))
                        }
                    };
                    format!("{op} -> {}", inline(&a.body, 0))
                })
                .collect();
            if let Some(d) = default {
                parts.push(format!("default -> {}", inline(d, 0)));
            }
            wrap(prec > 0, format!("select {}", parts.join(" ")))
        }
        Expr::Block(_) => "...".to_string(),
    }
}

fn pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var(n) => n.clone(),
        Pattern::Int(v) => v.to_string(),
        Pattern::Ctor { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let a: Vec<String> = args.iter().map(pattern).collect();
                format!("{name}({})", a.join(", "))
            }
        }
    }
}

fn type_ref(t: &TypeRef) -> String {
    // `Fn` keeps its parentheses even when empty: `Fn()` is a callback taking
    // nothing, not a type named `Fn`.
    let mut s = if t.args.is_empty() && t.name != "Fn" {
        t.name.clone()
    } else {
        let a: Vec<String> = t.args.iter().map(type_ref).collect();
        format!("{}({})", t.name, a.join(", "))
    };
    if let Some(r) = &t.ret {
        s.push_str(&format!(" -> {}", type_ref(r)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_source(src).unwrap()
    }

    #[test]
    fn normalizes_spacing_and_indent() {
        let src = "fn  add(a,b)=a+b*2\n";
        assert_eq!(fmt(src), "fn add(a, b) = a + b * 2\n");
    }

    #[test]
    fn keeps_needed_parens_only() {
        assert_eq!(fmt("fn f(a, b) = (a + b) * 2\n"), "fn f(a, b) = (a + b) * 2\n");
        assert_eq!(fmt("fn f(a, b) = (a * b) + 2\n"), "fn f(a, b) = a * b + 2\n");
        assert_eq!(fmt("fn f(a, b) = a - (b - 1)\n"), "fn f(a, b) = a - (b - 1)\n");
    }

    /// Indexing binds tighter than every operator, so its base parenthesizes
    /// anything looser and nothing else needs parentheses around it.
    #[test]
    fn formats_indexing() {
        let src = "fn f(b: Buf(I64), i) = b[i + 1] * 2 - b[i]\n";
        assert_eq!(fmt(src), src);
        let src = "fn g(b: Buf(I64), i) =\n  b[i] = -b[i + 1]\n  b[b[i]] = 7\n  b[0]\n";
        assert_eq!(fmt(src), src);
        assert_eq!(fmt("fn f(b:Buf(I64))=b[ 0 ]\n"), "fn f(b: Buf(I64)) = b[0]\n");
    }

    #[test]
    fn preserves_pipelines() {
        let src = "fn main() = range(1, 10) |> map(\\x -> x * x) |> sum |> println\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn restores_logical_operators() {
        let src = "fn f(a, b) = a && !b || a\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn formats_types_and_match() {
        let src = "type Shape\n  Circle(r: Float)\n  Rect(Float, Float)\n\nfn area(s) = match s\n  Circle(r) -> 3.0 * r * r\n  Rect(w, h) -> w * h\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn keeps_comments() {
        let src = "# header\nfn f() = 1  # trailing\n\n# about g\nfn g() = 2\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn is_idempotent_on_messy_input() {
        let messy = "type   T\n     A\n     B(Int)\nfn  f(x)=match x\n       A->1\n       B(n)->n+1\nfn main()=println(f(A))\n";
        let once = fmt(&messy);
        assert_eq!(fmt(&once), once);
        assert!(once.contains("fn f(x) = match x"), "{once}");
    }

    #[test]
    fn formats_traits_and_impls() {
        let src = "trait D(a)\n  d(a) -> Str\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n";
        assert_eq!(fmt(src), src);
    }
}

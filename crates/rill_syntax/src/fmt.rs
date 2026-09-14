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
    // Line 0 does not exist, so the vector is one longer than the file and
    // indexed by the line number as everything else counts it.
    let mut blank = vec![false];
    blank.extend(src.lines().map(|l| l.trim().is_empty()));
    let mut f =
        Fmt { out: String::new(), comments: Comments::new(comments), last_line: 0, blank };
    f.module(&module);
    Ok(f.out)
}

// ---- statement boundaries -------------------------------------------------

fn stmt_expr(s: &Stmt) -> &Expr {
    match s {
        Stmt::Bind { value, .. } | Stmt::PatBind { value, .. } => value,
        Stmt::Fn(f) => &f.body,
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
        let mut out: Vec<String> = self.own[self.next..].iter().map(|(_, t)| t.clone()).collect();
        self.next = self.own.len();
        // A trailing comment nothing claimed — on a line the formatter
        // folded away — is kept at the end rather than lost.
        out.extend(std::mem::take(&mut self.trailing).into_values());
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
    Asm(&'a AsmFn),
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
            Decl::Asm(d) => d.line,
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
    /// Which source lines were blank. What is printed is anchored to the
    /// line a construct *starts* on, so a statement written across three
    /// lines leaves two behind it that nothing accounts for; without asking
    /// whether any of them was actually blank, the next statement would be
    /// taken for one the author had separated and a blank line invented
    /// between them.
    blank: Vec<bool>,
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
        self.last_line > 0
            && line > self.last_line + 1
            && (self.last_line + 1..line).any(|l| self.blank.get(l).copied().unwrap_or(false))
    }

    /// Emits a construct plus the trailing comments it carried.
    fn push_at(&mut self, indent: usize, text: &str, src_line: usize) {
        self.mark(src_line);
        let text = self.with_trailing(text, src_line);
        self.push(indent, &text);
    }

    /// The trailing comments a rendered construct carries: the one on its
    /// first line and, where it runs on over the lines the author broke it
    /// into, the ones on those — each kept on the output line at the same
    /// offset from the first, which is where the break was kept too.
    fn with_trailing(&mut self, text: &str, src_line: usize) -> String {
        let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
        for (j, line) in lines.iter_mut().enumerate() {
            if let Some(c) = self.comments.on(src_line + j) {
                line.push_str("  ");
                line.push_str(&c);
            }
        }
        lines.join("\n")
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
        if let Some(n) = &m.name {
            self.push_at(0, &format!("module {n}"), m.name_line);
        }
        let mut decls: Vec<Decl> = Vec::new();
        decls.extend(m.imports.iter().map(Decl::Import));
        decls.extend(m.types.iter().map(Decl::Type));
        decls.extend(m.traits.iter().map(Decl::Trait));
        decls.extend(m.impls.iter().map(Decl::Impl));
        decls.extend(m.externs.iter().map(Decl::Extern));
        decls.extend(m.asms.iter().map(Decl::Asm));
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
                        &format!(
                            "extern{lib}{} fn {}({}){ret}",
                            if e.blocking { " blocking" } else { "" },
                            e.name,
                            params.join(", ")
                        ),
                        e.line,
                    );
                }
                Decl::Asm(a) => {
                    let params: Vec<String> =
                        a.params.iter().map(|(n, t)| format!("{n}: {}", type_ref(t))).collect();
                    let ret = match &a.ret {
                        Some(t) => format!(" -> {}", type_ref(t)),
                        None => String::new(),
                    };
                    let cons = match &a.constraints {
                        Some(c) => format!(" : \"{}\"", escape(c)),
                        None => String::new(),
                    };
                    let naked = if a.naked { "naked " } else { "" };
                    self.push_at(
                        0,
                        &format!(
                            "{naked}asm fn {}({}){ret} = \"{}\"{cons}",
                            a.name,
                            params.join(", "),
                            escape(&a.template)
                        ),
                        a.line,
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
        if let Some(a) = &t.alias {
            self.push_at(0, &format!("type {}{} = {}", t.name, params, type_ref(a)), t.line);
            return;
        }
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
        let supers = if t.supers.is_empty() { String::new() } else { format!(": {}", t.supers.join(", ")) };
        self.push_at(0, &format!("trait {}({}){supers}", t.name, t.param), t.line);
        for m in &t.methods {
            self.lead(m.line, 1);
            let params: Vec<String> = m
                .params
                .iter()
                .zip(&m.names)
                .map(|(ty, n)| match n {
                    Some(n) => format!("{n}: {}", type_ref(ty)),
                    None => type_ref(ty),
                })
                .collect();
            let head = format!("{}({}) -> {}", m.name, params.join(", "), type_ref(&m.ret));
            let text = match &m.default {
                None => head,
                Some(Expr::Block(stmts)) => format!("{head} ={}", self.block_lines(stmts, 2)),
                Some(body) => format!("{head} = {}", self.render(body, 2)),
            };
            self.push_at(1, &text, m.line);
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
        let kw = if f.exported {
            "export fn"
        } else if f.private {
            "private fn"
        } else {
            "fn"
        };
        let header = format!("{kw} {}({}){}{} =", f.name, params.join(", "), ret, wheres(f));

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
            if let Stmt::Fn(f) = s {
                // A local function prints exactly like a top-level one, one
                // level in; `fndef` places its own comments and blank lines.
                self.fndef(f, indent);
                continue;
            }
            let line = stmt_line(s);
            self.lead(line, indent);
            let text = match s {
                Stmt::Bind { name, value, .. } => {
                    format!("{name} = {}", self.render(value, indent + 1))
                }
                Stmt::PatBind { pattern: p, value, .. } => {
                    format!("{} = {}", pattern(p), self.render(value, indent + 1))
                }
                Stmt::Fn(_) => unreachable!("handled above"),
                Stmt::Expr(e) => self.render(e, indent + 1),
            };
            self.push_at(indent, &text, line);
        }
    }

    /// A local function as text, for the block forms that build a string
    /// rather than pushing lines.
    fn fndef_text(&mut self, f: &FnDef, ind: usize) -> String {
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
        let header = format!("fn {}({}){}{} =", f.name, params.join(", "), ret, wheres(f));
        match &f.body {
            Expr::Block(stmts) => {
                let body = self.block_lines(stmts, ind + 1);
                format!("{header}{body}")
            }
            body => format!("{header} {}", self.render(body, ind + 1)),
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
                Stmt::PatBind { pattern: p, value, .. } => {
                    format!("{} = {}", pattern(p), self.render(value, ind + 1))
                }
                Stmt::Fn(f) => self.fndef_text(f, ind),
                Stmt::Expr(e) => self.render(e, ind + 1),
            };
            let text = self.with_trailing(&text, line);
            s.push_str(&format!("\n{}{}", pad(ind), text));
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
                    let head = match &arm.guard {
                        Some(g) => {
                            format!("{} if {}", pattern(&arm.pattern), self.render(g, cont + 1))
                        }
                        None => pattern(&arm.pattern),
                    };
                    if let Expr::Block(stmts) = &arm.body {
                        s.push_str(&format!("\n{}{head} ->", pad(cont)));
                        if let Some(c) = self.comments.on(arm.line) {
                            s.push_str(&format!("  {c}"));
                        }
                        s.push_str(&self.block_lines(stmts, cont + 1));
                    } else {
                        let body = self.render(&arm.body, cont + 1);
                        let text = self.with_trailing(&format!("{head} -> {body}"), arm.line);
                        s.push_str(&format!("\n{}{text}", pad(cont)));
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
                        Stmt::PatBind { pattern: p, value, .. } => {
                            format!("{} = {}", pattern(p), self.render(value, cont + 1))
                        }
                        Stmt::Fn(f) => self.fndef_text(f, cont),
                        Stmt::Expr(e) => self.render(e, cont + 1),
                    };
                    parts.push(format!("{}{}", pad(cont), text));
                }
                parts.join("\n").trim_start().to_string()
            }
            Expr::Append { target, value, .. } => {
                format!("{} += {}", inline(target, P_INDEX, cont), inline(value, 0, cont))
            }
            // `v[i] = for ... with ...`: the value may carry a block of its
            // own, which the inline printer cannot say (it printed `...` and
            // lost the body).
            Expr::IndexSet { base, index, value, .. } => format!(
                "{}[{}] = {}",
                inline(base, P_INDEX, cont),
                inline(index, 0, cont),
                self.render(value, cont)
            ),
            Expr::IndexOp { op, base, index, value, .. } => format!(
                "{}[{}] {}= {}",
                inline(base, P_INDEX, cont),
                inline(index, 0, cont),
                binop(*op).0,
                self.render(value, cont)
            ),
            Expr::For { var, from, to, acc, body, .. } => {
                let head = format!(
                    "for {var} in {}..{}{}",
                    inline(from, 0, cont),
                    inline(to, 0, cont),
                    with_str(acc, cont)
                );
                match body.as_ref() {
                    Expr::Block(stmts) => format!("{head}{}", self.block_lines(stmts, cont)),
                    other => format!("{head} {}", self.render(other, cont)),
                }
            }
            Expr::While { cond, acc, body, .. } => {
                let head = format!("while {}{}", inline(cond, 0, cont), with_pat(acc, cont));
                match body.as_ref() {
                    Expr::Block(stmts) => format!("{head}{}", self.block_lines(stmts, cont)),
                    other => format!("{head} {}", self.render(other, cont)),
                }
            }
            // `if` and the prefix forms can carry a multi-line branch (a
            // `match`), which has to stay on its own lines.
            Expr::If { cond, then, els, .. } => {
                let then_blk = matches!(then.as_ref(), Expr::Block(_));
                if then_blk || Self::else_needs_layout(els) {
                    let kw = cont.saturating_sub(1);
                    let mut s = format!("if {} then", inline(cond, 0, cont));
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
                    format!("if {} then {t} else {e}", inline(cond, 0, cont))
                } else {
                    inline(expr, 0, cont)
                }
            }
            Expr::Lambda { params, body, .. } => {
                // A block body goes on the lines after the `->`, as it was
                // written; anything else that came out multi-line stays
                // beside it.
                if let Expr::Block(stmts) = body.as_ref() {
                    return format!("\\{} ->{}", params.join(", "), self.block_lines(stmts, cont));
                }
                let b = self.render(body, cont);
                if b.contains('\n') {
                    format!("\\{} -> {b}", params.join(", "))
                } else {
                    inline(expr, 0, cont)
                }
            }
            Expr::Spawn { body, .. } => {
                let b = self.render(body, cont);
                if b.contains('\n') {
                    format!("spawn {b}")
                } else {
                    inline(expr, 0, cont)
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
                            let call = format!("recv({})", inline(chan, 0, cont));
                            match bind {
                                Some(n) => format!("{n} = {call}"),
                                None => call,
                            }
                        }
                        SelectOp::Send { chan, value } => {
                            format!("send({}, {})", inline(chan, 0, cont), inline(value, 0, cont))
                        }
                        SelectOp::Timeout { ms } => format!("timeout({})", inline(ms, 0, cont)),
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
            other => inline(other, 0, cont),
        }
    }
}

fn stmt_line(s: &Stmt) -> usize {
    match s {
        Stmt::Bind { line, .. } | Stmt::PatBind { line, .. } => *line,
        Stmt::Fn(f) => f.line,
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
        Expr::Len { line, .. } => *line,
        Expr::Try { line, .. } | Expr::Field { line, .. } => *line,
        Expr::Index { line, .. } => *line,
        Expr::Slice { line, .. } => *line,
        Expr::IndexSet { line, .. } => *line,
        Expr::IndexOp { line, .. } => *line,
        Expr::Append { line, .. } => *line,
        Expr::For { line, .. } => *line,
        Expr::While { line, .. } => *line,
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

fn inline(e: &Expr, prec: u8, ind: usize) -> String {
    match e {
        Expr::Int(v) => v.to_string(),
        Expr::Float(v) => float_lit(*v),
        Expr::Str(s) => format!("\"{}\"", escape(s)),
        Expr::Bool(b) => b.to_string(),
        Expr::Var(n, _) => n.clone(),
        Expr::Update { base, fields, .. } => {
            let set = fields
                .iter()
                .map(|(n, v, _)| format!("{n} = {}", inline(v, 0, ind)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {} | {set} }}", inline(base, 0, ind))
        }
        Expr::Call { name, args, piped, stacked, .. } => {
            if *piped && !args.is_empty() {
                let head = inline(&args[0], P_PIPE, ind);
                let rest: Vec<String> = args[1..].iter().map(|a| inline(a, 0, ind)).collect();
                let call = if rest.is_empty() {
                    name.clone()
                } else {
                    format!("{name}({})", rest.join(", "))
                };
                // A `|>` that began a line stays at the head of one.
                let gap = if *stacked { format!("\n{}", pad(ind)) } else { " ".to_string() };
                wrap(prec > P_PIPE, format!("{head}{gap}|> {call}"))
            } else if *stacked && !args.is_empty() {
                // Arguments written one to a line stay that way: the first
                // beside the name, the rest a level in, and what they break
                // inside themselves a level in again.
                let a: Vec<String> = args.iter().map(|a| inline(a, 0, ind + 1)).collect();
                let name = if is_tuple(name, args.len()) { "" } else { name };
                format!("{name}({})", a.join(&format!(",\n{}", pad(ind))))
            } else {
                let a: Vec<String> = args.iter().map(|a| inline(a, 0, ind)).collect();
                format!("{}({})", if is_tuple(name, args.len()) { "" } else { name }, a.join(", "))
            }
        }
        Expr::Len { expr, .. } => format!("#{}", inline(expr, P_UNARY, ind)),
        // `?` binds as tightly as a call, so nothing round it needs bracketing.
        Expr::Try { expr, .. } => format!("{}?", inline(expr, P_INDEX, ind)),
        Expr::Field { base, name, .. } => format!("{}.{name}", inline(base, P_INDEX, ind)),
        Expr::Index { base, index, .. } => {
            format!("{}[{}]", inline(base, P_INDEX, ind), inline(index, 0, ind))
        }
        Expr::Slice { base, from, to, .. } => format!(
            "{}[{}:{}]",
            inline(base, P_INDEX, ind),
            from.as_ref().map(|e| inline(e, 0, ind)).unwrap_or_default(),
            to.as_ref().map(|e| inline(e, 0, ind)).unwrap_or_default()
        ),
        Expr::IndexSet { base, index, value, .. } => {
            format!("{}[{}] = {}", inline(base, P_INDEX, ind), inline(index, 0, ind), inline(value, 0, ind))
        }
        Expr::IndexOp { op, base, index, value, .. } => format!(
            "{}[{}] {}= {}",
            inline(base, P_INDEX, ind),
            inline(index, 0, ind),
            binop(*op).0,
            inline(value, 0, ind)
        ),
        // Nothing in an expression can consume a `..`, so neither end of the
        // range ever needs parentheses.
        Expr::Append { target, value, .. } => {
            format!("{} += {}", inline(target, P_INDEX, ind), inline(value, 0, ind))
        }
        Expr::For { var, from, to, acc, body, .. } => {
            format!("for {var} in {}..{}{} {}", inline(from, 0, ind), inline(to, 0, ind), with_str(acc, ind), inline(body, 0, ind))
        }
        Expr::While { cond, acc, body, .. } => {
            format!("while {}{} {}", inline(cond, 0, ind), with_pat(acc, ind), inline(body, 0, ind))
        }
        Expr::Unary { op: UnOp::Neg, expr } => {
            wrap(prec > P_UNARY, format!("-{}", inline(expr, P_UNARY, ind)))
        }
        Expr::Binary { op, lhs, rhs, newline, .. } => {
            let (sym, p) = binop(*op);
            // Comparisons don't chain, so a comparison on either side of one
            // keeps its parentheses: `(a < 0) != (b < 0)` is not `a < 0 !=
            // b < 0`, which does not parse.
            let (lp, rp) = if p == P_CMP { (p + 1, p + 1) } else { (p, p + 1) };
            // An operator that began a line stays at the head of one, a
            // level in from the statement it continues.
            let gap = if *newline { format!("\n{}", pad(ind)) } else { " ".to_string() };
            wrap(prec > p, format!("{}{gap}{sym} {}", inline(lhs, lp, ind), inline(rhs, rp, ind)))
        }
        Expr::If { cond, then, els, newline } => {
            // An operator, or a `then` and `else`, that began a line stays
            // at the head of one, a level in from the statement.
            let gap = if *newline { format!("\n{}", pad(ind)) } else { " ".to_string() };
            // Restore the operators these shapes desugar from.
            match (then.as_ref(), els.as_ref()) {
                (Expr::Bool(false), Expr::Bool(true)) => {
                    wrap(prec > P_UNARY, format!("!{}", inline(cond, P_UNARY, ind)))
                }
                // Both operators are left-associative: the left operand sits
                // at the same level, the right one binds tighter.
                (Expr::Bool(true), rhs) => wrap(
                    prec > P_OR,
                    format!("{}{gap}|| {}", inline(cond, P_OR, ind), inline(rhs, P_OR + 1, ind)),
                ),
                (lhs, Expr::Bool(false)) => wrap(
                    prec > P_AND,
                    format!("{}{gap}&& {}", inline(cond, P_AND, ind), inline(lhs, P_AND + 1, ind)),
                ),
                _ => wrap(
                    prec > 0,
                    format!(
                        "if {}{gap}then {}{gap}else {}",
                        inline(cond, 0, ind),
                        inline(then, 0, ind),
                        inline(els, 0, ind)
                    ),
                ),
            }
        }
        Expr::Lambda { params, body, .. } => {
            wrap(prec > 0, format!("\\{} -> {}", params.join(", "), inline(body, 0, ind)))
        }
        Expr::Spawn { body, .. } => wrap(prec > 0, format!("spawn {}", inline(body, 0, ind))),
        // Inside parentheses there is no layout, so the arms are printed on
        // one line — which is exactly the form that parses back there.
        Expr::Match { scrutinee, arms, .. } => {
            let arms: Vec<String> = arms
                .iter()
                .map(|a| match &a.guard {
                    Some(g) => format!(
                        "{} if {} -> {}",
                        pattern(&a.pattern),
                        inline(g, 0, ind),
                        inline(&a.body, 0, ind)
                    ),
                    None => format!("{} -> {}", pattern(&a.pattern), inline(&a.body, 0, ind)),
                })
                .collect();
            wrap(prec > 0, format!("match {} {}", inline(scrutinee, 0, ind), arms.join(" ")))
        }
        Expr::Select { arms, default, .. } => {
            let mut parts: Vec<String> = arms
                .iter()
                .map(|a| {
                    let op = match &a.op {
                        SelectOp::Recv { chan, bind } => {
                            let call = format!("recv({})", inline(chan, 0, ind));
                            match bind {
                                Some(n) => format!("{n} = {call}"),
                                None => call,
                            }
                        }
                        SelectOp::Send { chan, value } => {
                            format!("send({}, {})", inline(chan, 0, ind), inline(value, 0, ind))
                        }
                        SelectOp::Timeout { ms } => format!("timeout({})", inline(ms, 0, ind)),
                    };
                    format!("{op} -> {}", inline(&a.body, 0, ind))
                })
                .collect();
            if let Some(d) = default {
                parts.push(format!("default -> {}", inline(d, 0, ind)));
            }
            wrap(prec > 0, format!("select {}", parts.join(" ")))
        }
        Expr::Block(_) => "...".to_string(),
    }
}

fn pattern(p: &Pattern) -> String {
    match p {
        Pattern::As(n, inner, _) => format!("{n} @ {}", pattern(inner)),
        Pattern::Or(alts, _) => {
            alts.iter().map(pattern).collect::<Vec<_>>().join(" | ")
        }
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var(n) => n.clone(),
        Pattern::Int(v) => v.to_string(),
        Pattern::Ctor { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let a: Vec<String> = args.iter().map(pattern).collect();
                if is_tuple(name, args.len()) {
                    format!("({})", a.join(", "))
                } else {
                    format!("{name}({})", a.join(", "))
                }
            }
        }
    }
}

/// `Tuple2`..`Tuple4` are what the tuple notation stands for, and are printed
/// back the way they were written.
fn is_tuple(name: &str, n: usize) -> bool {
    (2..=8).contains(&n) && name == format!("Tuple{n}")
}

/// `with acc = init`, when the loop carries something.
fn with_str(acc: &Option<(String, Box<Expr>)>, ind: usize) -> String {
    match acc {
        Some((n, e)) => format!(" with {n} = {}", inline(e, 0, ind)),
        None => String::new(),
    }
}

fn with_pat(acc: &Option<(Vec<Param>, Box<Expr>)>, ind: usize) -> String {
    let one = |p: &Param| match &p.ann {
        Some(t) => format!("{}: {}", p.name, type_ref(t)),
        None => p.name.clone(),
    };
    match acc {
        Some((ps, e)) if ps.len() == 1 => format!(" with {} = {}", one(&ps[0]), inline(e, 0, ind)),
        Some((ps, e)) => {
            format!(" with ({}) = {}", ps.iter().map(one).collect::<Vec<_>>().join(", "), inline(e, 0, ind))
        }
        None => String::new(),
    }
}

/// ` where Describe(a), Ord(b)`, or nothing.
fn wheres(f: &FnDef) -> String {
    if f.wheres.is_empty() {
        return String::new();
    }
    let bs: Vec<String> = f.wheres.iter().map(|b| format!("{}({})", b.trait_name, b.var)).collect();
    format!(" where {}", bs.join(", "))
}

fn type_ref(t: &TypeRef) -> String {
    // `Fn` keeps its parentheses even when empty: `Fn()` is a callback taking
    // nothing, not a type named `Fn`.
    if t.name == "->" {
        let a: Vec<String> = t.args.iter().map(type_ref).collect();
        let r = t.ret.as_ref().expect("an arrow has a result");
        return format!("({}) -> {}", a.join(", "), type_ref(r));
    }
    if is_tuple(&t.name, t.args.len()) {
        let a: Vec<String> = t.args.iter().map(type_ref).collect();
        return format!("({})", a.join(", "));
    }
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

    /// `for` and `op=` survive the round trip; both are spelled out only
    /// after the formatter has had its look.
    #[test]
    fn formats_for_and_compound_assignment() {
        let src = "fn main() =\n  v = buf_int(4)\n\n  for i in 0..4\n    v[i] = i\n    v[i] += 1\n\n  println(v[3])\n";
        assert_eq!(fmt(src), src);
        assert_eq!(
            fmt("fn f(v: Buf(I64), n) =\n  for i in 0..n - 1\n    v[i] *= 2\n"),
            "fn f(v: Buf(I64), n) =\n  for i in 0..n - 1\n    v[i] *= 2\n"
        );
    }

    /// A slice keeps whichever ends were written.
    #[test]
    fn formats_slices() {
        let src = "fn f(s) = s[1:#s - 1] + s[:2] + s[3:] + s[:]\n";
        assert_eq!(fmt(src), src);
    }

    /// `#` opens a comment only when a space or the line's end follows it;
    /// against a name it is the length operator, and both survive the round
    /// trip on the same line.
    #[test]
    fn formats_length_and_comments() {
        let src = "# how long\nfn f(s) = #s + 1  # both on one line\n";
        assert_eq!(fmt(src), src);
        // A space after the `#` keeps it a comment, wherever it sits.
        assert_eq!(fmt("fn f(s) =\n  # why\n  #s\n"), "fn f(s) =\n  # why\n  #s\n");
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

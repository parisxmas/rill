//! Recursive-descent parser for the M1 subset of Rill.
//!
//! Grammar (layout tokens produced by the lexer):
//!
//! ```text
//! module  := Newline* ((fndef | typedef) sep)* Eof
//! typedef := "type" UpperIdent Indent ctor (Newline ctor)* Dedent
//! ctor    := UpperIdent ("(" field ("," field)* ")")?
//! field   := (Ident ":")? UpperIdent
//! fndef   := "fn" Ident "(" params? ")" ("->" UpperIdent)? "=" body
//! param   := Ident (":" UpperIdent)?
//! body    := expr                     -- inline, same line
//!          | Indent stmts Dedent      -- indented block
//! stmts   := stmt (Newline stmt)*
//! stmt    := Ident "=" expr | expr
//! expr    := pipe
//! pipe    := or ("|>" or)*            -- `x |> f(a)` desugars to `f(x, a)`
//! or      := and ("||" and)*          -- desugars to if (short-circuit)
//! and     := cmp ("&&" cmp)*          -- desugars to if (short-circuit)
//! cmp     := add (cmpop add)?
//! add     := mul (("+" | "-") mul)*
//! mul     := unary (("*" | "/" | "%") unary)*
//! unary   := "-" unary | primary
//! primary := Int | Float | Str | Ident ("(" args ")")? | "(" expr ")"
//!          | "\" Ident ("," Ident)* "->" expr
//!          | "if" expr "then" expr "else" expr
//!          | "match" expr Indent arm (Newline arm)* Dedent
//! arm     := pattern "->" expr
//! pattern := "_" | Ident | ("-")? Int
//!          | UpperIdent ("(" pattern ("," pattern)* ")")?   -- nests freely
//! ```

use crate::ast::*;
use crate::lexer::{lex, Tok, Token};
use crate::SyntaxError;

pub fn parse(src: &str) -> Result<Module, SyntaxError> {
    let toks = lex(src)?;
    Parser { toks, pos: 0 }.module()
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek2(&self) -> &Tok {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)].tok
    }

    fn here(&self) -> (usize, usize) {
        let t = &self.toks[self.pos];
        (t.line, t.col)
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: &Tok, ctx: &str) -> Result<(), SyntaxError> {
        if self.peek() == want {
            self.bump();
            Ok(())
        } else {
            let (line, col) = self.here();
            Err(SyntaxError::new(
                line,
                col,
                format!("expected {want} {ctx}, found {}", self.peek()),
            ))
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.bump();
        }
    }

    /// Lets a construct continue on the next line: if `want` is the next
    /// token once end-of-line markers are skipped, skip them. Nothing moves
    /// otherwise, so this cannot swallow the start of a new statement.
    /// Indentation changes are left alone — an indented continuation would
    /// unbalance the enclosing block.
    fn continues_with(&mut self, want: &Tok) -> bool {
        let mut j = self.pos;
        while matches!(self.toks[j].tok, Tok::Newline) {
            j += 1;
        }
        if self.toks[j].tok == *want {
            self.pos = j;
            true
        } else {
            false
        }
    }

    fn module(&mut self) -> Result<Module, SyntaxError> {
        let mut imports = Vec::new();
        let mut types = Vec::new();
        let mut traits = Vec::new();
        let mut impls = Vec::new();
        let mut externs = Vec::new();
        let mut functions = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Eof => break,
                Tok::Fn => functions.push(self.fndef(false)?),
                Tok::Export => {
                    self.bump();
                    functions.push(self.fndef(true)?);
                }
                Tok::Type => types.push(self.typedef()?),
                Tok::Trait => traits.push(self.traitdef()?),
                Tok::Impl => impls.push(self.impldef()?),
                Tok::Extern => externs.push(self.externdef()?),
                Tok::Import => {
                    let (line, col) = self.here();
                    self.bump();
                    match self.bump() {
                        Tok::Str(path) => {
                            let alias = match (self.peek(), self.peek2()) {
                                (Tok::Ident(kw), Tok::Ident(_)) if kw == "as" => {
                                    self.bump();
                                    Some(self.ident("a module alias after `as`")?)
                                }
                                _ => None,
                            };
                            imports.push(ImportDecl { path, alias, line });
                        }
                        other => {
                            return Err(SyntaxError::new(
                                line,
                                col,
                                format!(
                                    "expected a string path after `import`, found {other}"
                                ),
                            ));
                        }
                    }
                }
                other => {
                    let (line, col) = self.here();
                    return Err(SyntaxError::new(
                        line,
                        col,
                        format!("expected `fn`, `type` or `import` at top level, found {other}"),
                    ));
                }
            }
        }
        Ok(Module { imports, types, traits, impls, externs, functions, prelude_fns: 0 })
    }

    /// `extern ["lib"] fn name(p: T, ...) [-> T]`.
    fn externdef(&mut self) -> Result<ExternFn, SyntaxError> {
        let (line, col) = self.here();
        self.expect(&Tok::Extern, "to begin a foreign declaration")?;
        let lib = match self.peek() {
            Tok::Str(_) => match self.bump() {
                Tok::Str(l) => Some(l),
                _ => unreachable!(),
            },
            _ => None,
        };
        self.expect(&Tok::Fn, "after `extern`")?;
        let name = self.ident("the C function's name")?;
        self.expect(&Tok::LParen, "after the function name")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                let pname = self.ident("a parameter name")?;
                self.expect(&Tok::Colon, "— foreign parameters must be annotated")?;
                params.push((pname, self.type_ref("a parameter type")?));
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "to close the parameter list")?;
        let ret = if matches!(self.peek(), Tok::Arrow) {
            self.bump();
            Some(self.type_ref("a return type after `->`")?)
        } else {
            None
        };
        let _ = col;
        Ok(ExternFn { name, lib, params, ret, line })
    }

    /// `trait Name(a)` + indented method signatures `m(a, Int) -> Str`.
    fn traitdef(&mut self) -> Result<TraitDef, SyntaxError> {
        let (line, _) = self.here();
        self.expect(&Tok::Trait, "to begin a trait definition")?;
        let name = self.upper_ident("a trait name after `trait`")?;
        self.expect(&Tok::LParen, "after the trait name")?;
        let param = self.ident("the trait's type parameter")?;
        self.expect(&Tok::RParen, "to close the trait parameter")?;
        self.expect(&Tok::Indent, "— method signatures must be indented under `trait`")?;
        let mut methods = vec![self.method_sig()?];
        loop {
            match self.peek() {
                Tok::Newline => {
                    self.bump();
                    if matches!(self.peek(), Tok::Dedent) {
                        self.bump();
                        break;
                    }
                    methods.push(self.method_sig()?);
                }
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                other => {
                    let (line, col) = self.here();
                    return Err(SyntaxError::new(
                        line,
                        col,
                        format!("expected end of line after a method signature, found {other}"),
                    ));
                }
            }
        }
        Ok(TraitDef { name, param, methods, line })
    }

    fn method_sig(&mut self) -> Result<MethodSig, SyntaxError> {
        let (line, _) = self.here();
        let name = self.ident("a method name")?;
        self.expect(&Tok::LParen, "after the method name")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                params.push(self.type_ref("a parameter type")?);
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "to close the method's parameter list")?;
        self.expect(&Tok::Arrow, "before the method's return type")?;
        let ret = self.type_ref("a return type")?;
        Ok(MethodSig { name, params, ret, line })
    }

    /// `impl Name(Target)` + indented `fn` definitions.
    fn impldef(&mut self) -> Result<ImplDef, SyntaxError> {
        let (line, _) = self.here();
        self.expect(&Tok::Impl, "to begin an impl block")?;
        let trait_name = self.upper_ident("a trait name after `impl`")?;
        self.expect(&Tok::LParen, "after the trait name")?;
        let target = self.type_ref("the implementing type")?;
        self.expect(&Tok::RParen, "to close the impl target")?;
        self.expect(&Tok::Indent, "— method definitions must be indented under `impl`")?;
        let mut fns = vec![self.fndef(false)?];
        loop {
            match self.peek() {
                Tok::Newline => {
                    self.bump();
                    if matches!(self.peek(), Tok::Dedent) {
                        self.bump();
                        break;
                    }
                    fns.push(self.fndef(false)?);
                }
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                other => {
                    let (line, col) = self.here();
                    return Err(SyntaxError::new(
                        line,
                        col,
                        format!("expected end of line after an impl method, found {other}"),
                    ));
                }
            }
        }
        Ok(ImplDef { trait_name, target, fns, line })
    }

    fn ident(&mut self, ctx: &str) -> Result<String, SyntaxError> {
        let (line, col) = self.here();
        match self.bump() {
            Tok::Ident(name) => Ok(name),
            other => Err(SyntaxError::new(line, col, format!("expected {ctx}, found {other}"))),
        }
    }

    fn upper_ident(&mut self, ctx: &str) -> Result<String, SyntaxError> {
        let (line, col) = self.here();
        let name = self.ident(ctx)?;
        if !starts_upper(&name) {
            return Err(SyntaxError::new(
                line,
                col,
                format!("expected {ctx}; `{name}` must start with an uppercase letter"),
            ));
        }
        Ok(name)
    }

    /// A type reference: `Int`, `Shape`, `List(Int)`, or a lowercase type
    /// parameter like `a` (validated against the enclosing definition later).
    fn type_ref(&mut self, ctx: &str) -> Result<TypeRef, SyntaxError> {
        let (line, _) = self.here();
        let name = self.ident(ctx)?;
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::LParen) {
            self.bump();
            // An empty list is only meaningful for `Fn()`, a callback taking
            // nothing; the checker rejects it anywhere else.
            if !matches!(self.peek(), Tok::RParen) {
                loop {
                    args.push(self.type_ref("a type argument")?);
                    if matches!(self.peek(), Tok::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
            }
            self.expect(&Tok::RParen, "to close the type argument list")?;
        }
        // `Fn(I32) -> I32` — a callback that answers. The arrow binds to the
        // callback rather than to the enclosing signature, which is the only
        // reading available: a foreign function cannot return one, so an
        // `extern`'s own `->` never follows a `Fn(...)` written as a result.
        let mut ret = None;
        if name == "Fn" && matches!(self.peek(), Tok::Arrow) {
            self.bump();
            ret = Some(Box::new(self.type_ref("a callback's result type after `->`")?));
        }
        Ok(TypeRef { name, args, ret, line })
    }

    fn typedef(&mut self) -> Result<TypeDef, SyntaxError> {
        let (line, _) = self.here();
        self.expect(&Tok::Type, "to begin a type definition")?;
        let name = self.upper_ident("a type name after `type`")?;
        let mut params = Vec::new();
        if matches!(self.peek(), Tok::LParen) {
            self.bump();
            loop {
                let (pline, pcol) = self.here();
                let p = self.ident("a type parameter")?;
                if starts_upper(&p) {
                    return Err(SyntaxError::new(
                        pline,
                        pcol,
                        format!("type parameters must be lowercase (found `{p}`)"),
                    ));
                }
                params.push(p);
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect(&Tok::RParen, "to close the type parameter list")?;
        }
        self.expect(&Tok::Indent, "— constructors must be indented under `type`")?;
        let mut ctors = vec![self.ctor()?];
        loop {
            match self.peek() {
                Tok::Newline => {
                    self.bump();
                    if matches!(self.peek(), Tok::Dedent) {
                        self.bump();
                        break;
                    }
                    ctors.push(self.ctor()?);
                }
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                other => {
                    let (line, col) = self.here();
                    return Err(SyntaxError::new(
                        line,
                        col,
                        format!("expected end of line after a constructor, found {other}"),
                    ));
                }
            }
        }
        Ok(TypeDef { name, params, ctors, line })
    }

    fn ctor(&mut self) -> Result<CtorDef, SyntaxError> {
        let (line, _) = self.here();
        let name = self.upper_ident("a constructor name")?;
        let mut fields = Vec::new();
        if matches!(self.peek(), Tok::LParen) {
            self.bump();
            loop {
                // Either `name: Type` or a bare `Type` (which may be a
                // lowercase type parameter or an application like `List(a)`).
                let (fline, _) = self.here();
                let first = self.ident("a field")?;
                if matches!(self.peek(), Tok::Colon) {
                    self.bump();
                    let ty = self.type_ref("a field type after `:`")?;
                    fields.push((Some(first), ty));
                } else {
                    let mut args = Vec::new();
                    if matches!(self.peek(), Tok::LParen) {
                        self.bump();
                        loop {
                            args.push(self.type_ref("a type argument")?);
                            if matches!(self.peek(), Tok::Comma) {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                        self.expect(&Tok::RParen, "to close the type argument list")?;
                    }
                    fields.push((None, TypeRef { name: first, args, ret: None, line: fline }));
                }
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect(&Tok::RParen, "to close the constructor's field list")?;
        }
        Ok(CtorDef { name, fields, line })
    }

    fn fndef(&mut self, exported: bool) -> Result<FnDef, SyntaxError> {
        let (line, col) = self.here();
        self.expect(&Tok::Fn, "to begin a function definition")?;
        let name = self.ident("a function name after `fn`")?;
        if starts_upper(&name) {
            return Err(SyntaxError::new(
                line,
                col,
                format!("function names must start with a lowercase letter (found `{name}`)"),
            ));
        }
        self.expect(&Tok::LParen, "after the function name")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                let pname = self.ident("a parameter name")?;
                let ann = if matches!(self.peek(), Tok::Colon) {
                    self.bump();
                    Some(self.type_ref("a parameter type after `:`")?)
                } else {
                    None
                };
                params.push(Param { name: pname, ann });
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "to close the parameter list")?;
        let ret_ann = if matches!(self.peek(), Tok::Arrow) {
            self.bump();
            Some(self.type_ref("a return type after `->`")?)
        } else {
            None
        };
        self.expect(&Tok::Eq, "before the function body")?;
        let body = self.body()?;
        Ok(FnDef { name, exported, params, ret_ann, body, line, lifted_from: None })
    }

    /// Function body: inline expression, or an indented block of statements.
    fn body(&mut self) -> Result<Expr, SyntaxError> {
        if matches!(self.peek(), Tok::Indent) {
            self.bump();
            let mut stmts = vec![self.stmt()?];
            loop {
                match self.peek() {
                    Tok::Newline => {
                        self.bump();
                        if matches!(self.peek(), Tok::Dedent) {
                            self.bump();
                            break;
                        }
                        stmts.push(self.stmt()?);
                    }
                    Tok::Dedent => {
                        self.bump();
                        break;
                    }
                    other => {
                        let (line, col) = self.here();
                        return Err(SyntaxError::new(
                            line,
                            col,
                            format!("expected end of line in block, found {other}"),
                        ));
                    }
                }
            }
            Ok(Expr::Block(stmts))
        } else {
            self.expr()
        }
    }

    fn stmt(&mut self) -> Result<Stmt, SyntaxError> {
        if matches!(self.peek(), Tok::Fn) {
            return Ok(Stmt::Fn(Box::new(self.fndef(false)?)));
        }
        if matches!(self.peek(), Tok::For) {
            return Ok(Stmt::Expr(self.for_loop()?));
        }
        if let (Tok::Ident(_), Tok::Eq) = (self.peek(), self.peek2()) {
            let (line, _) = self.here();
            let name = self.ident("a name")?;
            self.bump(); // `=`
            let value = self.expr()?;
            Ok(Stmt::Bind { name, value, line })
        } else {
            // `b[i] = v` is the one assignment Rill has, and only to a buffer
            // element — which was already mutable through `buf_set`. Nothing
            // else can appear to the left of `=`.
            let e = self.expr()?;
            if let Tok::OpEq(c) = *self.peek() {
                let (line, col) = self.here();
                self.bump();
                let value = self.expr()?;
                let op = match c {
                    '+' => BinOp::Add,
                    '-' => BinOp::Sub,
                    '*' => BinOp::Mul,
                    _ => BinOp::Div,
                };
                return match e {
                    Expr::Index { base, index, line } => Ok(Stmt::Expr(Expr::IndexOp {
                        op,
                        base,
                        index,
                        value: Box::new(value),
                        line,
                    })),
                    _ => Err(SyntaxError::new(
                        line,
                        col,
                        format!("`{c}=` writes a buffer element: write `b[i] {c}= v`"),
                    )),
                };
            }
            if !matches!(self.peek(), Tok::Eq) {
                return Ok(Stmt::Expr(e));
            }
            let (line, col) = self.here();
            self.bump();
            let value = self.expr()?;
            match e {
                Expr::Index { base, index, line } => Ok(Stmt::Expr(Expr::IndexSet {
                    base,
                    index,
                    value: Box::new(value),
                    line,
                })),
                _ => Err(SyntaxError::new(
                    line,
                    col,
                    "only a buffer element can be assigned to: write `b[i] = v`".to_string(),
                )),
            }
        }
    }

    /// `for i in a..b`, over the half-open range.
    fn for_loop(&mut self) -> Result<Expr, SyntaxError> {
        let (line, col) = self.here();
        self.bump();
        let var = self.ident("a loop variable after `for`")?;
        match self.peek() {
            Tok::Ident(kw) if kw == "in" => {
                self.bump();
            }
            other => {
                return Err(SyntaxError::new(
                    line,
                    col,
                    format!("expected `in` after the loop variable, found {other}"),
                ))
            }
        }
        let from = self.expr()?;
        self.expect(&Tok::DotDot, "between the ends of the range")?;
        let to = self.expr()?;
        let body = self.body()?;
        Ok(Expr::For {
            var,
            from: Box::new(from),
            to: Box::new(to),
            body: Box::new(body),
            line,
        })
    }

    fn expr(&mut self) -> Result<Expr, SyntaxError> {
        self.pipe()
    }

    fn pipe(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.or_expr()?;
        while matches!(self.peek(), Tok::PipeGt) {
            let (line, col) = self.here();
            self.bump();
            let rhs = self.or_expr()?;
            lhs = match rhs {
                Expr::Var(name, line) => {
                    Expr::Call { name, args: vec![lhs], line, piped: true }
                }
                Expr::Call { name, mut args, line, .. } => {
                    args.insert(0, lhs);
                    Expr::Call { name, args, line, piped: true }
                }
                _ => {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "right side of `|>` must be a function name or call".to_string(),
                    ));
                }
            };
        }
        Ok(lhs)
    }

    /// `a || b` desugars to `if a then true else b` (short-circuit for free).
    fn or_expr(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.and_expr()?;
        while matches!(self.peek(), Tok::PipePipe) {
            self.bump();
            let rhs = self.and_expr()?;
            lhs = Expr::If {
                cond: Box::new(lhs),
                then: Box::new(Expr::Bool(true)),
                els: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    /// `a && b` desugars to `if a then b else false`.
    fn and_expr(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.cmp()?;
        while matches!(self.peek(), Tok::AmpAmp) {
            self.bump();
            let rhs = self.cmp()?;
            lhs = Expr::If {
                cond: Box::new(lhs),
                then: Box::new(rhs),
                els: Box::new(Expr::Bool(false)),
            };
        }
        Ok(lhs)
    }

    fn cmp(&mut self) -> Result<Expr, SyntaxError> {
        let lhs = self.add()?;
        let op = match self.peek() {
            Tok::EqEq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            _ => return Ok(lhs),
        };
        let (line, _) = self.here();
        self.bump();
        let rhs = self.add()?;
        Ok(Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line })
    }

    fn add(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(lhs),
            };
            let (line, _) = self.here();
            self.bump();
            let rhs = self.mul()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line };
        }
    }

    fn mul(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => return Ok(lhs),
            };
            let (line, _) = self.here();
            self.bump();
            let rhs = self.unary()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line };
        }
    }

    fn unary(&mut self) -> Result<Expr, SyntaxError> {
        if matches!(self.peek(), Tok::Minus) {
            self.bump();
            let expr = self.unary()?;
            Ok(Expr::Unary { op: UnOp::Neg, expr: Box::new(expr) })
        } else if matches!(self.peek(), Tok::Bang) {
            // `!e` desugars to `if e then false else true`.
            self.bump();
            let expr = self.unary()?;
            Ok(Expr::If {
                cond: Box::new(expr),
                then: Box::new(Expr::Bool(false)),
                els: Box::new(Expr::Bool(true)),
            })
        } else {
            self.postfix()
        }
    }

    /// `e[i]`, as many times as it is written. Binds tighter than anything
    /// else, so `b[i] + 1` and `b[i][j]` read the way they look.
    fn postfix(&mut self) -> Result<Expr, SyntaxError> {
        let mut base = self.primary()?;
        while matches!(self.peek(), Tok::LBracket) {
            let (line, _) = self.here();
            self.bump();
            let index = self.expr()?;
            self.expect(&Tok::RBracket, "to close the index")?;
            base = Expr::Index { base: Box::new(base), index: Box::new(index), line };
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, SyntaxError> {
        let (line, col) = self.here();
        match self.bump() {
            Tok::Int(v) => Ok(Expr::Int(v)),
            Tok::Float(v) => Ok(Expr::Float(v)),
            Tok::Str(v) => Ok(Expr::Str(v)),
            Tok::True => Ok(Expr::Bool(true)),
            Tok::False => Ok(Expr::Bool(false)),
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            args.push(self.expr()?);
                            if matches!(self.peek(), Tok::Comma) {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(&Tok::RParen, "to close the argument list")?;
                    Ok(Expr::Call { name, args, line, piped: false })
                } else {
                    Ok(Expr::Var(name, line))
                }
            }
            Tok::LParen => {
                let expr = self.expr()?;
                self.expect(&Tok::RParen, "to close the parenthesized expression")?;
                Ok(expr)
            }
            Tok::Spawn => {
                let body = self.expr()?;
                Ok(Expr::Spawn { body: Box::new(body), line })
            }
            Tok::Select => {
                let mut arms = Vec::new();
                let mut default = None;
                if matches!(self.peek(), Tok::Indent) {
                    self.bump();
                    loop {
                        self.select_arm(&mut arms, &mut default)?;
                        match self.peek() {
                            Tok::Newline => {
                                self.bump();
                                if matches!(self.peek(), Tok::Dedent) {
                                    self.bump();
                                    break;
                                }
                            }
                            Tok::Dedent => {
                                self.bump();
                                break;
                            }
                            other => {
                                let (line, col) = self.here();
                                return Err(SyntaxError::new(
                                    line,
                                    col,
                                    format!(
                                        "expected end of line after a select arm, found {other}"
                                    ),
                                ));
                            }
                        }
                    }
                } else {
                    // No layout inside parentheses: arms just follow on.
                    self.select_arm(&mut arms, &mut default)?;
                    loop {
                        let save = self.pos;
                        let (n_arms, had_default) = (arms.len(), default.is_some());
                        if self.select_arm(&mut arms, &mut default).is_err() {
                            arms.truncate(n_arms);
                            if !had_default {
                                default = None;
                            }
                            self.pos = save;
                            break;
                        }
                    }
                }
                if arms.is_empty() {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "`select` needs at least one channel arm".to_string(),
                    ));
                }
                Ok(Expr::Select { arms, default, line })
            }
            Tok::Backslash => {
                let mut params = vec![self.ident("a parameter name after `\\`")?];
                while matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    params.push(self.ident("a parameter name")?);
                }
                self.expect(&Tok::Arrow, "after the lambda parameters")?;
                let body = self.expr()?;
                Ok(Expr::Lambda { params, body: Box::new(body), line })
            }
            Tok::If => {
                let cond = self.expr()?;
                self.continues_with(&Tok::Then);
                self.expect(&Tok::Then, "after the `if` condition")?;
                // Both branches take an inline expression or an indented
                // block; `else if` chains stay flat because an `if` right
                // after `else` is just the expression form again.
                let then = self.body()?;
                self.continues_with(&Tok::Else);
                self.expect(&Tok::Else, "after the `then` branch")?;
                let els = self.body()?;
                Ok(Expr::If { cond: Box::new(cond), then: Box::new(then), els: Box::new(els) })
            }
            Tok::Match => {
                let scrutinee = self.expr()?;
                let mut arms = Vec::new();
                if matches!(self.peek(), Tok::Indent) {
                    self.bump();
                    arms.push(self.arm()?);
                    loop {
                        match self.peek() {
                            Tok::Newline => {
                                self.bump();
                                if matches!(self.peek(), Tok::Dedent) {
                                    self.bump();
                                    break;
                                }
                                arms.push(self.arm()?);
                            }
                            Tok::Dedent => {
                                self.bump();
                                break;
                            }
                            other => {
                                let (line, col) = self.here();
                                return Err(SyntaxError::new(
                                    line,
                                    col,
                                    format!(
                                        "expected end of line after a match arm, found {other}"
                                    ),
                                ));
                            }
                        }
                    }
                } else {
                    // Inside parentheses there is no layout, so arms simply
                    // follow one another. Nothing else in the grammar can
                    // continue an expression with `pattern ->`, so trying to
                    // parse one more arm is unambiguous.
                    arms.push(self.arm()?);
                    while let Some(a) = self.try_parse(|p| p.arm()) {
                        arms.push(a);
                    }
                }
                Ok(Expr::Match { scrutinee: Box::new(scrutinee), arms, line })
            }
            other => {
                Err(SyntaxError::new(line, col, format!("expected an expression, found {other}")))
            }
        }
    }

    /// `x = recv(ch) -> e`, `recv(ch) -> e`, `send(ch, v) -> e`, or
    /// `default -> e`.
    fn select_arm(
        &mut self,
        arms: &mut Vec<SelectArm>,
        default: &mut Option<Box<Expr>>,
    ) -> Result<(), SyntaxError> {
        let (line, col) = self.here();
        // `default -> e`
        if let Tok::Ident(name) = self.peek() {
            if name == "default" && matches!(self.peek2(), Tok::Arrow) {
                self.bump();
                self.bump();
                if default.is_some() {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "`select` can only have one `default` arm".to_string(),
                    ));
                }
                *default = Some(Box::new(self.expr()?));
                return Ok(());
            }
        }
        // Optional binding for a receive.
        let bind = match (self.peek(), self.peek2()) {
            (Tok::Ident(_), Tok::Eq) => {
                let n = self.ident("a name")?;
                self.bump();
                Some(n)
            }
            _ => None,
        };
        let (opline, opcol) = self.here();
        let call = self.primary()?;
        let op = match call {
            Expr::Call { ref name, ref args, .. } if name == "recv" && args.len() == 1 => {
                SelectOp::Recv { chan: args[0].clone(), bind }
            }
            Expr::Call { ref name, ref args, .. } if name == "send" && args.len() == 2 => {
                if bind.is_some() {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "a `send` arm produces no value to bind".to_string(),
                    ));
                }
                SelectOp::Send { chan: args[0].clone(), value: args[1].clone() }
            }
            _ => {
                return Err(SyntaxError::new(
                    opline,
                    opcol,
                    "each `select` arm must be `recv(ch)`, `send(ch, v)` or `default`"
                        .to_string(),
                ));
            }
        };
        self.expect(&Tok::Arrow, "after the select arm's channel operation")?;
        let body = self.body()?;
        arms.push(SelectArm { op, body, line });
        Ok(())
    }

    /// Runs a parse, rewinding completely if it fails. Used where the end of
    /// a repetition can only be found by trying to parse one more item.
    fn try_parse<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, SyntaxError>,
    ) -> Option<T> {
        let save = self.pos;
        match f(self) {
            Ok(v) => Some(v),
            Err(_) => {
                self.pos = save;
                None
            }
        }
    }

    fn arm(&mut self) -> Result<Arm, SyntaxError> {
        let (line, _) = self.here();
        let pattern = self.pattern()?;
        self.expect(&Tok::Arrow, "after the pattern in a match arm")?;
        // An arm's body is an inline expression, or — like a function's —
        // an indented block of statements.
        let body = self.body()?;
        Ok(Arm { pattern, body, line })
    }

    /// Patterns nest freely: `Some(Cons(0, rest))`.
    fn pattern(&mut self) -> Result<Pattern, SyntaxError> {
        let (line, col) = self.here();
        match self.bump() {
            Tok::Int(v) => Ok(Pattern::Int(v)),
            Tok::Minus => match self.bump() {
                Tok::Int(v) => Ok(Pattern::Int(-v)),
                other => Err(SyntaxError::new(
                    line,
                    col,
                    format!("expected an integer after `-` in a pattern, found {other}"),
                )),
            },
            Tok::Ident(name) if name == "_" => Ok(Pattern::Wildcard),
            Tok::Ident(name) if !starts_upper(&name) => Ok(Pattern::Var(name)),
            Tok::Ident(name) => {
                let mut args = Vec::new();
                if matches!(self.peek(), Tok::LParen) {
                    self.bump();
                    loop {
                        args.push(self.pattern()?);
                        if matches!(self.peek(), Tok::Comma) {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "to close the constructor pattern")?;
                }
                Ok(Pattern::Ctor { name, args, line })
            }
            other => Err(SyntaxError::new(line, col, format!("expected a pattern, found {other}"))),
        }
    }
}

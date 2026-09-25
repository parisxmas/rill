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
    Parser { toks, pos: 0, last_line: 0 }.module()
}

fn ordinal(n: usize) -> String {
    match n {
        1 => "first".into(),
        2 => "second".into(),
        3 => "third".into(),
        n => format!("{n}th"),
    }
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// A constructor's name, possibly behind a module alias: `Circle` or
/// `geo.Circle`. The lexer joins the alias and the dot to the name, so the
/// segment after the last dot is the one that says.
fn names_ctor(s: &str) -> bool {
    starts_upper(s.rsplit('.').next().unwrap_or(s))
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// The line of the token last taken, so that the next one can be seen
    /// to have begun a line: the formatter keeps such breaks.
    last_line: usize,
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

    /// Whether the token at hand is the first on its line.
    fn starts_line(&self) -> bool {
        self.toks[self.pos].line > self.last_line
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        self.last_line = self.toks[self.pos].line;
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
        let mut asms = Vec::new();
        let mut functions = Vec::new();
        let mut machines = Vec::new();
        let mut systems = Vec::new();
        let mut name = None;
        let mut name_line = 0;
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Eof => break,
                // `machine name(...)`: a state machine, a name rather than a
                // keyword, meaning this only here before a name and `(`.
                Tok::Ident(w) if w == "machine" && matches!(self.peek2(), Tok::Ident(_)) => {
                    let m = self.machinedef()?;
                    functions.extend(crate::machine::lower(&m)?);
                    machines.push(m);
                }
                // `system name`: the routes between machines, drawn and checked.
                Tok::Ident(w) if w == "system" && matches!(self.peek2(), Tok::Ident(_)) => {
                    systems.push(self.systemdef()?);
                }
                // `module name`: what the file is, said first. A name, not
                // a keyword, meaning this only here.
                Tok::Ident(w) if w == "module" && matches!(self.peek2(), Tok::Ident(_)) => {
                    let (line, col) = self.here();
                    let first = name.is_none()
                        && imports.is_empty()
                        && types.is_empty()
                        && traits.is_empty()
                        && impls.is_empty()
                        && externs.is_empty()
                        && asms.is_empty()
                        && functions.is_empty();
                    if !first {
                        return Err(SyntaxError::new(
                            line,
                            col,
                            "`module` names the file and comes first, before anything else in it"
                                .to_string(),
                        ));
                    }
                    self.bump();
                    name = Some(self.ident("the module's name")?);
                    name_line = line;
                }
                Tok::Fn => functions.push(self.fndef(false)?),
                // `private fn`: a function the file keeps to itself. A name
                // rather than a keyword, meaning this only here, before `fn`.
                Tok::Ident(w) if w == "private" && matches!(self.peek2(), Tok::Fn) => {
                    self.bump();
                    let mut f = self.fndef(false)?;
                    f.private = true;
                    functions.push(f);
                }
                // `private fast fn`: both.
                Tok::Ident(w)
                    if w == "private"
                        && matches!(self.peek2(), Tok::Ident(x) if x == "fast")
                        && matches!(self.toks.get(self.pos + 2).map(|t| &t.tok), Some(Tok::Fn)) =>
                {
                    self.bump();
                    self.bump();
                    let mut f = self.fndef(false)?;
                    f.private = true;
                    f.fast = true;
                    functions.push(f);
                }
                // `fast fn`: its float arithmetic may be reassociated. A name
                // rather than a keyword, like `private`, and only here.
                Tok::Ident(w) if w == "fast" && matches!(self.peek2(), Tok::Fn) => {
                    self.bump();
                    let mut f = self.fndef(false)?;
                    f.fast = true;
                    functions.push(f);
                }
                Tok::Export => {
                    self.bump();
                    functions.push(self.fndef(true)?);
                }
                Tok::Type => types.push(self.typedef()?),
                Tok::Trait => traits.push(self.traitdef()?),
                Tok::Impl => impls.push(self.impldef()?),
                Tok::Extern => externs.push(self.externdef()?),
                Tok::Asm => asms.push(self.asmdef(false)?),
                Tok::Naked => {
                    self.bump();
                    asms.push(self.asmdef(true)?);
                }
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
        Ok(Module { imports, types, traits, impls, externs, asms, functions, prelude_fns: 0, imported_fns: Vec::new(), name, name_line, machines, systems, imported_machines: Vec::new() })
    }

    /// `machine name(params)` and, indented under it, one `state` after
    /// another, each with its arms indented under it. See `MachineDef`.
    fn machinedef(&mut self) -> Result<MachineDef, SyntaxError> {
        let (line, col) = self.here();
        self.bump();
        let name = self.ident("a machine name after `machine`")?;
        if starts_upper(&name) {
            return Err(SyntaxError::new(
                line,
                col,
                format!("machine names must start with a lowercase letter (found `{name}`): a machine is a function, and its states are the constructors of a type declared with `type`"),
            ));
        }
        self.expect(&Tok::LParen, "after the machine name")?;
        let params = self.params()?;
        self.expect(&Tok::RParen, "to close the parameter list")?;
        if params.is_empty() {
            return Err(SyntaxError::new(line, col, "a machine takes at least one parameter: the channel its signals arrive on".to_string()));
        }
        self.expect(&Tok::Indent, "— a machine's states must be indented under `machine`")?;
        let mut states = Vec::new();
        loop {
            self.skip_newlines_only();
            let (sline, scol) = self.here();
            match self.peek() {
                Tok::Ident(w) if w == "state" => {
                    self.bump();
                    states.push(self.statedef(sline)?);
                }
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                Tok::Eof => break,
                other => {
                    return Err(SyntaxError::new(sline, scol, format!("expected `state` in a machine, found {other}")));
                }
            }
        }
        if states.is_empty() {
            return Err(SyntaxError::new(line, col, "a machine needs at least one `state`".to_string()));
        }
        Ok(MachineDef { name, params, states, line })
    }

    /// `system name`, and indented under it one route a line: `a -> b: S1,
    /// S2` or `a <-> b: S1`, the ends being machine names or `env` — and
    /// `block name`, with routes and blocks indented under it.
    fn systemdef(&mut self) -> Result<SystemDef, SyntaxError> {
        let (line, col) = self.here();
        self.bump();
        let name = self.ident("a system name after `system`")?;
        if starts_upper(&name) {
            return Err(SyntaxError::new(line, col, format!("system names are lowercase, like a machine's (found `{name}`)")));
        }
        self.expect(&Tok::Indent, "— a system's routes must be indented under `system`")?;
        let (routes, blocks, lists) = self.routes_and_blocks("a system", true)?;
        if routes.is_empty() && blocks.is_empty() {
            return Err(SyntaxError::new(line, col, "a system needs at least one route".to_string()));
        }
        Ok(SystemDef { name, lists, routes, blocks, line })
    }

    /// `block name` and, indented under it, its routes and blocks.
    fn blockdef(&mut self) -> Result<BlockDef, SyntaxError> {
        let (line, col) = self.here();
        self.bump();
        let name = self.ident("a block name after `block`")?;
        if starts_upper(&name) {
            return Err(SyntaxError::new(line, col, format!("block names are lowercase, like a system's (found `{name}`)")));
        }
        self.expect(&Tok::Indent, "— a block's routes must be indented under `block`")?;
        let (routes, blocks, _) = self.routes_and_blocks("a block", false)?;
        if routes.is_empty() && blocks.is_empty() {
            return Err(SyntaxError::new(line, col, "a block needs at least one route, or a block".to_string()));
        }
        Ok(BlockDef { name, routes, blocks, line })
    }

    /// The lines under a system or a block, to the dedent: routes and
    /// blocks, each on lines of its own — and in a system, signal lists:
    /// `signals Calls = OffHook, Digit`.
    fn routes_and_blocks(&mut self, what: &str, in_system: bool) -> Result<(Vec<Route>, Vec<BlockDef>, Vec<SignalList>), SyntaxError> {
        let mut routes = Vec::new();
        let mut blocks = Vec::new();
        let mut lists = Vec::new();
        loop {
            self.skip_newlines_only();
            let (rline, rcol) = self.here();
            match self.peek() {
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                Tok::Eof => break,
                Tok::Ident(w) if w == "block" && matches!(self.peek2(), Tok::Ident(_)) => {
                    blocks.push(self.blockdef()?);
                }
                Tok::Ident(w) if w == "signals" && matches!(self.peek2(), Tok::Ident(_)) => {
                    if !in_system {
                        return Err(SyntaxError::new(rline, rcol, "a signal list is the system's: write it under `system`, before the blocks".to_string()));
                    }
                    self.bump();
                    let name = self.ident("a signal list's name")?;
                    if !starts_upper(&name) {
                        return Err(SyntaxError::new(rline, rcol, format!("a signal list is named like a signal, capitalised (found `{name}`)")));
                    }
                    self.expect(&Tok::Eq, "— a signal list is `signals Name = Signal, Signal`")?;
                    let signals = self.signal_names(rline, rcol)?;
                    lists.push(SignalList { name, signals, line: rline });
                    self.end_of_route_line(rline, rcol, "a signal list")?;
                }
                Tok::Ident(_) => {
                    let from = self.ident("a route's source")?;
                    let (both, delay) = match self.peek() {
                        Tok::Arrow => {
                            self.bump();
                            (false, false)
                        }
                        Tok::WaveArrow => {
                            self.bump();
                            (false, true)
                        }
                        Tok::Lt => {
                            self.bump();
                            match self.peek() {
                                Tok::Arrow => {
                                    self.bump();
                                    (true, false)
                                }
                                Tok::WaveArrow => {
                                    self.bump();
                                    (true, true)
                                }
                                other => return Err(SyntaxError::new(rline, rcol, format!("expected `->` or `~>` after `<` — a route both ways is written `<->`, with delay `<~>` (found {other})"))),
                            }
                        }
                        other => return Err(SyntaxError::new(rline, rcol, format!("expected `->`, `<->`, `~>` or `<~>` after `{from}` in a route, found {other}"))),
                    };
                    let to = self.ident("a route's destination")?;
                    self.expect(&Tok::Colon, "before the route's signals")?;
                    let signals = self.signal_names(rline, rcol)?;
                    routes.push(Route { from, to, both, delay, signals, line: rline });
                    self.end_of_route_line(rline, rcol, "a route")?;
                }
                other => return Err(SyntaxError::new(rline, rcol, format!("expected a route in {what}, found {other}"))),
            }
        }
        Ok((routes, blocks, lists))
    }

    /// `Signal, Signal, ...`: constructors — or, on a route, the names of
    /// the system's signal lists, which are capitalised the same way.
    fn signal_names(&mut self, rline: usize, rcol: usize) -> Result<Vec<String>, SyntaxError> {
        let mut signals = Vec::new();
        loop {
            let sig = self.ident("a signal name")?;
            // `alias.Signal` is a constructor of an aliased import
            if !starts_upper(sig.rsplit('.').next().unwrap_or(&sig)) {
                return Err(SyntaxError::new(rline, rcol, format!("a route carries signals, which are constructors: `{sig}` is not one")));
            }
            signals.push(sig);
            if matches!(self.peek(), Tok::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(signals)
    }

    fn end_of_route_line(&mut self, rline: usize, rcol: usize, what: &str) -> Result<(), SyntaxError> {
        match self.peek() {
            Tok::Newline => {
                self.bump();
                Ok(())
            }
            Tok::Dedent | Tok::Eof => Ok(()),
            other => Err(SyntaxError::new(rline, rcol, format!("expected end of line after {what}, found {other}"))),
        }
    }

    /// Newlines between the states of a machine, which the layout gives as
    /// `Newline` tokens at the same depth.
    fn skip_newlines_only(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.bump();
        }
    }

    /// `state Ctor(fields)` and, indented under it, the arms: signal
    /// patterns like a match's, and `after ms -> body` at most once — and
    /// `state`s of its own, which make it a composite state.
    fn statedef(&mut self, line: usize) -> Result<StateDef, SyntaxError> {
        let (pl, pc) = self.here();
        // `state *`: the arms every state answers, after its own.
        let pattern = if matches!(self.peek(), Tok::Star) {
            self.bump();
            Pattern::Wildcard
        } else {
            self.pattern()?
        };
        if !matches!(pattern, Pattern::Ctor { .. } | Pattern::Wildcard) {
            return Err(SyntaxError::new(pl, pc, "a state is a constructor of the machine's state type, its fields named: `state Dialling(digits)` — or `*`, for every state".to_string()));
        }
        self.expect(&Tok::Indent, "— a state's arms must be indented under `state`")?;
        let mut arms = Vec::new();
        let mut after = None;
        let mut saves = Vec::new();
        let mut whens = Vec::new();
        let mut priorities = Vec::new();
        let mut substates = Vec::new();
        loop {
            let (aline, acol) = self.here();
            match self.peek() {
                // a state within the state: this one is a composite
                Tok::Ident(w) if w == "state" => {
                    if matches!(pattern, Pattern::Wildcard) {
                        return Err(SyntaxError::new(aline, acol, "`state *` is every state's arms; it has no states of its own".to_string()));
                    }
                    self.bump();
                    substates.push(self.statedef(aline)?);
                    // the substate took its own dedent; what follows is the next line at this depth, or the end
                    match self.peek() {
                        Tok::Newline => {
                            self.bump();
                        }
                        _ => {}
                    }
                    if matches!(self.peek(), Tok::Dedent) {
                        self.bump();
                        break;
                    }
                    if matches!(self.peek(), Tok::Eof) {
                        break;
                    }
                    continue;
                }
                // `priority Sig`: answered before what came earlier.
                Tok::Ident(w) if w == "priority" => {
                    self.bump();
                    let pattern = self.pattern()?;
                    let guard = if matches!(self.peek(), Tok::If) {
                        self.bump();
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    if matches!(self.peek(), Tok::Arrow) {
                        return Err(SyntaxError::new(aline, acol, "`priority` says which signal goes first; the arm that answers it is written as any other".to_string()));
                    }
                    priorities.push((pattern, guard, aline));
                }
                // `when cond -> body`: taken with no signal, when nothing
                // is waiting and the condition holds.
                Tok::Ident(w) if w == "when" => {
                    self.bump();
                    let cond = self.expr()?;
                    self.expect(&Tok::Arrow, "after the condition of a `when` arm")?;
                    let body = self.body()?;
                    whens.push((cond, body, aline));
                }
                // `save Sig`: kept in the mailbox for a later state.
                Tok::Ident(w) if w == "save" => {
                    self.bump();
                    let pattern = self.pattern()?;
                    let guard = if matches!(self.peek(), Tok::If) {
                        self.bump();
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    if matches!(self.peek(), Tok::Arrow) {
                        return Err(SyntaxError::new(aline, acol, "`save` keeps the signal for a later state and does nothing with it now, so it has no `->` and no body".to_string()));
                    }
                    saves.push((pattern, guard, aline));
                }
                Tok::Ident(w) if w == "after" => {
                    self.bump();
                    let ms = self.expr()?;
                    self.expect(&Tok::Arrow, "after the wait of an `after` arm")?;
                    let body = self.body()?;
                    if after.is_some() {
                        return Err(SyntaxError::new(aline, acol, "a state has one `after` arm: the one wait it keeps".to_string()));
                    }
                    after = Some((ms, body, aline));
                }
                _ => arms.push(self.arm()?),
            }
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
                Tok::Eof => break,
                other => {
                    let (l, c) = self.here();
                    return Err(SyntaxError::new(l, c, format!("expected end of line after a state's arm, found {other}")));
                }
            }
        }
        if arms.is_empty() && after.is_none() && whens.is_empty() && substates.is_empty() {
            return Err(SyntaxError::new(line, 0, "a state answers at least one signal, or an `after`".to_string()));
        }
        if let Some(sub) = substates.iter().find(|s| matches!(s.pattern, Pattern::Wildcard)) {
            return Err(SyntaxError::new(sub.line, 0, "`state *` is the machine's, not a composite state's: its arms are every state's already".to_string()));
        }
        Ok(StateDef { pattern, arms, after, saves, whens, priorities, substates, line })
    }

    /// The parameters of a function or a machine, between the parentheses.
    fn params(&mut self) -> Result<Vec<Param>, SyntaxError> {
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
        Ok(params)
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
        // `blocking`: a name meaning this only here, before `fn`.
        let blocking = matches!(self.peek(), Tok::Ident(w) if w == "blocking");
        if blocking {
            self.bump();
        }
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
        Ok(ExternFn { name, lib, blocking, params, ret, line })
    }

    /// `asm fn name(p: T, ...) [-> T] = "template" [: "constraints"]`.
    ///
    /// The parameter list is `extern`'s, for the same reason: what may cross
    /// into a machine instruction is what may cross into C.
    fn asmdef(&mut self, naked: bool) -> Result<AsmFn, SyntaxError> {
        let (line, col) = self.here();
        self.expect(&Tok::Asm, if naked { "after `naked`" } else { "to begin an assembly declaration" })?;
        self.expect(&Tok::Fn, "after `asm`")?;
        let name = self.ident("the function's name")?;
        self.expect(&Tok::LParen, "after the function name")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                let pname = self.ident("a parameter name")?;
                self.expect(&Tok::Colon, "— assembly parameters must be annotated")?;
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
        self.expect(&Tok::Eq, "before the instruction template")?;
        let template = match self.bump() {
            Tok::Str(t) => t,
            other => {
                return Err(SyntaxError::new(
                    line,
                    col,
                    format!("expected the instruction template as a string, found {other}"),
                ));
            }
        };
        let constraints = if matches!(self.peek(), Tok::Colon) {
            self.bump();
            match self.bump() {
                Tok::Str(c) => Some(c),
                other => {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        format!("expected the constraint string after `:`, found {other}"),
                    ));
                }
            }
        } else {
            None
        };
        Ok(AsmFn { name, params, ret, template, constraints, naked, line })
    }

    /// `trait Name(a)` + indented method signatures `m(a, Int) -> Str`.
    fn traitdef(&mut self) -> Result<TraitDef, SyntaxError> {
        let (line, _) = self.here();
        self.expect(&Tok::Trait, "to begin a trait definition")?;
        let name = self.upper_ident("a trait name after `trait`")?;
        self.expect(&Tok::LParen, "after the trait name")?;
        let param = self.ident("the trait's type parameter")?;
        self.expect(&Tok::RParen, "to close the trait parameter")?;
        // `: Show, Describe` — what a type must implement as well.
        let mut supers = Vec::new();
        if matches!(self.peek(), Tok::Colon) {
            self.bump();
            loop {
                supers.push(self.upper_ident("a trait name after `:`")?);
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
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
        Ok(TraitDef { name, param, methods, supers, line })
    }

    fn method_sig(&mut self) -> Result<MethodSig, SyntaxError> {
        let (line, _) = self.here();
        let name = self.ident("a method name")?;
        self.expect(&Tok::LParen, "after the method name")?;
        let mut params = Vec::new();
        let mut names = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                // `x: a` names the parameter, which a default body needs;
                // a bare type is what a signature has always said.
                let named = matches!((self.peek(), self.peek2()), (Tok::Ident(_), Tok::Colon));
                if named {
                    names.push(Some(self.ident("a parameter name")?));
                    self.bump();
                } else {
                    names.push(None);
                }
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
        let default = if matches!(self.peek(), Tok::Eq) {
            if let Some(i) = names.iter().position(|n| n.is_none()) {
                let (l, c) = self.here();
                return Err(SyntaxError::new(
                    l,
                    c,
                    format!(
                        "a default body needs its parameters named: the {} of `{name}` has \
                         no name to be written in terms of",
                        ordinal(i + 1)
                    ),
                ));
            }
            self.bump();
            Some(self.body()?)
        } else {
            None
        };
        Ok(MethodSig { name, params, names, ret, default, line })
    }

    /// `impl Name(Target)` + indented `fn` definitions.
    fn impldef(&mut self) -> Result<ImplDef, SyntaxError> {
        let (line, _) = self.here();
        self.expect(&Tok::Impl, "to begin an impl block")?;
        let trait_name = self.upper_ident("a trait name after `impl`")?;
        self.expect(&Tok::LParen, "after the trait name")?;
        let target = self.type_ref("the implementing type")?;
        self.expect(&Tok::RParen, "to close the impl target")?;
        // A trait whose every method has a default is implemented by saying
        // so and nothing else (2026-09-25); the checker names any method
        // that has no default and was left out.
        if !matches!(self.peek(), Tok::Indent) {
            return Ok(ImplDef { trait_name, target, fns: Vec::new(), line });
        }
        self.bump();
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
        let (line, col) = self.here();
        if matches!(self.peek(), Tok::LParen) {
            self.bump();
            let mut args = Vec::new();
            if !matches!(self.peek(), Tok::RParen) {
                args.push(self.type_ref(ctx)?);
                while matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    args.push(self.type_ref(ctx)?);
                }
            }
            self.expect(&Tok::RParen, "to close the tuple type")?;
            // `(A, B) -> C` is a function's type, and `()` before the arrow
            // takes nothing; without the arrow the parentheses are a tuple.
            if matches!(self.peek(), Tok::Arrow) {
                self.bump();
                let ret = self.type_ref("a function type's result after `->`")?;
                return Ok(TypeRef { name: "->".into(), args, ret: Some(Box::new(ret)), line });
            }
            let name = tuple_name(args.len(), line, col)?;
            return Ok(TypeRef { name, args, ret: None, line });
        }
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
        // `type Meters = Float`: a name for a type, with no constructors.
        if matches!(self.peek(), Tok::Eq) {
            self.bump();
            let target = self.type_ref("the type the alias names")?;
            return Ok(TypeDef { name, params, ctors: Vec::new(), alias: Some(target), line });
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
        Ok(TypeDef { name, params, ctors, alias: None, line })
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
        // `where Describe(a), Ord(b)`: what the type variables must implement.
        // Not a keyword — a library has a function called `where` — but a
        // name that means this in the one place it can stand, between a
        // signature and its `=`.
        let mut wheres = Vec::new();
        if matches!(self.peek(), Tok::Ident(w) if w == "where") {
            self.bump();
            loop {
                let (bline, _) = self.here();
                let trait_name = self.upper_ident("a trait name after `where`")?;
                self.expect(&Tok::LParen, "after the trait name in a `where`")?;
                let var = self.ident("the type variable the trait is required of")?;
                self.expect(&Tok::RParen, "to close the requirement")?;
                wheres.push(Bound { trait_name, var, line: bline });
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.expect(&Tok::Eq, "before the function body")?;
        let body = self.body()?;
        Ok(FnDef { name, exported, params, ret_ann, body, line, lifted_from: None, wheres, private: false, fast: false })
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
            // `fn put(m, k, v) = m[k] = v`: a write is a statement, and a
            // body written on one line is a block of one (2026-09-25).
            let e = self.expr()?;
            if !matches!(self.peek(), Tok::Eq | Tok::OpEq(_)) {
                return Ok(e);
            }
            match self.store_tail(e)? {
                Stmt::Expr(x) => Ok(x),
                _ => unreachable!("a write is an expression statement"),
            }
        }
    }

    fn stmt(&mut self) -> Result<Stmt, SyntaxError> {
        if matches!(self.peek(), Tok::Fn) {
            return Ok(Stmt::Fn(Box::new(self.fndef(false)?)));
        }
        // `Pair(a, b) = e` and `(a, b) = e` take a value apart. Only these two
        // shapes can start one, and only an `=` after the pattern settles it.
        let starts_pattern = matches!(self.peek(), Tok::LParen)
            || matches!(self.peek(), Tok::Ident(n) if names_ctor(n));
        if starts_pattern && self.has_binding_eq() {
            let (line, _) = self.here();
            let pattern = self.pattern()?;
            self.expect(&Tok::Eq, "after the pattern of a binding")?;
            let value = self.expr()?;
            return Ok(Stmt::PatBind { pattern, value, line });
        }
        if let (Tok::Ident(_), Tok::Eq) = (self.peek(), self.peek2()) {
            let (line, col) = self.here();
            let name = self.ident("a name")?;
            // `p.x = v` reads as a binding because the lexer joins a name to
            // what follows a dot, and it would quietly make a binding called
            // `p.x` rather than saying what it is: a record field, which is
            // part of a value and does not change.
            if let Some((base, field)) = name.split_once('.') {
                return Err(SyntaxError::new(
                    line,
                    col,
                    format!(
                        "`{name}` is a field of `{base}`, and a field cannot be assigned \
                         to — a value does not change. Build the one you want with the \
                         constructor, keeping the fields (`{field}` among them) it should \
                         keep"
                    ),
                ));
            }
            self.bump(); // `=`
            let value = self.expr()?;
            Ok(Stmt::Bind { name, value, line })
        } else {
            // `b[i] = v` is the one assignment Rill has, and only to a buffer
            // element — which was already mutable through `buf_set`. Nothing
            // else can appear to the left of `=`.
            let e = self.expr()?;
            self.store_tail(e)
        }
    }

    /// What follows an expression written as a statement: `= v` or `op= v`
    /// after `b[i]`, which make it a write, or nothing.
    fn store_tail(&mut self, e: Expr) -> Result<Stmt, SyntaxError> {
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
                // `b += s` on a name is an append, and only `+` is one.
                Expr::Var(n, line) if c == '+' => Ok(Stmt::Expr(Expr::Append {
                    target: Box::new(Expr::Var(n, line)),
                    value: Box::new(value),
                    line,
                })),
                _ => Err(SyntaxError::new(
                    line,
                    col,
                    format!(
                        "`{c}=` writes a buffer element: write `b[i] {c}= v` \
                         (or `b += s` to add to a string builder)"
                    ),
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
        // `with acc = init` makes the loop worth something: the body's value
        // is the next `acc`, and the loop is the last one.
        let acc = match self.peek() {
            Tok::Ident(kw) if kw == "with" => {
                self.bump();
                let name = self.ident("a name after `with`")?;
                self.expect(&Tok::Eq, "after the name of what the loop carries")?;
                Some((name, Box::new(self.expr()?)))
            }
            _ => None,
        };
        let body = self.body()?;
        Ok(Expr::For {
            var,
            from: Box::new(from),
            to: Box::new(to),
            acc,
            body: Box::new(body),
            line,
        })
    }

    /// `while cond`, with or without a state to carry.
    fn while_loop(&mut self) -> Result<Expr, SyntaxError> {
        let (line, _) = self.here();
        self.bump();
        let cond = self.expr()?;
        let acc = match self.peek() {
            Tok::Ident(kw) if kw == "with" => {
                self.bump();
                // A name, or a tuple of them — `with (lo, hi) = (0, n)` — each
                // annotated like a parameter where it has to be.
                let names = if matches!(self.peek(), Tok::LParen) {
                    self.bump();
                    let mut names = vec![self.state_name()?];
                    while matches!(self.peek(), Tok::Comma) {
                        self.bump();
                        names.push(self.state_name()?);
                    }
                    self.expect(&Tok::RParen, "to close what the loop carries")?;
                    if names.len() < 2 || names.len() > 8 {
                        return Err(SyntaxError::new(
                            line,
                            0,
                            format!("a loop carries two to eight things in a tuple, not {}", names.len()),
                        ));
                    }
                    names
                } else {
                    vec![self.state_name()?]
                };
                self.expect(&Tok::Eq, "after the name of what the loop carries")?;
                Some((names, Box::new(self.expr()?)))
            }
            _ => None,
        };
        let body = self.body()?;
        Ok(Expr::While { cond: Box::new(cond), acc, body: Box::new(body), line })
    }

    /// One thing a `while` carries: a name, and a type where the body could
    /// not tell it otherwise.
    fn state_name(&mut self) -> Result<Param, SyntaxError> {
        let name = self.ident("a name after `with`")?;
        let ann = if matches!(self.peek(), Tok::Colon) {
            self.bump();
            Some(self.type_ref("a type after `:`")?)
        } else {
            None
        };
        Ok(Param { name, ann })
    }

    fn expr(&mut self) -> Result<Expr, SyntaxError> {
        self.pipe()
    }

    fn pipe(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.or_expr()?;
        while matches!(self.peek(), Tok::PipeGt) {
            let (line, col) = self.here();
            let newline = self.starts_line();
            self.bump();
            let rhs = self.or_expr()?;
            lhs = match rhs {
                Expr::Var(name, line) => {
                    Expr::Call { name, args: vec![lhs], line, piped: true, stacked: newline }
                }
                Expr::Call { name, mut args, line, .. } => {
                    args.insert(0, lhs);
                    Expr::Call { name, args, line, piped: true, stacked: newline }
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
            let newline = self.starts_line();
            self.bump();
            let rhs = self.and_expr()?;
            lhs = Expr::If {
                cond: Box::new(lhs),
                then: Box::new(Expr::Bool(true)),
                els: Box::new(rhs),
                newline,
            };
        }
        Ok(lhs)
    }

    /// `a && b` desugars to `if a then b else false`.
    fn and_expr(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.cmp()?;
        while matches!(self.peek(), Tok::AmpAmp) {
            let newline = self.starts_line();
            self.bump();
            let rhs = self.cmp()?;
            lhs = Expr::If {
                cond: Box::new(lhs),
                then: Box::new(rhs),
                els: Box::new(Expr::Bool(false)),
                newline,
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
        let newline = self.starts_line();
        let (line, _) = self.here();
        self.bump();
        let rhs = self.add()?;
        Ok(Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line, newline })
    }

    fn add(&mut self) -> Result<Expr, SyntaxError> {
        let mut lhs = self.mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(lhs),
            };
            let newline = self.starts_line();
            let (line, _) = self.here();
            self.bump();
            let rhs = self.mul()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line, newline };
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
            let newline = self.starts_line();
            let (line, _) = self.here();
            self.bump();
            let rhs = self.unary()?;
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), line, newline };
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
                newline: false,
            })
        } else if matches!(self.peek(), Tok::Hash) {
            let (line, _) = self.here();
            self.bump();
            let expr = self.unary()?;
            Ok(Expr::Len { expr: Box::new(expr), line })
        } else {
            self.postfix()
        }
    }

    /// `e[i]` and `e?`, as many times as they are written. Both bind tighter
    /// than anything else, so `b[i] + 1`, `b[i][j]` and `f(x)? + 1` read the
    /// way they look.
    fn postfix(&mut self) -> Result<Expr, SyntaxError> {
        let mut base = self.primary()?;
        while matches!(self.peek(), Tok::LBracket | Tok::Question | Tok::Dot)
            || (matches!(self.peek(), Tok::LParen) && applicable(&base))
        {
            let (line, _) = self.here();
            // `add(1)(2)`, `fs[0](x)`: calling what an expression is worth.
            // Kept as a call to `()` with the callee in front — the lifter
            // names the callee and calls the name, which is all a call of a
            // function value ever is (2026-09-25).
            if matches!(self.peek(), Tok::LParen) {
                self.bump();
                let mut args = vec![base];
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
                base = Expr::Call { name: APPLY.to_string(), args, line, piped: false, stacked: false };
                continue;
            }
            if matches!(self.peek(), Tok::Question) {
                self.bump();
                base = Expr::Try { expr: Box::new(base), line };
                continue;
            }
            // A dot against a name was joined to it by the lexer, so one
            // reaching here follows something else: `f(x).y`, `v[0].name`.
            if matches!(self.peek(), Tok::Dot) {
                self.bump();
                let name = match self.peek().clone() {
                    Tok::Ident(n) => {
                        self.bump();
                        n
                    }
                    other => {
                        let (l, c) = self.here();
                        return Err(SyntaxError::new(
                            l,
                            c,
                            format!("expected a field name after `.`, found {other}"),
                        ));
                    }
                };
                base = Expr::Field { base: Box::new(base), name, line };
                continue;
            }
            self.bump();
            // `[i]` is one element; a `:` anywhere in it makes it a slice, and
            // either end may be left out for the string's own.
            let from = if matches!(self.peek(), Tok::Colon) {
                None
            } else {
                Some(Box::new(self.expr()?))
            };
            if !matches!(self.peek(), Tok::Colon) {
                self.expect(&Tok::RBracket, "to close the index")?;
                let index = from.expect("an index without a colon has a value");
                base = Expr::Index { base: Box::new(base), index, line };
                continue;
            }
            self.bump();
            let to = if matches!(self.peek(), Tok::RBracket) {
                None
            } else {
                Some(Box::new(self.expr()?))
            };
            self.expect(&Tok::RBracket, "to close the slice")?;
            base = Expr::Slice { base: Box::new(base), from, to, line };
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
                    let mut stacked = false;
                    if !matches!(self.peek(), Tok::RParen) {
                        loop {
                            stacked = stacked || self.starts_line();
                            args.push(self.expr()?);
                            if matches!(self.peek(), Tok::Comma) {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(&Tok::RParen, "to close the argument list")?;
                    Ok(Expr::Call { name, args, line, piped: false, stacked })
                } else {
                    Ok(Expr::Var(name, line))
                }
            }
            Tok::LBrace => {
                // `{ p | x = 9, y = 0 }`. The `|` reads as "except", which is
                // the one thing this notation says: everything the base had,
                // apart from these.
                let base = self.expr()?;
                self.expect(&Tok::Pipe, "after the value being updated")?;
                let mut fields = Vec::new();
                loop {
                    let (fline, fcol) = self.here();
                    let name = match self.bump() {
                        Tok::Ident(n) if !starts_upper(&n) => n,
                        other => {
                            return Err(SyntaxError::new(
                                fline,
                                fcol,
                                format!("expected a field name to update, found {other}"),
                            ))
                        }
                    };
                    self.expect(&Tok::Eq, "after the field name")?;
                    fields.push((name, self.expr()?, fline));
                    if matches!(self.peek(), Tok::Comma) {
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.expect(&Tok::RBrace, "to close the update")?;
                Ok(Expr::Update { base: Box::new(base), fields, line })
            }
            Tok::LParen => {
                if matches!(self.peek(), Tok::RParen) {
                    self.bump();
                    return Ok(Expr::Unit);
                }
                let mut stacked = self.starts_line();
                let first = self.expr()?;
                if !matches!(self.peek(), Tok::Comma) {
                    self.expect(&Tok::RParen, "to close the parenthesized expression")?;
                    return Ok(first);
                }
                // `(a, b)` is a tuple, which is the prelude's `Tuple2` under a
                // notation short enough to use for what it is for: handing
                // back more than one thing.
                let mut args = vec![first];
                while matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    stacked = stacked || self.starts_line();
                    args.push(self.expr()?);
                }
                self.expect(&Tok::RParen, "to close the tuple")?;
                let name = tuple_name(args.len(), line, col)?;
                Ok(Expr::Call { name, args, line, piped: false, stacked })
            }
            Tok::For => {
                self.pos -= 1;
                self.for_loop()
            }
            Tok::While => {
                self.pos -= 1;
                self.while_loop()
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
                // `\ -> e` takes nothing: a thunk, or a method of an object
                // that needs no argument beyond the value it closed over.
                let mut params = if matches!(self.peek(), Tok::Arrow) {
                    Vec::new()
                } else {
                    vec![self.ident("a parameter name after `\\`")?]
                };
                while matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    params.push(self.ident("a parameter name")?);
                }
                self.expect(&Tok::Arrow, "after the lambda parameters")?;
                // An inline expression, or an indented block on the lines
                // after the `->`, as a function body or an `if` branch is.
                let body = self.body()?;
                Ok(Expr::Lambda { params, body: Box::new(body), line })
            }
            Tok::If => {
                let cond = self.expr()?;
                self.continues_with(&Tok::Then);
                let then_nl = self.starts_line();
                self.expect(&Tok::Then, "after the `if` condition")?;
                // Both branches take an inline expression or an indented
                // block; `else if` chains stay flat because an `if` right
                // after `else` is just the expression form again.
                let then = self.body()?;
                self.continues_with(&Tok::Else);
                let else_nl = self.starts_line();
                self.expect(&Tok::Else, "after the `then` branch")?;
                let els = self.body()?;
                Ok(Expr::If {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    els: Box::new(els),
                    newline: then_nl || else_nl,
                })
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
            Expr::Call { ref name, ref args, .. } if name == "timeout" && args.len() == 1 => {
                if bind.is_some() {
                    return Err(SyntaxError::new(
                        line,
                        col,
                        "a `timeout` arm produces no value to bind".to_string(),
                    ));
                }
                SelectOp::Timeout { ms: args[0].clone() }
            }
            _ => {
                return Err(SyntaxError::new(
                    opline,
                    opcol,
                    "each `select` arm must be `recv(ch)`, `send(ch, v)`, `timeout(ms)` or `default`"
                        .to_string(),
                ));
            }
        };
        self.expect(&Tok::Arrow, "after the select arm's channel operation")?;
        let body = self.body()?;
        arms.push(SelectArm { op, body, line });
        Ok(())
    }

    /// Whether a plain `=` follows on this line, outside any brackets — which
    /// is what tells `Pair(a, b) = e` from an expression that merely starts
    /// with a constructor. `==` is a different token and does not count.
    fn has_binding_eq(&self) -> bool {
        let mut depth = 0usize;
        for t in &self.toks[self.pos..] {
            match t.tok {
                Tok::LParen | Tok::LBracket => depth += 1,
                Tok::RParen | Tok::RBracket => depth = depth.saturating_sub(1),
                Tok::Eq if depth == 0 => return true,
                Tok::Newline | Tok::Indent | Tok::Dedent | Tok::Eof => return false,
                _ => {}
            }
        }
        false
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
        let first = self.pattern()?;
        // `1 | 2 ->`. Only here, at the top of an arm: a pattern inside a
        // constructor is a field's, and `Cons(1 | 2, _)` would be asking one
        // arm to be two in a place where the expansion cannot be written back
        // out. What it buys is the common case — several constants, or several
        // constructors that take no arguments, going the same way.
        let pattern = if matches!(self.peek(), Tok::Pipe) {
            let mut alts = vec![first];
            while matches!(self.peek(), Tok::Pipe) {
                self.bump();
                alts.push(self.pattern()?);
            }
            Pattern::Or(alts, line)
        } else {
            first
        };
        // `pattern if cond ->` — the rest of what a pattern cannot say. The
        // condition is an ordinary expression, and `->` ends it, so nothing
        // here needs bracketing.
        let guard = if matches!(self.peek(), Tok::If) {
            self.bump();
            Some(self.expr()?)
        } else {
            None
        };
        self.expect(&Tok::Arrow, "after the pattern in a match arm")?;
        // An arm's body is an inline expression, or — like a function's —
        // an indented block of statements.
        let body = self.body()?;
        Ok(Arm { pattern, guard, body, line })
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
            Tok::Ident(name) if !names_ctor(&name) => {
                // `whole @ Circle(r)`: a name, and then the pattern it is the
                // whole of. Unlike `|` this nests, because it asks nothing of
                // the arm — it is one more binding at a position that is
                // already being matched.
                if matches!(self.peek(), Tok::At) {
                    self.bump();
                    let inner = self.pattern()?;
                    Ok(Pattern::As(name, Box::new(inner), line))
                } else {
                    Ok(Pattern::Var(name))
                }
            }
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
            Tok::LParen => {
                let mut args = vec![self.pattern()?];
                while matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    args.push(self.pattern()?);
                }
                self.expect(&Tok::RParen, "to close the tuple pattern")?;
                let name = tuple_name(args.len(), line, col)?;
                Ok(Pattern::Ctor { name, args, line })
            }
            other => Err(SyntaxError::new(line, col, format!("expected a pattern, found {other}"))),
        }
    }
}

/// The prelude type a tuple of this size is written with.
pub fn tuple_name(n: usize, line: usize, col: usize) -> Result<String, SyntaxError> {
    if (2..=8).contains(&n) {
        Ok(format!("Tuple{n}"))
    } else {
        Err(SyntaxError::new(line, col, format!("a tuple holds two to eight things, not {n}")))
    }
}

/// The name a call of an expression's value goes by until the lifter names
/// the value: `f(1)(2)` is `()(f(1), 2)`. Not a name anything can write.
pub const APPLY: &str = "()";

/// What a `(` straight after can call: the value of a call, an element, a
/// field of something computed, or of one of those.
fn applicable(e: &Expr) -> bool {
    match e {
        Expr::Call { name, .. } => !name.starts_with("Tuple"),
        Expr::Index { .. } | Expr::Field { .. } | Expr::Try { .. } => true,
        _ => false,
    }
}

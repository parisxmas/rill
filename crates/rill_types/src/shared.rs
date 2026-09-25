//! What strands share, checked for a `--parallel` build.
//!
//! Storage — a `Buf`, a `Map`, a `StrBuf`, or anything holding one — is the
//! one kind of value two strands can disagree about: a write from one under
//! a read or a write from another is a race, and on several workers a race
//! is what the machine makes of it. A channel is the way a value crosses
//! between strands, and a `spawn` captures what its body names; both are
//! hand-overs when the side that gave the value is done with it, and
//! sharing when it is not. Sharing to read is fine. Sharing and writing is
//! refused here, before the program runs — unless the program says the
//! writes are kept apart, `apart(x)`, which is a claim the compiler cannot
//! check and the reader can find.
//!
//! A function value is opaque: what it holds and writes is not in its type.
//! A strand that uses one the function around still uses is told to make
//! the claim too, since the compiler cannot tell a lambda that sums from
//! one that writes what it closed over.
//!
//! Three questions, answered over the checked program: which names a value
//! may be the storage of (`roots`: through bindings, fields, matches, and
//! calls that answer with a parameter); which parameters a function writes
//! or answers with (a fixpoint over every function, so the answer holds
//! through calls); and which names the function around a `spawn` or a
//! `send` still speaks afterwards (the rest of every block the statement is
//! in). Every answer errs on the side of a claim: an extern, a method, a
//! closure, or a runtime call that is handed storage is taken to write it.

use std::collections::{HashMap, HashSet};

use rill_syntax::ast::Pattern;

use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Share {
    No,
    Storage,
    Opaque,
}

/// Whether a value of this type is, or may hold, storage — or is a function
/// value, whose holdings are not in its type.
fn share_of(ty: &Ty, adts: &[AdtDef], seen: &mut Vec<usize>) -> Share {
    match ty {
        Ty::Buf(_) | Ty::Map(..) | Ty::StrBuf => Share::Storage,
        Ty::Fn(..) => Share::Opaque,
        Ty::Adt(i, args) => {
            if seen.contains(i) {
                return Share::No;
            }
            seen.push(*i);
            let mut out = Share::No;
            for c in &adts[*i].ctors {
                for f in &c.fields {
                    out = out.max(share_of(&subst_gen(f, args), adts, seen));
                }
            }
            seen.pop();
            out
        }
        _ => Share::No,
    }
}

type Aliases = HashMap<String, HashSet<String>>;

struct Pass<'a> {
    tmod: &'a TModule,
    /// For each function, which parameters its body writes, and which its
    /// answer may be the storage of.
    writes: Vec<Vec<bool>>,
    returns: Vec<Vec<bool>>,
    /// Names spoken in the rest of each block the statement being visited
    /// is in, innermost last.
    rest: Vec<HashSet<String>>,
    errors: Vec<TypeError>,
    report: bool,
}

/// Runtime calls that only read the storage they are handed.
const READS: &[&str] = &["rill_strbuf_len", "rill_buf_cmp", "rill_map_len", "rill_buf_check", "rill_buf_check_off"];

pub fn check_shared(tmod: &TModule) -> Result<(), Vec<TypeError>> {
    walk_program(tmod)
}

/// A ring: storage put into itself. `m[k] = v` where `v` holds `m` — a
/// closure over it, a record with it in — is the one shape reference
/// counting cannot let go of, and the one shape a program with immutable
/// values can make it: a value is built from what exists before it, so
/// nothing but storage can hold what holds it. The same for a channel
/// sent a value that holds it. Refused where it is written; a ring made
/// through two maps is not seen here, and is what the allocator's report
/// at exit is for.
pub fn check_rings(tmod: &TModule) -> Result<(), Vec<TypeError>> {
    Rings::check(tmod)
}

fn walk_program(tmod: &TModule) -> Result<(), Vec<TypeError>> {
    let n = tmod.fns.len();
    let mut pass = Pass {
        tmod,
        writes: tmod.fns.iter().map(|f| vec![false; f.params.len()]).collect(),
        returns: tmod.fns.iter().map(|f| vec![false; f.params.len()]).collect(),
        rest: Vec::new(),
        errors: Vec::new(),
        report: false,
    };
    // The fixpoint: a function writes a parameter it hands to a function
    // that writes that position, and answers with one it hands to a
    // function that answers with that position. Round until nothing new.
    loop {
        let mut changed = false;
        for i in 0..n {
            let f = &tmod.fns[i];
            let mut aliases: Aliases = HashMap::new();
            for (p, _) in &f.params {
                aliases.insert(p.clone(), HashSet::from([p.clone()]));
            }
            let written = pass.writes_of(&f.body, &aliases);
            let answered = pass.roots(&f.body, &aliases);
            for (k, (p, _)) in f.params.iter().enumerate() {
                if written.contains(p) && !pass.writes[i][k] {
                    pass.writes[i][k] = true;
                    changed = true;
                }
                if answered.contains(p) && !pass.returns[i][k] {
                    pass.returns[i][k] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    pass.report = true;
    for i in 0..n {
        let f = &tmod.fns[i];
        let mut aliases: Aliases = HashMap::new();
        for (p, _) in &f.params {
            aliases.insert(p.clone(), HashSet::from([p.clone()]));
        }
        pass.rest.clear();
        pass.visit(&f.body, &mut aliases);
    }
    if pass.errors.is_empty() {
        Ok(())
    } else {
        pass.errors.sort_by_key(|e| e.line);
        pass.errors.dedup_by(|a, b| a.line == b.line && a.msg == b.msg);
        Err(pass.errors)
    }
}

impl<'a> Pass<'a> {
    fn share(&self, ty: &Ty) -> Share {
        share_of(ty, &self.tmod.adts, &mut Vec::new())
    }

    fn is_prelude(&self, fn_idx: usize, name: &str) -> bool {
        fn_idx < self.tmod.prelude_fns && self.tmod.fns[fn_idx].name == name
    }

    /// The names whose storage this value may be, or hold.
    fn roots(&self, e: &TExpr, aliases: &Aliases) -> HashSet<String> {
        let mut out = HashSet::new();
        self.roots_into(e, aliases, &mut out);
        out
    }

    fn roots_into(&self, e: &TExpr, aliases: &Aliases, out: &mut HashSet<String>) {
        match &e.kind {
            TK::Local(n) => {
                out.insert(n.clone());
                if let Some(rs) = aliases.get(n) {
                    out.extend(rs.iter().cloned());
                }
            }
            TK::FieldGet { base, .. }
            | TK::BufGet { buf: base, .. }
            | TK::MapFind { map: base, .. }
            | TK::MapValAt { map: base, .. }
            | TK::MapKeyAt { map: base, .. }
            | TK::Slice { base, .. }
            | TK::Unique { val: base } => self.roots_into(base, aliases, out),
            TK::CallFn { fn_idx, args, .. } => {
                if self.is_prelude(*fn_idx, "apart") {
                    if let Some(a) = args.first() {
                        self.roots_into(a, aliases, out);
                    }
                    return;
                }
                for (i, a) in args.iter().enumerate() {
                    if self.returns[*fn_idx].get(i) == Some(&true) {
                        self.roots_into(a, aliases, out);
                    }
                }
            }
            TK::CallClosure { callee, args } => {
                self.roots_into(callee, aliases, out);
                for a in args {
                    if self.share(&a.ty) != Share::No {
                        self.roots_into(a, aliases, out);
                    }
                }
            }
            TK::MethodCall { args, .. } | TK::CallExtern { args, .. } | TK::CallCtor { args, .. } => {
                for a in args {
                    if self.share(&a.ty) != Share::No {
                        self.roots_into(a, aliases, out);
                    }
                }
            }
            TK::Lambda { captures, .. } => {
                for (n, _) in captures {
                    out.insert(n.clone());
                    if let Some(rs) = aliases.get(n) {
                        out.extend(rs.iter().cloned());
                    }
                }
            }
            TK::If { then, els, .. } => {
                self.roots_into(then, aliases, out);
                self.roots_into(els, aliases, out);
            }
            TK::Match { scrutinee, arms } => {
                let sr = self.roots(scrutinee, aliases);
                for arm in arms {
                    let mut inner = aliases.clone();
                    let mut bound = Vec::new();
                    pattern_names(&arm.pattern, &mut bound);
                    for b in bound {
                        inner.insert(b, sr.clone());
                    }
                    self.roots_into(&arm.body, &inner, out);
                }
            }
            TK::Block(stmts) => {
                let mut inner = aliases.clone();
                for s in stmts {
                    match s {
                        TStmt::Bind { name, value } => {
                            let rs = self.roots(value, &inner);
                            inner.insert(name.clone(), rs);
                        }
                        TStmt::Expr(x) => {
                            if std::ptr::eq(x, stmts.last().and_then(|l| match l {
                                TStmt::Expr(x) => Some(x),
                                _ => None,
                            }).unwrap_or(x)) && matches!(stmts.last(), Some(TStmt::Expr(_))) {
                                self.roots_into(x, &inner, out);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The names whose storage this expression writes, anywhere in it — a
    /// lambda's body included, since the lambda may be called.
    fn writes_of(&self, e: &TExpr, aliases: &Aliases) -> HashSet<String> {
        let mut out = HashSet::new();
        self.writes_into(e, aliases, &mut out);
        out
    }

    fn writes_into(&self, e: &TExpr, aliases: &Aliases, out: &mut HashSet<String>) {
        let storage_args = |args: &[TExpr], out: &mut HashSet<String>| {
            for a in args {
                if self.share(&a.ty) != Share::No {
                    self.roots_into(a, aliases, out);
                }
            }
        };
        match &e.kind {
            TK::BufSet { buf, .. } | TK::BufPtr { buf } => self.roots_into(buf, aliases, out),
            TK::MapSet { map, .. } | TK::MapDel { map, .. } | TK::MapClear { map } => {
                self.roots_into(map, aliases, out)
            }
            TK::Builtin { rt, args } => {
                if rt.starts_with("rill_") && !READS.contains(rt) {
                    storage_args(args, out);
                }
            }
            TK::CallExtern { args, .. } | TK::MethodCall { args, .. } => storage_args(args, out),
            TK::CallClosure { callee, args } => {
                self.roots_into(callee, aliases, out);
                storage_args(args, out);
            }
            TK::CallFn { fn_idx, args, .. } => {
                for (i, a) in args.iter().enumerate() {
                    if self.writes[*fn_idx].get(i) == Some(&true) {
                        self.roots_into(a, aliases, out);
                    }
                }
            }
            TK::Match { scrutinee, arms } => {
                self.writes_into(scrutinee, aliases, out);
                let sr = self.roots(scrutinee, aliases);
                for arm in arms {
                    let mut inner = aliases.clone();
                    let mut bound = Vec::new();
                    pattern_names(&arm.pattern, &mut bound);
                    for b in bound {
                        inner.insert(b, sr.clone());
                    }
                    if let Some(g) = &arm.guard {
                        self.writes_into(g, &inner, out);
                    }
                    self.writes_into(&arm.body, &inner, out);
                }
                return;
            }
            TK::Block(stmts) => {
                let mut inner = aliases.clone();
                for s in stmts {
                    match s {
                        TStmt::Bind { name, value } => {
                            self.writes_into(value, &inner, out);
                            let rs = self.roots(value, &inner);
                            inner.insert(name.clone(), rs);
                        }
                        TStmt::Expr(x) => self.writes_into(x, &inner, out),
                    }
                }
                return;
            }
            _ => {}
        }
        walk_texpr(e, &mut |c| self.writes_into(c, aliases, out));
    }

    /// Whether `apart(x)` is written anywhere in this expression for a
    /// value rooted at one of these names: the program's claim.
    fn vouched(&self, e: &TExpr, aliases: &Aliases, out: &mut HashSet<String>) {
        if let TK::CallFn { fn_idx, args, .. } = &e.kind {
            if self.is_prelude(*fn_idx, "apart") {
                if let Some(a) = args.first() {
                    self.roots_into(a, aliases, out);
                }
            }
        }
        walk_texpr(e, &mut |c| self.vouched(c, aliases, out));
    }

    /// Whether any name in the rest of the enclosing blocks is, or aliases,
    /// one of these roots.
    fn kept(&self, roots: &HashSet<String>, aliases: &Aliases) -> bool {
        self.rest.iter().flatten().any(|n| {
            roots.contains(n) || aliases.get(n).is_some_and(|rs| rs.iter().any(|r| roots.contains(r)))
        })
    }

    /// The walk that reports: every `spawn` and every `send`, against what
    /// the function around still uses afterwards.
    fn visit(&mut self, e: &TExpr, aliases: &mut Aliases) {
        match &e.kind {
            TK::Spawn { captures, body } => {
                let written = self.writes_of(body, aliases);
                let mut claimed = HashSet::new();
                self.vouched(body, aliases, &mut claimed);
                let mut used = HashSet::new();
                names_in(body, &mut used);
                for (n, ty) in captures {
                    let kind = self.share(ty);
                    if kind == Share::No {
                        continue;
                    }
                    let mut roots = HashSet::from([n.clone()]);
                    if let Some(rs) = aliases.get(n) {
                        roots.extend(rs.iter().cloned());
                    }
                    if claimed.iter().any(|c| roots.contains(c)) || !self.kept(&roots, aliases) {
                        continue;
                    }
                    let msg = match kind {
                        Share::Storage if written.iter().any(|w| roots.contains(w)) => format!(
                            "`{n}` is written by this strand and still used here after the `spawn`: \
                             two strands writing one piece of storage is a race. Hand it over — do \
                             not use it after the spawn — or send each strand its own, or, where \
                             the writes are kept apart on purpose, say so: `apart({n})`"
                        ),
                        Share::Opaque if used.contains(n) => format!(
                            "`{n}` is a function value, and what it holds and writes is not in its \
                             type: this strand uses it and the function around still does. If it \
                             writes nothing it holds, or its writes are kept apart, say so: \
                             `apart({n})`"
                        ),
                        _ => continue,
                    };
                    self.errors.push(TypeError::new(e.line, msg));
                }
                let mut inner = aliases.clone();
                self.visit(body, &mut inner);
                return;
            }
            TK::ChanSend { ch, val } => {
                self.visit(ch, aliases);
                self.visit(val, aliases);
                if self.share(&val.ty) != Share::No {
                    let roots = self.roots(val, aliases);
                    let mut claimed = HashSet::new();
                    self.vouched(val, aliases, &mut claimed);
                    if !roots.is_empty() && !claimed.iter().any(|c| roots.contains(c)) && self.kept(&roots, aliases) {
                        let n = roots.iter().min().cloned().unwrap_or_default();
                        self.errors.push(TypeError::new(
                            e.line,
                            format!(
                                "`{n}` is sent down a channel and still used here after: the strand \
                                 that receives it and this one would share it. Send a value that is \
                                 done with, or, where the two are kept apart on purpose, say so: \
                                 `apart({n})`"
                            ),
                        ));
                    }
                }
                return;
            }
            TK::Match { scrutinee, arms } => {
                self.visit(scrutinee, aliases);
                let sr = self.roots(scrutinee, aliases);
                for arm in arms {
                    let mut inner = aliases.clone();
                    let mut bound = Vec::new();
                    pattern_names(&arm.pattern, &mut bound);
                    for b in bound {
                        inner.insert(b, sr.clone());
                    }
                    if let Some(g) = &arm.guard {
                        self.visit(g, &mut inner);
                    }
                    self.visit(&arm.body, &mut inner);
                }
                return;
            }
            TK::Block(stmts) => {
                let mut inner = aliases.clone();
                for (i, s) in stmts.iter().enumerate() {
                    let mut after = HashSet::new();
                    for later in &stmts[i + 1..] {
                        match later {
                            TStmt::Bind { value, .. } => names_in(value, &mut after),
                            TStmt::Expr(x) => names_in(x, &mut after),
                        }
                    }
                    self.rest.push(after);
                    match s {
                        TStmt::Bind { name, value } => {
                            self.visit(value, &mut inner);
                            let rs = self.roots(value, &inner);
                            inner.insert(name.clone(), rs);
                        }
                        TStmt::Expr(x) => self.visit(x, &mut inner),
                    }
                    self.rest.pop();
                }
                return;
            }
            TK::Lambda { body, .. } => {
                let mut inner = aliases.clone();
                self.visit(body, &mut inner);
                return;
            }
            _ => {}
        }
        let mut children: Vec<&TExpr> = Vec::new();
        walk_texpr(e, &mut |c| children.push(c));
        for c in children {
            self.visit(c, aliases);
        }
    }
}

/// Where a value stands to the names around it, in three sets of names:
/// `eq` — names whose value this may be; `holds` — names whose value this
/// may be or reach; `inside` — names whose value this may be or be reached
/// from. A projection (`r.f`, `m[k]`, a match binding) is strictly inside
/// its base: it holds nothing the base does not, and it is not the base.
/// The sharing check above needs none of this — a strand writing a field
/// of `r` is writing `r` — but a ring does: `f.subs[k] = Sub(f.db)` puts
/// one field of `f` into another and holds nothing that holds `f`.
#[derive(Clone, Default, Debug)]
struct Reach {
    eq: HashSet<String>,
    holds: HashSet<String>,
    inside: HashSet<String>,
}

impl Reach {
    fn union(&mut self, o: &Reach) {
        self.eq.extend(o.eq.iter().cloned());
        self.holds.extend(o.holds.iter().cloned());
        self.inside.extend(o.inside.iter().cloned());
    }

    /// A value strictly inside this one.
    fn part(&self) -> Reach {
        let strict: HashSet<String> = self.holds.difference(&self.eq).cloned().collect();
        Reach { eq: HashSet::new(), holds: strict.clone(), inside: self.inside.union(&strict).cloned().collect() }
    }

    /// A fresh value built holding this one.
    fn held(&self) -> Reach {
        Reach { eq: HashSet::new(), holds: self.holds.clone(), inside: HashSet::new() }
    }
}

type Scope = HashMap<String, Reach>;

/// Per parameter of a function: whether its answer may be the parameter,
/// reach it, or be reached from it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Answer {
    eq: bool,
    holds: bool,
    inside: bool,
}

struct Rings<'a> {
    tmod: &'a TModule,
    answers: Vec<Vec<Answer>>,
    errors: Vec<TypeError>,
}

impl<'a> Rings<'a> {
    /// `m[k] = v` with `v` holding `m`, or `send(ch, v)` with `v` holding
    /// `ch`: a ring, refused. `v` reaches `m` when some name's value is
    /// reached from `v` and reaches `m`.
    fn check(tmod: &TModule) -> Result<(), Vec<TypeError>> {
        let n = tmod.fns.len();
        let mut pass = Rings { tmod, answers: tmod.fns.iter().map(|f| vec![Answer::default(); f.params.len()]).collect(), errors: Vec::new() };
        loop {
            let mut changed = false;
            for i in 0..n {
                let f = &tmod.fns[i];
                let r = pass.reach(&f.body, &pass.params_of(f));
                for (k, (p, _)) in f.params.iter().enumerate() {
                    let a = Answer { eq: r.eq.contains(p), holds: r.holds.contains(p), inside: r.inside.contains(p) };
                    let cur = &mut pass.answers[i][k];
                    let next = Answer { eq: cur.eq || a.eq, holds: cur.holds || a.holds, inside: cur.inside || a.inside };
                    if next != *cur {
                        *cur = next;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for f in &tmod.fns {
            let scope = pass.params_of(f);
            pass.visit(&f.body, &scope);
        }
        if pass.errors.is_empty() {
            Ok(())
        } else {
            pass.errors.sort_by_key(|e| e.line);
            pass.errors.dedup_by(|a, b| a.line == b.line && a.msg == b.msg);
            Err(pass.errors)
        }
    }

    fn params_of(&self, f: &TFn) -> Scope {
        f.params.iter().map(|(p, _)| (p.clone(), Self::name(p, &Scope::new()))).collect()
    }

    fn name(n: &str, scope: &Scope) -> Reach {
        let mut r = scope.get(n).cloned().unwrap_or_default();
        r.eq.insert(n.to_string());
        r.holds.insert(n.to_string());
        r.inside.insert(n.to_string());
        r
    }

    fn share(&self, ty: &Ty) -> Share {
        share_of(ty, &self.tmod.adts, &mut Vec::new())
    }

    fn is_prelude(&self, fn_idx: usize, name: &str) -> bool {
        fn_idx < self.tmod.prelude_fns && self.tmod.fns[fn_idx].name == name
    }

    /// `n`, `n.links`, `n.links.count`: a chain of fields read from a name,
    /// spelled as the program spells it.
    fn path_of(&self, e: &TExpr) -> Option<String> {
        match &e.kind {
            TK::Local(n) => Some(n.clone()),
            TK::FieldGet { base, adt, ctor, index } => {
                let field = self.tmod.adts[*adt].ctors[*ctor].names.get(*index).cloned().flatten()?;
                self.path_of(base).map(|p| format!("{p}.{field}"))
            }
            _ => None,
        }
    }

    /// The names an arm's pattern binds, each with what it stands to the
    /// scrutinee: the whole of it for a bare name, a part of it otherwise.
    fn bind_pattern(&self, p: &Pattern, s: &Reach, scope: &mut Scope) {
        match p {
            Pattern::Var(n) => {
                scope.insert(n.clone(), s.clone());
            }
            Pattern::As(n, inner, _) => {
                scope.insert(n.clone(), s.clone());
                self.bind_pattern(inner, s, scope);
            }
            Pattern::Ctor { args, .. } => {
                let part = s.part();
                let mut names = Vec::new();
                for a in args {
                    pattern_names(a, &mut names);
                }
                for n in names {
                    scope.insert(n, part.clone());
                }
            }
            Pattern::Or(alts, _) => {
                for a in alts {
                    self.bind_pattern(a, s, scope);
                }
            }
            Pattern::Wildcard | Pattern::Int(_) => {}
        }
    }

    fn reach(&self, e: &TExpr, scope: &Scope) -> Reach {
        let mut out = Reach::default();
        self.reach_into(e, scope, &mut out);
        out
    }

    fn reach_into(&self, e: &TExpr, scope: &Scope, out: &mut Reach) {
        match &e.kind {
            TK::Local(n) => out.union(&Self::name(n, scope)),
            TK::FieldGet { base, .. } => {
                // A field of a named value is a name of its own — `n.links`
                // — so that `n.links[k] = Node(n.links)` is seen to be the
                // one storage put into itself, though nothing else names it.
                out.union(&self.reach(base, scope).part());
                if let Some(path) = self.path_of(e) {
                    out.union(&Self::name(&path, scope));
                }
            }
            TK::BufGet { buf: base, .. }
            | TK::MapFind { map: base, .. }
            | TK::MapValAt { map: base, .. }
            | TK::MapKeyAt { map: base, .. }
            | TK::Slice { base, .. } => out.union(&self.reach(base, scope).part()),
            TK::Unique { val } => self.reach_into(val, scope, out),
            TK::CallFn { fn_idx, args, .. } => {
                if self.is_prelude(*fn_idx, "apart") {
                    if let Some(a) = args.first() {
                        self.reach_into(a, scope, out);
                    }
                    return;
                }
                for (i, a) in args.iter().enumerate() {
                    let Some(ans) = self.answers[*fn_idx].get(i).copied() else { continue };
                    if !(ans.eq || ans.holds || ans.inside) {
                        continue;
                    }
                    let r = self.reach(a, scope);
                    if ans.eq {
                        out.union(&r);
                    }
                    if ans.holds {
                        out.union(&r.held());
                    }
                    if ans.inside && !ans.eq {
                        out.union(&r.part());
                    }
                }
            }
            TK::CallClosure { callee, args } => {
                // What a closure answers with is not in its type: anything
                // it holds, or any argument, may come back.
                out.union(&self.reach(callee, scope).held());
                for a in args {
                    if self.share(&a.ty) != Share::No {
                        out.union(&self.reach(a, scope));
                    }
                }
            }
            TK::MethodCall { args, .. } | TK::CallExtern { args, .. } => {
                for a in args {
                    if self.share(&a.ty) != Share::No {
                        out.union(&self.reach(a, scope));
                    }
                }
            }
            TK::CallCtor { args, .. } => {
                for a in args {
                    if self.share(&a.ty) != Share::No {
                        out.union(&self.reach(a, scope).held());
                    }
                }
            }
            TK::Lambda { captures, .. } => {
                for (n, _) in captures {
                    out.union(&Self::name(n, scope).held());
                }
            }
            TK::If { then, els, .. } => {
                self.reach_into(then, scope, out);
                self.reach_into(els, scope, out);
            }
            TK::Match { scrutinee, arms } => {
                let sr = self.reach(scrutinee, scope);
                for arm in arms {
                    let mut inner = scope.clone();
                    self.bind_pattern(&arm.pattern, &sr, &mut inner);
                    self.reach_into(&arm.body, &inner, out);
                }
            }
            TK::Block(stmts) => {
                let mut inner = scope.clone();
                let last = stmts.len().saturating_sub(1);
                for (i, s) in stmts.iter().enumerate() {
                    match s {
                        TStmt::Bind { name, value } => {
                            let r = self.reach(value, &inner);
                            inner.insert(name.clone(), r);
                        }
                        TStmt::Expr(x) if i == last => self.reach_into(x, &inner, out),
                        TStmt::Expr(_) => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn visit(&mut self, e: &TExpr, scope: &Scope) {
        let (holder, val, what) = match &e.kind {
            TK::MapSet { map, val, .. } => (Some(map), Some(val), "map"),
            TK::ChanSend { ch, val } => (Some(ch), Some(val), "channel"),
            _ => (None, None, ""),
        };
        if let (Some(holder), Some(val)) = (holder, val) {
            if self.share(&val.ty) != Share::No {
                let into = self.reach(holder, scope).inside;
                let held = self.reach(val, scope).holds;
                if let Some(n) = into.iter().filter(|r| held.contains(*r)).min() {
                    let how = if what == "map" {
                        "putting into it a value that holds it — a closure over it, or a record with it in"
                    } else {
                        "sending down it a value that holds it"
                    };
                    self.errors.push(TypeError::new(
                        e.line,
                        format!(
                            "`{n}` would hold itself: this is {how}, and a count never lets go of a ring, \
                             so the {what} and what it holds would live until the program ends. Keep \
                             `{n}` out of what goes into it — hand it in as an argument instead"
                        ),
                    ));
                }
            }
        }
        match &e.kind {
            TK::Match { scrutinee, arms } => {
                self.visit(scrutinee, scope);
                let sr = self.reach(scrutinee, scope);
                for arm in arms {
                    let mut inner = scope.clone();
                    self.bind_pattern(&arm.pattern, &sr, &mut inner);
                    if let Some(g) = &arm.guard {
                        self.visit(g, &inner);
                    }
                    self.visit(&arm.body, &inner);
                }
            }
            TK::Block(stmts) => {
                let mut inner = scope.clone();
                for s in stmts {
                    match s {
                        TStmt::Bind { name, value } => {
                            self.visit(value, &inner);
                            let r = self.reach(value, &inner);
                            inner.insert(name.clone(), r);
                        }
                        TStmt::Expr(x) => self.visit(x, &inner),
                    }
                }
            }
            TK::Lambda { body, .. } | TK::Spawn { body, .. } => self.visit(body, &scope.clone()),
            _ => {
                let mut children: Vec<&TExpr> = Vec::new();
                walk_texpr(e, &mut |c| children.push(c));
                for c in children {
                    self.visit(c, scope);
                }
            }
        }
    }
}

/// Every name an expression speaks, its lambdas' captures included.
fn names_in(e: &TExpr, out: &mut HashSet<String>) {
    match &e.kind {
        TK::Local(n) => {
            out.insert(n.clone());
        }
        TK::Lambda { captures, .. } => {
            for (n, _) in captures {
                out.insert(n.clone());
            }
        }
        TK::Spawn { captures, .. } => {
            for (n, _) in captures {
                out.insert(n.clone());
            }
        }
        _ => {}
    }
    walk_texpr(e, &mut |c| names_in(c, out));
}

fn pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Var(n) => out.push(n.clone()),
        Pattern::As(n, inner, _) => {
            out.push(n.clone());
            pattern_names(inner, out);
        }
        Pattern::Ctor { args, .. } => {
            for a in args {
                pattern_names(a, out);
            }
        }
        Pattern::Or(alts, _) => {
            for a in alts {
                pattern_names(a, out);
            }
        }
        Pattern::Wildcard | Pattern::Int(_) => {}
    }
}

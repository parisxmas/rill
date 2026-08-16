//! Local functions, lifted out.
//!
//! `fn go(i) = ...` written inside a body sees the names around it and may
//! call itself and the functions beside it. Nothing after this pass knows
//! that: each one becomes an ordinary top-level function whose captured names
//! arrive as leading parameters, and every call to it gains those arguments.
//!
//! A closure would have been the other way to do it, and would have cost an
//! environment box and turned a self tail call into a call and a return — and
//! a loop written as a local function has to stay a loop. Lifting keeps the
//! tail call a tail call and adds no allocation at all.
//!
//! The lifted name carries the enclosing function's, separated by a dot no
//! Rill identifier can contain, and `lifted_from` records the parent so the
//! checker can infer the two together: a captured buffer must have the type it
//! had where it was captured, since a `Buf` width is never inferred.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::SyntaxError;

pub fn lift(module: &mut Module) -> Result<(), SyntaxError> {
    let mut used: HashSet<String> = module.functions.iter().map(|f| f.name.clone()).collect();
    let mut lifted: Vec<FnDef> = Vec::new();
    for f in &mut module.functions {
        let mut env: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
        let body = std::mem::replace(&mut f.body, Expr::Bool(false));
        f.body = walk(body, &mut env, &f.name, &mut used, &mut lifted)?;
    }
    module.functions.extend(lifted);
    Ok(())
}

/// One local function on its way out: what it was called, what it is called
/// now, and the names it takes from around it.
struct Local {
    name: String,
    lifted: String,
    caps: Vec<String>,
}

/// A value that can be written twice for nothing: a name or a literal. Naming
/// one anyway would cost a binding, and a binding of a counted value costs a
/// retain and a release — which is the whole difference in a hot loop.
fn is_simple(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Var(..) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_)
    )
}

/// `b[i] op= v` spelled out. The base and the index are named first unless
/// they already are: either may be a call, and running one twice would run
/// what it does twice.
fn expand_index_op(op: BinOp, base: Expr, index: Expr, value: Expr, line: usize) -> Expr {
    let mut binds = Vec::new();
    let mut once = |e: Expr, n: &str, binds: &mut Vec<Stmt>| {
        if is_simple(&e) {
            e
        } else {
            binds.push(Stmt::Bind { name: n.to_string(), value: e, line });
            Expr::Var(n.to_string(), line)
        }
    };
    let b = once(base, "[]=.b", &mut binds);
    let i = once(index, "[]=.i", &mut binds);
    let set = Expr::IndexSet {
        base: Box::new(b.clone()),
        index: Box::new(i.clone()),
        value: Box::new(Expr::Binary {
            op,
            lhs: Box::new(Expr::Index { base: Box::new(b), index: Box::new(i), line }),
            rhs: Box::new(value),
            line,
        }),
        line,
    };
    if binds.is_empty() {
        return set;
    }
    binds.push(Stmt::Expr(set));
    Expr::Block(binds)
}

/// `for i in a..b` is a local function over the half-open range, which is all
/// a counting loop ever was. The end is evaluated once.
fn expand_for(var: String, from: Expr, to: Expr, body: Expr, line: usize) -> Expr {
    let hi = "for.hi".to_string();
    let step = Expr::Call {
        name: "for".to_string(),
        args: vec![Expr::Binary {
            op: BinOp::Add,
            lhs: Box::new(Expr::Var(var.clone(), line)),
            rhs: Box::new(Expr::Int(1)),
            line,
        }],
        line,
        piped: false,
    };
    let loop_fn = FnDef {
        name: "for".to_string(),
        exported: false,
        params: vec![Param { name: var.clone(), ann: None }],
        ret_ann: None,
        body: Expr::If {
            cond: Box::new(Expr::Binary {
                op: BinOp::Ge,
                lhs: Box::new(Expr::Var(var, line)),
                rhs: Box::new(Expr::Var(hi.clone(), line)),
                line,
            }),
            then: Box::new(Expr::Int(0)),
            els: Box::new(Expr::Block(vec![Stmt::Expr(body), Stmt::Expr(step)])),
        },
        line,
        lifted_from: None,
    };
    Expr::Block(vec![
        Stmt::Bind { name: hi, value: to, line },
        Stmt::Fn(Box::new(loop_fn)),
        Stmt::Expr(Expr::Call { name: "for".to_string(), args: vec![from], line, piped: false }),
    ])
}

fn walk(
    e: Expr,
    env: &mut Vec<String>,
    parent: &str,
    used: &mut HashSet<String>,
    out: &mut Vec<FnDef>,
) -> Result<Expr, SyntaxError> {
    Ok(match e {
        Expr::IndexOp { op, base, index, value, line } => {
            walk(expand_index_op(op, *base, *index, *value, line), env, parent, used, out)?
        }
        Expr::Append { target, value, line } => Expr::Append {
            target: Box::new(walk(*target, env, parent, used, out)?),
            value: Box::new(walk(*value, env, parent, used, out)?),
            line,
        },
        Expr::Slice { base, from, to, line } => Expr::Slice {
            base: Box::new(walk(*base, env, parent, used, out)?),
            from: match from {
                Some(f) => Some(Box::new(walk(*f, env, parent, used, out)?)),
                None => None,
            },
            to: match to {
                Some(t) => Some(Box::new(walk(*t, env, parent, used, out)?)),
                None => None,
            },
            line,
        },
        Expr::For { var, from, to, body, line } => {
            walk(expand_for(var, *from, *to, *body, line), env, parent, used, out)?
        }
        Expr::Block(stmts) => {
            Expr::Block(walk_block(expand_pat_binds(stmts), env, parent, used, out)?)
        }
        Expr::Call { name, args, line, piped } => Expr::Call {
            name,
            args: walk_all(args, env, parent, used, out)?,
            line,
            piped,
        },
        Expr::Unary { op, expr } => {
            Expr::Unary { op, expr: Box::new(walk(*expr, env, parent, used, out)?) }
        }
        Expr::Binary { op, lhs, rhs, line } => Expr::Binary {
            op,
            lhs: Box::new(walk(*lhs, env, parent, used, out)?),
            rhs: Box::new(walk(*rhs, env, parent, used, out)?),
            line,
        },
        Expr::If { cond, then, els } => Expr::If {
            cond: Box::new(walk(*cond, env, parent, used, out)?),
            then: Box::new(walk(*then, env, parent, used, out)?),
            els: Box::new(walk(*els, env, parent, used, out)?),
        },
        Expr::Match { scrutinee, arms, line } => {
            let scrutinee = Box::new(walk(*scrutinee, env, parent, used, out)?);
            let mut new = Vec::with_capacity(arms.len());
            for a in arms {
                let depth = env.len();
                pattern_names(&a.pattern, env);
                let body = walk(a.body, env, parent, used, out)?;
                env.truncate(depth);
                new.push(Arm { pattern: a.pattern, body, line: a.line });
            }
            Expr::Match { scrutinee, arms: new, line }
        }
        Expr::Lambda { params, body, line } => {
            let depth = env.len();
            env.extend(params.iter().cloned());
            let body = Box::new(walk(*body, env, parent, used, out)?);
            env.truncate(depth);
            Expr::Lambda { params, body, line }
        }
        Expr::Spawn { body, line } => {
            Expr::Spawn { body: Box::new(walk(*body, env, parent, used, out)?), line }
        }
        Expr::Select { arms, default, line } => {
            let mut new = Vec::with_capacity(arms.len());
            for a in arms {
                let depth = env.len();
                let op = match a.op {
                    SelectOp::Recv { chan, bind } => {
                        let chan = walk(chan, env, parent, used, out)?;
                        if let Some(b) = &bind {
                            env.push(b.clone());
                        }
                        SelectOp::Recv { chan, bind }
                    }
                    SelectOp::Send { chan, value } => SelectOp::Send {
                        chan: walk(chan, env, parent, used, out)?,
                        value: walk(value, env, parent, used, out)?,
                    },
                };
                let body = walk(a.body, env, parent, used, out)?;
                env.truncate(depth);
                new.push(SelectArm { op, body, line: a.line });
            }
            let default = match default {
                Some(d) => Some(Box::new(walk(*d, env, parent, used, out)?)),
                None => None,
            };
            Expr::Select { arms: new, default, line }
        }
        Expr::Len { expr, line } => {
            Expr::Len { expr: Box::new(walk(*expr, env, parent, used, out)?), line }
        }
        Expr::Index { base, index, line } => Expr::Index {
            base: Box::new(walk(*base, env, parent, used, out)?),
            index: Box::new(walk(*index, env, parent, used, out)?),
            line,
        },
        Expr::IndexSet { base, index, value, line } => Expr::IndexSet {
            base: Box::new(walk(*base, env, parent, used, out)?),
            index: Box::new(walk(*index, env, parent, used, out)?),
            value: Box::new(walk(*value, env, parent, used, out)?),
            line,
        },
        other => other,
    })
}

/// `Pat = e` is bound over everything after it, which is what makes it the
/// one-armed `match` that a binding taking a value apart has always been.
fn expand_pat_binds(stmts: Vec<Stmt>) -> Vec<Stmt> {
    let mut out = Vec::new();
    let mut it = stmts.into_iter();
    while let Some(s) = it.next() {
        match s {
            Stmt::PatBind { pattern, value, line } => {
                let rest = expand_pat_binds(it.collect());
                out.push(Stmt::Expr(Expr::Match {
                    scrutinee: Box::new(value),
                    arms: vec![Arm { pattern, body: Expr::Block(rest), line }],
                    line,
                }));
                return out;
            }
            other => out.push(other),
        }
    }
    out
}

fn walk_all(
    es: Vec<Expr>,
    env: &mut Vec<String>,
    parent: &str,
    used: &mut HashSet<String>,
    out: &mut Vec<FnDef>,
) -> Result<Vec<Expr>, SyntaxError> {
    es.into_iter().map(|e| walk(e, env, parent, used, out)).collect()
}

fn walk_block(
    stmts: Vec<Stmt>,
    env: &mut Vec<String>,
    parent: &str,
    used: &mut HashSet<String>,
    out: &mut Vec<FnDef>,
) -> Result<Vec<Stmt>, SyntaxError> {
    let sibling_names: HashSet<String> = stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Fn(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    if sibling_names.is_empty() {
        // Nothing to lift here; only the bindings this block adds matter.
        let depth = env.len();
        let mut kept = Vec::with_capacity(stmts.len());
        for s in stmts {
            kept.push(match s {
                Stmt::Bind { name, value, line } => {
                    let value = walk(value, env, parent, used, out)?;
                    env.push(name.clone());
                    Stmt::Bind { name, value, line }
                }
                Stmt::Expr(e) => Stmt::Expr(walk(e, env, parent, used, out)?),
                Stmt::PatBind { .. } => unreachable!("pattern bindings are expanded first"),
                Stmt::Fn(_) => unreachable!("no local functions in this block"),
            });
        }
        env.truncate(depth);
        return Ok(kept);
    }

    // Pass one: the names each local function can see — as of where it is
    // written, so it cannot reach a binding made after it. Its body is left
    // alone until the block's own rewriting is settled.
    let depth = env.len();
    let mut defs: Vec<(FnDef, String, Vec<String>)> = Vec::new();
    let mut rest: Vec<Stmt> = Vec::new();
    for s in stmts {
        match s {
            Stmt::Fn(f) => {
                let f = *f;
                let lifted = pick_name(parent, &f.name, used);
                let visible = env.clone();
                defs.push((f, lifted, visible));
            }
            Stmt::Bind { name, value, line } => {
                env.push(name.clone());
                rest.push(Stmt::Bind { name, value, line });
            }
            Stmt::Expr(e) => rest.push(Stmt::Expr(e)),
            Stmt::PatBind { .. } => unreachable!("pattern bindings are expanded first"),
        }
    }
    env.truncate(depth);

    // Pass two: what each one captures, and then what its siblings force it to
    // capture as well — if `g` calls `h`, `g` has to hand `h` what `h` needs.
    let mut locals: Vec<Local> = Vec::new();
    let mut refs: Vec<HashSet<String>> = Vec::new();
    for (f, lifted, visible) in &defs {
        let mut bound: Vec<String> = sibling_names.iter().cloned().collect();
        bound.extend(f.params.iter().map(|p| p.name.clone()));
        let mut free = Vec::new();
        free_of(&f.body, &mut bound, &mut free);
        let mut named = Vec::new();
        let mut inner_bound: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
        free_of(&f.body, &mut inner_bound, &mut named);
        let seen: HashSet<&String> = free.iter().collect();
        let caps: Vec<String> =
            visible.iter().filter(|n| seen.contains(n) && !sibling_names.contains(*n)).cloned()
                .collect();
        refs.push(named.iter().filter(|n| sibling_names.contains(*n)).cloned().collect());
        locals.push(Local { name: f.name.clone(), lifted: lifted.clone(), caps: dedup(caps) });
    }
    loop {
        let mut changed = false;
        for i in 0..locals.len() {
            let mut add: Vec<String> = Vec::new();
            for j in 0..locals.len() {
                if i != j && refs[i].contains(&locals[j].name) {
                    for c in &locals[j].caps {
                        if !locals[i].caps.contains(c) {
                            add.push(c.clone());
                        }
                    }
                }
            }
            if !add.is_empty() {
                locals[i].caps.extend(add);
                locals[i].caps = dedup(std::mem::take(&mut locals[i].caps));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Pass three: rewrite every call to a lifted name, then lift what is
    // nested inside the rewritten bodies — in that order, so a nested
    // function's free names already include what a sibling call passes. That
    // holds for what a `for` loop turns into as much as for a function the
    // author wrote, which is why nothing above this point was walked.
    let by_name: HashMap<String, &Local> =
        locals.iter().map(|l| (l.name.clone(), l)).collect();
    let mut kept: Vec<Stmt> = Vec::new();
    for s in rest {
        match s {
            Stmt::Bind { name, value, line } => {
                let value = walk(rewrite(value, &by_name)?, env, parent, used, out)?;
                env.push(name.clone());
                kept.push(Stmt::Bind { name, value, line })
            }
            Stmt::Expr(e) => {
                kept.push(Stmt::Expr(walk(rewrite(e, &by_name)?, env, parent, used, out)?))
            }
            Stmt::PatBind { .. } => unreachable!("pattern bindings are expanded first"),
            Stmt::Fn(_) => unreachable!("local functions were taken out above"),
        }
    }
    env.truncate(depth);
    for ((f, _, _), l) in defs.into_iter().zip(&locals) {
        let mut params: Vec<Param> =
            l.caps.iter().map(|c| Param { name: c.clone(), ann: None }).collect();
        params.extend(f.params);
        // The lifted function's own scope is exactly its parameter list.
        let mut inner: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let body = walk(rewrite(f.body, &by_name)?, &mut inner, &l.lifted, used, out)?;
        out.push(FnDef {
            name: l.lifted.clone(),
            exported: false,
            params,
            ret_ann: f.ret_ann,
            body,
            line: f.line,
            lifted_from: Some(parent.to_string()),
        });
    }
    Ok(kept)
}

/// A name for the lifted function: the enclosing one's, a dot, its own. The
/// dot is impossible in a Rill identifier, so nothing the author wrote can
/// collide; two blocks inside one function that use the same name can, and
/// the second gets a number.
fn pick_name(parent: &str, name: &str, used: &mut HashSet<String>) -> String {
    let base = format!("{parent}.{name}");
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2;
    loop {
        let cand = format!("{base}.{n}");
        if used.insert(cand.clone()) {
            return cand;
        }
        n += 1;
    }
}

fn dedup(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(x.clone())).collect()
}

fn rewrite(e: Expr, locals: &HashMap<String, &Local>) -> Result<Expr, SyntaxError> {
    Ok(match e {
        Expr::Var(n, line) => {
            if locals.contains_key(&n) {
                return Err(SyntaxError::new(
                    line,
                    0,
                    format!(
                        "`{n}` is a local function, and a local function can only be \
                         called — it takes the names around it as arguments, so there \
                         is no one value to hand out for it"
                    ),
                ));
            }
            Expr::Var(n, line)
        }
        Expr::Call { name, args, line, piped } => {
            let mut args: Vec<Expr> =
                args.into_iter().map(|a| rewrite(a, locals)).collect::<Result<_, _>>()?;
            match locals.get(&name) {
                Some(l) => {
                    let mut all: Vec<Expr> =
                        l.caps.iter().map(|c| Expr::Var(c.clone(), line)).collect();
                    all.append(&mut args);
                    Expr::Call { name: l.lifted.clone(), args: all, line, piped: false }
                }
                None => Expr::Call { name, args, line, piped },
            }
        }
        Expr::Unary { op, expr } => Expr::Unary { op, expr: Box::new(rewrite(*expr, locals)?) },
        Expr::Binary { op, lhs, rhs, line } => Expr::Binary {
            op,
            lhs: Box::new(rewrite(*lhs, locals)?),
            rhs: Box::new(rewrite(*rhs, locals)?),
            line,
        },
        Expr::If { cond, then, els } => Expr::If {
            cond: Box::new(rewrite(*cond, locals)?),
            then: Box::new(rewrite(*then, locals)?),
            els: Box::new(rewrite(*els, locals)?),
        },
        Expr::Match { scrutinee, arms, line } => Expr::Match {
            scrutinee: Box::new(rewrite(*scrutinee, locals)?),
            arms: arms
                .into_iter()
                .map(|a| {
                    Ok(Arm { pattern: a.pattern, body: rewrite(a.body, locals)?, line: a.line })
                })
                .collect::<Result<_, SyntaxError>>()?,
            line,
        },
        Expr::Lambda { params, body, line } => {
            Expr::Lambda { params, body: Box::new(rewrite(*body, locals)?), line }
        }
        Expr::Spawn { body, line } => {
            Expr::Spawn { body: Box::new(rewrite(*body, locals)?), line }
        }
        Expr::Select { arms, default, line } => Expr::Select {
            arms: arms
                .into_iter()
                .map(|a| {
                    let op = match a.op {
                        SelectOp::Recv { chan, bind } => {
                            SelectOp::Recv { chan: rewrite(chan, locals)?, bind }
                        }
                        SelectOp::Send { chan, value } => SelectOp::Send {
                            chan: rewrite(chan, locals)?,
                            value: rewrite(value, locals)?,
                        },
                    };
                    Ok(SelectArm { op, body: rewrite(a.body, locals)?, line: a.line })
                })
                .collect::<Result<_, SyntaxError>>()?,
            default: match default {
                Some(d) => Some(Box::new(rewrite(*d, locals)?)),
                None => None,
            },
            line,
        },
        Expr::Len { expr, line } => {
            Expr::Len { expr: Box::new(rewrite(*expr, locals)?), line }
        }
        Expr::Slice { base, from, to, line } => Expr::Slice {
            base: Box::new(rewrite(*base, locals)?),
            from: match from {
                Some(f) => Some(Box::new(rewrite(*f, locals)?)),
                None => None,
            },
            to: match to {
                Some(t) => Some(Box::new(rewrite(*t, locals)?)),
                None => None,
            },
            line,
        },
        Expr::Index { base, index, line } => Expr::Index {
            base: Box::new(rewrite(*base, locals)?),
            index: Box::new(rewrite(*index, locals)?),
            line,
        },
        Expr::IndexSet { base, index, value, line } => Expr::IndexSet {
            base: Box::new(rewrite(*base, locals)?),
            index: Box::new(rewrite(*index, locals)?),
            value: Box::new(rewrite(*value, locals)?),
            line,
        },
        Expr::Append { target, value, line } => Expr::Append {
            target: Box::new(rewrite(*target, locals)?),
            value: Box::new(rewrite(*value, locals)?),
            line,
        },
        Expr::IndexOp { op, base, index, value, line } => Expr::IndexOp {
            op,
            base: Box::new(rewrite(*base, locals)?),
            index: Box::new(rewrite(*index, locals)?),
            value: Box::new(rewrite(*value, locals)?),
            line,
        },
        Expr::For { var, from, to, body, line } => Expr::For {
            var,
            from: Box::new(rewrite(*from, locals)?),
            to: Box::new(rewrite(*to, locals)?),
            body: Box::new(rewrite(*body, locals)?),
            line,
        },
        Expr::Block(stmts) => Expr::Block(
            stmts
                .into_iter()
                .map(|s| {
                    Ok(match s {
                        Stmt::Bind { name, value, line } => {
                            Stmt::Bind { name, value: rewrite(value, locals)?, line }
                        }
                        Stmt::PatBind { pattern, value, line } => {
                            Stmt::PatBind { pattern, value: rewrite(value, locals)?, line }
                        }
                        Stmt::Expr(e) => Stmt::Expr(rewrite(e, locals)?),
                        Stmt::Fn(f) => {
                            let mut f = *f;
                            f.body = rewrite(f.body, locals)?;
                            Stmt::Fn(Box::new(f))
                        }
                    })
                })
                .collect::<Result<_, SyntaxError>>()?,
        ),
        other => other,
    })
}

/// Names used in `e` that nothing inside it binds. Callee names count: a name
/// in scope may be a function value being called.
fn free_of(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match e {
        Expr::Var(n, _) => {
            if !bound.contains(n) {
                out.push(n.clone());
            }
        }
        Expr::Call { name, args, .. } => {
            if !bound.contains(name) {
                out.push(name.clone());
            }
            for a in args {
                free_of(a, bound, out);
            }
        }
        Expr::Unary { expr, .. } => free_of(expr, bound, out),
        Expr::Binary { lhs, rhs, .. } => {
            free_of(lhs, bound, out);
            free_of(rhs, bound, out);
        }
        Expr::If { cond, then, els } => {
            free_of(cond, bound, out);
            free_of(then, bound, out);
            free_of(els, bound, out);
        }
        Expr::Match { scrutinee, arms, .. } => {
            free_of(scrutinee, bound, out);
            for a in arms {
                let depth = bound.len();
                pattern_names(&a.pattern, bound);
                free_of(&a.body, bound, out);
                bound.truncate(depth);
            }
        }
        Expr::Lambda { params, body, .. } => {
            let depth = bound.len();
            bound.extend(params.iter().cloned());
            free_of(body, bound, out);
            bound.truncate(depth);
        }
        Expr::Spawn { body, .. } => free_of(body, bound, out),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                let depth = bound.len();
                match &a.op {
                    SelectOp::Recv { chan, bind } => {
                        free_of(chan, bound, out);
                        if let Some(b) = bind {
                            bound.push(b.clone());
                        }
                    }
                    SelectOp::Send { chan, value } => {
                        free_of(chan, bound, out);
                        free_of(value, bound, out);
                    }
                }
                free_of(&a.body, bound, out);
                bound.truncate(depth);
            }
            if let Some(d) = default {
                free_of(d, bound, out);
            }
        }
        Expr::Len { expr, .. } => free_of(expr, bound, out),
        Expr::Slice { base, from, to, .. } => {
            free_of(base, bound, out);
            if let Some(f) = from {
                free_of(f, bound, out);
            }
            if let Some(t) = to {
                free_of(t, bound, out);
            }
        }
        Expr::Index { base, index, .. } => {
            free_of(base, bound, out);
            free_of(index, bound, out);
        }
        Expr::IndexSet { base, index, value, .. }
        | Expr::IndexOp { base, index, value, .. } => {
            free_of(base, bound, out);
            free_of(index, bound, out);
            free_of(value, bound, out);
        }
        Expr::Append { target, value, .. } => {
            free_of(target, bound, out);
            free_of(value, bound, out);
        }
        Expr::For { var, from, to, body, .. } => {
            free_of(from, bound, out);
            free_of(to, bound, out);
            let depth = bound.len();
            bound.push(var.clone());
            free_of(body, bound, out);
            bound.truncate(depth);
        }
        Expr::Block(stmts) => {
            let depth = bound.len();
            for s in stmts {
                match s {
                    Stmt::Bind { name, value, .. } => {
                        free_of(value, bound, out);
                        bound.push(name.clone());
                    }
                    Stmt::PatBind { pattern, value, .. } => {
                        free_of(value, bound, out);
                        pattern_names(pattern, bound);
                    }
                    Stmt::Expr(x) => free_of(x, bound, out),
                    Stmt::Fn(f) => {
                        let d = bound.len();
                        bound.push(f.name.clone());
                        bound.extend(f.params.iter().map(|p| p.name.clone()));
                        free_of(&f.body, bound, out);
                        bound.truncate(d);
                        bound.push(f.name.clone());
                    }
                }
            }
            bound.truncate(depth);
        }
        _ => {}
    }
}

fn pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Var(n) => out.push(n.clone()),
        Pattern::Ctor { args, .. } => {
            for a in args {
                pattern_names(a, out);
            }
        }
        Pattern::Wildcard | Pattern::Int(_) => {}
    }
}

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
//! Rill identifier can contain. `lifted_from` records the parent when there is
//! something captured, so the checker infers the two together: a captured
//! buffer must have the type it had where it was captured, since a `Buf` width
//! is never inferred from use. A local function that captures nothing needs no
//! such tie, and without it the checker generalizes it like any other
//! top-level function — so it can be used at more than one type.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::SyntaxError;

pub fn lift(module: &mut Module) -> Result<(), SyntaxError> {
    let mut used: HashSet<String> = module.functions.iter().map(|f| f.name.clone()).collect();
    let mut lifted: Vec<FnDef> = Vec::new();
    for f in &mut module.functions {
        let mut env: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
        let body = std::mem::replace(&mut f.body, Expr::Bool(false));
        f.body = walk(expand_body(body)?, &mut env, &f.name, &mut used, &mut lifted)?;
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
///
/// With `with acc = init` the body's value is what the next turn is handed and
/// the loop is worth the last one — a parameter, which is to say a register,
/// where otherwise a program has to keep the running value in a one-element
/// buffer and reach through a pointer for every touch of it.
fn expand_for(
    var: String,
    from: Expr,
    to: Expr,
    acc: Option<(String, Box<Expr>)>,
    body: Expr,
    line: usize,
    used: &mut HashSet<String>,
) -> Expr {
    // The end and the loop function need names, and one loop can be written
    // inside another — where the same two names twice would have the inner
    // loop's end answer for the outer one's. They are taken from the same
    // pool the lifted function names come from, so each is its own.
    let hi = pick_name("for", "hi", used);
    let go = pick_name("for", "go", used);
    let lo = pick_name("for", "lo", used);
    // `v[i]` with `i` the loop's own variable is in bounds for the whole
    // loop if `lo >= 0` and `hi <= #v`, and that can be asked once, before
    // the loop, instead of at every element: `for.check(v, lo, hi)` goes in
    // front and each such access becomes `for.at`/`for.set`, which the
    // checker turns into an unchecked read or write when `v` is a buffer (a
    // map is looked up as before). Only where neither name is rebound in the
    // body, so the `v` and `i` inside are the ones the check saw. A buffer
    // never changes length, so the check holds however late the access runs.
    let hoisted = hoistable_bases(&body, &var);
    let body = if hoisted.is_empty() { body } else { hoist_indexes(body, &var, &hoisted, &go) };
    let next = Expr::Binary {
        op: BinOp::Add,
        lhs: Box::new(Expr::Var(var.clone(), line)),
        rhs: Box::new(Expr::Int(1)),
        line,
    };
    let (params, done, els, call_args) = match acc {
        Some((name, init)) => (
            vec![Param { name: var.clone(), ann: None }, Param { name: name.clone(), ann: None }],
            Expr::Var(name, line),
            // The body is the next value, so the call takes it directly.
            Expr::Call { name: go.clone(), args: vec![next, body], line, piped: false },
            vec![Expr::Var(lo.clone(), line), *init],
        ),
        None => (
            vec![Param { name: var.clone(), ann: None }],
            Expr::Int(0),
            Expr::Block(vec![
                Stmt::Expr(body),
                Stmt::Expr(Expr::Call {
                    name: go.clone(),
                    args: vec![next],
                    line,
                    piped: false,
                }),
            ]),
            vec![Expr::Var(lo.clone(), line)],
        ),
    };
    let loop_fn = FnDef {
        name: go.clone(),
        exported: false,
        params,
        ret_ann: None,
        body: Expr::If {
            cond: Box::new(Expr::Binary {
                op: BinOp::Ge,
                lhs: Box::new(Expr::Var(var, line)),
                rhs: Box::new(Expr::Var(hi.clone(), line)),
                line,
            }),
            then: Box::new(done),
            els: Box::new(els),
        },
        line,
        lifted_from: None,
    };
    let mut stmts = vec![
        Stmt::Bind { name: lo.clone(), value: from, line },
        Stmt::Bind { name: hi.clone(), value: to, line },
    ];
    for b in &hoisted {
        stmts.push(Stmt::Expr(Expr::Call {
            name: "for.check".to_string(),
            args: vec![
                Expr::Var(b.clone(), line),
                Expr::Var(lo.clone(), line),
                Expr::Var(hi.clone(), line),
                Expr::Str(go.clone()),
            ],
            line,
            piped: false,
        }));
    }
    stmts.push(Stmt::Fn(Box::new(loop_fn)));
    stmts.push(Stmt::Expr(Expr::Call { name: go, args: call_args, line, piped: false }));
    Expr::Block(stmts)
}

/// The names indexed by the loop variable itself — `v[i]`, `v[i] = x`,
/// `v[i] += x` — where neither `v` nor `i` is bound again anywhere in the
/// body.
fn hoistable_bases(body: &Expr, var: &str) -> Vec<String> {
    let mut bases = Vec::new();
    collect_indexed_by(body, var, &mut bases);
    let mut bound = HashSet::new();
    collect_bound_names(body, &mut bound);
    if bound.contains(var) {
        return Vec::new();
    }
    dedup(bases).into_iter().filter(|b| b != var && !bound.contains(b)).collect()
}

fn collect_indexed_by(e: &Expr, var: &str, out: &mut Vec<String>) {
    match e {
        Expr::Index { base, index, .. }
        | Expr::IndexSet { base, index, .. }
        | Expr::IndexOp { base, index, .. } => {
            if let (Expr::Var(b, _), Expr::Var(i, _)) = (base.as_ref(), index.as_ref()) {
                if i == var {
                    out.push(b.clone());
                }
            }
        }
        _ => {}
    }
    each_child_all(e, &mut |c| collect_indexed_by(c, var, out));
}

/// Every name an expression binds: block bindings, patterns, local
/// functions and their parameters, lambda parameters, loop variables.
fn collect_bound_names(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Block(stmts) => {
            for st in stmts {
                match st {
                    Stmt::Bind { name, .. } => {
                        out.insert(name.clone());
                    }
                    Stmt::PatBind { pattern, .. } => {
                        let mut names = Vec::new();
                        pattern_names(pattern, &mut names);
                        out.extend(names);
                    }
                    Stmt::Fn(f) => {
                        out.insert(f.name.clone());
                        out.extend(f.params.iter().map(|p| p.name.clone()));
                    }
                    Stmt::Expr(_) => {}
                }
            }
        }
        Expr::Match { arms, .. } => {
            for a in arms {
                let mut names = Vec::new();
                pattern_names(&a.pattern, &mut names);
                out.extend(names);
            }
        }
        Expr::Lambda { params, .. } => out.extend(params.iter().cloned()),
        Expr::For { var, acc, .. } => {
            out.insert(var.clone());
            if let Some((a, _)) = acc {
                out.insert(a.clone());
            }
        }
        Expr::Select { arms, .. } => {
            for a in arms {
                if let SelectOp::Recv { bind: Some(n), .. } = &a.op {
                    out.insert(n.clone());
                }
            }
        }
        _ => {}
    }
    each_child_all(e, &mut |c| collect_bound_names(c, out));
}

/// Rewrites `v[i]`, `v[i] = x` and `v[i] op= x` over the hoisted bases into
/// `for.at`/`for.set` calls, everywhere in the body.
fn hoist_indexes(e: Expr, var: &str, bases: &[String], go: &str) -> Expr {
    let is_hit = |base: &Expr, index: &Expr| -> Option<String> {
        if let (Expr::Var(b, _), Expr::Var(i, _)) = (base, index) {
            if i == var && bases.iter().any(|x| x == b) {
                return Some(b.clone());
            }
        }
        None
    };
    let at = |b: &str, line: usize| Expr::Call {
        name: "for.at".to_string(),
        args: vec![
            Expr::Var(b.to_string(), line),
            Expr::Var(var.to_string(), line),
            Expr::Str(go.to_string()),
        ],
        line,
        piped: false,
    };
    match e {
        Expr::Index { base, index, line } => match is_hit(&base, &index) {
            Some(b) => at(&b, line),
            None => Expr::Index {
                base: Box::new(hoist_indexes(*base, var, bases, go)),
                index: Box::new(hoist_indexes(*index, var, bases, go)),
                line,
            },
        },
        Expr::IndexSet { base, index, value, line } => match is_hit(&base, &index) {
            Some(b) => Expr::Call {
                name: "for.set".to_string(),
                args: vec![
                    Expr::Var(b, line),
                    Expr::Var(var.to_string(), line),
                    hoist_indexes(*value, var, bases, go),
                    Expr::Str(go.to_string()),
                ],
                line,
                piped: false,
            },
            None => Expr::IndexSet {
                base: Box::new(hoist_indexes(*base, var, bases, go)),
                index: Box::new(hoist_indexes(*index, var, bases, go)),
                value: Box::new(hoist_indexes(*value, var, bases, go)),
                line,
            },
        },
        Expr::IndexOp { op, base, index, value, line } => match is_hit(&base, &index) {
            Some(b) => Expr::Call {
                name: "for.set".to_string(),
                args: vec![
                    Expr::Var(b.clone(), line),
                    Expr::Var(var.to_string(), line),
                    Expr::Binary {
                        op,
                        lhs: Box::new(at(&b, line)),
                        rhs: Box::new(hoist_indexes(*value, var, bases, go)),
                        line,
                    },
                    Expr::Str(go.to_string()),
                ],
                line,
                piped: false,
            },
            None => Expr::IndexOp {
                op,
                base: Box::new(hoist_indexes(*base, var, bases, go)),
                index: Box::new(hoist_indexes(*index, var, bases, go)),
                value: Box::new(hoist_indexes(*value, var, bases, go)),
                line,
            },
        },
        other => map_children(other, &mut |c| hoist_indexes(c, var, bases, go)),
    }
}

/// Visits every immediate sub-expression, blocks and local function bodies
/// included — unlike `each_child`, which stops at a block.
fn each_child_all<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(..) => {}
        Expr::Call { args, .. } => args.iter().for_each(f),
        Expr::Unary { expr, .. } | Expr::Len { expr, .. } | Expr::Try { expr, .. } => f(expr),
        Expr::Spawn { body, .. } | Expr::Lambda { body, .. } => f(body),
        Expr::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        Expr::If { cond, then, els } => {
            f(cond);
            f(then);
            f(els);
        }
        Expr::Match { scrutinee, arms, .. } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        Expr::Select { arms, default, .. } => {
            for a in arms {
                match &a.op {
                    SelectOp::Recv { chan, .. } => f(chan),
                    SelectOp::Send { chan, value } => {
                        f(chan);
                        f(value);
                    }
                }
                f(&a.body);
            }
            if let Some(d) = default {
                f(d);
            }
        }
        Expr::Index { base, index, .. } => {
            f(base);
            f(index);
        }
        Expr::Field { base, .. } => f(base),
        Expr::Update { base, fields, .. } => {
            f(base);
            for (_, v, _) in fields {
                f(v);
            }
        }
        Expr::Slice { base, from, to, .. } => {
            f(base);
            if let Some(x) = from {
                f(x);
            }
            if let Some(x) = to {
                f(x);
            }
        }
        Expr::IndexSet { base, index, value, .. } | Expr::IndexOp { base, index, value, .. } => {
            f(base);
            f(index);
            f(value);
        }
        Expr::Append { target, value, .. } => {
            f(target);
            f(value);
        }
        Expr::For { from, to, acc, body, .. } => {
            f(from);
            f(to);
            if let Some((_, init)) = acc {
                f(init);
            }
            f(body);
        }
        Expr::Block(stmts) => {
            for st in stmts {
                match st {
                    Stmt::Bind { value, .. } | Stmt::PatBind { value, .. } => f(value),
                    Stmt::Fn(fd) => f(&fd.body),
                    Stmt::Expr(x) => f(x),
                }
            }
        }
    }
}

/// Rebuilds an expression with `f` applied to each immediate sub-expression.
fn map_children(e: Expr, f: &mut impl FnMut(Expr) -> Expr) -> Expr {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(..) => e,
        Expr::Call { name, args, line, piped } => {
            Expr::Call { name, args: args.into_iter().map(|a| f(a)).collect(), line, piped }
        }
        Expr::Unary { op, expr } => Expr::Unary { op, expr: Box::new(f(*expr)) },
        Expr::Len { expr, line } => Expr::Len { expr: Box::new(f(*expr)), line },
        Expr::Try { expr, line } => Expr::Try { expr: Box::new(f(*expr)), line },
        Expr::Spawn { body, line } => Expr::Spawn { body: Box::new(f(*body)), line },
        Expr::Lambda { params, body, line } => {
            Expr::Lambda { params, body: Box::new(f(*body)), line }
        }
        Expr::Binary { op, lhs, rhs, line } => {
            Expr::Binary { op, lhs: Box::new(f(*lhs)), rhs: Box::new(f(*rhs)), line }
        }
        Expr::If { cond, then, els } => {
            Expr::If { cond: Box::new(f(*cond)), then: Box::new(f(*then)), els: Box::new(f(*els)) }
        }
        Expr::Match { scrutinee, arms, line } => Expr::Match {
            scrutinee: Box::new(f(*scrutinee)),
            arms: arms
                .into_iter()
                .map(|a| Arm {
                    pattern: a.pattern,
                    guard: a.guard.map(|g| f(g)),
                    body: f(a.body),
                    line: a.line,
                })
                .collect(),
            line,
        },
        Expr::Select { arms, default, line } => Expr::Select {
            arms: arms
                .into_iter()
                .map(|a| SelectArm {
                    op: match a.op {
                        SelectOp::Recv { chan, bind } => SelectOp::Recv { chan: f(chan), bind },
                        SelectOp::Send { chan, value } => {
                            SelectOp::Send { chan: f(chan), value: f(value) }
                        }
                    },
                    body: f(a.body),
                    line: a.line,
                })
                .collect(),
            default: default.map(|d| Box::new(f(*d))),
            line,
        },
        Expr::Index { base, index, line } => {
            Expr::Index { base: Box::new(f(*base)), index: Box::new(f(*index)), line }
        }
        Expr::Field { base, name, line } => Expr::Field { base: Box::new(f(*base)), name, line },
        Expr::Update { base, fields, line } => Expr::Update {
            base: Box::new(f(*base)),
            fields: fields.into_iter().map(|(n, v, l)| (n, f(v), l)).collect(),
            line,
        },
        Expr::Slice { base, from, to, line } => Expr::Slice {
            base: Box::new(f(*base)),
            from: from.map(|x| Box::new(f(*x))),
            to: to.map(|x| Box::new(f(*x))),
            line,
        },
        Expr::IndexSet { base, index, value, line } => Expr::IndexSet {
            base: Box::new(f(*base)),
            index: Box::new(f(*index)),
            value: Box::new(f(*value)),
            line,
        },
        Expr::IndexOp { op, base, index, value, line } => Expr::IndexOp {
            op,
            base: Box::new(f(*base)),
            index: Box::new(f(*index)),
            value: Box::new(f(*value)),
            line,
        },
        Expr::Append { target, value, line } => {
            Expr::Append { target: Box::new(f(*target)), value: Box::new(f(*value)), line }
        }
        Expr::For { var, from, to, acc, body, line } => Expr::For {
            var,
            from: Box::new(f(*from)),
            to: Box::new(f(*to)),
            acc: acc.map(|(n, init)| (n, Box::new(f(*init)))),
            body: Box::new(f(*body)),
            line,
        },
        Expr::Block(stmts) => Expr::Block(
            stmts
                .into_iter()
                .map(|st| match st {
                    Stmt::Bind { name, value, line } => Stmt::Bind { name, value: f(value), line },
                    Stmt::PatBind { pattern, value, line } => {
                        Stmt::PatBind { pattern, value: f(value), line }
                    }
                    Stmt::Fn(mut fd) => {
                        fd.body = f(fd.body);
                        Stmt::Fn(fd)
                    }
                    Stmt::Expr(x) => Stmt::Expr(f(x)),
                })
                .collect(),
        ),
    }
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
        Expr::For { var, from, to, acc, body, line } => {
            walk(expand_for(var, *from, *to, acc, *body, line, used), env, parent, used, out)?
        }
        Expr::Block(stmts) => {
            let mut n = 0usize;
            Expr::Block(walk_block(
                expand_pat_binds(expand_try(stmts, &mut n)?),
                env,
                parent,
                used,
                out,
            )?)
        }
        Expr::Call { name, args, line, piped } => {
            // `add(2, _)` is the function you get by leaving that argument to
            // be given later. It is written out as the lambda it means and
            // then walked as one, so everything after this pass sees an
            // ordinary closure and nothing knows an underscore was involved.
            if args.iter().any(is_placeholder) {
                let lam = placeholder_lambda(name, args, line, piped);
                walk(lam, env, parent, used, out)?
            } else {
                Expr::Call { name, args: walk_all(args, env, parent, used, out)?, line, piped }
            }
        }
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
                // `1 | 2 -> body` is one arm per alternative, and the split
                // happens here, before anything is walked: each one then binds
                // its own names and walks its own copy of the guard and body.
                // Splitting after the walk would have shared one set of
                // bindings between alternatives, which is only right when they
                // name their fields in the same order, and nothing says they
                // must. Nothing downstream of this pass ever sees an `Or`.
                for pattern in alternatives(a.pattern, a.line)? {
                    let depth = env.len();
                    pattern_names(&pattern, env);
                    let guard = match a.guard.clone() {
                        Some(g) => Some(walk(g, env, parent, used, out)?),
                        None => None,
                    };
                    let body = walk(a.body.clone(), env, parent, used, out)?;
                    env.truncate(depth);
                    new.push(Arm { pattern, guard, body, line: a.line });
                }
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
        Expr::Field { base, name, line } => Expr::Field {
            base: Box::new(walk(*base, env, parent, used, out)?),
            name,
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
                    arms: vec![Arm { pattern, guard: None, body: Expr::Block(rest), line }],
                    line,
                }));
                return out;
            }
            other => out.push(other),
        }
    }
    out
}

/// A function's body, with any `?` in it spelled out. A body written as one
/// expression is a block of one statement as far as this is concerned, which
/// is what lets `fn f(p) = Ok(#read_file(p)?)` mean what it reads as.
fn expand_body(body: Expr) -> Result<Expr, SyntaxError> {
    if matches!(body, Expr::Block(_)) {
        return Ok(body);
    }
    let mut n = 0usize;
    let mut stmts = expand_try(vec![Stmt::Expr(body)], &mut n)?;
    if stmts.len() == 1 {
        if let Stmt::Expr(e) = stmts.remove(0) {
            return Ok(e);
        }
        unreachable!("a body is an expression");
    }
    Ok(Expr::Block(stmts))
}

/// `e?` spelled out.
///
/// A `?` says: the value inside the `Ok`, or else the whole function's `Err`.
/// In a language where everything is an expression that means a `match` whose
/// `Ok` arm is everything the block had left to do — the same rewriting
/// `expand_pat_binds` does, with a second arm for the error:
///
/// ```text
///     text = read_file(p)?          match read_file(p)
///     Ok(#text)              ==>      Ok(text) -> Ok(#text)
///                                     Err(e) -> Err(e)
/// ```
///
/// Which is why a `?` belongs where the block it is in is what the function
/// returns: the `Err` arm leaves by the same door the block does. Written in a
/// branch of its own the arms disagree, and the checker says so.
fn expand_try(stmts: Vec<Stmt>, n: &mut usize) -> Result<Vec<Stmt>, SyntaxError> {
    let mut out = Vec::new();
    let mut it = stmts.into_iter();
    while let Some(s) = it.next() {
        // A `?` written inside a statement rather than at the top of one is
        // named first, in the order it is evaluated — and then the list is
        // started again, because those names are `?` at the top of a
        // statement themselves.
        let mut pre: Vec<Stmt> = Vec::new();
        let s = match s {
            Stmt::Bind { name, value, line } => {
                Stmt::Bind { name, value: hoist(value, &mut pre, n, true)?, line }
            }
            Stmt::PatBind { pattern, value, line } => {
                Stmt::PatBind { pattern, value: hoist(value, &mut pre, n, true)?, line }
            }
            Stmt::Expr(e) => Stmt::Expr(hoist(e, &mut pre, n, true)?),
            // A local function is its own body, with its own blocks.
            other => {
                out.push(other);
                continue;
            }
        };
        if !pre.is_empty() {
            pre.push(s);
            pre.extend(it);
            out.extend(expand_try(pre, n)?);
            return Ok(out);
        }
        let (pattern, inner, line) = match s {
            Stmt::Bind { name, value: Expr::Try { expr, line }, .. } => {
                (Pattern::Var(name), *expr, line)
            }
            // `(a, b) = f()?` takes the value apart inside the `Ok`, which
            // is one pattern rather than two matches.
            Stmt::PatBind { pattern, value: Expr::Try { expr, line }, .. } => {
                (pattern, *expr, line)
            }
            Stmt::Expr(Expr::Try { expr, line }) => (Pattern::Wildcard, *expr, line),
            other => {
                out.push(other);
                continue;
            }
        };
        let rest = expand_try(it.collect(), n)?;
        if rest.is_empty() {
            // Nothing left to do with the value: `f()?` last in a block is
            // `f()`, and taking it apart to put it back would be a lie about
            // what the program does.
            out.push(Stmt::Expr(inner));
            return Ok(out);
        }
        let err = "try.e".to_string();
        out.push(Stmt::Expr(Expr::Match {
            scrutinee: Box::new(inner),
            arms: vec![
                Arm {
                    pattern: Pattern::Ctor { name: "Ok".into(), args: vec![pattern], line },
                    guard: None,
                    body: Expr::Block(rest),
                    line,
                },
                Arm {
                    pattern: Pattern::Ctor {
                        name: "Err".into(),
                        args: vec![Pattern::Var(err.clone())],
                        line,
                    },
                    guard: None,
                    body: Expr::Call {
                        name: "Err".into(),
                        args: vec![Expr::Var(err, line)],
                        line,
                        piped: false,
                    },
                    line,
                },
            ],
            line,
        }));
        return Ok(out);
    }
    Ok(out)
}

/// Names every `?` written inside an expression rather than at the top of one,
/// in the order the expression evaluates. `top` says this position is the
/// statement's own value, where a `?` may simply stay.
///
/// Only positions that certainly run are walked into. The far side of `&&`, a
/// branch, an arm, a lambda — none of those is somewhere a value can be
/// pulled out of, so a `?` there is refused rather than quietly moved to
/// where it would run every time.
fn hoist(e: Expr, pre: &mut Vec<Stmt>, n: &mut usize, top: bool) -> Result<Expr, SyntaxError> {
    Ok(match e {
        Expr::Try { expr, line } => {
            let inner = hoist(*expr, pre, n, false)?;
            if top {
                return Ok(Expr::Try { expr: Box::new(inner), line });
            }
            let name = format!("try.{n}");
            *n += 1;
            pre.push(Stmt::Bind {
                name: name.clone(),
                value: Expr::Try { expr: Box::new(inner), line },
                line,
            });
            Expr::Var(name, line)
        }
        Expr::Call { name, args, line, piped } => Expr::Call {
            name,
            args: args
                .into_iter()
                .map(|a| hoist(a, pre, n, false))
                .collect::<Result<Vec<_>, _>>()?,
            line,
            piped,
        },
        Expr::Unary { op, expr } => {
            Expr::Unary { op, expr: Box::new(hoist(*expr, pre, n, false)?) }
        }
        // Every binary operator runs both sides: `&&` and `||` are not among
        // them, having been an `if` since the parser.
        Expr::Binary { op, lhs, rhs, line } => {
            Expr::Binary {
                op,
                lhs: Box::new(hoist(*lhs, pre, n, false)?),
                rhs: Box::new(hoist(*rhs, pre, n, false)?),
                line,
            }
        }
        Expr::Len { expr, line } => {
            Expr::Len { expr: Box::new(hoist(*expr, pre, n, false)?), line }
        }
        Expr::Index { base, index, line } => Expr::Index {
            base: Box::new(hoist(*base, pre, n, false)?),
            index: Box::new(hoist(*index, pre, n, false)?),
            line,
        },
        Expr::Field { base, name, line } => {
            Expr::Field { base: Box::new(hoist(*base, pre, n, false)?), name, line }
        }
        Expr::IndexSet { base, index, value, line } => Expr::IndexSet {
            base: Box::new(hoist(*base, pre, n, false)?),
            index: Box::new(hoist(*index, pre, n, false)?),
            value: Box::new(hoist(*value, pre, n, false)?),
            line,
        },
        Expr::IndexOp { op, base, index, value, line } => Expr::IndexOp {
            op,
            base: Box::new(hoist(*base, pre, n, false)?),
            index: Box::new(hoist(*index, pre, n, false)?),
            value: Box::new(hoist(*value, pre, n, false)?),
            line,
        },
        Expr::Append { target, value, line } => Expr::Append {
            target: Box::new(hoist(*target, pre, n, false)?),
            value: Box::new(hoist(*value, pre, n, false)?),
            line,
        },
        Expr::Slice { base, from, to, line } => Expr::Slice {
            base: Box::new(hoist(*base, pre, n, false)?),
            from: match from {
                Some(f) => Some(Box::new(hoist(*f, pre, n, false)?)),
                None => None,
            },
            to: match to {
                Some(t) => Some(Box::new(hoist(*t, pre, n, false)?)),
                None => None,
            },
            line,
        },
        // The scrutinee of a `match` and the condition of an `if` do run,
        // every time; their arms and branches are another matter, and a
        // block among them expands on its own.
        Expr::Match { scrutinee, arms, line } => {
            let scrutinee = Box::new(hoist(*scrutinee, pre, n, false)?);
            for a in &arms {
                no_stray_try(&a.body)?;
            }
            Expr::Match { scrutinee, arms, line }
        }
        Expr::If { cond, then, els } => {
            let cond = Box::new(hoist(*cond, pre, n, false)?);
            no_stray_try(&then)?;
            no_stray_try(&els)?;
            Expr::If { cond, then, els }
        }
        other => {
            no_stray_try(&other)?;
            other
        }
    })
}

/// A `?` left where nothing can be done with it. Blocks are not walked into:
/// each expands against its own rest, and one in tail position is exactly
/// where a `?` belongs.
fn no_stray_try(e: &Expr) -> Result<(), SyntaxError> {
    if let Expr::Try { line, .. } = e {
        return Err(SyntaxError::new(
            *line,
            1,
            "`?` cannot be written here: it hands the function's caller an \
             `Err`, so it belongs where the block it is in is what the \
             function returns. Bind the call to a name first."
                .to_string(),
        ));
    }
    if matches!(e, Expr::Block(_)) {
        return Ok(());
    }
    let mut found = Ok(());
    each_child(e, &mut |c| {
        if found.is_ok() {
            found = no_stray_try(c);
        }
    });
    found
}

/// Applies `f` to every immediate sub-expression, whatever the shape.
fn each_child<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(..) => {}
        Expr::Call { args, .. } => args.iter().for_each(f),
        Expr::Unary { expr, .. } | Expr::Len { expr, .. } | Expr::Try { expr, .. } => f(expr),
        Expr::Field { base, .. } => f(base),
        Expr::Update { base, fields, .. } => {
            f(base);
            for (_, v, _) in fields {
                f(v);
            }
        }
        Expr::Binary { lhs, rhs, .. } | Expr::Append { target: lhs, value: rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        Expr::If { cond, then, els } => {
            f(cond);
            f(then);
            f(els);
        }
        Expr::Match { scrutinee, arms, .. } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        Expr::Lambda { body, .. } | Expr::Spawn { body, .. } => f(body),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                match &a.op {
                    SelectOp::Recv { chan, .. } => f(chan),
                    SelectOp::Send { chan, value } => {
                        f(chan);
                        f(value);
                    }
                }
                f(&a.body);
            }
            if let Some(d) = default {
                f(d);
            }
        }
        Expr::Index { base, index, .. } => {
            f(base);
            f(index);
        }
        Expr::IndexSet { base, index, value, .. } | Expr::IndexOp { base, index, value, .. } => {
            f(base);
            f(index);
            f(value);
        }
        Expr::Slice { base, from, to, .. } => {
            f(base);
            if let Some(x) = from {
                f(x);
            }
            if let Some(x) = to {
                f(x);
            }
        }
        Expr::For { from, to, acc, body, .. } => {
            f(from);
            f(to);
            if let Some((_, init)) = acc {
                f(init);
            }
            f(body);
        }
        Expr::Block(_) => {}
    }
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
        let body =
            walk(expand_body(rewrite(f.body, &by_name)?)?, &mut inner, &l.lifted, used, out)?;
        out.push(FnDef {
            name: l.lifted.clone(),
            exported: false,
            params,
            ret_ann: f.ret_ann,
            body,
            line: f.line,
            // Only what closed over something has to be inferred with the
            // function it was written in; see the note at the top.
            lifted_from: (!l.caps.is_empty()).then(|| parent.to_string()),
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
                    Ok(Arm {
                        pattern: a.pattern,
                        guard: match a.guard {
                            Some(g) => Some(rewrite(g, locals)?),
                            None => None,
                        },
                        body: rewrite(a.body, locals)?,
                        line: a.line,
                    })
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
        Expr::For { var, from, to, acc, body, line } => Expr::For {
            var,
            from: Box::new(rewrite(*from, locals)?),
            to: Box::new(rewrite(*to, locals)?),
            acc: match acc {
                Some((n, e)) => Some((n, Box::new(rewrite(*e, locals)?))),
                None => None,
            },
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
        Expr::For { var, from, to, acc, body, .. } => {
            free_of(from, bound, out);
            free_of(to, bound, out);
            let depth = bound.len();
            bound.push(var.clone());
            if let Some((n, init)) = acc {
                free_of(init, bound, out);
                bound.push(n.clone());
            }
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

/// `_` standing where an argument would be. In a pattern an underscore means
/// "anything and do not name it"; here it means "not this one, yet", which is
/// the same thought pointed the other way.
fn is_placeholder(e: &Expr) -> bool {
    matches!(e, Expr::Var(n, _) if n == "_")
}

/// `f(a, _)` as `\arg.1 -> f(a, arg.1)`.
///
/// The parameters are named for the position they were left at rather than
/// from a counter, and two lambdas can therefore both hold an `arg.0`. That is
/// safe, and worth saying why: a placeholder's parameter is used exactly once,
/// in the argument slot the underscore was written in, which is inside its own
/// lambda and nowhere else. `f(_, g(_))` gives `\arg.0 -> f(arg.0, \arg.0 ->
/// g(arg.0))`, and the outer `arg.0` sits in f's first argument, outside the
/// inner lambda entirely. There is no way to write an underscore whose
/// parameter would have to reach past a nearer one of the same name.
fn placeholder_lambda(name: String, args: Vec<Expr>, line: usize, piped: bool) -> Expr {
    let mut params = Vec::new();
    let args = args
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            if is_placeholder(&a) {
                let p = format!("arg.{i}");
                params.push(p.clone());
                Expr::Var(p, line)
            } else {
                a
            }
        })
        .collect();
    Expr::Lambda { params, body: Box::new(Expr::Call { name, args, line, piped }), line }
}

/// The alternatives one arm's pattern stands for: its own, or the several an
/// `|` holds.
///
/// They must bind the same names. The body is one body, written once, so a name
/// it uses has to mean something whichever alternative was taken — and where
/// they disagree the expansion would produce arms whose bodies mention
/// variables they never bound, which surfaces much later as an unknown
/// variable pointing at a line that plainly does bind it.
fn alternatives(p: Pattern, line: usize) -> Result<Vec<Pattern>, SyntaxError> {
    let Pattern::Or(alts, oline) = p else { return Ok(vec![p]) };
    let mut want = Vec::new();
    pattern_names(&alts[0], &mut want);
    want.sort();
    for alt in &alts[1..] {
        let mut got = Vec::new();
        pattern_names(alt, &mut got);
        got.sort();
        if got != want {
            return Err(SyntaxError::new(
                if oline > 0 { oline } else { line },
                1,
                format!(
                    "every alternative of an `|` pattern must bind the same names: one binds {}, another binds {}",
                    shown_names(&want),
                    shown_names(&got)
                ),
            ));
        }
    }
    Ok(alts)
}

fn shown_names(v: &[String]) -> String {
    if v.is_empty() { "nothing".to_string() } else { v.join(", ") }
}

fn pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Var(n) => out.push(n.clone()),
        Pattern::Ctor { args, .. } => {
            for a in args {
                pattern_names(a, out);
            }
        }
        Pattern::As(n, inner, _) => {
            out.push(n.clone());
            pattern_names(inner, out);
        }
        Pattern::Or(alts, _) => {
            // Every alternative binds the same names, so the first speaks for
            // all of them — and by the time anything else asks, the arm has
            // been split and there is no `Or` left to ask about.
            if let Some(first) = alts.first() {
                pattern_names(first, out);
            }
        }
        Pattern::Wildcard | Pattern::Int(_) => {}
    }
}

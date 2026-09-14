//! Type aliases, expanded. `type Meters = Float` says a name, not a type:
//! every annotation that writes `Meters` is read as `Float` before anything
//! else looks at the program, and the alias is not among the types. `type
//! Pair(a) = (a, a)` takes arguments, put in for its parameters. An alias
//! may name another; one that names itself, however far round, is refused.

use std::collections::HashMap;

use crate::ast::*;
use crate::SyntaxError;

pub fn expand(m: &mut Module) -> Result<(), SyntaxError> {
    let aliases: HashMap<String, TypeDef> = m
        .types
        .iter()
        .filter(|t| t.alias.is_some())
        .map(|t| (t.name.clone(), t.clone()))
        .collect();
    if aliases.is_empty() {
        return Ok(());
    }
    m.types.retain(|t| t.alias.is_none());
    for t in &mut m.types {
        for c in &mut t.ctors {
            for (_, tr) in &mut c.fields {
                expand_ref(tr, &aliases, 0)?;
            }
        }
    }
    for f in &mut m.functions {
        fn_refs(f, &aliases)?;
    }
    for t in &mut m.traits {
        for ms in &mut t.methods {
            for tr in &mut ms.params {
                expand_ref(tr, &aliases, 0)?;
            }
            expand_ref(&mut ms.ret, &aliases, 0)?;
        }
    }
    for im in &mut m.impls {
        expand_ref(&mut im.target, &aliases, 0)?;
        for f in &mut im.fns {
            fn_refs(f, &aliases)?;
        }
    }
    for e in &mut m.externs {
        for (_, tr) in &mut e.params {
            expand_ref(tr, &aliases, 0)?;
        }
        if let Some(tr) = &mut e.ret {
            expand_ref(tr, &aliases, 0)?;
        }
    }
    for a in &mut m.asms {
        for (_, tr) in &mut a.params {
            expand_ref(tr, &aliases, 0)?;
        }
        if let Some(tr) = &mut a.ret {
            expand_ref(tr, &aliases, 0)?;
        }
    }
    Ok(())
}

fn fn_refs(f: &mut FnDef, aliases: &HashMap<String, TypeDef>) -> Result<(), SyntaxError> {
    for p in &mut f.params {
        if let Some(tr) = &mut p.ann {
            expand_ref(tr, aliases, 0)?;
        }
    }
    if let Some(tr) = &mut f.ret_ann {
        expand_ref(tr, aliases, 0)?;
    }
    let body = std::mem::replace(&mut f.body, Expr::Bool(false));
    let mut err = None;
    f.body = expr_refs(body, aliases, &mut err);
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// The annotations inside a body: a loop's carried names, and a local
/// function's parameters and result. This runs before `lift`, so both are
/// still where they were written.
fn expr_refs(e: Expr, aliases: &HashMap<String, TypeDef>, err: &mut Option<SyntaxError>) -> Expr {
    let mut e = e;
    match &mut e {
        Expr::While { acc: Some((params, _)), .. } => {
            for p in params {
                if let Some(tr) = &mut p.ann {
                    if let Err(x) = expand_ref(tr, aliases, 0) {
                        err.get_or_insert(x);
                    }
                }
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                if let Stmt::Fn(f) = s {
                    if let Err(x) = fn_refs(f, aliases) {
                        err.get_or_insert(x);
                    }
                }
            }
        }
        _ => {}
    }
    crate::lift::map_children(e, &mut |c| expr_refs(c, aliases, err))
}

fn expand_ref(
    tr: &mut TypeRef,
    aliases: &HashMap<String, TypeDef>,
    depth: usize,
) -> Result<(), SyntaxError> {
    for a in &mut tr.args {
        expand_ref(a, aliases, depth)?;
    }
    if let Some(r) = &mut tr.ret {
        expand_ref(r, aliases, depth)?;
    }
    let Some(def) = aliases.get(&tr.name) else { return Ok(()) };
    if depth > 32 {
        return Err(SyntaxError::new(
            tr.line,
            0,
            format!("type alias `{}` names itself, however far round", tr.name),
        ));
    }
    if tr.args.len() != def.params.len() {
        return Err(SyntaxError::new(
            tr.line,
            0,
            format!(
                "type alias `{}` takes {} type argument(s), but {} given",
                tr.name,
                def.params.len(),
                tr.args.len()
            ),
        ));
    }
    let mut body = def.alias.clone().expect("an alias names a type");
    substitute(&mut body, &def.params, &tr.args);
    // Whatever the expansion is, an error about it points at the use.
    relocate(&mut body, tr.line);
    *tr = body;
    expand_ref(tr, aliases, depth + 1)
}

fn substitute(tr: &mut TypeRef, params: &[String], args: &[TypeRef]) {
    if tr.args.is_empty() && tr.ret.is_none() {
        if let Some(i) = params.iter().position(|p| *p == tr.name) {
            *tr = args[i].clone();
            return;
        }
    }
    for a in &mut tr.args {
        substitute(a, params, args);
    }
    if let Some(r) = &mut tr.ret {
        substitute(r, params, args);
    }
}

fn relocate(tr: &mut TypeRef, line: usize) {
    tr.line = line;
    for a in &mut tr.args {
        relocate(a, line);
    }
    if let Some(r) = &mut tr.ret {
        relocate(r, line);
    }
}

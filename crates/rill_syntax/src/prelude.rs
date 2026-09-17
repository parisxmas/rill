//! The Rill prelude and its merge-with-shadowing logic.
//!
//! `parse_program` parses user source, then prepends the embedded prelude
//! with these rules:
//! - a user function shadows the prelude function of the same name;
//! - a user type that reuses a prelude type name or constructor name drops
//!   that prelude type;
//! - any prelude function that (transitively) references a dropped name is
//!   dropped too, so shadowing never produces errors inside the prelude.

use std::collections::HashSet;

use crate::ast::*;
use crate::parser::parse;
use crate::SyntaxError;

pub const PRELUDE: &str = include_str!("prelude.rill");

pub fn parse_program(src: &str) -> Result<Module, SyntaxError> {
    let user = parse(src)?;
    with_prelude(user)
}

/// Merges the embedded prelude under an already-assembled user module
/// (used by the multi-file loader after import resolution).
pub fn with_prelude(user: Module) -> Result<Module, SyntaxError> {
    let prelude = parse(PRELUDE).expect("the embedded prelude must parse");
    let mut m = merge(prelude, user);
    crate::alias::expand(&mut m)?;
    crate::lift::lift(&mut m)?;
    Ok(m)
}

fn merge(prelude: Module, user: Module) -> Module {
    let (name, name_line) = (user.name.clone(), user.name_line);
    let user_types: HashSet<&str> = user.types.iter().map(|t| t.name.as_str()).collect();
    let user_ctors: HashSet<&str> =
        user.types.iter().flat_map(|t| t.ctors.iter().map(|c| c.name.as_str())).collect();
    // A foreign declaration shadows a prelude function the same way a
    // definition does.
    let user_fns: HashSet<&str> = user
        .functions
        .iter()
        .map(|f| f.name.as_str())
        .chain(user.externs.iter().map(|e| e.name.as_str()))
        .chain(user.asms.iter().map(|a| a.name.as_str()))
        .collect();

    // Names that are no longer provided by the prelude.
    let mut dropped: HashSet<String> = HashSet::new();

    let mut types = Vec::new();
    for t in prelude.types {
        let clash = user_types.contains(t.name.as_str())
            || t.ctors.iter().any(|c| user_ctors.contains(c.name.as_str()));
        if clash {
            dropped.insert(t.name.clone());
            for c in &t.ctors {
                dropped.insert(c.name.clone());
            }
        } else {
            types.push(t);
        }
    }

    // Drop shadowed functions, then transitively drop functions referencing
    // anything dropped.
    let mut fns: Vec<(FnDef, HashSet<String>)> = prelude
        .functions
        .into_iter()
        .map(|f| {
            let mut names = HashSet::new();
            collect_expr_names(&f.body, &mut names);
            for p in &f.params {
                if let Some(ann) = &p.ann {
                    collect_type_names(ann, &mut names);
                }
            }
            if let Some(r) = &f.ret_ann {
                collect_type_names(r, &mut names);
            }
            (f, names)
        })
        .collect();
    loop {
        let mut changed = false;
        fns.retain(|(f, names)| {
            let out = if user_fns.contains(f.name.as_str()) {
                false
            } else {
                !names.iter().any(|n| dropped.contains(n))
            };
            if !out {
                changed |= dropped.insert(f.name.clone());
            }
            out
        });
        if !changed {
            break;
        }
    }

    let mut functions: Vec<FnDef> = fns.into_iter().map(|(f, _)| f).collect();
    // Everything appended past here is the program's own.
    let prelude_fns = functions.len();
    types.extend(user.types);
    functions.extend(user.functions);
    Module {
        name,
        name_line,
        imports: Vec::new(),
        types,
        traits: user.traits,
        impls: user.impls,
        externs: user.externs,
        asms: user.asms,
        functions,
        prelude_fns,
        imported_fns: user.imported_fns,
        machines: user.machines,
        systems: user.systems,
        imported_machines: user.imported_machines,
    }
}

fn collect_type_names(tr: &TypeRef, out: &mut HashSet<String>) {
    out.insert(tr.name.clone());
    for a in &tr.args {
        collect_type_names(a, out);
    }
    if let Some(r) = &tr.ret {
        collect_type_names(r, out);
    }
}

fn collect_pattern_names(p: &Pattern, out: &mut HashSet<String>) {
    if let Pattern::Ctor { name, args, .. } = p {
        out.insert(name.clone());
        for a in args {
            collect_pattern_names(a, out);
        }
    }
}

fn collect_expr_names(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Var(n, _) => {
            out.insert(n.clone());
        }
        Expr::Update { base, fields, .. } => {
            collect_expr_names(base, out);
            for (_, v, _) in fields {
                collect_expr_names(v, out);
            }
        }
        Expr::Call { name, args, .. } => {
            out.insert(name.clone());
            for a in args {
                collect_expr_names(a, out);
            }
        }
        Expr::Len { expr, .. } | Expr::Try { expr, .. } => collect_expr_names(expr, out),
        Expr::Field { base, .. } => collect_expr_names(base, out),
        Expr::Slice { base, from, to, .. } => {
            collect_expr_names(base, out);
            if let Some(f) = from {
                collect_expr_names(f, out);
            }
            if let Some(t) = to {
                collect_expr_names(t, out);
            }
        }
        Expr::Index { base, index, .. } => {
            collect_expr_names(base, out);
            collect_expr_names(index, out);
        }
        Expr::IndexSet { base, index, value, .. }
        | Expr::IndexOp { base, index, value, .. } => {
            collect_expr_names(base, out);
            collect_expr_names(index, out);
            collect_expr_names(value, out);
        }
        Expr::Append { target, value, .. } => {
            collect_expr_names(target, out);
            collect_expr_names(value, out);
        }
        Expr::For { from, to, acc, body, .. } => {
            collect_expr_names(from, out);
            collect_expr_names(to, out);
            if let Some((_, init)) = acc {
                collect_expr_names(init, out);
            }
            collect_expr_names(body, out);
        }
        Expr::While { cond, acc, body, .. } => {
            collect_expr_names(cond, out);
            if let Some((_, init)) = acc {
                collect_expr_names(init, out);
            }
            collect_expr_names(body, out);
        }
        Expr::Unary { expr, .. } => collect_expr_names(expr, out),
        Expr::Binary { lhs, rhs, .. } => {
            collect_expr_names(lhs, out);
            collect_expr_names(rhs, out);
        }
        Expr::If { cond, then, els, .. } => {
            collect_expr_names(cond, out);
            collect_expr_names(then, out);
            collect_expr_names(els, out);
        }
        Expr::Match { scrutinee, arms, .. } => {
            collect_expr_names(scrutinee, out);
            for arm in arms {
                collect_pattern_names(&arm.pattern, out);
                if let Some(g) = &arm.guard {
                    collect_expr_names(g, out);
                }
                collect_expr_names(&arm.body, out);
            }
        }
        Expr::Lambda { body, .. } => collect_expr_names(body, out),
        Expr::Spawn { body, .. } => collect_expr_names(body, out),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                match &a.op {
                    SelectOp::Recv { chan, .. } => collect_expr_names(chan, out),
                    SelectOp::Send { chan, value } => {
                        collect_expr_names(chan, out);
                        collect_expr_names(value, out);
                    }
                    SelectOp::Timeout { ms } => collect_expr_names(ms, out),
                }
                collect_expr_names(&a.body, out);
            }
            if let Some(d) = default {
                collect_expr_names(d, out);
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                match s {
                    Stmt::Bind { value, .. } => collect_expr_names(value, out),
                    Stmt::PatBind { pattern, value, .. } => {
                        collect_pattern_names(pattern, out);
                        collect_expr_names(value, out);
                    }
                    Stmt::Fn(f) => collect_expr_names(&f.body, out),
                    Stmt::Expr(v) => collect_expr_names(v, out),
                }
            }
        }
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Unit => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_parses_and_merges() {
        let m = parse_program("fn main() = println(sum(range(1, 10)))\n").unwrap();
        assert!(m.functions.iter().any(|f| f.name == "sum"));
        assert!(m.types.iter().any(|t| t.name == "List"));
    }

    #[test]
    fn user_fn_shadows_prelude() {
        let m = parse_program("fn sum(l) = 42\n\nfn main() = println(1)\n").unwrap();
        assert_eq!(m.functions.iter().filter(|f| f.name == "sum").count(), 1);
    }

    #[test]
    fn user_ctor_clash_drops_prelude_type_and_dependents() {
        let src = "type IntList\n  Nil\n  Cons(Int, IntList)\n\nfn main() = println(1)\n";
        let m = parse_program(src).unwrap();
        assert!(!m.types.iter().any(|t| t.name == "List"));
        // map depends on Nil/Cons, so it must be gone.
        assert!(!m.functions.iter().any(|f| f.name == "map"));
        // but Option-based helpers survive... nth depends on List too — gone.
        assert!(!m.functions.iter().any(|f| f.name == "nth"));
        // string helpers that don't touch lists... repeat survives.
        assert!(m.functions.iter().any(|f| f.name == "repeat"));
    }
}

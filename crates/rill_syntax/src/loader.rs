//! Multi-file program loading.
//!
//! `import "name"` loads `name.rill` relative to the importing file's
//! directory (subdirectories via `import "lib/name"`). Imports resolve
//! depth-first before the importing file's own definitions, so library
//! functions are generalized before their users type-check. Each file loads
//! once (cycles are harmless); all files share one flat namespace, and a name
//! defined in two files is an error naming both. The prelude merges last and
//! is shadowed by any user definition.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{
    Expr, ExternFn, FnDef, ImplDef, Module, Pattern, SelectOp, Stmt, TraitDef, TypeDef,
    TypeRef,
};
use crate::parser::parse;
use crate::prelude::with_prelude;

pub fn load_program(entry: &Path) -> Result<Module, String> {
    let mut loader = Loader {
        visited: HashSet::new(),
        aliases: HashMap::new(),
        fn_origin: HashMap::new(),
        name_origin: HashMap::new(),
        types: Vec::new(),
        traits: Vec::new(),
        impls: Vec::new(),
        externs: Vec::new(),
        functions: Vec::new(),
    };
    loader.visit(entry, None, None)?;
    with_prelude(Module {
        imports: Vec::new(),
        types: loader.types,
        traits: loader.traits,
        impls: loader.impls,
        externs: loader.externs,
        functions: loader.functions,
        prelude_fns: 0,
    })
    .map_err(|e| e.to_string())
}

struct Loader {
    visited: HashSet<PathBuf>,
    /// Function name -> defining file.
    fn_origin: HashMap<String, PathBuf>,
    /// Type, constructor, trait, method or impl key -> defining file.
    name_origin: HashMap<String, PathBuf>,
    /// Canonical path -> the alias it was imported under, if any.
    aliases: HashMap<PathBuf, Option<String>>,
    types: Vec<TypeDef>,
    traits: Vec<TraitDef>,
    impls: Vec<ImplDef>,
    externs: Vec<ExternFn>,
    functions: Vec<FnDef>,
}

impl Loader {
    fn visit(
        &mut self,
        path: &Path,
        alias: Option<&str>,
        imported_from: Option<(&Path, usize)>,
    ) -> Result<(), String> {
        let display = path.display();
        let canon = path.canonicalize().map_err(|e| match imported_from {
            Some((by, line)) => {
                format!("{}: line {line}: cannot import `{display}`: {e}", by.display())
            }
            None => format!("cannot read `{display}`: {e}"),
        })?;
        if !self.visited.insert(canon.clone()) {
            let had = self.aliases.get(&canon).cloned().flatten();
            if had.as_deref() != alias {
                return Err(format!(
                    "`{}` is imported both {} and {}; pick one",
                    canon.display(),
                    had.map(|a| format!("as `{a}`")).unwrap_or_else(|| "unqualified".into()),
                    alias.map(|a| format!("as `{a}`")).unwrap_or("unqualified".into()),
                ));
            }
            return Ok(());
        }
        self.aliases.insert(canon.clone(), alias.map(str::to_string));

        let src = std::fs::read_to_string(&canon)
            .map_err(|e| format!("cannot read `{display}`: {e}"))?;
        let module = parse(&src).map_err(|e| format!("{display}: {e}"))?;

        let dir = canon.parent().map(Path::to_path_buf).unwrap_or_default();
        for imp in &module.imports {
            let child = dir.join(format!("{}.rill", imp.path));
            self.visit(&child, imp.alias.as_deref(), Some((path, imp.line)))?;
        }

        // An aliased module keeps its names to itself: everything it defines
        // is renamed `alias.name`, and its own references are renamed with it.
        let mut module = module;
        if let Some(a) = alias {
            qualify(&mut module, a);
        }

        for t in module.types {
            self.claim_name(&t.name, "type", &canon)?;
            for c in &t.ctors {
                self.claim_name(&c.name, "constructor", &canon)?;
            }
            self.types.push(t);
        }
        for t in module.traits {
            self.claim_name(&t.name, "trait", &canon)?;
            for m in &t.methods {
                self.claim_name(&m.name, "method", &canon)?;
            }
            self.traits.push(t);
        }
        for i in module.impls {
            let key = format!("{}({})", i.trait_name, i.target.name);
            self.claim_name(&key, "impl", &canon)?;
            self.impls.push(i);
        }
        for e in module.externs {
            // A C symbol has one meaning per program, so declaring it twice
            // is fine as long as nothing else claims the name.
            if !self.externs.iter().any(|x: &ExternFn| x.name == e.name) {
                self.externs.push(e);
            }
        }
        for f in module.functions {
            if let Some(prev) = self.fn_origin.get(&f.name) {
                return Err(format!(
                    "function `{}` is defined in both `{}` and `{}`",
                    f.name,
                    prev.display(),
                    canon.display()
                ));
            }
            self.fn_origin.insert(f.name.clone(), canon.clone());
            self.functions.push(f);
        }
        Ok(())
    }

    /// Types, constructors, traits and methods live in separate namespaces —
    /// `type Pair` with a `Pair` constructor is perfectly normal — so the key
    /// carries the kind.
    fn claim_name(&mut self, name: &str, what: &str, file: &Path) -> Result<(), String> {
        let key = format!("{what}:{name}");
        if let Some(prev) = self.name_origin.get(&key) {
            return Err(format!(
                "{what} `{name}` is defined in both `{}` and `{}`",
                prev.display(),
                file.display()
            ));
        }
        self.name_origin.insert(key, file.to_path_buf());
        Ok(())
    }
}

/// Renames everything a module defines to `alias.name`, and rewrites its own
/// references to match. References to names it imported are left alone.
fn qualify(m: &mut Module, alias: &str) {
    let mut own: HashSet<String> = HashSet::new();
    for t in &m.types {
        own.insert(t.name.clone());
        for c in &t.ctors {
            own.insert(c.name.clone());
        }
    }
    for f in &m.functions {
        own.insert(f.name.clone());
    }
    let q = |n: &str| format!("{alias}.{n}");

    for t in &mut m.types {
        t.name = q(&t.name);
        for c in &mut t.ctors {
            c.name = q(&c.name);
            for (_, ty) in &mut c.fields {
                qualify_type(ty, &own, alias);
            }
        }
    }
    for f in &mut m.functions {
        f.name = q(&f.name);
        for p in &mut f.params {
            if let Some(t) = &mut p.ann {
                qualify_type(t, &own, alias);
            }
        }
        if let Some(t) = &mut f.ret_ann {
            qualify_type(t, &own, alias);
        }
        qualify_expr(&mut f.body, &own, alias);
    }
}

fn qualify_type(t: &mut TypeRef, own: &HashSet<String>, alias: &str) {
    if own.contains(&t.name) {
        t.name = format!("{alias}.{}", t.name);
    }
    for a in &mut t.args {
        qualify_type(a, own, alias);
    }
    if let Some(r) = &mut t.ret {
        qualify_type(r, own, alias);
    }
}

fn qualify_pattern(p: &mut Pattern, own: &HashSet<String>, alias: &str) {
    if let Pattern::Ctor { name, args, .. } = p {
        if own.contains(name) {
            *name = format!("{alias}.{name}");
        }
        for a in args {
            qualify_pattern(a, own, alias);
        }
    }
}

fn qualify_expr(e: &mut Expr, own: &HashSet<String>, alias: &str) {
    match e {
        Expr::Var(n, _) => {
            if own.contains(n) {
                *n = format!("{alias}.{n}");
            }
        }
        Expr::Call { name, args, .. } => {
            if own.contains(name) {
                *name = format!("{alias}.{name}");
            }
            for a in args {
                qualify_expr(a, own, alias);
            }
        }
        Expr::Len { expr, .. } => qualify_expr(expr, own, alias),
        Expr::Slice { base, from, to, .. } => {
            qualify_expr(base, own, alias);
            if let Some(f) = from {
                qualify_expr(f, own, alias);
            }
            if let Some(t) = to {
                qualify_expr(t, own, alias);
            }
        }
        Expr::Index { base, index, .. } => {
            qualify_expr(base, own, alias);
            qualify_expr(index, own, alias);
        }
        Expr::IndexSet { base, index, value, .. }
        | Expr::IndexOp { base, index, value, .. } => {
            qualify_expr(base, own, alias);
            qualify_expr(index, own, alias);
            qualify_expr(value, own, alias);
        }
        Expr::For { from, to, body, .. } => {
            qualify_expr(from, own, alias);
            qualify_expr(to, own, alias);
            qualify_expr(body, own, alias);
        }
        Expr::Unary { expr, .. } => qualify_expr(expr, own, alias),
        Expr::Binary { lhs, rhs, .. } => {
            qualify_expr(lhs, own, alias);
            qualify_expr(rhs, own, alias);
        }
        Expr::If { cond, then, els } => {
            qualify_expr(cond, own, alias);
            qualify_expr(then, own, alias);
            qualify_expr(els, own, alias);
        }
        Expr::Match { scrutinee, arms, .. } => {
            qualify_expr(scrutinee, own, alias);
            for arm in arms {
                qualify_pattern(&mut arm.pattern, own, alias);
                qualify_expr(&mut arm.body, own, alias);
            }
        }
        Expr::Lambda { body, .. } | Expr::Spawn { body, .. } => qualify_expr(body, own, alias),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                match &mut a.op {
                    SelectOp::Recv { chan, .. } => qualify_expr(chan, own, alias),
                    SelectOp::Send { chan, value } => {
                        qualify_expr(chan, own, alias);
                        qualify_expr(value, own, alias);
                    }
                }
                qualify_expr(&mut a.body, own, alias);
            }
            if let Some(d) = default {
                qualify_expr(d, own, alias);
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                match s {
                    Stmt::Bind { value, .. } => qualify_expr(value, own, alias),
                    Stmt::PatBind { pattern, value, .. } => {
                        qualify_pattern(pattern, own, alias);
                        qualify_expr(value, own, alias);
                    }
                    Stmt::Fn(f) => qualify_expr(&mut f.body, own, alias),
                    Stmt::Expr(v) => qualify_expr(v, own, alias),
                }
            }
        }
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("rill-loader-{}", std::process::id()));
        std::fs::create_dir_all(d.join("lib")).unwrap();
        d
    }

    #[test]
    fn resolves_transitive_imports() {
        let d = dir();
        std::fs::write(d.join("a.rill"), "import \"b\"\n\nfn main() = println(two())\n").unwrap();
        std::fs::write(d.join("b.rill"), "import \"lib/c\"\n\nfn two() = one() + one()\n")
            .unwrap();
        std::fs::write(d.join("lib/c.rill"), "fn one() = 1\n").unwrap();
        let m = load_program(&d.join("a.rill")).unwrap();
        // dependency-first ordering: c's fn before b's before a's
        let names: Vec<&str> = m
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .filter(|n| matches!(*n, "one" | "two" | "main"))
            .collect();
        assert_eq!(names, vec!["one", "two", "main"]);
    }

    #[test]
    fn duplicate_across_files_names_both() {
        let d = dir();
        std::fs::write(d.join("x.rill"), "import \"y\"\n\nfn f() = 1\n\nfn main() = println(f())\n")
            .unwrap();
        std::fs::write(d.join("y.rill"), "fn f() = 2\n").unwrap();
        let err = load_program(&d.join("x.rill")).unwrap_err();
        assert!(err.contains("y.rill") && err.contains("x.rill"), "{err}");
    }

    #[test]
    fn missing_import_points_at_import_line() {
        let d = dir();
        std::fs::write(d.join("m.rill"), "import \"nope\"\n\nfn main() = println(1)\n").unwrap();
        let err = load_program(&d.join("m.rill")).unwrap_err();
        assert!(err.contains("line 1") && err.contains("nope"), "{err}");
    }

    #[test]
    fn import_cycles_are_harmless() {
        let d = dir();
        std::fs::write(d.join("p.rill"), "import \"q\"\n\nfn main() = println(g())\n").unwrap();
        std::fs::write(d.join("q.rill"), "import \"p\"\n\nfn g() = 7\n").unwrap();
        load_program(&d.join("p.rill")).unwrap();
    }
}

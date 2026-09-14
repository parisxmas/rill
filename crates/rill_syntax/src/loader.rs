//! Multi-file program loading.
//!
//! `import "name"` loads `name.rill` relative to the importing file's
//! directory (subdirectories via `import "lib/name"`), and failing that from
//! the search path — see `resolve`. Imports resolve
//! depth-first before the importing file's own definitions, so library
//! functions are generalized before their users type-check. Each file loads
//! once (cycles are harmless); all files share one flat namespace, and a name
//! defined in two files is an error naming both. The prelude merges last and
//! is shadowed by any user definition.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{
    AsmFn, Expr, ExternFn, FnDef, ImplDef, Module, Pattern, SelectOp, Stmt, TraitDef, TypeDef,
    TypeRef,
};
use crate::parser::parse;
use crate::prelude::with_prelude;

pub fn load_program(entry: &Path) -> Result<Module, String> {
    load_program_with(entry, None)
}

/// The same, with the entry file's text supplied rather than read. What an
/// editor has in front of somebody is not what is on disk, and the answers it
/// wants are about what is in front of them.
pub fn load_program_text(entry: &Path, text: &str) -> Result<Module, String> {
    load_program_with(entry, Some(text))
}

fn load_program_with(entry: &Path, text: Option<&str>) -> Result<Module, String> {
    let mut loader = Loader {
        overlay: text.map(|t| (entry.canonicalize().unwrap_or_else(|_| entry.into()), t.to_string())),
        visited: HashSet::new(),
        aliases: HashMap::new(),
        fn_origin: HashMap::new(),
        name_origin: HashMap::new(),
        types: Vec::new(),
        traits: Vec::new(),
        impls: Vec::new(),
        externs: Vec::new(),
        asms: Vec::new(),
        functions: Vec::new(),
    };
    loader.visit(entry, None, None)?;
    with_prelude(Module {
        imports: Vec::new(),
        types: loader.types,
        traits: loader.traits,
        impls: loader.impls,
        externs: loader.externs,
        asms: loader.asms,
        functions: loader.functions,
        prelude_fns: 0,
    })
    .map_err(|e| e.to_string())
}

/// Where `import "name"` looks. A file beside the importer wins, because a
/// program's own files are what it means most of the time and nothing on the
/// search path should be able to take that over. Only when there is no such
/// file does the search path get a turn, which is what lets two programs in
/// different directories share a library without either one spelling out the
/// way to the other.
///
/// Nothing found comes back as the path beside the importer, so the error
/// names the import as it was written rather than the last place searched.
fn resolve(dir: &Path, name: &str) -> PathBuf {
    let beside = dir.join(format!("{name}.rill"));
    if beside.is_file() {
        return beside;
    }
    for base in search_path(dir) {
        let candidate = base.join(format!("{name}.rill"));
        if candidate.is_file() {
            return candidate;
        }
    }
    beside
}

/// `RILL_PATH` first — a colon-separated list, for a library that lives
/// outside the project entirely — and then every `lib/` from the importing
/// file's own directory upwards.
///
/// The walk stops at the directory holding `.git`, and stops before the home
/// directory and the filesystem root in any case: a project with no `lib/` of
/// its own must fail to find the import rather than quietly find `/usr/lib`.
fn search_path(dir: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Ok(var) = std::env::var("RILL_PATH") {
        bases.extend(var.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    for ancestor in dir.ancestors() {
        if ancestor.parent().is_none() || Some(ancestor) == home.as_deref() {
            break;
        }
        bases.push(ancestor.join("lib"));
        if ancestor.join(".git").exists() {
            break;
        }
    }
    bases
}

struct Loader {
    visited: HashSet<PathBuf>,
    /// Function name -> defining file.
    fn_origin: HashMap<String, PathBuf>,
    /// Type, constructor, trait, method or impl key -> defining file.
    name_origin: HashMap<String, PathBuf>,
    /// Canonical path -> the alias it was imported under, if any.
    aliases: HashMap<PathBuf, Option<String>>,
    /// One file whose text is given rather than read, for an editor.
    overlay: Option<(PathBuf, String)>,
    types: Vec<TypeDef>,
    traits: Vec<TraitDef>,
    impls: Vec<ImplDef>,
    externs: Vec<ExternFn>,
    asms: Vec<AsmFn>,
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

        let src = match &self.overlay {
            Some((p, text)) if *p == canon => text.clone(),
            _ => std::fs::read_to_string(&canon)
                .map_err(|e| format!("cannot read `{display}`: {e}"))?,
        };
        let module = parse(&src).map_err(|e| format!("{display}: {e}"))?;

        let dir = canon.parent().map(Path::to_path_buf).unwrap_or_default();
        for imp in &module.imports {
            let child = resolve(&dir, &imp.path);
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
            // A method's name is its trait's: two traits may share one, so
            // what a file claims is the qualified name.
            for m in &t.methods {
                self.claim_name(&format!("{}.{}", t.name, m.name), "method", &canon)?;
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
        for a in module.asms {
            // The same rule as a C symbol above, and for the same reason: two
            // files may both declare the instruction they need.
            if !self.asms.iter().any(|x: &AsmFn| x.name == a.name) {
                self.asms.push(a);
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
        if let Some(a) = &mut t.alias {
            qualify_type(a, &own, alias);
        }
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
        // The parameters are names of this function's own, whatever the module
        // calls its functions.
        let mut scope: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
        qualify_expr(&mut f.body, &own, alias, &mut scope);
    }
    // An impl's target is one of the module's types, renamed like the rest;
    // its methods keep their names — a trait's methods are global — but their
    // bodies refer to the module's own functions and types like any other.
    for i in &mut m.impls {
        qualify_type(&mut i.target, &own, alias);
        for f in &mut i.fns {
            for p in &mut f.params {
                if let Some(t) = &mut p.ann {
                    qualify_type(t, &own, alias);
                }
            }
            if let Some(t) = &mut f.ret_ann {
                qualify_type(t, &own, alias);
            }
            let mut scope: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
            qualify_expr(&mut f.body, &own, alias, &mut scope);
        }
    }
}

/// Names bound where this expression is, which are nobody's to rename.
///
/// Without this, a module imported under an alias has every local binding
/// that happens to share a name with one of its own functions rewritten into a
/// reference to that function — so `ground = height(x, z)` inside a file that
/// also defines `fn ground(x, z)` becomes a function where a number was meant,
/// and the file compiles alone and fails the moment somebody imports it
/// `as v`. The rule is the ordinary one: the nearest binding wins.
fn bound(scope: &[String], n: &str) -> bool {
    scope.iter().any(|s| s == n)
}

/// Runs `f` with `names` added to the scope, and takes them off again.
fn under<T>(scope: &mut Vec<String>, names: impl IntoIterator<Item = String>, f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let depth = scope.len();
    scope.extend(names);
    let out = f(scope);
    scope.truncate(depth);
    out
}

/// The names a pattern binds.
fn pattern_binds(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Var(n) => out.push(n.clone()),
        Pattern::Ctor { args, .. } => {
            for a in args {
                pattern_binds(a, out);
            }
        }
        _ => {}
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

fn qualify_expr(e: &mut Expr, own: &HashSet<String>, alias: &str, scope: &mut Vec<String>) {
    match e {
        Expr::Var(n, _) => {
            if own.contains(n) && !bound(scope, n) {
                *n = format!("{alias}.{n}");
            }
        }
        Expr::Update { base, fields, .. } => {
            qualify_expr(base, own, alias, scope);
            for (_, v, _) in fields {
                qualify_expr(v, own, alias, scope);
            }
        }
        Expr::Call { name, args, .. } => {
            if own.contains(name) && !bound(scope, name) {
                *name = format!("{alias}.{name}");
            }
            for a in args {
                qualify_expr(a, own, alias, scope);
            }
        }
        Expr::Len { expr, .. } | Expr::Try { expr, .. } => qualify_expr(expr, own, alias, scope),
        Expr::Field { base, .. } => qualify_expr(base, own, alias, scope),
        Expr::Slice { base, from, to, .. } => {
            qualify_expr(base, own, alias, scope);
            if let Some(f) = from {
                qualify_expr(f, own, alias, scope);
            }
            if let Some(t) = to {
                qualify_expr(t, own, alias, scope);
            }
        }
        Expr::Index { base, index, .. } => {
            qualify_expr(base, own, alias, scope);
            qualify_expr(index, own, alias, scope);
        }
        Expr::IndexSet { base, index, value, .. }
        | Expr::IndexOp { base, index, value, .. } => {
            qualify_expr(base, own, alias, scope);
            qualify_expr(index, own, alias, scope);
            qualify_expr(value, own, alias, scope);
        }
        Expr::Append { target, value, .. } => {
            qualify_expr(target, own, alias, scope);
            qualify_expr(value, own, alias, scope);
        }
        Expr::For { var, from, to, acc, body, .. } => {
            // The bounds and the accumulator's first value are worked out
            // where the loop is written, before either name exists.
            qualify_expr(from, own, alias, scope);
            qualify_expr(to, own, alias, scope);
            let mut names = vec![var.clone()];
            if let Some((a, init)) = acc {
                qualify_expr(init, own, alias, scope);
                names.push(a.clone());
            }
            under(scope, names, |sc| qualify_expr(body, own, alias, sc));
        }
        Expr::While { cond, acc, body, .. } => {
            let mut names = Vec::new();
            if let Some((ps, init)) = acc {
                qualify_expr(init, own, alias, scope);
                for p in ps.iter_mut() {
                    if let Some(t) = &mut p.ann {
                        qualify_type(t, own, alias);
                    }
                    names.push(p.name.clone());
                }
            }
            under(scope, names, |sc| {
                qualify_expr(cond, own, alias, sc);
                qualify_expr(body, own, alias, sc);
            });
        }
        Expr::Unary { expr, .. } => qualify_expr(expr, own, alias, scope),
        Expr::Binary { lhs, rhs, .. } => {
            qualify_expr(lhs, own, alias, scope);
            qualify_expr(rhs, own, alias, scope);
        }
        Expr::If { cond, then, els, newline } => {
            qualify_expr(cond, own, alias, scope);
            qualify_expr(then, own, alias, scope);
            qualify_expr(els, own, alias, scope);
        }
        Expr::Match { scrutinee, arms, .. } => {
            qualify_expr(scrutinee, own, alias, scope);
            for arm in arms {
                qualify_pattern(&mut arm.pattern, own, alias);
                let mut names = Vec::new();
                pattern_binds(&arm.pattern, &mut names);
                under(scope, names, |sc| {
                    if let Some(g) = arm.guard.as_mut() {
                        qualify_expr(g, own, alias, sc);
                    }
                    qualify_expr(&mut arm.body, own, alias, sc);
                });
            }
        }
        Expr::Lambda { params, body, .. } => {
            under(scope, params.clone(), |sc| qualify_expr(body, own, alias, sc));
        }
        Expr::Spawn { body, .. } => qualify_expr(body, own, alias, scope),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                let mut names = Vec::new();
                match &mut a.op {
                    SelectOp::Recv { chan, bind } => {
                        qualify_expr(chan, own, alias, scope);
                        if let Some(n) = bind {
                            names.push(n.clone());
                        }
                    }
                    SelectOp::Send { chan, value } => {
                        qualify_expr(chan, own, alias, scope);
                        qualify_expr(value, own, alias, scope);
                    }
                    SelectOp::Timeout { ms } => qualify_expr(ms, own, alias, scope),
                }
                under(scope, names, |sc| qualify_expr(&mut a.body, own, alias, sc));
            }
            if let Some(d) = default {
                qualify_expr(d, own, alias, scope);
            }
        }
        Expr::Block(stmts) => {
            // A binding is in scope for the rest of the block and not for its
            // own right-hand side: `x = x + 1` reads whatever `x` meant
            // before, which here is the module's function of that name.
            let depth = scope.len();
            for s in stmts {
                match s {
                    Stmt::Bind { name, value, .. } => {
                        qualify_expr(value, own, alias, scope);
                        scope.push(name.clone());
                    }
                    Stmt::PatBind { pattern, value, .. } => {
                        qualify_expr(value, own, alias, scope);
                        qualify_pattern(pattern, own, alias);
                        pattern_binds(pattern, scope);
                    }
                    // A local function may call itself, so its own name goes
                    // into scope before its body is walked.
                    Stmt::Fn(f) => {
                        scope.push(f.name.clone());
                        let params: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
                        under(scope, params, |sc| qualify_expr(&mut f.body, own, alias, sc));
                    }
                    Stmt::Expr(v) => qualify_expr(v, own, alias, scope),
                }
            }
            scope.truncate(depth);
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

    /// A program in a directory of its own imports a library it is not beside
    /// and finds the project's `lib/`, which is the whole point: neither file
    /// has to know how far apart they are.
    #[test]
    fn the_search_path_finds_a_shared_library() {
        let d = dir();
        let nested = d.join("apps/one");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(d.join("lib/shared.rill"), "fn shared() = 41\n").unwrap();
        std::fs::write(
            nested.join("app.rill"),
            "import \"shared\"\n\nfn main() = println(shared() + 1)\n",
        )
        .unwrap();
        let m = load_program(&nested.join("app.rill")).unwrap();
        assert!(m.functions.iter().any(|f| f.name == "shared"), "lib/shared.rill was not found");
    }

    /// And never takes over a name the program keeps beside itself: the near
    /// file wins, or a library dropped into `lib/` could silently replace one
    /// of the program's own.
    #[test]
    fn a_file_beside_the_importer_beats_the_search_path() {
        let d = dir();
        let nested = d.join("apps/two");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(d.join("lib/pick.rill"), "fn pick() = 1\n").unwrap();
        std::fs::write(nested.join("pick.rill"), "fn pick() = 2\n\nfn near() = 0\n").unwrap();
        std::fs::write(nested.join("app.rill"), "import \"pick\"\n\nfn main() = println(pick())\n")
            .unwrap();
        let m = load_program(&nested.join("app.rill")).unwrap();
        assert!(m.functions.iter().any(|f| f.name == "near"), "the far `pick` was loaded");
    }

    #[test]
    fn missing_import_points_at_import_line() {
        let d = dir();
        std::fs::write(d.join("m.rill"), "import \"nope\"\n\nfn main() = println(1)\n").unwrap();
        let err = load_program(&d.join("m.rill")).unwrap_err();
        assert!(err.contains("line 1") && err.contains("nope"), "{err}");
    }

    /// An alias renames what the module defines, and nothing else. A local
    /// binding, a parameter, a loop variable or a name a pattern bound is the
    /// function's own, even when the module happens to define something of
    /// that name — and it used to be rewritten into a reference to it, so a
    /// file that compiled perfectly well alone stopped compiling the moment
    /// somebody imported it `as v`.
    #[test]
    fn an_alias_does_not_rename_local_names() {
        let d = dir();
        std::fs::write(
            d.join("u.rill"),
            "import \"w\" as v\n\nfn main() = println(v.pick(1.0) + v.total(3) + v.first(Some(9)))\n",
        )
        .unwrap();
        std::fs::write(
            d.join("w.rill"),
            concat!(
                "fn ground(x: Float, z: Float) -> Float = x + z\n",
                "\n",
                // The binding is fed by the very function it shadows.
                "fn pick(a) =\n",
                "  ground = ground(a, 2.0)\n",
                "  if a < ground then ground else a\n",
                "\n",
                "fn total(n) =\n",
                "  sum = 0\n",
                "  for i in 0..n with sum = 0\n",
                "    sum + i\n",
                "\n",
                "fn first(o) = match o\n",
                "  Some(ground) -> ground\n",
                "  None -> 0\n",
            ),
        )
        .unwrap();
        let m = load_program(&d.join("u.rill")).unwrap();
        let src = format!("{:?}", m.functions);
        assert!(!src.contains("Var(\"v.ground\""), "a local binding was qualified: {src}");
        assert!(!src.contains("Var(\"v.sum\""), "a loop's accumulator was qualified: {src}");
    }

    #[test]
    fn import_cycles_are_harmless() {
        let d = dir();
        std::fs::write(d.join("p.rill"), "import \"q\"\n\nfn main() = println(g())\n").unwrap();
        std::fs::write(d.join("q.rill"), "import \"p\"\n\nfn g() = 7\n").unwrap();
        load_program(&d.join("p.rill")).unwrap();
    }
}

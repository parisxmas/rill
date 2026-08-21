//! rill_types: type checking and inference for Rill.
//!
//! M2 scope: Hindley–Milner style inference with let-polymorphism at the
//! top-level function granularity, generic ADTs, first-class functions and
//! lambdas. The checker produces a fully typed AST (`TModule`) that codegen
//! monomorphizes — generic functions are instantiated per concrete type, so
//! nothing is boxed.
//!
//! Design notes:
//! - `Ty::Var` is an inference variable; none survive in the output.
//! - `Ty::Gen(i)` is a quantified type parameter: of the enclosing function's
//!   scheme inside `TFn` bodies, or of the ADT inside `CtorSig::fields`.
//! - Functions are generalized in definition order; a function used before
//!   its definition is pinned to the types of that earlier use (no
//!   polymorphic recursion), like OCaml.
//! - Unconstrained numeric/printable types default to Int.

use std::collections::{HashMap, HashSet};

use rill_syntax::{
    Arm, BinOp, Expr, FnDef, Module, Param, Pattern, SelectOp, Stmt, TypeRef, UnOp,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    /// An opaque C pointer. Not reference counted: whatever produced it owns
    /// it, which is why the FFI is the only place it appears.
    Ptr,
    /// ADT index into `TModule::adts`, plus type arguments.
    Adt(usize, Vec<Ty>),
    /// A function value: parameter types and return type.
    Fn(Vec<Ty>, Box<Ty>),
    /// A typed channel (`channel()`, `send`, `recv`).
    Chan(Box<Ty>),
    /// A counted block of memory holding elements of one C width. The only
    /// mutable thing in the language, and the only one C can be handed a
    /// pointer into: `Buf(F32)` is what a vertex array is.
    Buf(CTy),
    /// A hash table from keys to values. Storage like a `Buf` — what it holds
    /// changes without the name changing — but its two type arguments are
    /// ordinary types rather than a C width, so they are inferred as usual.
    Map(Box<Ty>, Box<Ty>),
    /// Bytes on their way to becoming a string. Storage like a `Buf`, and for
    /// the same reason: a string is a value, so building one by `+` copies
    /// what is already there every time.
    StrBuf,
    /// Inference variable; never present in checker output.
    Var(u32),
    /// Quantified type parameter (of the enclosing function or ADT).
    Gen(u32),
}

/// A type as C sees it, in a foreign signature. Rill's own numbers are 64-bit
/// while C is full of 32-bit ones, so an `extern` declaration names the exact
/// width the symbol was compiled with and the call boundary converts. Widths
/// are only spellable in an `extern`; the Rill side of the call still deals in
/// plain `Int` and `Float`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CTy {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    /// C's `size_t` and `ssize_t`: as wide as the target's pointer, which is
    /// the one width a signature cannot state as a number and stay portable.
    /// 64 bits everywhere Rill runs natively, 32 on `wasm32`.
    USize,
    ISize,
    Bool,
    Ptr,
    Unit,
    /// A C function pointer, taking these types and returning that one
    /// (`Unit` when the callback answers with nothing). Only a top-level Rill
    /// function can be passed for one: codegen wraps it in a C-ABI trampoline,
    /// and a closure has an environment C has nowhere to put.
    Fn(Vec<CTy>, Box<CTy>),
}

impl CTy {
    /// The Rill-visible type of an argument or result at this position.
    pub fn rill_ty(&self) -> Ty {
        match self {
            CTy::I8 | CTy::U8 | CTy::I16 | CTy::U16 | CTy::I32 | CTy::U32 | CTy::I64
            | CTy::U64 | CTy::USize | CTy::ISize => Ty::Int,
            CTy::F32 | CTy::F64 => Ty::Float,
            CTy::Bool => Ty::Bool,
            CTy::Ptr => Ty::Ptr,
            CTy::Unit => Ty::Unit,
            CTy::Fn(ps, r) => {
                Ty::Fn(ps.iter().map(|p| p.rill_ty()).collect(), Box::new(r.rill_ty()))
            }
        }
    }

    /// Width in bits of the integer forms that state one; `None` for the rest,
    /// `USize`/`ISize` among them — theirs is the target's to say.
    pub fn int_bits(&self) -> Option<u32> {
        match self {
            CTy::I8 | CTy::U8 => Some(8),
            CTy::I16 | CTy::U16 => Some(16),
            CTy::I32 | CTy::U32 => Some(32),
            CTy::I64 | CTy::U64 => Some(64),
            _ => None,
        }
    }

    /// How much memory one of these takes, which is what a buffer of them
    /// indexes by. `USize`/`ISize` are refused as buffer widths for exactly
    /// this reason — a buffer's layout is one thing and the target's word is
    /// another — so the 8 here is never an index.
    pub fn size_bytes(&self) -> u64 {
        match self {
            CTy::I8 | CTy::U8 | CTy::Bool => 1,
            CTy::I16 | CTy::U16 => 2,
            CTy::I32 | CTy::U32 | CTy::F32 => 4,
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Fn(..) => 8,
            CTy::USize | CTy::ISize => 8,
            CTy::Unit => 0,
        }
    }

    /// Whether a narrower result sign-extends back to Rill's `Int`.
    pub fn is_signed(&self) -> bool {
        matches!(self, CTy::I8 | CTy::I16 | CTy::I32 | CTy::I64 | CTy::ISize)
    }

    /// Whether the target decides the width, rather than the name.
    pub fn is_word_sized(&self) -> bool {
        matches!(self, CTy::USize | CTy::ISize)
    }

    /// The name this type is written with in a signature.
    pub fn name(&self) -> &'static str {
        match self {
            CTy::I8 => "I8",
            CTy::U8 => "U8",
            CTy::I16 => "I16",
            CTy::U16 => "U16",
            CTy::I32 => "I32",
            CTy::U32 => "U32",
            CTy::I64 => "Int",
            CTy::U64 => "U64",
            CTy::F32 => "F32",
            CTy::F64 => "Float",
            CTy::USize => "USize",
            CTy::ISize => "ISize",
            CTy::Bool => "Bool",
            CTy::Ptr => "Ptr",
            CTy::Unit => "Unit",
            CTy::Fn(..) => "Fn",
        }
    }
}

/// A foreign function's signature, in C's own types.
#[derive(Debug, Clone)]
pub struct ExternSig {
    pub params: Vec<CTy>,
    pub ret: CTy,
}

#[derive(Debug, Clone)]
pub struct CtorSig {
    pub name: String,
    /// Field types; `Ty::Gen(i)` refers to the ADT's i-th type parameter.
    pub fields: Vec<Ty>,
    /// What each field was called, where it was called anything. A name is
    /// what `p.x` reads it by, and a field without one can only be matched.
    pub names: Vec<Option<String>>,
    pub tag: u64,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct AdtDef {
    pub name: String,
    pub nparams: usize,
    pub ctors: Vec<CtorSig>,
}

// ---- typed AST ------------------------------------------------------------

/// Trait metadata for error messages and dispatch.
#[derive(Debug, Clone)]
pub struct TraitInfo {
    pub name: String,
    pub methods: Vec<String>,
}

/// Index of the built-in `Show` trait: implementing it overrides a type's
/// derived printing.
pub const SHOW_TRAIT: usize = 0;
/// Index of the built-in `Ord` trait, which powers `<`, `<=`, `>` and `>=`.
pub const ORD_TRAIT: usize = 1;

/// What a trait is implemented for (v1: builtins and non-generic ADTs).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImplKey {
    Int,
    Float,
    Bool,
    Str,
    Adt(usize),
}

#[derive(Debug)]
pub struct TModule {
    pub adts: Vec<AdtDef>,
    /// Constructor name -> (ADT index, constructor index).
    pub ctors: HashMap<String, (usize, usize)>,
    pub fns: Vec<TFn>,
    pub fn_index: HashMap<String, usize>,
    pub traits: Vec<TraitInfo>,
    /// (trait, method, implementing type) -> function index.
    pub impl_methods: HashMap<(usize, usize, ImplKey), usize>,
    /// C functions the program calls, by symbol name.
    pub externs: HashMap<String, ExternSig>,
    /// Libraries those symbols come from, for the linker.
    pub libs: Vec<String>,
    /// How many leading entries of `fns` came from the prelude. Trait impl
    /// methods are appended after the module's own functions, so everything
    /// from here on is code the program's author wrote.
    pub prelude_fns: usize,
}

#[derive(Debug)]
pub struct TFn {
    pub name: String,
    /// `export fn` — kept as a host-callable entry point on wasm.
    pub exported: bool,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    /// Number of quantified type parameters (`Ty::Gen(0..ngen)`).
    pub ngen: usize,
    /// Requirements on those type parameters, checked when the function is
    /// instantiated at a concrete type. The line is the use inside the body
    /// that imposed the requirement.
    pub constraints: Vec<(u32, Constraint, usize)>,
    pub body: TExpr,
    pub line: usize,
}

#[derive(Debug)]
pub struct TExpr {
    pub ty: Ty,
    pub line: usize,
    pub kind: TK,
}

#[derive(Debug)]
pub enum TK {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// A direct runtime-function builtin (`str_len`, `int_to_str`, ...).
    Builtin { rt: &'static str, args: Vec<TExpr> },
    /// A local (parameter, binding, pattern binding, or capture).
    Local(String),
    /// A top-level function used as a value, at the given instantiation.
    FnRef { fn_idx: usize, subst: Vec<Ty> },
    CallFn { fn_idx: usize, subst: Vec<Ty>, args: Vec<TExpr> },
    /// A trait method call; the receiver type picks the implementation at
    /// monomorphization time (fully static dispatch).
    MethodCall { trait_id: usize, method: usize, recv: Ty, args: Vec<TExpr> },
    /// A call straight into a C function declared with `extern`.
    CallExtern { name: String, args: Vec<TExpr> },
    CallCtor { adt: usize, ctor: usize, targs: Vec<Ty>, args: Vec<TExpr> },
    /// Calling a closure held in an expression (always a Local in M2 syntax).
    CallClosure { callee: Box<TExpr>, args: Vec<TExpr> },
    Println(Box<TExpr>),
    /// `print(x)` — like `println` without the trailing newline.
    Print(Box<TExpr>),
    /// `show(x)` — structural rendering of any value to Str (resolved per
    /// concrete type during monomorphization).
    Show(Box<TExpr>),
    /// `channel()` or `channel(capacity)` — a new channel of the
    /// expression's `Chan(elem)` type.
    ChanNew(Option<Box<TExpr>>),
    /// `send(ch, v)` — transfers ownership of `v` into the channel.
    ChanSend { ch: Box<TExpr>, val: Box<TExpr> },
    /// `recv(ch)` — receives an owned value.
    ChanRecv { ch: Box<TExpr> },
    /// `buf_f32(n)` and friends: a counted buffer of `n` elements.
    BufNew { width: CTy, count: Box<TExpr> },
    /// Reading and writing one element, bounds-checked.
    /// `s[i:j]` — the bytes from `i` up to but not `j`, either end the
    /// string's own when it was left out.
    Slice { base: Box<TExpr>, from: Box<TExpr>, to: Option<Box<TExpr>> },
    /// `map_new()` — an empty table of the expression's `Map(k, v)` type.
    MapNew,
    /// `m[k] = v` — the key and the value are moved into the table.
    MapSet { map: Box<TExpr>, key: Box<TExpr>, val: Box<TExpr> },
    /// `map_find(m, k)` — the slot holding `k`, or -1. Everything a program
    /// asks a map is written from this and the two `*_at` readers, which is
    /// why `map_get` can live in the prelude and return an `Option`.
    MapFind { map: Box<TExpr>, key: Box<TExpr> },
    MapDel { map: Box<TExpr>, key: Box<TExpr> },
    MapLen { map: Box<TExpr> },
    MapClear { map: Box<TExpr> },
    /// `map_next(m, i)` — the first slot at or after `i` holding an entry.
    MapNext { map: Box<TExpr>, from: Box<TExpr> },
    /// The key or the value in a slot, owned.
    MapKeyAt { map: Box<TExpr>, idx: Box<TExpr> },
    MapValAt { map: Box<TExpr>, idx: Box<TExpr> },
    /// `p.x` — one field of a box, read by the position its name has. The
    /// value comes back owned, as everything a program holds does.
    FieldGet { base: Box<TExpr>, adt: usize, ctor: usize, index: usize },
    BufGet { buf: Box<TExpr>, idx: Box<TExpr> },
    BufSet { buf: Box<TExpr>, idx: Box<TExpr>, val: Box<TExpr> },
    BufLen { buf: Box<TExpr> },
    /// The address of the first element, for handing to C.
    BufPtr { buf: Box<TExpr> },
    /// `spawn body` — runs `body` on a new green thread; like a zero-param
    /// closure, capturing its free variables by value.
    Spawn { captures: Vec<(String, Ty)>, body: Box<TExpr> },
    /// `select` — the runtime picks a ready arm; `default` runs when none is.
    Select { arms: Vec<TSelectArm>, default: Option<Box<TExpr>> },
    Neg(Box<TExpr>),
    Binary { op: BinOp, lhs: Box<TExpr>, rhs: Box<TExpr> },
    If { cond: Box<TExpr>, then: Box<TExpr>, els: Box<TExpr> },
    Match { scrutinee: Box<TExpr>, arms: Vec<TArm> },
    Block(Vec<TStmt>),
    Lambda { params: Vec<(String, Ty)>, captures: Vec<(String, Ty)>, body: Box<TExpr> },
}

#[derive(Debug)]
pub struct TSelectArm {
    pub op: TSelectOp,
    /// Name and type the received value is bound to, for a receive arm.
    pub bind: Option<(String, Ty)>,
    pub body: TExpr,
}

#[derive(Debug)]
pub enum TSelectOp {
    Recv { chan: TExpr, elem: Ty },
    Send { chan: TExpr, value: TExpr },
}

#[derive(Debug)]
pub struct TArm {
    pub pattern: Pattern,
    /// The arm is taken when the pattern matches and this is true. A false
    /// one falls through to the arms below.
    pub guard: Option<TExpr>,
    pub body: TExpr,
    pub line: usize,
}

#[derive(Debug)]
pub enum TStmt {
    Bind { name: String, value: TExpr },
    Expr(TExpr),
}

// ---- errors ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub line: usize,
    pub msg: String,
}

impl TypeError {
    fn new(line: usize, msg: impl Into<String>) -> Self {
        TypeError { line, msg: msg.into() }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "type error at line {}: {}", self.line, self.msg)
        } else {
            write!(f, "type error: {}", self.msg)
        }
    }
}

impl std::error::Error for TypeError {}

/// A requirement on a type that inference cannot settle on the spot. When
/// the type turns out to be a function's type parameter the requirement rides
/// along in its scheme and is checked once the type is known, so generic code
/// like `fn sort(l)` can use `<` without being pinned to one type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Constraint {
    /// Int or Float (arithmetic, ordering, negation).
    Numeric,
    /// Int, Float or Str (`+` — addition or string concatenation).
    Addable,
    /// Something `<` can compare: a number, a string, or a type with an
    /// `Ord` implementation.
    Ordered,
    /// Anything with structural equality: everything except functions and
    /// channels.
    Basic,
}

// ---- checker --------------------------------------------------------------

pub fn check(module: &Module) -> Result<TModule, TypeError> {
    check_all(module).map_err(|mut es| es.remove(0))
}

/// The same, reporting everything one pass can find rather than stopping at
/// the first. A body that does not check is recorded and stepped over, so the
/// functions after it are still checked — which is one error per function,
/// and what an editor wants to underline.
pub fn check_all(module: &Module) -> Result<TModule, Vec<TypeError>> {
    match Checker::new(module) {
        Ok(c) => c.run(module),
        Err(e) => Err(vec![e]),
    }
}

#[derive(Clone)]
enum SigState {
    /// Still var-based; uses before the definition pin these vars.
    Provisional { params: Vec<Ty>, ret: Ty },
    /// Generalized: `Gen(0..ngen)` may appear in params/ret.
    Generalized { params: Vec<Ty>, ret: Ty, ngen: usize },
}

/// A checked trait method: parameter/return types with `Gen(0)` standing for
/// the trait parameter.
#[derive(Clone)]
struct MethodInfo {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
}

struct Checker {
    adts: Vec<AdtDef>,
    ctors: HashMap<String, (usize, usize)>,
    fn_index: HashMap<String, usize>,
    sigs: Vec<SigState>,
    subst: Vec<Option<Ty>>,
    pending: Vec<(u32, Constraint, usize, &'static str)>,
    traits: Vec<(String, Vec<MethodInfo>)>,
    externs: HashMap<String, ExternSig>,
    libs: Vec<String>,
    /// Method name -> (trait index, method index). Methods are global.
    method_index: HashMap<String, (usize, usize)>,
    impl_methods: HashMap<(usize, usize, ImplKey), usize>,
    /// Impl functions get their signature from the trait, not inference:
    /// parameter types, return type, and how many type parameters the target
    /// type constructor has (zero for a plain type).
    forced_sigs: HashMap<String, (Vec<Ty>, Ty, usize)>,
    /// All functions to check: user functions plus mangled impl methods.
    expanded_fns: Vec<FnDef>,
    /// For each function index, the name it was written inside if it was
    /// lifted out of one. Only used to say something useful when a local
    /// function is asked to be two types at once — see below.
    lifted: Vec<Option<String>>,
    /// Carried through to `TModule`; see the field there.
    prelude_fns: usize,
    /// Per function: the inference-var id behind each `Gen` index, recorded at
    /// generalization time. Used to rebuild substitutions for calls that were
    /// made while the callee's signature was still provisional (recursion and
    /// forward references).
    gen_vars: Vec<Vec<u32>>,
}

impl Checker {
    fn new(module: &Module) -> Result<Checker, TypeError> {
        let mut c = Checker {
            adts: Vec::new(),
            ctors: HashMap::new(),
            fn_index: HashMap::new(),
            sigs: Vec::new(),
            subst: Vec::new(),
            pending: Vec::new(),
            traits: Vec::new(),
            externs: HashMap::new(),
            libs: Vec::new(),
            method_index: HashMap::new(),
            impl_methods: HashMap::new(),
            forced_sigs: HashMap::new(),
            expanded_fns: Vec::new(),
            lifted: Vec::new(),
            prelude_fns: 0,
            gen_vars: Vec::new(),
        };

        // ADT names first so fields can reference any ADT (incl. recursive).
        let mut adt_index: HashMap<String, usize> = HashMap::new();
        for td in &module.types {
            if is_builtin_type(&td.name) {
                return Err(TypeError::new(
                    td.line,
                    format!("`{}` is a builtin type and cannot be redefined", td.name),
                ));
            }
            if adt_index.insert(td.name.clone(), c.adts.len()).is_some() {
                return Err(TypeError::new(
                    td.line,
                    format!("type `{}` is defined more than once", td.name),
                ));
            }
            c.adts.push(AdtDef {
                name: td.name.clone(),
                nparams: td.params.len(),
                ctors: Vec::new(),
            });
        }
        for (i, td) in module.types.iter().enumerate() {
            let mut param_scope: HashMap<&str, u32> = HashMap::new();
            for (k, p) in td.params.iter().enumerate() {
                if param_scope.insert(p, k as u32).is_some() {
                    return Err(TypeError::new(
                        td.line,
                        format!("duplicate type parameter `{p}`"),
                    ));
                }
            }
            for (tag, ctor) in td.ctors.iter().enumerate() {
                if c.ctors.insert(ctor.name.clone(), (i, tag)).is_some() {
                    return Err(TypeError::new(
                        ctor.line,
                        format!(
                            "constructor `{}` is defined more than once \
                             (constructor names are global)",
                            ctor.name
                        ),
                    ));
                }
                let mut fields = Vec::new();
                let mut names = Vec::new();
                for (name, tr) in &ctor.fields {
                    if let Some(n) = name {
                        if names.contains(&Some(n.clone())) {
                            return Err(TypeError::new(
                                ctor.line,
                                format!(
                                    "constructor `{}` has two fields called `{n}`",
                                    ctor.name
                                ),
                            ));
                        }
                    }
                    names.push(name.clone());
                    let ty = resolve_type_ref(tr, &adt_index, &c.adts, Some(&param_scope))?;
                    if ty == Ty::Unit {
                        return Err(TypeError::new(
                            tr.line,
                            "constructor fields cannot be Unit".to_string(),
                        ));
                    }
                    fields.push(ty);
                }
                c.adts[i].ctors.push(CtorSig {
                    name: ctor.name.clone(),
                    fields,
                    names,
                    tag: tag as u64,
                    line: ctor.line,
                });
            }
        }

        // Built-in traits. They are not declared anywhere: implementing
        // `Show` replaces a type's derived printing, and implementing `Ord`
        // gives it `<`, `<=`, `>` and `>=`.
        c.traits.push((
            "Show".to_string(),
            vec![MethodInfo {
                name: "show".to_string(),
                params: vec![Ty::Gen(0)],
                ret: Ty::Str,
            }],
        ));
        c.traits.push((
            "Ord".to_string(),
            vec![MethodInfo {
                name: "compare".to_string(),
                params: vec![Ty::Gen(0), Ty::Gen(0)],
                ret: Ty::Int,
            }],
        ));
        // `compare` is callable directly; `show` stays the built-in function
        // and consults the impl table when the concrete type is known.
        c.method_index.insert("compare".to_string(), (ORD_TRAIT, 0));

        // Traits: method signatures with Gen(0) as the trait parameter.
        for td in &module.traits {
            if c.traits.iter().any(|(n, _)| *n == td.name) {
                return Err(TypeError::new(
                    td.line,
                    format!("trait `{}` is already defined", td.name),
                ));
            }
            if td.param.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
                return Err(TypeError::new(
                    td.line,
                    format!("trait parameter `{}` must be lowercase", td.param),
                ));
            }
            let tid = c.traits.len();
            let mut scope: HashMap<&str, u32> = HashMap::new();
            scope.insert(td.param.as_str(), 0);
            let mut methods = Vec::new();
            for m in &td.methods {
                if is_builtin_fn(&m.name) {
                    return Err(TypeError::new(
                        m.line,
                        format!("`{}` is a built-in function and cannot be a method", m.name),
                    ));
                }
                if c.method_index.contains_key(&m.name) {
                    return Err(TypeError::new(
                        m.line,
                        format!("method `{}` is already defined (method names are global)", m.name),
                    ));
                }
                let mut params = Vec::new();
                for tr in &m.params {
                    params.push(resolve_type_ref(tr, &adt_index, &c.adts, Some(&scope))?);
                }
                let ret = resolve_type_ref(&m.ret, &adt_index, &c.adts, Some(&scope))?;
                c.method_index.insert(m.name.clone(), (tid, methods.len()));
                methods.push(MethodInfo { name: m.name.clone(), params, ret });
            }
            c.traits.push((td.name.clone(), methods));
        }

        // Impls: each method becomes a mangled top-level function whose
        // signature is forced by the trait (v1 targets: builtins and
        // non-generic ADTs).
        c.expanded_fns = module.functions.clone();
        c.prelude_fns = module.prelude_fns;
        for im in &module.impls {
            let tid = match c.traits.iter().position(|(n, _)| *n == im.trait_name) {
                Some(t) => t,
                None => {
                    return Err(TypeError::new(
                        im.line,
                        format!("unknown trait `{}`", im.trait_name),
                    ));
                }
            };
            // The target names a type constructor, optionally with its
            // parameters: `impl Show(Money)` or `impl Show(List(a))`. A generic
            // impl covers the constructor at every instantiation, so its
            // methods become generic functions over the same parameters.
            let (key, target_ty, tgen) = match im.target.name.as_str() {
                "Int" => (ImplKey::Int, Ty::Int, 0),
                "Float" => (ImplKey::Float, Ty::Float, 0),
                "Bool" => (ImplKey::Bool, Ty::Bool, 0),
                "Str" => (ImplKey::Str, Ty::Str, 0),
                other => match adt_index.get(other) {
                    Some(&i) => {
                        let n = c.adts[i].nparams;
                        if im.target.args.len() != n {
                            return Err(TypeError::new(
                                im.target.line,
                                format!(
                                    "`{other}` takes {n} type parameter(s), but the impl target \
                                     names {}",
                                    im.target.args.len()
                                ),
                            ));
                        }
                        let mut seen: HashSet<&str> = HashSet::new();
                        for a in &im.target.args {
                            let lower = a.name.chars().next().is_some_and(|ch| ch.is_lowercase());
                            if !lower || !a.args.is_empty() || !seen.insert(a.name.as_str()) {
                                return Err(TypeError::new(
                                    a.line,
                                    "an impl target's type arguments must be distinct type \
                                     parameters, as in `impl Show(List(a))`"
                                        .to_string(),
                                ));
                            }
                        }
                        let args: Vec<Ty> = (0..n).map(|k| Ty::Gen(k as u32)).collect();
                        (ImplKey::Adt(i), Ty::Adt(i, args), n)
                    }
                    None => {
                        return Err(TypeError::new(
                            im.target.line,
                            format!("unknown type `{other}` in impl target"),
                        ));
                    }
                },
            };
            let trait_methods = c.traits[tid].1.clone();
            let mut implemented = vec![false; trait_methods.len()];
            for f in &im.fns {
                let mid = match trait_methods.iter().position(|m| m.name == f.name) {
                    Some(m) => m,
                    None => {
                        return Err(TypeError::new(
                            f.line,
                            format!("`{}` is not a method of trait `{}`", f.name, im.trait_name),
                        ));
                    }
                };
                if implemented[mid] {
                    return Err(TypeError::new(
                        f.line,
                        format!("method `{}` is implemented twice", f.name),
                    ));
                }
                implemented[mid] = true;
                let sig = &trait_methods[mid];
                if f.params.len() != sig.params.len() {
                    return Err(TypeError::new(
                        f.line,
                        format!(
                            "method `{}` takes {} parameter(s) in trait `{}`, but this \
                             implementation has {}",
                            f.name,
                            sig.params.len(),
                            im.trait_name,
                            f.params.len()
                        ),
                    ));
                }
                if f.params.iter().any(|p| p.ann.is_some()) || f.ret_ann.is_some() {
                    return Err(TypeError::new(
                        f.line,
                        "impl methods take their types from the trait; remove the annotations"
                            .to_string(),
                    ));
                }
                let inst = [target_ty.clone()];
                let params: Vec<Ty> = sig.params.iter().map(|t| subst_gen(t, &inst)).collect();
                let ret = subst_gen(&sig.ret, &inst);
                let mangled = format!("{}.{}.{}", im.trait_name, im.target.name, f.name);
                if c.impl_methods.insert((tid, mid, key.clone()), c.expanded_fns.len()).is_some()
                {
                    return Err(TypeError::new(
                        im.line,
                        format!(
                            "trait `{}` is already implemented for `{}`",
                            im.trait_name, im.target.name
                        ),
                    ));
                }
                c.forced_sigs.insert(mangled.clone(), (params, ret, tgen));
                let mut renamed = f.clone();
                renamed.name = mangled;
                c.expanded_fns.push(renamed);
            }
            if let Some(mid) = implemented.iter().position(|b| !b) {
                return Err(TypeError::new(
                    im.line,
                    format!(
                        "impl of `{}` for `{}` is missing method `{}`",
                        im.trait_name, im.target.name, trait_methods[mid].name
                    ),
                ));
            }
        }

        // Foreign declarations. Their types are written out, never inferred,
        // and only shapes the C ABI can carry are allowed.
        for e in &module.externs {
            if is_builtin_fn(&e.name) || c.method_index.contains_key(&e.name) {
                return Err(TypeError::new(
                    e.line,
                    format!("`{}` is already a built-in or a trait method", e.name),
                ));
            }
            let mut params = Vec::new();
            for (pname, tr) in &e.params {
                let what = format!("parameter `{pname}`");
                let cty = resolve_c_type(tr, &adt_index, &c.adts, &what)?;
                if cty == CTy::Unit {
                    return Err(TypeError::new(
                        tr.line,
                        format!("{what}: `Unit` is not a value C can be passed"),
                    ));
                }
                params.push(cty);
            }
            let ret = match &e.ret {
                Some(tr) => {
                    let r = resolve_c_type(tr, &adt_index, &c.adts, "return type")?;
                    if matches!(r, CTy::Fn(..)) {
                        return Err(TypeError::new(
                            tr.line,
                            "return type: a foreign function returning a callback \
                             cannot be called back into; declare it `Ptr`"
                                .to_string(),
                        ));
                    }
                    r
                }
                None => CTy::Unit,
            };
            c.externs.insert(e.name.clone(), ExternSig { params, ret });
            if let Some(lib) = &e.lib {
                if !c.libs.contains(lib) {
                    c.libs.push(lib.clone());
                }
            }
        }

        // Provisional function signatures.
        let expanded = c.expanded_fns.clone();
        for f in &expanded {
            if c.fn_index.contains_key(&f.name) {
                return Err(TypeError::new(
                    f.name_line(),
                    format!("function `{}` is defined more than once", f.name),
                ));
            }
            if is_builtin_fn(&f.name) {
                return Err(TypeError::new(
                    f.line,
                    format!("`{}` is a built-in function", f.name),
                ));
            }
            if c.externs.contains_key(&f.name) {
                return Err(TypeError::new(
                    f.line,
                    format!("`{}` is already declared as a foreign function", f.name),
                ));
            }
            if c.method_index.contains_key(&f.name) {
                return Err(TypeError::new(
                    f.line,
                    format!("`{}` is a trait method; functions cannot share its name", f.name),
                ));
            }
            if let Some((params, ret, tgen)) = c.forced_sigs.get(&f.name).cloned() {
                // The signature comes from the trait. A generic impl's type
                // parameters become inference variables while the body is
                // checked, then generalize back at the end — and must, which
                // `run` verifies.
                let inst: Vec<Ty> = (0..tgen).map(|_| c.fresh()).collect();
                c.fn_index.insert(f.name.clone(), c.sigs.len());
                c.sigs.push(SigState::Provisional {
                    params: params.iter().map(|t| subst_gen(t, &inst)).collect(),
                    ret: subst_gen(&ret, &inst),
                });
                continue;
            }
            if f.name == "main" {
                if !f.params.is_empty() {
                    return Err(TypeError::new(f.line, "`main` must take no parameters"));
                }
                if f.ret_ann.is_some() {
                    return Err(TypeError::new(
                        f.line,
                        "`main` must not have a return type annotation",
                    ));
                }
                c.fn_index.insert("main".into(), c.sigs.len());
                c.sigs.push(SigState::Provisional { params: vec![], ret: Ty::Unit });
                continue;
            }
            let mut params = Vec::new();
            for Param { name, ann } in &f.params {
                let ty = match ann {
                    Some(tr) => {
                        let ty = resolve_type_ref(tr, &adt_index, &c.adts, None)?;
                        if ty == Ty::Unit {
                            return Err(TypeError::new(
                                tr.line,
                                format!("parameter `{name}` cannot be Unit"),
                            ));
                        }
                        ty
                    }
                    None => c.fresh(),
                };
                params.push(ty);
            }
            let ret = match &f.ret_ann {
                Some(tr) => resolve_type_ref(tr, &adt_index, &c.adts, None)?,
                None => c.fresh(),
            };
            c.fn_index.insert(f.name.clone(), c.sigs.len());
            c.sigs.push(SigState::Provisional { params, ret });
        }
        if !c.fn_index.contains_key("main") {
            return Err(TypeError::new(0, "no `main` function defined"));
        }
        Ok(c)
    }

    fn run(mut self, module: &Module) -> Result<TModule, Vec<TypeError>> {
        let _ = module;
        self.gen_vars = vec![Vec::new(); self.sigs.len()];

        // Phase 1: infer and generalize every function, impl methods
        // included. Order matters: a function is only generalized once
        // everything it calls has been, so mutually recursive functions are
        // handled as one group and a function whose type is settled by a
        // later definition still comes out right.
        let fns_to_check = std::mem::take(&mut self.expanded_fns);
        self.lifted = vec![None; self.sigs.len()];
        for f in &fns_to_check {
            if let (Some(&i), Some(outer)) = (self.fn_index.get(&f.name), f.lifted_from.as_ref()) {
                self.lifted[i] = Some(outer.clone());
            }
        }
        let mut tfns: Vec<Option<TFn>> = (0..fns_to_check.len()).map(|_| None).collect();
        // A function whose body does not check is stepped over: its signature
        // stays provisional, which is a fresh variable at every call, so the
        // functions after it mostly check as though it were fine and each
        // says its own first problem rather than the first one in the file.
        let mut errors: Vec<TypeError> = Vec::new();
        for group in dependency_groups(&fns_to_check, &self.fn_index) {
            // Infer the whole group with every signature still provisional...
            let mut inferred = Vec::new();
            for &k in &group {
                let f = &fns_to_check[k];
                let idx = self.fn_index[&f.name];
                let start = self.pending.len();
                match self.infer_fn_body(f, idx) {
                    Ok(body) => inferred.push((k, idx, body, start, self.pending.len())),
                    Err(e) => errors.push(e),
                }
            }
            // ...then generalize it, so recursive calls inside see the same
            // types the group settled on.
            for (k, idx, body, start, end) in inferred {
                match self.generalize_fn(&fns_to_check[k], idx, body, start, end) {
                    Ok(tfn) => tfns[idx] = Some(tfn),
                    Err(e) => errors.push(e),
                }
            }
        }
        self.pending.clear();
        if !errors.is_empty() {
            // Two functions can arrive at the same complaint about the same
            // line; an editor should show it once.
            errors.sort_by_key(|e| e.line);
            errors.dedup_by(|a, b| a.line == b.line && a.msg == b.msg);
            return Err(errors);
        }

        // Phase 2: zonk all bodies now that every signature is generalized,
        // rebuilding substitutions for calls made against provisional sigs.
        let mut tfns: Vec<TFn> = tfns.into_iter().map(|f| f.unwrap()).collect();
        for (idx, tfn) in tfns.iter_mut().enumerate() {
            let genmap: HashMap<u32, u32> = self.gen_vars[idx]
                .iter()
                .enumerate()
                .map(|(g, &v)| (v, g as u32))
                .collect();
            let mut defaults: HashMap<u32, Ty> = HashMap::new();
            self.zonk_expr(&mut tfn.body, &genmap, &mut defaults);
        }

        // A generic impl must stay generic: if the body pinned the target's
        // type parameter to something concrete, the impl does not actually
        // cover the type constructor it claims to.
        for (name, (_, _, tgen)) in &self.forced_sigs {
            let f = &tfns[self.fn_index[name]];
            if f.ngen < *tgen {
                let (trait_name, target) = name.split_once('.').unwrap_or(("", name));
                let target = target.split_once('.').map(|(t, _)| t).unwrap_or(target);
                return Err(vec![TypeError::new(
                    f.line,
                    format!(
                        "this implementation of `{trait_name}` is not general enough: it only \
                         works for some `{target}` types, but the impl covers all of them"
                    ),
                )]);
            }
        }

        Ok(TModule {
            adts: self.adts,
            ctors: self.ctors,
            fns: tfns,
            fn_index: self.fn_index,
            traits: self
                .traits
                .into_iter()
                .map(|(name, ms)| TraitInfo {
                    name,
                    methods: ms.into_iter().map(|m| m.name).collect(),
                })
                .collect(),
            impl_methods: self.impl_methods,
            externs: self.externs,
            libs: self.libs,
            prelude_fns: self.prelude_fns,
        })
    }

    // ---- inference machinery --------------------------------------------

    fn fresh(&mut self) -> Ty {
        let v = self.subst.len() as u32;
        self.subst.push(None);
        Ty::Var(v)
    }

    /// Shallow resolution: follows var links at the top level only.
    fn resolve(&self, ty: &Ty) -> Ty {
        let mut ty = ty.clone();
        while let Ty::Var(v) = ty {
            match &self.subst[v as usize] {
                Some(next) => ty = next.clone(),
                None => return Ty::Var(v),
            }
        }
        ty
    }

    fn occurs(&self, v: u32, ty: &Ty) -> bool {
        match self.resolve(ty) {
            Ty::Var(w) => w == v,
            Ty::Adt(_, args) => args.iter().any(|a| self.occurs(v, a)),
            Ty::Fn(params, ret) => {
                params.iter().any(|p| self.occurs(v, p)) || self.occurs(v, &ret)
            }
            Ty::Chan(e) => self.occurs(v, &e),
            Ty::Map(k, val) => self.occurs(v, &k) || self.occurs(v, &val),
            Ty::Buf(_) => false,
            _ => false,
        }
    }

    fn unify(&mut self, a: &Ty, b: &Ty, line: usize, ctx: &str) -> Result<(), TypeError> {
        let (ra, rb) = (self.resolve(a), self.resolve(b));
        match (&ra, &rb) {
            (Ty::Var(v), Ty::Var(w)) if v == w => Ok(()),
            (Ty::Var(v), other) | (other, Ty::Var(v)) => {
                if self.occurs(*v, other) {
                    return Err(TypeError::new(line, format!("{ctx}: infinite type")));
                }
                self.subst[*v as usize] = Some(other.clone());
                Ok(())
            }
            (Ty::Adt(i, xs), Ty::Adt(j, ys)) if i == j => {
                for (x, y) in xs.clone().iter().zip(ys.clone().iter()) {
                    self.unify(x, y, line, ctx)?;
                }
                Ok(())
            }
            (Ty::Fn(xp, xr), Ty::Fn(yp, yr)) if xp.len() == yp.len() => {
                let (xp, xr, yp, yr) = (xp.clone(), xr.clone(), yp.clone(), yr.clone());
                for (x, y) in xp.iter().zip(yp.iter()) {
                    self.unify(x, y, line, ctx)?;
                }
                self.unify(&xr, &yr, line, ctx)
            }
            (Ty::Buf(x), Ty::Buf(y)) if x == y => Ok(()),
            (Ty::Chan(x), Ty::Chan(y)) => {
                let (x, y) = (x.clone(), y.clone());
                self.unify(&x, &y, line, ctx)
            }
            (Ty::Map(xk, xv), Ty::Map(yk, yv)) => {
                let (xk, xv, yk, yv) = (xk.clone(), xv.clone(), yk.clone(), yv.clone());
                self.unify(&xk, &yk, line, ctx)?;
                self.unify(&xv, &yv, line, ctx)
            }
            _ if ra == rb => Ok(()),
            _ => Err(TypeError::new(
                line,
                format!("{ctx}: expected {}, found {}", self.type_name(&ra), self.type_name(&rb)),
            )),
        }
    }

    fn type_name(&self, ty: &Ty) -> String {
        match self.resolve(ty) {
            // Only the head is resolved by the match; the arguments below go
            // back through this so that a variable inside one is resolved too.
            Ty::Int => "Int".into(),
            Ty::Float => "Float".into(),
            Ty::Bool => "Bool".into(),
            Ty::Str => "Str".into(),
            Ty::Unit => "Unit".into(),
            Ty::Ptr => "Ptr".into(),
            Ty::Adt(i, args) => {
                if args.is_empty() {
                    self.adts[i].name.clone()
                } else {
                    let args: Vec<String> = args.iter().map(|a| self.type_name(a)).collect();
                    format!("{}({})", self.adts[i].name, args.join(", "))
                }
            }
            Ty::Fn(params, ret) => {
                let params: Vec<String> = params.iter().map(|p| self.type_name(p)).collect();
                format!("({}) -> {}", params.join(", "), self.type_name(&ret))
            }
            Ty::Chan(e) => format!("Chan({})", self.type_name(&e)),
            Ty::Map(k, v) => format!("Map({}, {})", self.type_name(&k), self.type_name(&v)),
            Ty::StrBuf => "StrBuf".into(),
            Ty::Buf(w) => format!("Buf({})", w.name()),
            Ty::Var(_) => "_".into(),
            Ty::Gen(i) => format!("'{}", (b'a' + (i % 26) as u8) as char),
        }
    }

    fn check_need(&self, ty: &Ty, need: Constraint, line: usize, ctx: &str) -> Result<(), TypeError> {
        let ok = match need {
            Constraint::Numeric => matches!(ty, Ty::Int | Ty::Float),
            Constraint::Addable => matches!(ty, Ty::Int | Ty::Float | Ty::Str),
            Constraint::Ordered => {
                matches!(ty, Ty::Int | Ty::Float | Ty::Str) || self.has_impl(ORD_TRAIT, ty)
            }
            Constraint::Basic => {
                matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Adt(..))
            }
        };
        if ok {
            Ok(())
        } else {
            let allowed = match need {
                Constraint::Numeric => "Int or Float",
                Constraint::Addable => "Int, Float or Str",
                Constraint::Ordered => "Int, Float, Str or a type with an `Ord` implementation",
                Constraint::Basic => "a value type (not a function, channel or storage)",
            };
            // Storage is the interesting refusal: a map or a buffer is a
            // thing that changes, so printing or comparing one is a question
            // about a moment rather than about a value. Both can be asked as
            // one, and the answer says how.
            let hint = match ty {
                Ty::Map(..) => " — `map_items(m)` is a map as a value",
                Ty::Buf(_) => " — a buffer is storage; measure it or read an element",
                _ => "",
            };
            Err(TypeError::new(
                line,
                format!("{ctx} requires {allowed}, found {}{hint}", self.type_name(ty)),
            ))
        }
    }

    /// Whether a concrete type implements one of the built-in traits.
    fn has_impl(&self, trait_id: usize, ty: &Ty) -> bool {
        impl_key(ty).is_some_and(|k| self.impl_methods.contains_key(&(trait_id, 0, k)))
    }

    /// An argument at a position that wants a `Ptr`. A buffer handed to one
    /// is its first element's address — which is what C is always given, and
    /// what `buf_data` used to be written for. The buffer stays counted and
    /// stays the caller's; what crosses is an address, good for as long as the
    /// call it was written in.
    fn arg_for(
        &mut self,
        a: &Expr,
        want: &Ty,
        env: &mut HashMap<String, Ty>,
        line: usize,
        ctx: &str,
    ) -> Result<TExpr, TypeError> {
        let at = self.infer(a, env)?;
        if matches!(want, Ty::Ptr) && matches!(self.resolve(&at.ty), Ty::Buf(_)) {
            let line = at.line;
            return Ok(TExpr { ty: Ty::Ptr, line, kind: TK::BufPtr { buf: Box::new(at) } });
        }
        self.unify(want, &at.ty, line, ctx)?;
        Ok(at)
    }

    fn require(
        &mut self,
        ty: &Ty,
        need: Constraint,
        line: usize,
        ctx: &'static str,
    ) -> Result<(), TypeError> {
        match self.resolve(ty) {
            Ty::Var(v) => {
                self.pending.push((v, need, line, ctx));
                Ok(())
            }
            ty => self.check_need(&ty, need, line, ctx),
        }
    }

    /// Instantiates a signature: `Gen(i)` -> fresh vars. Returns the fresh
    /// vars used, in Gen order, as the recorded substitution.
    fn instantiate(&mut self, params: &[Ty], ret: &Ty, ngen: usize) -> (Vec<Ty>, Ty, Vec<Ty>) {
        let inst: Vec<Ty> = (0..ngen).map(|_| self.fresh()).collect();
        let p = params.iter().map(|t| subst_gen(t, &inst)).collect();
        let r = subst_gen(ret, &inst);
        (p, r, inst)
    }

    // ---- per-function check + finalize ----------------------------------

    /// Infers a function body against its provisional signature, leaving
    /// generalization to the caller.
    fn infer_fn_body(&mut self, f: &FnDef, idx: usize) -> Result<TExpr, TypeError> {
        let (sig_params, sig_ret) = match self.sigs[idx].clone() {
            SigState::Provisional { params, ret } => (params, ret),
            SigState::Generalized { .. } => unreachable!("checked twice"),
        };

        let mut env: HashMap<String, Ty> = HashMap::new();
        for (p, ty) in f.params.iter().zip(&sig_params) {
            if env.insert(p.name.clone(), ty.clone()).is_some() {
                return Err(TypeError::new(
                    f.line,
                    format!("duplicate parameter `{}` in `{}`", p.name, f.name),
                ));
            }
        }
        let body = self.infer(&f.body, &mut env)?;
        if f.name != "main" {
            self.unify(&sig_ret, &body.ty, f.line, "function body type")?;
        }
        Ok(body)
    }

    /// Turns a checked body into a scheme: settles the requirements it
    /// collected, then quantifies whatever type variables are still free.
    fn generalize_fn(
        &mut self,
        f: &FnDef,
        idx: usize,
        body: TExpr,
        pending_start: usize,
        pending_end: usize,
    ) -> Result<TFn, TypeError> {
        let (sig_params, sig_ret) = match self.sigs[idx].clone() {
            SigState::Provisional { params, ret } => (params, ret),
            SigState::Generalized { .. } => unreachable!("generalized twice"),
        };

        // --- finalize this function's signature ---
        // 1. Requirements collected while checking the body: settle the ones
        //    whose type is already known, keep the rest for step 3.
        let window: Vec<_> = self.pending[pending_start..pending_end].to_vec();
        let mut deferred = Vec::new();
        for (v, need, line, ctx) in window {
            match self.resolve(&Ty::Var(v)) {
                Ty::Var(v) => deferred.push((v, need, line, ctx)),
                ty => self.check_need(&ty, need, line, ctx)?,
            }
        }

        // 2. Generalize remaining free vars in the signature. This only
        //    records the mapping — nothing is bound in the global
        //    substitution, so later functions can still refer to these vars.
        let mut genmap: HashMap<u32, u32> = HashMap::new();
        let mut order: Vec<u32> = Vec::new();
        let mut params = Vec::new();
        for (p, ty) in f.params.iter().zip(&sig_params) {
            params.push((p.name.clone(), self.generalize(ty, &mut genmap, &mut order)));
        }
        let ret = self.generalize(&sig_ret, &mut genmap, &mut order);
        let ngen = genmap.len();
        self.gen_vars[idx] = order;

        // 3. A requirement on a type parameter travels with the scheme; one
        //    on a variable that stayed local has nothing to constrain it, so
        //    it defaults to Int the way an unannotated literal would.
        let mut constraints = Vec::new();
        for (v, need, line, ctx) in deferred {
            match genmap.get(&v) {
                Some(&g) => {
                    if !constraints.iter().any(|(cg, cn, _)| *cg == g && *cn == need) {
                        constraints.push((g, need, line));
                    }
                }
                None => {
                    self.subst[v as usize] = Some(Ty::Int);
                    self.check_need(&Ty::Int, need, line, ctx)?;
                }
            }
        }
        self.sigs[idx] = SigState::Generalized {
            params: params.iter().map(|(_, t)| t.clone()).collect(),
            ret: ret.clone(),
            ngen,
        };

        // The body is zonked in phase 2, once every signature is generalized.
        Ok(TFn { name: f.name.clone(), exported: f.exported, params, ret, ngen, constraints, body, line: f.line })
    }

    /// Resolves a type; unresolved vars get fresh `Gen` indices.
    fn generalize(&self, ty: &Ty, genmap: &mut HashMap<u32, u32>, order: &mut Vec<u32>) -> Ty {
        match self.resolve(ty) {
            Ty::Var(v) => {
                if let Some(&g) = genmap.get(&v) {
                    return Ty::Gen(g);
                }
                let g = genmap.len() as u32;
                genmap.insert(v, g);
                order.push(v);
                Ty::Gen(g)
            }
            Ty::Adt(i, args) => {
                Ty::Adt(i, args.iter().map(|a| self.generalize(a, genmap, order)).collect())
            }
            Ty::Fn(p, r) => Ty::Fn(
                p.iter().map(|t| self.generalize(t, genmap, order)).collect(),
                Box::new(self.generalize(&r, genmap, order)),
            ),
            Ty::Chan(e) => Ty::Chan(Box::new(self.generalize(&e, genmap, order))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(self.generalize(&k, genmap, order)),
                Box::new(self.generalize(&v, genmap, order)),
            ),
            Ty::Buf(w) => Ty::Buf(w.clone()),
            other => other,
        }
    }

    /// Phase-2 resolution: vars of the enclosing function's scheme map to
    /// `Gen`, anything else defaults to Int. Defaults are recorded per
    /// function (`defaults`), never bound globally, so one function's
    /// defaulting cannot leak into another's generalized vars.
    fn zonk(
        &self,
        ty: &Ty,
        genmap: &HashMap<u32, u32>,
        defaults: &mut HashMap<u32, Ty>,
    ) -> Ty {
        match self.resolve(ty) {
            Ty::Var(v) => {
                if let Some(&g) = genmap.get(&v) {
                    return Ty::Gen(g);
                }
                defaults.entry(v).or_insert(Ty::Int).clone()
            }
            Ty::Adt(i, args) => {
                Ty::Adt(i, args.iter().map(|a| self.zonk(a, genmap, defaults)).collect())
            }
            Ty::Fn(p, r) => Ty::Fn(
                p.iter().map(|t| self.zonk(t, genmap, defaults)).collect(),
                Box::new(self.zonk(&r, genmap, defaults)),
            ),
            Ty::Chan(e) => Ty::Chan(Box::new(self.zonk(&e, genmap, defaults))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(self.zonk(&k, genmap, defaults)),
                Box::new(self.zonk(&v, genmap, defaults)),
            ),
            Ty::Buf(w) => Ty::Buf(w.clone()),
            other => other,
        }
    }

    /// Rebuilds/normalizes a call's instantiation list. Calls made while the
    /// callee was provisional recorded an empty substitution; the callee's
    /// `gen_vars` tell us which inference vars ended up quantified, and
    /// resolving those in the caller's context yields the true instantiation.
    fn zonk_call_subst(
        &self,
        fn_idx: usize,
        subst: &mut Vec<Ty>,
        genmap: &HashMap<u32, u32>,
        defaults: &mut HashMap<u32, Ty>,
    ) {
        let ngen = match &self.sigs[fn_idx] {
            SigState::Generalized { ngen, .. } => *ngen,
            SigState::Provisional { .. } => unreachable!("all sigs generalized in phase 2"),
        };
        if subst.len() != ngen {
            debug_assert!(subst.is_empty(), "partial substitution cannot happen");
            *subst = self.gen_vars[fn_idx]
                .iter()
                .map(|&v| self.zonk(&Ty::Var(v), genmap, defaults))
                .collect();
        } else {
            for t in subst.iter_mut() {
                *t = self.zonk(t, genmap, defaults);
            }
        }
    }

    fn zonk_expr(
        &self,
        e: &mut TExpr,
        genmap: &HashMap<u32, u32>,
        defaults: &mut HashMap<u32, Ty>,
    ) {
        e.ty = self.zonk(&e.ty, genmap, defaults);
        match &mut e.kind {
            TK::Int(_) | TK::Float(_) | TK::Str(_) | TK::Bool(_) | TK::Local(_) => {}
            TK::Builtin { args, .. } => {
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::FnRef { fn_idx, subst } => {
                self.zonk_call_subst(*fn_idx, subst, genmap, defaults);
            }
            TK::CallFn { fn_idx, subst, args } => {
                self.zonk_call_subst(*fn_idx, subst, genmap, defaults);
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::CallExtern { args, .. } => {
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::BufNew { count, .. } => self.zonk_expr(count, genmap, defaults),
            TK::BufLen { buf } | TK::BufPtr { buf } => {
                self.zonk_expr(buf, genmap, defaults)
            }
            TK::Slice { base, from, to } => {
                self.zonk_expr(base, genmap, defaults);
                self.zonk_expr(from, genmap, defaults);
                if let Some(t) = to {
                    self.zonk_expr(t, genmap, defaults);
                }
            }
            TK::BufGet { buf, idx } => {
                self.zonk_expr(buf, genmap, defaults);
                self.zonk_expr(idx, genmap, defaults);
            }
            TK::BufSet { buf, idx, val } => {
                self.zonk_expr(buf, genmap, defaults);
                self.zonk_expr(idx, genmap, defaults);
                self.zonk_expr(val, genmap, defaults);
            }
            TK::FieldGet { base, .. } => self.zonk_expr(base, genmap, defaults),
            TK::MapNew => {}
            TK::MapLen { map } | TK::MapClear { map } => self.zonk_expr(map, genmap, defaults),
            TK::MapFind { map, key: other }
            | TK::MapDel { map, key: other }
            | TK::MapNext { map, from: other }
            | TK::MapKeyAt { map, idx: other }
            | TK::MapValAt { map, idx: other } => {
                self.zonk_expr(map, genmap, defaults);
                self.zonk_expr(other, genmap, defaults);
            }
            TK::MapSet { map, key, val } => {
                self.zonk_expr(map, genmap, defaults);
                self.zonk_expr(key, genmap, defaults);
                self.zonk_expr(val, genmap, defaults);
            }
            TK::MethodCall { recv, args, .. } => {
                *recv = self.zonk(recv, genmap, defaults);
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::CallCtor { targs, args, .. } => {
                for t in targs.iter_mut() {
                    *t = self.zonk(t, genmap, defaults);
                }
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::CallClosure { callee, args } => {
                self.zonk_expr(callee, genmap, defaults);
                for a in args {
                    self.zonk_expr(a, genmap, defaults);
                }
            }
            TK::Println(a) | TK::Print(a) | TK::Show(a) | TK::Neg(a) => {
                self.zonk_expr(a, genmap, defaults)
            }
            TK::ChanNew(cap) => {
                if let Some(c) = cap {
                    self.zonk_expr(c, genmap, defaults);
                }
            }
            TK::ChanSend { ch, val } => {
                self.zonk_expr(ch, genmap, defaults);
                self.zonk_expr(val, genmap, defaults);
            }
            TK::ChanRecv { ch } => self.zonk_expr(ch, genmap, defaults),
            TK::Spawn { captures, body } => {
                for (_, t) in captures.iter_mut() {
                    *t = self.zonk(t, genmap, defaults);
                }
                self.zonk_expr(body, genmap, defaults);
            }
            TK::Select { arms, default } => {
                for a in arms.iter_mut() {
                    match &mut a.op {
                        TSelectOp::Recv { chan, elem } => {
                            self.zonk_expr(chan, genmap, defaults);
                            *elem = self.zonk(elem, genmap, defaults);
                        }
                        TSelectOp::Send { chan, value } => {
                            self.zonk_expr(chan, genmap, defaults);
                            self.zonk_expr(value, genmap, defaults);
                        }
                    }
                    if let Some((_, t)) = a.bind.as_mut() {
                        *t = self.zonk(t, genmap, defaults);
                    }
                    self.zonk_expr(&mut a.body, genmap, defaults);
                }
                if let Some(d) = default {
                    self.zonk_expr(d, genmap, defaults);
                }
            }
            TK::Binary { lhs, rhs, .. } => {
                self.zonk_expr(lhs, genmap, defaults);
                self.zonk_expr(rhs, genmap, defaults);
            }
            TK::If { cond, then, els } => {
                self.zonk_expr(cond, genmap, defaults);
                self.zonk_expr(then, genmap, defaults);
                self.zonk_expr(els, genmap, defaults);
            }
            TK::Match { scrutinee, arms } => {
                self.zonk_expr(scrutinee, genmap, defaults);
                for arm in arms {
                    if let Some(g) = arm.guard.as_mut() {
                        self.zonk_expr(g, genmap, defaults);
                    }
                    self.zonk_expr(&mut arm.body, genmap, defaults);
                }
            }
            TK::Block(stmts) => {
                for s in stmts {
                    match s {
                        TStmt::Bind { value, .. } => self.zonk_expr(value, genmap, defaults),
                        TStmt::Expr(v) => self.zonk_expr(v, genmap, defaults),
                    }
                }
            }
            TK::Lambda { params, captures, body } => {
                for (_, t) in params.iter_mut().chain(captures.iter_mut()) {
                    *t = self.zonk(t, genmap, defaults);
                }
                self.zonk_expr(body, genmap, defaults);
            }
        }
    }

    // ---- expression inference -------------------------------------------

    fn infer(&mut self, expr: &Expr, env: &mut HashMap<String, Ty>) -> Result<TExpr, TypeError> {
        match expr {
            Expr::Int(v) => Ok(TExpr { ty: Ty::Int, line: 0, kind: TK::Int(*v) }),
            Expr::Float(v) => Ok(TExpr { ty: Ty::Float, line: 0, kind: TK::Float(*v) }),
            Expr::Str(s) => Ok(TExpr { ty: Ty::Str, line: 0, kind: TK::Str(s.clone()) }),
            Expr::Bool(v) => Ok(TExpr { ty: Ty::Bool, line: 0, kind: TK::Bool(*v) }),
            Expr::Var(name, line) => self.infer_var(name, *line, env),
            Expr::Field { base, name, line } => {
                let b = self.infer(base, env)?;
                self.field_of(b, name, *line)
            }
            Expr::Call { name, args, line, .. } => self.infer_call(name, args, *line, env),
            Expr::Unary { op: UnOp::Neg, expr } => {
                let inner = self.infer(expr, env)?;
                self.require(&inner.ty, Constraint::Numeric, inner.line, "negation")?;
                Ok(TExpr {
                    ty: inner.ty.clone(),
                    line: inner.line,
                    kind: TK::Neg(Box::new(inner)),
                })
            }
            Expr::Binary { op, lhs, rhs, line } => {
                let l = self.infer(lhs, env)?;
                let r = self.infer(rhs, env)?;
                // `m[k] + 1` is the shape `m[k] += 1` expands to, and reading
                // a map gives an `Option` — which is the answer to the
                // question the program asked, but not a number. Say which of
                // the two it is rather than leaving `Option(Int)` against
                // `Int` to be worked out.
                if let Err(e) = self.unify(&l.ty, &r.ty, *line, "operands of a binary operator") {
                    let (a, b) = (self.resolve(&l.ty), self.resolve(&r.ty));
                    for (opt, plain) in [(&a, &b), (&b, &a)] {
                        if let Ty::Adt(i, args) = opt {
                            if self.adts[*i].name == "Option"
                                && args.first().map(|t| self.resolve(t)).as_ref() == Some(plain)
                            {
                                return Err(TypeError::new(
                                    *line,
                                    format!(
                                        "{} — a map read answers with an `Option`, since the \
                                         key may not be there. `unwrap_or(m[k], 0)` is the \
                                         value or a default",
                                        e.msg,
                                    ),
                                ));
                            }
                        }
                    }
                    return Err(e);
                }
                let ty = match op {
                    BinOp::Add => {
                        self.require(&l.ty, Constraint::Addable, *line, "`+`")?;
                        l.ty.clone()
                    }
                    BinOp::Sub | BinOp::Mul | BinOp::Div => {
                        self.require(&l.ty, Constraint::Numeric, *line, "arithmetic")?;
                        l.ty.clone()
                    }
                    BinOp::Mod => {
                        self.unify(&l.ty, &Ty::Int, *line, "`%`")?;
                        Ty::Int
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        self.require(&l.ty, Constraint::Ordered, *line, "comparison")?;
                        Ty::Bool
                    }
                    BinOp::Eq | BinOp::Ne => {
                        self.require(&l.ty, Constraint::Basic, *line, "equality")?;
                        Ty::Bool
                    }
                };
                Ok(TExpr {
                    ty,
                    line: *line,
                    kind: TK::Binary { op: *op, lhs: Box::new(l), rhs: Box::new(r) },
                })
            }
            Expr::If { cond, then, els } => {
                let c = self.infer(cond, env)?;
                self.unify(&c.ty, &Ty::Bool, c.line, "`if` condition")?;
                let t = self.infer(then, env)?;
                let e = self.infer(els, env)?;
                self.unify(&t.ty, &e.ty, e.line, "`if` branches")?;
                Ok(TExpr {
                    ty: t.ty.clone(),
                    line: c.line,
                    kind: TK::If { cond: Box::new(c), then: Box::new(t), els: Box::new(e) },
                })
            }
            Expr::Match { scrutinee, arms, line } => self.infer_match(scrutinee, arms, *line, env),
            Expr::Spawn { body, line } => {
                let mut inner = env.clone();
                let tbody = self.infer(body, &mut inner)?;
                let mut free = Vec::new();
                let mut seen = HashSet::new();
                free_vars(body, &mut free, &mut seen);
                let captures: Vec<(String, Ty)> = free
                    .into_iter()
                    .filter_map(|n| env.get(&n).map(|t| (n, t.clone())))
                    .collect();
                Ok(TExpr {
                    ty: Ty::Unit,
                    line: *line,
                    kind: TK::Spawn { captures, body: Box::new(tbody) },
                })
            }
            Expr::Select { arms, default, line } => {
                let result_ty = self.fresh();
                let mut tarms = Vec::new();
                for arm in arms {
                    let mut arm_env = env.clone();
                    let (op, bind) = match &arm.op {
                        SelectOp::Recv { chan, bind } => {
                            let ch = self.infer(chan, env)?;
                            let elem = self.fresh();
                            self.unify(
                                &ch.ty,
                                &Ty::Chan(Box::new(elem.clone())),
                                arm.line,
                                "`recv` in select",
                            )?;
                            let b = bind.as_ref().map(|n| {
                                arm_env.insert(n.clone(), elem.clone());
                                (n.clone(), elem.clone())
                            });
                            (TSelectOp::Recv { chan: ch, elem }, b)
                        }
                        SelectOp::Send { chan, value } => {
                            let ch = self.infer(chan, env)?;
                            let v = self.infer(value, env)?;
                            self.unify(
                                &ch.ty,
                                &Ty::Chan(Box::new(v.ty.clone())),
                                arm.line,
                                "`send` in select",
                            )?;
                            (TSelectOp::Send { chan: ch, value: v }, None)
                        }
                    };
                    let body = self.infer(&arm.body, &mut arm_env)?;
                    self.unify(&result_ty, &body.ty, arm.line, "select arms")?;
                    tarms.push(TSelectArm { op, bind, body });
                }
                let tdefault = match default {
                    Some(d) => {
                        let td = self.infer(d, env)?;
                        self.unify(&result_ty, &td.ty, *line, "select arms")?;
                        Some(Box::new(td))
                    }
                    None => None,
                };
                Ok(TExpr {
                    ty: result_ty,
                    line: *line,
                    kind: TK::Select { arms: tarms, default: tdefault },
                })
            }
            Expr::Lambda { params, body, line } => {
                let param_tys: Vec<Ty> = params.iter().map(|_| self.fresh()).collect();
                let mut inner = env.clone();
                for (p, t) in params.iter().zip(&param_tys) {
                    inner.insert(p.clone(), t.clone());
                }
                let tbody = self.infer(body, &mut inner)?;

                // Captures: free variables of the body that refer to locals of
                // the enclosing scope (not the lambda's own parameters).
                let mut free = Vec::new();
                let mut seen = HashSet::new();
                free_vars(body, &mut free, &mut seen);
                let captures: Vec<(String, Ty)> = free
                    .into_iter()
                    .filter(|n| !params.contains(n))
                    .filter_map(|n| env.get(&n).map(|t| (n, t.clone())))
                    .collect();

                let ty = Ty::Fn(param_tys.clone(), Box::new(tbody.ty.clone()));
                Ok(TExpr {
                    ty,
                    line: *line,
                    kind: TK::Lambda {
                        params: params.iter().cloned().zip(param_tys).collect(),
                        captures,
                        body: Box::new(tbody),
                    },
                })
            }
            // `#x` is how long the one indexable thing in hand is. A base
            // still open cannot be a buffer, for the same reason as below.
            Expr::Len { expr, line } => {
                let b = self.infer(expr, env)?;
                match self.resolve(&b.ty) {
                    Ty::Buf(_) => Ok(TExpr {
                        ty: Ty::Int,
                        line: *line,
                        kind: TK::BufLen { buf: Box::new(b) },
                    }),
                    Ty::StrBuf => Ok(TExpr {
                        ty: Ty::Int,
                        line: *line,
                        kind: TK::Builtin { rt: "rill_strbuf_len", args: vec![b] },
                    }),
                    // How many entries, not how many slots: the slots are the
                    // table's business.
                    Ty::Map(..) => Ok(TExpr {
                        ty: Ty::Int,
                        line: *line,
                        kind: TK::MapLen { map: Box::new(b) },
                    }),
                    Ty::Str | Ty::Var(_) => {
                        self.unify(&Ty::Str, &b.ty, *line, "the operand of `#`")?;
                        Ok(TExpr {
                            ty: Ty::Int,
                            line: *line,
                            kind: TK::Builtin { rt: "rill_str_len", args: vec![b] },
                        })
                    }
                    other => Err(TypeError::new(
                        *line,
                        format!(
                            "`#` measures a string, a buffer, a map or a builder, but this \
                             is `{}`",
                            self.type_name(&other)
                        ),
                    )),
                }
            }
            // `b[i]` and `b[i] = v` are the reading and the writing of one
            // element, and mean exactly what `buf_get`/`buf_set` mean. A
            // string indexes to its bytes, and cannot be written.
            Expr::Index { base, index, line } => {
                let b = self.infer(base, env)?;
                // A map is read through the prelude's `map_get`, which is
                // where the `Option` comes from: a key may not be there, and
                // absence is an `Option` everywhere else in the language. The
                // base is inferred a second time by the call, which costs a
                // few type variables and no code.
                if matches!(self.resolve(&b.ty), Ty::Map(..)) {
                    let call = Expr::Call {
                        name: "map_get".to_string(),
                        args: vec![(**base).clone(), (**index).clone()],
                        line: *line,
                        piped: false,
                    };
                    return self.infer(&call, env);
                }
                let idx = self.infer(index, env)?;
                self.unify(&Ty::Int, &idx.ty, *line, "index")?;
                match self.resolve(&b.ty) {
                    Ty::Buf(w) => Ok(TExpr {
                        ty: w.rill_ty(),
                        line: *line,
                        kind: TK::BufGet { buf: Box::new(b), idx: Box::new(idx) },
                    }),
                    // A base still open here cannot be a buffer — a `Buf`
                    // width is never inferred, so one would have had to be
                    // annotated. That leaves a string, and saying so is what
                    // lets `fn f(s, i) = s[i]` type without an annotation.
                    Ty::Str | Ty::Var(_) => {
                        self.unify(&Ty::Str, &b.ty, *line, "the base of `[]`")?;
                        Ok(TExpr {
                            ty: Ty::Int,
                            line: *line,
                            kind: TK::Builtin { rt: "rill_str_get", args: vec![b, idx] },
                        })
                    }
                    other => Err(TypeError::new(
                        *line,
                        format!(
                            "`[]` indexes a buffer, a map or a string, but this is `{}` — a \
                             `Buf` width is never inferred, so annotate the parameter",
                            self.type_name(&other)
                        ),
                    )),
                }
            }
            Expr::IndexSet { base, index, value, line } => {
                let b = self.infer(base, env)?;
                // `m[k] = v` is `map_set`, and inserts as readily as it
                // replaces: a table has no element that was already there.
                //
                // A base still open here is one: a `Buf` width is never
                // inferred, so a buffer would have had to be annotated, and a
                // string is a value rather than storage. Saying so is what
                // lets `fn put(m, k, v) = m[k] = v` type without an
                // annotation, exactly as `s[i]` does on the reading side.
                if matches!(self.resolve(&b.ty), Ty::Var(_)) {
                    let (k, v) = (self.fresh(), self.fresh());
                    self.unify(
                        &b.ty,
                        &Ty::Map(Box::new(k.clone()), Box::new(v)),
                        *line,
                        "the base of `[] =`",
                    )?;
                    self.require(&k, Constraint::Basic, *line, "a map key")?;
                }
                if let Ty::Map(k, v) = self.resolve(&b.ty) {
                    let key = self.infer(index, env)?;
                    self.unify(&k, &key.ty, *line, "a map key")?;
                    let val = self.infer(value, env)?;
                    self.unify(&v, &val.ty, *line, "a map value")?;
                    return Ok(TExpr {
                        ty: Ty::Unit,
                        line: *line,
                        kind: TK::MapSet {
                            map: Box::new(b),
                            key: Box::new(key),
                            val: Box::new(val),
                        },
                    });
                }
                let width = match self.resolve(&b.ty) {
                    Ty::Buf(w) => w,
                    other => {
                        return Err(TypeError::new(
                            *line,
                            format!(
                                "only a buffer element or a map key can be assigned to, but \
                                 this is `{}`{}",
                                self.type_name(&other),
                                if matches!(other, Ty::Str) {
                                    " — strings are values, not storage"
                                } else {
                                    ""
                                }
                            ),
                        ))
                    }
                };
                let idx = self.infer(index, env)?;
                self.unify(&Ty::Int, &idx.ty, *line, "index")?;
                let val = self.infer(value, env)?;
                self.unify(&width.rill_ty(), &val.ty, *line, "buffer element")?;
                Ok(TExpr {
                    ty: Ty::Unit,
                    line: *line,
                    kind: TK::BufSet {
                        buf: Box::new(b),
                        idx: Box::new(idx),
                        val: Box::new(val),
                    },
                })
            }
            // `b += s` adds to a builder, which is the only `op=` whose
            // target is a name — what it changes is the storage, not the name.
            Expr::Append { target, value, line } => {
                let b = self.infer(target, env)?;
                match self.resolve(&b.ty) {
                    Ty::StrBuf => {}
                    other => {
                        return Err(TypeError::new(
                            *line,
                            format!(
                                "`+=` on a name adds to a string builder, but this is `{}` \
                                 — bindings do not change",
                                self.type_name(&other)
                            ),
                        ))
                    }
                }
                // A string goes in as it is; a number goes in as one byte,
                // which is what reading a string a character at a time gives
                // and what `chr` would otherwise allocate a string for.
                let v = self.infer(value, env)?;
                let rt = if matches!(self.resolve(&v.ty), Ty::Int) {
                    "rill_strbuf_byte"
                } else {
                    self.unify(&Ty::Str, &v.ty, *line, "what `+=` adds")?;
                    "rill_strbuf_add"
                };
                Ok(TExpr { ty: Ty::Unit, line: *line, kind: TK::Builtin { rt, args: vec![b, v] } })
            }
            // `lift` spells these out before the checker sees a program.
            Expr::IndexOp { line, .. } => {
                Err(TypeError::new(*line, "internal: `[]=` was not expanded".to_string()))
            }
            Expr::Try { line, .. } => {
                Err(TypeError::new(*line, "internal: `?` was not expanded".to_string()))
            }
            Expr::For { line, .. } => {
                Err(TypeError::new(*line, "internal: `for` was not expanded".to_string()))
            }
            // `s[i:j]` is `str_sub` with an end instead of a length, which
            // is what a slice means everywhere else and what makes `s[i:]`
            // and `s[:j]` sensible.
            Expr::Slice { base, from, to, line } => {
                let b = self.infer(base, env)?;
                match self.resolve(&b.ty) {
                    Ty::Str | Ty::Var(_) => {
                        self.unify(&Ty::Str, &b.ty, *line, "the value a slice is taken from")?
                    }
                    other => {
                        return Err(TypeError::new(
                            *line,
                            format!(
                                "`[:]` slices a string, but this is `{}`",
                                self.type_name(&other)
                            ),
                        ))
                    }
                }
                let f = match from {
                    Some(e) => self.infer(e, env)?,
                    None => TExpr { ty: Ty::Int, line: *line, kind: TK::Int(0) },
                };
                self.unify(&Ty::Int, &f.ty, *line, "the start of a slice")?;
                let t = match to {
                    Some(e) => {
                        let t = self.infer(e, env)?;
                        self.unify(&Ty::Int, &t.ty, *line, "the end of a slice")?;
                        Some(Box::new(t))
                    }
                    None => None,
                };
                Ok(TExpr {
                    ty: Ty::Str,
                    line: *line,
                    kind: TK::Slice { base: Box::new(b), from: Box::new(f), to: t },
                })
            }
            Expr::Block(stmts) => {
                let mut out = Vec::new();
                let mut ty = Ty::Unit;
                for stmt in stmts {
                    match stmt {
                        // `lift` runs before the checker, so neither of these
                        // is still around by now.
                        Stmt::PatBind { line, .. } => {
                            return Err(TypeError::new(
                                *line,
                                "internal: a pattern binding was not expanded".to_string(),
                            ))
                        }
                        Stmt::Fn(f) => {
                            return Err(TypeError::new(
                                f.line,
                                format!("internal: `{}` was not lifted", f.name),
                            ))
                        }
                        Stmt::Bind { name, value, .. } => {
                            let v = self.infer(value, env)?;
                            env.insert(name.clone(), v.ty.clone());
                            out.push(TStmt::Bind { name: name.clone(), value: v });
                            ty = Ty::Unit;
                        }
                        Stmt::Expr(e) => {
                            let v = self.infer(e, env)?;
                            ty = v.ty.clone();
                            out.push(TStmt::Expr(v));
                        }
                    }
                }
                Ok(TExpr { ty, line: 0, kind: TK::Block(out) })
            }
        }
    }

    /// One named field of a value. A type with more than one constructor has
    /// no field that is certainly there, which is the same reason a binding
    /// cannot take one apart; `match` is what asks those.
    fn field_of(&mut self, base: TExpr, field: &str, line: usize) -> Result<TExpr, TypeError> {
        let (adt, targs) = match self.resolve(&base.ty) {
            Ty::Adt(i, targs) => (i, targs),
            // Which field `.x` is depends on which type this is, and nothing
            // here has said yet. Inference does not work backwards from a
            // field name — two types may both have an `x` — so this is one of
            // the few places an annotation is not optional.
            Ty::Var(_) => {
                let what = match &base.kind {
                    TK::Local(n) => format!("`{n}`"),
                    _ => "this".to_string(),
                };
                return Err(TypeError::new(
                    line,
                    format!(
                        "the type of {what} is not known here, so `.{field}` cannot say \
                         which field it is — annotate it (`{}: SomeType`)",
                        match &base.kind {
                            TK::Local(n) => n.clone(),
                            _ => "x".to_string(),
                        }
                    ),
                ));
            }
            other => {
                return Err(TypeError::new(
                    line,
                    format!(
                        "`{}` has no fields, so `.{field}` means nothing here",
                        self.type_name(&other)
                    ),
                ))
            }
        };
        let def = &self.adts[adt];
        if def.ctors.len() != 1 {
            let names: Vec<&str> = def.ctors.iter().map(|c| c.name.as_str()).collect();
            return Err(TypeError::new(
                line,
                format!(
                    "`{}` has more than one constructor ({}), so `.{field}` cannot say \
                     which — `match` is how a type like this is taken apart",
                    def.name,
                    names.join(", ")
                ),
            ));
        }
        let ctor = &def.ctors[0];
        let Some(index) = ctor.names.iter().position(|n| n.as_deref() == Some(field)) else {
            let known: Vec<&str> = ctor.names.iter().filter_map(|n| n.as_deref()).collect();
            let has = if known.is_empty() {
                "its fields are not named, and a field is read by its name".to_string()
            } else {
                format!("it has {}", known.join(", "))
            };
            return Err(TypeError::new(
                line,
                format!("`{}` has no field `{field}` — {has}", def.name),
            ));
        };
        let ty = subst_gen(&ctor.fields[index], &targs);
        Ok(TExpr { ty, line, kind: TK::FieldGet { base: Box::new(base), adt, ctor: 0, index } })
    }

    fn infer_var(
        &mut self,
        name: &str,
        line: usize,
        env: &mut HashMap<String, Ty>,
    ) -> Result<TExpr, TypeError> {
        if let Some(ty) = env.get(name) {
            return Ok(TExpr { ty: ty.clone(), line, kind: TK::Local(name.to_string()) });
        }
        if let Some(&(adt, ci)) = self.ctors.get(name) {
            let nfields = self.adts[adt].ctors[ci].fields.len();
            if nfields != 0 {
                return Err(TypeError::new(
                    line,
                    format!("constructor `{name}` takes {nfields} argument(s)"),
                ));
            }
            let targs: Vec<Ty> = (0..self.adts[adt].nparams).map(|_| self.fresh()).collect();
            return Ok(TExpr {
                ty: Ty::Adt(adt, targs.clone()),
                line,
                kind: TK::CallCtor { adt, ctor: ci, targs, args: vec![] },
            });
        }
        if let Some(&idx) = self.fn_index.get(name) {
            if name == "main" {
                return Err(TypeError::new(line, "`main` cannot be used as a value"));
            }
            let (params, ret, subst) = match self.sigs[idx].clone() {
                SigState::Provisional { params, ret } => (params, ret, vec![]),
                SigState::Generalized { params, ret, ngen } => {
                    self.instantiate(&params, &ret, ngen)
                }
            };
            return Ok(TExpr {
                ty: Ty::Fn(params, Box::new(ret)),
                line,
                kind: TK::FnRef { fn_idx: idx, subst },
            });
        }
        // `p.x` and `geo.area` are the same shape, and the lexer cannot tell
        // them apart — so a dotted name that is nothing the program declared
        // is read as a field of what its first part names. `a.b.c` is the
        // field `c` of the field `b`, which is the same rule applied twice.
        if let Some((head, rest)) = name.split_once('.') {
            if env.contains_key(head) {
                let mut base = self.infer_var(head, line, env)?;
                for field in rest.split('.') {
                    base = self.field_of(base, field, line)?;
                }
                return Ok(base);
            }
        }
        Err(TypeError::new(line, format!("unknown variable `{name}`")))
    }

    fn infer_call(
        &mut self,
        name: &str,
        args: &[Expr],
        line: usize,
        env: &mut HashMap<String, Ty>,
    ) -> Result<TExpr, TypeError> {
        // A local holding a closure shadows functions and constructors.
        if env.contains_key(name) {
            let callee = self.infer_var(name, line, env)?;
            let mut targs = Vec::new();
            for a in args {
                targs.push(self.infer(a, env)?);
            }
            let ret = self.fresh();
            let want = Ty::Fn(targs.iter().map(|a| a.ty.clone()).collect(), Box::new(ret.clone()));
            self.unify(&callee.ty, &want, line, &format!("calling `{name}`"))?;
            return Ok(TExpr {
                ty: ret,
                line,
                kind: TK::CallClosure { callee: Box::new(callee), args: targs },
            });
        }

        if name == "println" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`println` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`println`")?;
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Println(Box::new(a)) });
        }
        if name == "channel" {
            // `channel()` rendezvous; `channel(n)` buffers n values.
            let cap = match args.len() {
                0 => None,
                1 => {
                    let a = self.infer(&args[0], env)?;
                    self.unify(&Ty::Int, &a.ty, line, "`channel` capacity")?;
                    Some(Box::new(a))
                }
                _ => {
                    return Err(TypeError::new(
                        line,
                        "`channel` takes an optional buffer size".to_string(),
                    ))
                }
            };
            let elem = self.fresh();
            return Ok(TExpr { ty: Ty::Chan(Box::new(elem)), line, kind: TK::ChanNew(cap) });
        }
        // A buffer's element width is part of its type rather than inferred,
        // so each width has its own constructor. Everything after that reads
        // the width off the buffer it is given.
        let buf_width = match name {
            "buf_i8" => Some(CTy::I8),
            "buf_u8" => Some(CTy::U8),
            "buf_i16" => Some(CTy::I16),
            "buf_u16" => Some(CTy::U16),
            "buf_i32" => Some(CTy::I32),
            "buf_u32" => Some(CTy::U32),
            "buf_int" => Some(CTy::I64),
            "buf_f32" => Some(CTy::F32),
            "buf_float" => Some(CTy::F64),
            "buf_ptr" => Some(CTy::Ptr),
            _ => None,
        };
        if let Some(width) = buf_width {
            if args.len() != 1 {
                return Err(TypeError::new(line, format!("`{name}` takes a length")));
            }
            let n = self.infer(&args[0], env)?;
            self.unify(&Ty::Int, &n.ty, line, "buffer length")?;
            return Ok(TExpr {
                ty: Ty::Buf(width.clone()),
                line,
                kind: TK::BufNew { width, count: Box::new(n) },
            });
        }
        if name == "buf_get" || name == "buf_set" || name == "buf_len" {
            let want = if name == "buf_set" { 3 } else if name == "buf_len" { 1 } else { 2 };
            if args.len() != want {
                return Err(TypeError::new(
                    line,
                    format!("`{name}` takes {want} argument(s), but {} given", args.len()),
                ));
            }
            let b = self.infer(&args[0], env)?;
            let width = match self.resolve(&b.ty) {
                Ty::Buf(w) => w,
                other => {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "`{name}` needs a buffer, but this is `{}` — a `Buf` \
                             width is never inferred, so annotate the parameter",
                            self.type_name(&other)
                        ),
                    ))
                }
            };
            if name == "buf_len" {
                return Ok(TExpr { ty: Ty::Int, line, kind: TK::BufLen { buf: Box::new(b) } });
            }
            let idx = self.infer(&args[1], env)?;
            self.unify(&Ty::Int, &idx.ty, line, "buffer index")?;
            if name == "buf_get" {
                return Ok(TExpr {
                    ty: width.rill_ty(),
                    line,
                    kind: TK::BufGet { buf: Box::new(b), idx: Box::new(idx) },
                });
            }
            let val = self.infer(&args[2], env)?;
            self.unify(&width.rill_ty(), &val.ty, line, "buffer element")?;
            return Ok(TExpr {
                ty: Ty::Unit,
                line,
                kind: TK::BufSet {
                    buf: Box::new(b),
                    idx: Box::new(idx),
                    val: Box::new(val),
                },
            });
        }
        // A map's two type arguments are ordinary types, so — unlike a
        // buffer's width — they unify like anything else and these all work
        // inside a generic function without an annotation.
        if let Some(arity) = map_builtin_arity(name) {
            if args.len() != arity {
                return Err(TypeError::new(
                    line,
                    format!("`{name}` takes {arity} argument(s), but {} given", args.len()),
                ));
            }
            if name == "map_new" {
                let (k, v) = (self.fresh(), self.fresh());
                self.require(&k, Constraint::Basic, line, "a map key")?;
                return Ok(TExpr {
                    ty: Ty::Map(Box::new(k), Box::new(v)),
                    line,
                    kind: TK::MapNew,
                });
            }
            let m = self.infer(&args[0], env)?;
            let (k, v) = (self.fresh(), self.fresh());
            self.unify(
                &m.ty,
                &Ty::Map(Box::new(k.clone()), Box::new(v.clone())),
                line,
                &format!("`{name}`"),
            )?;
            self.require(&k, Constraint::Basic, line, "a map key")?;
            let map = Box::new(m);
            return Ok(match name {
                "map_len" => TExpr { ty: Ty::Int, line, kind: TK::MapLen { map } },
                "map_clear" => TExpr { ty: Ty::Unit, line, kind: TK::MapClear { map } },
                "map_set" => {
                    let key = self.infer(&args[1], env)?;
                    self.unify(&k, &key.ty, line, "a map key")?;
                    let val = self.infer(&args[2], env)?;
                    self.unify(&v, &val.ty, line, "a map value")?;
                    TExpr {
                        ty: Ty::Unit,
                        line,
                        kind: TK::MapSet { map, key: Box::new(key), val: Box::new(val) },
                    }
                }
                "map_find" | "map_del" => {
                    let key = self.infer(&args[1], env)?;
                    self.unify(&k, &key.ty, line, "a map key")?;
                    let key = Box::new(key);
                    if name == "map_find" {
                        TExpr { ty: Ty::Int, line, kind: TK::MapFind { map, key } }
                    } else {
                        TExpr { ty: Ty::Bool, line, kind: TK::MapDel { map, key } }
                    }
                }
                _ => {
                    let idx = self.infer(&args[1], env)?;
                    self.unify(&Ty::Int, &idx.ty, line, "a map slot")?;
                    let idx = Box::new(idx);
                    match name {
                        "map_next" => {
                            TExpr { ty: Ty::Int, line, kind: TK::MapNext { map, from: idx } }
                        }
                        "map_key_at" => TExpr { ty: k, line, kind: TK::MapKeyAt { map, idx } },
                        _ => TExpr { ty: v, line, kind: TK::MapValAt { map, idx } },
                    }
                }
            });
        }
        if name == "send" {
            if args.len() != 2 {
                return Err(TypeError::new(line, "`send` takes a channel and a value"));
            }
            let ch = self.infer(&args[0], env)?;
            let val = self.infer(&args[1], env)?;
            let want = Ty::Chan(Box::new(val.ty.clone()));
            self.unify(&ch.ty, &want, line, "`send`")?;
            return Ok(TExpr {
                ty: Ty::Unit,
                line,
                kind: TK::ChanSend { ch: Box::new(ch), val: Box::new(val) },
            });
        }
        if name == "recv" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`recv` takes exactly one channel"));
            }
            let ch = self.infer(&args[0], env)?;
            let elem = self.fresh();
            self.unify(&ch.ty, &Ty::Chan(Box::new(elem.clone())), line, "`recv`")?;
            return Ok(TExpr { ty: elem, line, kind: TK::ChanRecv { ch: Box::new(ch) } });
        }
        if name == "show" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`show` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`show`")?;
            return Ok(TExpr { ty: Ty::Str, line, kind: TK::Show(Box::new(a)) });
        }
        if name == "print" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`print` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`print`")?;
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Print(Box::new(a)) });
        }
        let simple_builtin: Option<(&'static str, Vec<Ty>, Ty)> = match name {
            "strbuf" => Some(("rill_strbuf_new", vec![], Ty::StrBuf)),
            "strbuf_add" => Some(("rill_strbuf_add", vec![Ty::StrBuf, Ty::Str], Ty::Unit)),
            "strbuf_byte" => Some(("rill_strbuf_byte", vec![Ty::StrBuf, Ty::Int], Ty::Unit)),
            "strbuf_str" => Some(("rill_strbuf_str", vec![Ty::StrBuf], Ty::Str)),
            "str_len" => Some(("rill_str_len", vec![Ty::Str], Ty::Int)),
            "int_to_str" => Some(("rill_int_to_str", vec![Ty::Int], Ty::Str)),
            "float_to_str" => Some(("rill_float_to_str", vec![Ty::Float], Ty::Str)),
            "bool_to_str" => Some(("rill_bool_to_str", vec![Ty::Bool], Ty::Str)),
            "str_get" => Some(("rill_str_get", vec![Ty::Str, Ty::Int], Ty::Int)),
            "str_sub" => Some(("rill_str_sub", vec![Ty::Str, Ty::Int, Ty::Int], Ty::Str)),
            "str_find" => Some(("rill_str_find", vec![Ty::Str, Ty::Str], Ty::Int)),
            "str_find_from" => {
                Some(("rill_str_find_from", vec![Ty::Str, Ty::Str, Ty::Int], Ty::Int))
            }
            "str_cmp" => Some(("rill_str_cmp", vec![Ty::Str, Ty::Str], Ty::Int)),
            "str_to_int" => Some(("rill_str_to_int", vec![Ty::Str], Ty::Int)),
            "str_to_float" => Some(("rill_str_to_float", vec![Ty::Str], Ty::Float)),
            "chr" => Some(("rill_chr", vec![Ty::Int], Ty::Str)),
            // Bit twiddling, needed by anything that touches a binary
            // protocol. These compile to single instructions.
            "bit_and" => Some(("rill.and", vec![Ty::Int, Ty::Int], Ty::Int)),
            "bit_or" => Some(("rill.or", vec![Ty::Int, Ty::Int], Ty::Int)),
            "bit_xor" => Some(("rill.xor", vec![Ty::Int, Ty::Int], Ty::Int)),
            "bit_not" => Some(("rill.not", vec![Ty::Int], Ty::Int)),
            "shl" => Some(("rill.shl", vec![Ty::Int, Ty::Int], Ty::Int)),
            "shr" => Some(("rill.shr", vec![Ty::Int, Ty::Int], Ty::Int)),
            // The two numeric types never mix implicitly, so crossing between
            // them is written out. Both are single instructions.
            "to_float" => Some(("rill.sitofp", vec![Ty::Int], Ty::Float)),
            "to_int" => Some(("rill.fptosi", vec![Ty::Float], Ty::Int)),
            "arg_count" => Some(("rill_arg_count", vec![], Ty::Int)),
            "arg" => Some(("rill_arg", vec![Ty::Int], Ty::Str)),
            "read_file_raw" => Some(("rill_read_file", vec![Ty::Str], Ty::Str)),
            "io_ok" => Some(("rill_io_ok", vec![], Ty::Bool)),
            "write_file" => Some(("rill_write_file", vec![Ty::Str, Ty::Str], Ty::Bool)),
            "read_line" => Some(("rill_read_line", vec![], Ty::Str)),
            "scan_int" => Some(("rill_scan_int", vec![], Ty::Int)),
            "exit" => Some(("rill_exit", vec![Ty::Int], Ty::Unit)),
            // A monotonic clock, in nanoseconds. The origin is arbitrary and
            // only differences mean anything — this is for measuring intervals,
            // not for telling the time, and a wall clock that an administrator
            // can set backwards is the wrong instrument for that.
            "now_nanos" => Some(("rill_now_nanos", vec![], Ty::Int)),
            // Waiting, without a worker standing still to do it: the strand
            // goes into the same poller a socket would have parked it in.
            "sleep_ms" => Some(("rill_sleep_ms", vec![Ty::Int], Ty::Unit)),
            // Bridges to and from C strings; `to_cstr` allocates, so the
            // caller frees with `free_ptr`.
            "to_cstr" => Some(("rill_to_cstr", vec![Ty::Str], Ty::Ptr)),
            "from_cstr" => Some(("rill_from_cstr", vec![Ty::Ptr], Ty::Str)),
            "free_ptr" => Some(("rill_free_ptr", vec![Ty::Ptr], Ty::Unit)),
            "null_ptr" => Some(("rill_null_ptr", vec![], Ty::Ptr)),
            "is_null" => Some(("rill_is_null", vec![Ty::Ptr], Ty::Bool)),
            // Raw buffers, for handing an array to a C library. The index is
            // in elements, so `poke_f32(p, 3, x)` writes the fourth float;
            // `ptr_offset` moves by bytes when a layout needs it.
            "alloc_bytes" => Some(("rill_alloc_bytes", vec![Ty::Int], Ty::Ptr)),
            "ptr_offset" => Some(("rill_ptr_offset", vec![Ty::Ptr, Ty::Int], Ty::Ptr)),
            "poke_i8" => Some(("rill_poke_i8", vec![Ty::Ptr, Ty::Int, Ty::Int], Ty::Unit)),
            "poke_i16" => Some(("rill_poke_i16", vec![Ty::Ptr, Ty::Int, Ty::Int], Ty::Unit)),
            "poke_i32" => Some(("rill_poke_i32", vec![Ty::Ptr, Ty::Int, Ty::Int], Ty::Unit)),
            "poke_i64" => Some(("rill_poke_i64", vec![Ty::Ptr, Ty::Int, Ty::Int], Ty::Unit)),
            "poke_f32" => Some(("rill_poke_f32", vec![Ty::Ptr, Ty::Int, Ty::Float], Ty::Unit)),
            "poke_f64" => Some(("rill_poke_f64", vec![Ty::Ptr, Ty::Int, Ty::Float], Ty::Unit)),
            // Pointer slots: an array of C strings on the way in, a handle
            // written by a C out-parameter on the way back.
            "poke_ptr" => Some(("rill_poke_ptr", vec![Ty::Ptr, Ty::Int, Ty::Ptr], Ty::Unit)),
            "peek_ptr" => Some(("rill_peek_ptr", vec![Ty::Ptr, Ty::Int], Ty::Ptr)),
            "peek_i8" => Some(("rill_peek_i8", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_u8" => Some(("rill_peek_u8", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_i16" => Some(("rill_peek_i16", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_u16" => Some(("rill_peek_u16", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_i32" => Some(("rill_peek_i32", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_u32" => Some(("rill_peek_u32", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_i64" => Some(("rill_peek_i64", vec![Ty::Ptr, Ty::Int], Ty::Int)),
            "peek_f32" => Some(("rill_peek_f32", vec![Ty::Ptr, Ty::Int], Ty::Float)),
            "peek_f64" => Some(("rill_peek_f64", vec![Ty::Ptr, Ty::Int], Ty::Float)),
            // Sockets. A strand blocked on one parks rather than holding
            // up the worker running it.
            "tcp_listen" => Some(("rill_tcp_listen", vec![Ty::Int], Ty::Int)),
            "tcp_accept" => Some(("rill_tcp_accept", vec![Ty::Int], Ty::Int)),
            "tcp_connect" => Some(("rill_tcp_connect", vec![Ty::Str, Ty::Int], Ty::Int)),
            "sock_read" => Some(("rill_sock_read", vec![Ty::Int], Ty::Str)),
            // Waits for something to read, or for the time to run out, and
            // says which — the only way to put a deadline on a connection
            // that has gone quiet without closing it from another strand.
            "sock_wait" => Some(("rill_sock_wait", vec![Ty::Int, Ty::Int], Ty::Bool)),
            // The address at the other end, packed into one number, for a
            // server that wants to know how many connections it already has
            // from there.
            "sock_peer" => Some(("rill_sock_peer", vec![Ty::Int], Ty::Int)),
            "sock_write" => Some(("rill_sock_write", vec![Ty::Int, Ty::Str], Ty::Int)),
            "sock_close" => Some(("rill_sock_close", vec![Ty::Int], Ty::Unit)),
            _ => None,
        };
        if let Some((rt, param_tys, ret_ty)) = simple_builtin {
            if args.len() != param_tys.len() {
                return Err(TypeError::new(
                    line,
                    format!("`{name}` takes {} argument(s), but {} given", param_tys.len(), args.len()),
                ));
            }
            let mut targs = Vec::new();
            for (a, pt) in args.iter().zip(&param_tys) {
                targs.push(self.arg_for(a, pt, env, line, name)?);
            }
            return Ok(TExpr { ty: ret_ty, line, kind: TK::Builtin { rt, args: targs } });
        }

        if let Some(sig) = self.externs.get(name).cloned() {
            let params = &sig.params;
            let ret = sig.ret.rill_ty();
            if args.len() != params.len() {
                return Err(TypeError::new(
                    line,
                    format!(
                        "`{name}` takes {} argument(s), but {} given",
                        params.len(),
                        args.len()
                    ),
                ));
            }
            let mut targs = Vec::new();
            for (a, pt) in args.iter().zip(params) {
                targs.push(self.arg_for(a, &pt.rill_ty(), env, line, "foreign argument")?);
            }
            return Ok(TExpr {
                ty: ret,
                line,
                kind: TK::CallExtern { name: name.to_string(), args: targs },
            });
        }

        if let Some(&(tid, mid)) = self.method_index.get(name) {
            let sig = self.traits[tid].1[mid].clone();
            if args.len() != sig.params.len() {
                return Err(TypeError::new(
                    line,
                    format!(
                        "method `{name}` of trait `{}` takes {} argument(s), but {} given",
                        self.traits[tid].0,
                        sig.params.len(),
                        args.len()
                    ),
                ));
            }
            let recv = self.fresh();
            let inst = [recv.clone()];
            let mut targs = Vec::new();
            for (a, pt) in args.iter().zip(&sig.params) {
                let want = subst_gen(pt, &inst);
                let at = self.infer(a, env)?;
                self.unify(&want, &at.ty, line, "method argument")?;
                targs.push(at);
            }
            let ret = subst_gen(&sig.ret, &inst);
            return Ok(TExpr {
                ty: ret,
                line,
                kind: TK::MethodCall { trait_id: tid, method: mid, recv, args: targs },
            });
        }

        if let Some(&(adt, ci)) = self.ctors.get(name) {
            let fields = self.adts[adt].ctors[ci].fields.clone();
            if args.len() != fields.len() {
                return Err(TypeError::new(
                    line,
                    format!(
                        "constructor `{name}` takes {} argument(s), but {} given",
                        fields.len(),
                        args.len()
                    ),
                ));
            }
            let targs: Vec<Ty> = (0..self.adts[adt].nparams).map(|_| self.fresh()).collect();
            let mut targ_exprs = Vec::new();
            for (a, ft) in args.iter().zip(&fields) {
                let at = self.infer(a, env)?;
                let want = subst_gen(ft, &targs);
                self.unify(&want, &at.ty, line, "constructor argument")?;
                targ_exprs.push(at);
            }
            return Ok(TExpr {
                ty: Ty::Adt(adt, targs.clone()),
                line,
                kind: TK::CallCtor { adt, ctor: ci, targs, args: targ_exprs },
            });
        }

        if name == "main" {
            return Err(TypeError::new(line, "`main` cannot be called"));
        }
        let idx = match self.fn_index.get(name) {
            Some(&idx) => idx,
            None => return Err(TypeError::new(line, format!("unknown function `{name}`"))),
        };
        let (params, ret, subst) = match self.sigs[idx].clone() {
            SigState::Provisional { params, ret } => (params, ret, vec![]),
            SigState::Generalized { params, ret, ngen } => self.instantiate(&params, &ret, ngen),
        };
        if args.len() != params.len() {
            return Err(TypeError::new(
                line,
                format!(
                    "function `{name}` takes {} argument(s), but {} given",
                    params.len(),
                    args.len()
                ),
            ));
        }
        let mut targs = Vec::new();
        for (a, pt) in args.iter().zip(&params) {
            // A parameter annotated `Ptr` is a `Ptr` position like any other,
            // so a buffer handed to one decays there too.
            if matches!(self.resolve(pt), Ty::Ptr) {
                targs.push(self.arg_for(a, &Ty::Ptr, env, line, "function argument")?);
                continue;
            }
            let at = self.infer(a, env)?;
            // A function that closed over something is checked together with
            // the one it was written in, which is what lets it take a captured
            // buffer without being told its width — and is also why it settles
            // on one type rather than one per call. (One that captures nothing
            // is not tied to its parent and generalizes like any other.) The
            // error reads as an ordinary mismatch, so say what it really is.
            self.unify(pt, &at.ty, line, "function argument").map_err(|e| {
                match (&self.sigs[idx], self.lifted.get(idx).and_then(|l| l.as_ref())) {
                    (SigState::Provisional { .. }, Some(outer)) => {
                        // Lifting renamed it `outer.name`; say the name that
                        // was written.
                        let short = name.rsplit('.').next().unwrap_or(name);
                        TypeError::new(
                            line,
                            format!(
                                "{}\n  `{short}` uses names from `{outer}`, so it is checked \
                                 with it and has one type rather than one per call. A local \
                                 function that captures nothing does generalize; this one \
                                 would have to move to the top level.",
                                e.msg
                            ),
                        )
                    }
                    _ => e,
                }
            })?;
            targs.push(at);
        }
        Ok(TExpr { ty: ret, line, kind: TK::CallFn { fn_idx: idx, subst, args: targs } })
    }

    fn infer_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[Arm],
        line: usize,
        env: &mut HashMap<String, Ty>,
    ) -> Result<TExpr, TypeError> {
        let scrut = self.infer(scrutinee, env)?;
        // `?` is written for a `Result`, and the two arms it becomes only fit
        // one. Anything else gets told what it is rather than being shown a
        // pattern it never wrote.
        if is_try_match(arms) {
            let ok = match self.resolve(&scrut.ty) {
                Ty::Adt(i, _) => self.adts[i].name == "Result",
                Ty::Var(_) => true,
                _ => false,
            };
            if !ok {
                return Err(TypeError::new(
                    line,
                    format!(
                        "`?` takes the value out of a `Result`, and this is `{}` — \
                         `match` on it, or `unwrap_or` for a default",
                        self.type_name(&scrut.ty)
                    ),
                ));
            }
        }
        let result_ty = self.fresh();
        let mut tarms = Vec::new();
        // Rows seen so far, for the reachability check.
        let mut matrix: Vec<Vec<Pattern>> = Vec::new();

        for arm in arms {
            let mut arm_env = env.clone();
            let mut bound = HashSet::new();
            self.check_pattern(&arm.pattern, &scrut.ty, arm.line, &mut arm_env, &mut bound)?;

            if !self.useful(&matrix, &[arm.pattern.clone()], &[scrut.ty.clone()]) {
                return Err(TypeError::new(
                    arm.line,
                    "unreachable match arm: earlier arms already cover this pattern".to_string(),
                ));
            }
            // A guarded arm is checked against what came before it, but adds
            // nothing to what is covered: its condition may be false, and then
            // the value falls through to the arms below. So the rows below it
            // stay reachable, and the match still has to cover the case
            // without it — which is what makes `Some(n) if n > 0` alone not
            // exhaustive, and rightly.
            let guard = match &arm.guard {
                Some(g) => {
                    let tg = self.infer(g, &mut arm_env)?;
                    self.unify(&Ty::Bool, &tg.ty, tg.line, "a match guard")?;
                    Some(tg)
                }
                None => {
                    matrix.push(vec![arm.pattern.clone()]);
                    None
                }
            };

            let body = self.infer(&arm.body, &mut arm_env)?;
            // The one `match` a program did not write is the one `?` becomes,
            // and its arms disagreeing means only one thing: the `?` was
            // written where the block's value is not the function's. Say that
            // rather than showing the match nobody typed.
            self.unify(&result_ty, &body.ty, arm.line, "match arms").map_err(|e| {
                if is_try_match(arms) {
                    TypeError::new(
                        arm.line,
                        format!(
                            "`?` leaves the function with an `Err`, so the block it is in \
                             has to be what the function returns — and this one is worth \
                             `{}` instead. Take the `?` out to a block that is.",
                            self.type_name(&result_ty)
                        ),
                    )
                } else {
                    e
                }
            })?;
            tarms.push(TArm { pattern: arm.pattern.clone(), guard, body, line: arm.line });
        }

        if let Some(w) = self.witness(&matrix, &[scrut.ty.clone()]) {
            return Err(TypeError::new(
                line,
                format!("match is not exhaustive; no arm matches `{}`", show_pattern(&w[0])),
            ));
        }
        Ok(TExpr {
            ty: result_ty,
            line,
            kind: TK::Match { scrutinee: Box::new(scrut), arms: tarms },
        })
    }

    // ---- patterns --------------------------------------------------------

    /// Types a pattern against the value it matches, binding its variables.
    fn check_pattern(
        &mut self,
        p: &Pattern,
        ty: &Ty,
        line: usize,
        env: &mut HashMap<String, Ty>,
        bound: &mut HashSet<String>,
    ) -> Result<(), TypeError> {
        match p {
            // `lift` turned one arm holding alternatives into one arm per
            // alternative before this ran; there is nothing left to check.
            Pattern::Or(..) => unreachable!("`lift` expands `|` patterns"),
            Pattern::Wildcard => Ok(()),
            Pattern::Var(n) => {
                if !bound.insert(n.clone()) {
                    return Err(TypeError::new(
                        line,
                        format!("name `{n}` is bound twice in this pattern"),
                    ));
                }
                env.insert(n.clone(), ty.clone());
                Ok(())
            }
            Pattern::Int(_) => self.unify(ty, &Ty::Int, line, "match pattern"),
            Pattern::Ctor { name, args, line: cline } => {
                let &(adt, ci) = self.ctors.get(name).ok_or_else(|| {
                    TypeError::new(*cline, format!("unknown constructor `{name}`"))
                })?;
                let targs: Vec<Ty> = (0..self.adts[adt].nparams).map(|_| self.fresh()).collect();
                self.unify(ty, &Ty::Adt(adt, targs.clone()), *cline, "match pattern")?;
                let fields = self.adts[adt].ctors[ci].fields.clone();
                if args.len() != fields.len() {
                    return Err(TypeError::new(
                        *cline,
                        format!(
                            "constructor `{name}` has {} field(s), but the pattern names {}",
                            fields.len(),
                            args.len()
                        ),
                    ));
                }
                for (sub, ft) in args.iter().zip(&fields) {
                    let fty = subst_gen(ft, &targs);
                    self.check_pattern(sub, &fty, *cline, env, bound)?;
                }
                Ok(())
            }
        }
    }

    /// The constructor a pattern tests for, if it tests for one.
    fn head_of(&self, p: &Pattern) -> Option<Head> {
        match p {
            Pattern::Int(v) => Some(Head::Int(*v)),
            Pattern::Ctor { name, .. } => self.ctors.get(name).map(|&(_, ci)| Head::Ctor(ci)),
            _ => None,
        }
    }

    /// Types of the values a head exposes (its constructor fields).
    fn sub_types(&self, h: Head, col_ty: &Ty) -> Vec<Ty> {
        match (h, self.resolve(col_ty)) {
            (Head::Ctor(ci), Ty::Adt(adt, targs)) => self.adts[adt].ctors[ci]
                .fields
                .iter()
                .map(|f| subst_gen(f, &targs))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Rows that can still match once the first column is known to be `h`,
    /// with the head's fields spliced in where the column was.
    fn specialize(&self, matrix: &[Vec<Pattern>], h: Head, arity: usize) -> Vec<Vec<Pattern>> {
        let mut out = Vec::new();
        for row in matrix {
            let mut new = match (&row[0], h) {
                (Pattern::Wildcard, _) | (Pattern::Var(_), _) => {
                    vec![Pattern::Wildcard; arity]
                }
                (p, _) if self.head_of(p) == Some(h) => match p {
                    Pattern::Ctor { args, .. } => args.clone(),
                    _ => Vec::new(),
                },
                _ => continue,
            };
            new.extend_from_slice(&row[1..]);
            out.push(new);
        }
        out
    }

    /// Rows that can still match when the first column is none of the heads
    /// the matrix tests for.
    fn default_matrix(&self, matrix: &[Vec<Pattern>]) -> Vec<Vec<Pattern>> {
        matrix
            .iter()
            .filter(|row| matches!(row[0], Pattern::Wildcard | Pattern::Var(_)))
            .map(|row| row[1..].to_vec())
            .collect()
    }

    /// The distinct heads tested in the first column, in order.
    fn column_heads(&self, matrix: &[Vec<Pattern>]) -> Vec<Head> {
        let mut heads = Vec::new();
        for row in matrix {
            if let Some(h) = self.head_of(&row[0]) {
                if !heads.contains(&h) {
                    heads.push(h);
                }
            }
        }
        heads
    }

    /// True when the heads cover every constructor of the column's type, so
    /// no default case is reachable.
    fn is_complete(&self, col_ty: &Ty, heads: &[Head]) -> bool {
        match self.resolve(col_ty) {
            Ty::Adt(adt, _) => {
                let n = self.adts[adt].ctors.len();
                (0..n).all(|ci| heads.contains(&Head::Ctor(ci)))
            }
            // Int (and everything else) has no finite constructor set.
            _ => false,
        }
    }

    /// Maranget's usefulness test: can any value match `q` without having
    /// matched an earlier row of `matrix`?
    fn useful(&self, matrix: &[Vec<Pattern>], q: &[Pattern], tys: &[Ty]) -> bool {
        if tys.is_empty() {
            return matrix.is_empty();
        }
        let col_ty = tys[0].clone();
        match self.head_of(&q[0]) {
            Some(h) => {
                let sub = self.sub_types(h, &col_ty);
                let arity = sub.len();
                let m2 = self.specialize(matrix, h, arity);
                let q2 = self.specialize(std::slice::from_ref(&q.to_vec()), h, arity);
                let mut tys2 = sub;
                tys2.extend_from_slice(&tys[1..]);
                match q2.first() {
                    Some(row) => self.useful(&m2, row, &tys2),
                    None => false,
                }
            }
            None => {
                let heads = self.column_heads(matrix);
                if !heads.is_empty() && self.is_complete(&col_ty, &heads) {
                    heads.iter().any(|&h| {
                        let sub = self.sub_types(h, &col_ty);
                        let arity = sub.len();
                        let m2 = self.specialize(matrix, h, arity);
                        let mut q2 = vec![Pattern::Wildcard; arity];
                        q2.extend_from_slice(&q[1..]);
                        let mut tys2 = sub;
                        tys2.extend_from_slice(&tys[1..]);
                        self.useful(&m2, &q2, &tys2)
                    })
                } else {
                    self.useful(&self.default_matrix(matrix), &q[1..], &tys[1..])
                }
            }
        }
    }

    /// An example value the matrix fails to match, or `None` when it is
    /// exhaustive. Drives the "no arm matches ..." error.
    fn witness(&self, matrix: &[Vec<Pattern>], tys: &[Ty]) -> Option<Vec<Pattern>> {
        if tys.is_empty() {
            return matrix.is_empty().then(Vec::new);
        }
        let col_ty = tys[0].clone();
        let heads = self.column_heads(matrix);
        if !heads.is_empty() && self.is_complete(&col_ty, &heads) {
            let adt = match self.resolve(&col_ty) {
                Ty::Adt(adt, _) => adt,
                _ => unreachable!("only ADTs have complete signatures"),
            };
            for ci in 0..self.adts[adt].ctors.len() {
                let h = Head::Ctor(ci);
                let sub = self.sub_types(h, &col_ty);
                let arity = sub.len();
                let m2 = self.specialize(matrix, h, arity);
                let mut tys2 = sub;
                tys2.extend_from_slice(&tys[1..]);
                if let Some(w) = self.witness(&m2, &tys2) {
                    let mut out = vec![Pattern::Ctor {
                        name: self.adts[adt].ctors[ci].name.clone(),
                        args: w[..arity].to_vec(),
                        line: 0,
                    }];
                    out.extend_from_slice(&w[arity..]);
                    return Some(out);
                }
            }
            None
        } else {
            let rest = self.witness(&self.default_matrix(matrix), &tys[1..])?;
            let mut out = vec![self.missing_head(&col_ty, &heads)];
            out.extend(rest);
            Some(out)
        }
    }

    /// A pattern for a value the matrix's first column never tests for.
    fn missing_head(&self, col_ty: &Ty, heads: &[Head]) -> Pattern {
        if heads.is_empty() {
            return Pattern::Wildcard;
        }
        match self.resolve(col_ty) {
            Ty::Adt(adt, _) => {
                for (ci, c) in self.adts[adt].ctors.iter().enumerate() {
                    if !heads.contains(&Head::Ctor(ci)) {
                        return Pattern::Ctor {
                            name: c.name.clone(),
                            args: vec![Pattern::Wildcard; c.fields.len()],
                            line: 0,
                        };
                    }
                }
                Pattern::Wildcard
            }
            Ty::Int => {
                let mut v = 0;
                while heads.contains(&Head::Int(v)) {
                    v += 1;
                }
                Pattern::Int(v)
            }
            _ => Pattern::Wildcard,
        }
    }
}

/// Which value shape a pattern tests for; within one column all constructor
/// heads belong to the same ADT, so the constructor index identifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    Ctor(usize),
    Int(i64),
}

fn show_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Or(..) => unreachable!("`lift` expands `|` patterns"),
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var(n) => n.clone(),
        Pattern::Int(v) => v.to_string(),
        Pattern::Ctor { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let a: Vec<String> = args.iter().map(show_pattern).collect();
                format!("{name}({})", a.join(", "))
            }
        }
    }
}

// ---- helpers --------------------------------------------------------------

/// Whether these are the two arms `lift` writes for a `?`. The error binding
/// it names them with cannot be spelled in Rill, so nothing else can look
/// like this.
fn is_try_match(arms: &[Arm]) -> bool {
    arms.len() == 2
        && matches!(
            &arms[1].pattern,
            Pattern::Ctor { name, args, .. }
                if name == "Err"
                    && matches!(args.as_slice(), [Pattern::Var(v)] if v == "try.e")
        )
}

/// Substitutes `Gen(i)` -> `args[i]` in a type.
fn subst_gen(ty: &Ty, args: &[Ty]) -> Ty {
    match ty {
        Ty::Gen(i) => args[*i as usize].clone(),
        Ty::Adt(a, ts) => Ty::Adt(*a, ts.iter().map(|t| subst_gen(t, args)).collect()),
        Ty::Fn(p, r) => Ty::Fn(
            p.iter().map(|t| subst_gen(t, args)).collect(),
            Box::new(subst_gen(r, args)),
        ),
        Ty::Chan(e) => Ty::Chan(Box::new(subst_gen(e, args))),
        Ty::Map(k, v) => Ty::Map(Box::new(subst_gen(k, args)), Box::new(subst_gen(v, args))),
        Ty::Buf(w) => Ty::Buf(w.clone()),
        other => other.clone(),
    }
}

/// Groups functions so that each is checked only after everything it calls,
/// with mutually recursive functions kept together. Tarjan's algorithm emits
/// exactly that order.
fn dependency_groups(fns: &[FnDef], index: &HashMap<String, usize>) -> Vec<Vec<usize>> {
    let n = fns.len();
    let mut slot_of = vec![usize::MAX; n];
    for (k, f) in fns.iter().enumerate() {
        slot_of[index[&f.name]] = k;
    }
    let mut edges: Vec<Vec<usize>> = Vec::with_capacity(n);
    for f in fns {
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        free_vars(&f.body, &mut names, &mut seen);
        // A lifted function is inferred with the one it was written inside:
        // a captured value must have the type it had where it was captured,
        // and a `Buf` width is never inferred from use.
        if let Some(p) = &f.lifted_from {
            names.push(p.clone());
        }
        let mut out: Vec<usize> = names
            .iter()
            .filter_map(|nm| index.get(nm))
            .map(|&i| slot_of[i])
            .filter(|&k| k != usize::MAX)
            .collect();
        out.sort_unstable();
        out.dedup();
        edges.push(out);
    }

    let mut idx = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut groups: Vec<Vec<usize>> = Vec::new();

    // Iterative Tarjan: the call graph can be as deep as the program is long.
    for root in 0..n {
        if idx[root] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((v, ei)) = work.pop() {
            if ei == 0 {
                idx[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if ei < edges[v].len() {
                let w = edges[v][ei];
                work.push((v, ei + 1));
                if idx[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(idx[w]);
                }
                continue;
            }
            if low[v] == idx[v] {
                let mut group = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                group.sort_unstable();
                groups.push(caller_first(group, &edges));
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
        }
    }
    groups
}

/// The members of one group, callers before the functions they call.
///
/// A group is inferred with every signature in it still provisional, so what
/// a caller says about its callee's parameters is only known once the caller's
/// body has been through. That matters wherever the checker has to look at a
/// type as it stands — indexing and the `Buf` builtins, whose element type is
/// not a variable — and it is what lets a function lifted out of another one
/// take a buffer without an annotation. A real cycle has no such order; what
/// is left keeps its index order.
fn caller_first(group: Vec<usize>, edges: &[Vec<usize>]) -> Vec<usize> {
    if group.len() < 2 {
        return group;
    }
    let inside: HashSet<usize> = group.iter().copied().collect();
    let mut callers: HashMap<usize, usize> = group.iter().map(|&k| (k, 0)).collect();
    for &k in &group {
        for &c in &edges[k] {
            if c != k && inside.contains(&c) {
                *callers.get_mut(&c).unwrap() += 1;
            }
        }
    }
    let mut out: Vec<usize> = Vec::with_capacity(group.len());
    let mut seen: HashSet<usize> = HashSet::new();
    // Depth-first from whatever nothing in the group calls, then anything a
    // cycle left over, in index order.
    let roots: Vec<usize> = group.iter().copied().filter(|k| callers[k] == 0).collect();
    for start in roots.into_iter().chain(group.iter().copied()) {
        let mut work = vec![start];
        while let Some(v) = work.pop() {
            if !inside.contains(&v) || !seen.insert(v) {
                continue;
            }
            out.push(v);
            let mut next: Vec<usize> =
                edges[v].iter().copied().filter(|c| inside.contains(c) && !seen.contains(c)).collect();
            next.sort_unstable();
            next.reverse();
            work.extend(next);
        }
    }
    out
}

/// A position in a foreign signature. Width names (`I32`, `F32`, ...) exist
/// only here, so they are matched before falling back to the ordinary type
/// namespace — which keeps `unknown type` errors and the `Str` advice intact
/// for everything else.
fn resolve_c_type(
    tr: &TypeRef,
    adt_index: &HashMap<String, usize>,
    adts: &[AdtDef],
    what: &str,
) -> Result<CTy, TypeError> {
    let width = match tr.name.as_str() {
        "I8" => Some(CTy::I8),
        "U8" => Some(CTy::U8),
        "I16" => Some(CTy::I16),
        "U16" => Some(CTy::U16),
        "I32" => Some(CTy::I32),
        "U32" => Some(CTy::U32),
        "I64" => Some(CTy::I64),
        "U64" => Some(CTy::U64),
        "F32" => Some(CTy::F32),
        "F64" => Some(CTy::F64),
        "USize" => Some(CTy::USize),
        "ISize" => Some(CTy::ISize),
        _ => None,
    };
    if let Some(cty) = width {
        if !tr.args.is_empty() {
            return Err(TypeError::new(
                tr.line,
                format!("`{}` does not take type arguments", tr.name),
            ));
        }
        return Ok(cty);
    }
    // `Fn(...)` is a C function pointer, optionally with a result: `Fn(I32) ->
    // I32`. Without the arrow it hands nothing back, which is what a plain
    // notification callback wants.
    if tr.name == "Fn" {
        let mut params = Vec::new();
        for a in &tr.args {
            let p = resolve_c_type(a, adt_index, adts, what)?;
            if matches!(p, CTy::Fn(..) | CTy::Unit) {
                return Err(TypeError::new(
                    a.line,
                    format!(
                        "{what}: a callback parameter cannot be `{}`",
                        p.name()
                    ),
                ));
            }
            params.push(p);
        }
        let ret = match &tr.ret {
            Some(r) => {
                let cty = resolve_c_type(r, adt_index, adts, what)?;
                // A callback answering with another callback would need a
                // trampoline built at run time, from a value C chose.
                if matches!(cty, CTy::Fn(..)) {
                    return Err(TypeError::new(
                        r.line,
                        format!("{what}: a callback cannot return `Fn`"),
                    ));
                }
                cty
            }
            None => CTy::Unit,
        };
        return Ok(CTy::Fn(params, Box::new(ret)));
    }
    if let Some(r) = &tr.ret {
        return Err(TypeError::new(
            r.line,
            format!("{what}: only `Fn` takes a `->` result type"),
        ));
    }
    let ty = resolve_type_ref(tr, adt_index, adts, None)?;
    check_ffi_type(&ty, tr.line, what)
}

/// The C ABI can carry numbers, pointers and nothing else Rill has: a `Str`
/// is a counted heap object rather than a `char*`, so it has to be bridged
/// explicitly.
fn check_ffi_type(ty: &Ty, line: usize, what: &str) -> Result<CTy, TypeError> {
    match ty {
        // Unadorned `Int` and `Float` mean C's 64-bit forms. Anything narrower
        // has to say so: a `float` parameter handed a double reads back the
        // wrong half of the register, silently.
        Ty::Int => Ok(CTy::I64),
        Ty::Float => Ok(CTy::F64),
        Ty::Bool => Ok(CTy::Bool),
        Ty::Ptr => Ok(CTy::Ptr),
        Ty::Unit => Ok(CTy::Unit),
        Ty::Str => Err(TypeError::new(
            line,
            format!(
                "{what}: a foreign function cannot take `Str` directly — pass \
                 `to_cstr(s)` and declare it as `Ptr`"
            ),
        )),
        other => Err(TypeError::new(
            line,
            format!("{what}: `{other:?}` cannot cross the C boundary; use `Ptr`"),
        )),
    }
}

/// The impl-table key for a concrete type, if traits can target it. An impl
/// covers a whole type constructor, so `List(Int)` and `List(Str)` share a key
/// and the receiver's type arguments pick the instance.
/// A type as a program would write it, for anything that has to show one
/// after checking: `rill types`, an editor asking what a name is. The
/// checker's own `type_name` is this with inference variables resolved first,
/// which is what it needs mid-inference and what nothing else does.
pub fn show_type(ty: &Ty, adts: &[AdtDef]) -> String {
    match ty {
        Ty::Int => "Int".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "Bool".into(),
        Ty::Str => "Str".into(),
        Ty::Unit => "Unit".into(),
        Ty::Ptr => "Ptr".into(),
        Ty::StrBuf => "StrBuf".into(),
        Ty::Buf(w) => format!("Buf({})", w.name()),
        Ty::Adt(i, args) => {
            if args.is_empty() {
                adts[*i].name.clone()
            } else {
                let args: Vec<String> = args.iter().map(|a| show_type(a, adts)).collect();
                format!("{}({})", adts[*i].name, args.join(", "))
            }
        }
        Ty::Fn(params, ret) => {
            let params: Vec<String> = params.iter().map(|p| show_type(p, adts)).collect();
            format!("({}) -> {}", params.join(", "), show_type(ret, adts))
        }
        Ty::Chan(e) => format!("Chan({})", show_type(e, adts)),
        Ty::Map(k, v) => format!("Map({}, {})", show_type(k, adts), show_type(v, adts)),
        Ty::Var(_) => "_".into(),
        Ty::Gen(i) => format!("'{}", (b'a' + (*i % 26) as u8) as char),
    }
}

/// A function's signature as a program would write it, with the requirements
/// its body imposed on each type parameter — which is the part a reader cannot
/// work out and the checker can.
pub fn show_signature(f: &TFn, adts: &[AdtDef]) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(n, t)| format!("{n}: {}", show_type(t, adts)))
        .collect();
    let mut s = format!("fn {}({}) -> {}", f.name, params.join(", "), show_type(&f.ret, adts));
    let mut needs: Vec<String> = Vec::new();
    for (g, c, _) in &f.constraints {
        let what = match c {
            Constraint::Numeric => "a number",
            Constraint::Addable => "addable",
            Constraint::Ordered => "ordered",
            Constraint::Basic => "a value type",
        };
        let name = show_type(&Ty::Gen(*g), adts);
        let one = format!("{name} is {what}");
        if !needs.contains(&one) {
            needs.push(one);
        }
    }
    if !needs.is_empty() {
        s.push_str(&format!("    # where {}", needs.join(", ")));
    }
    s
}

/// Applies `f` to every immediate sub-expression of a checked expression.
/// Written here rather than beside its one user because more than one thing
/// wants to walk a body now: codegen, and whatever is answering an editor.
pub fn walk_texpr<'a>(e: &'a TExpr, f: &mut impl FnMut(&'a TExpr)) {
    match &e.kind {
        TK::Int(_) | TK::Float(_) | TK::Str(_) | TK::Bool(_) | TK::Local(_) | TK::FnRef { .. } => {}
        TK::MapNew => {}
        TK::Builtin { args, .. }
        | TK::CallFn { args, .. }
        | TK::MethodCall { args, .. }
        | TK::CallExtern { args, .. }
        | TK::CallCtor { args, .. } => args.iter().for_each(f),
        TK::CallClosure { callee, args } => {
            f(callee);
            args.iter().for_each(f);
        }
        TK::Println(x)
        | TK::Print(x)
        | TK::Show(x)
        | TK::Neg(x)
        | TK::Spawn { body: x, .. }
        | TK::Lambda { body: x, .. } => f(x),
        TK::ChanNew(x) => {
            if let Some(x) = x {
                f(x)
            }
        }
        TK::ChanSend { ch, val } => {
            f(ch);
            f(val);
        }
        TK::ChanRecv { ch }
        | TK::BufLen { buf: ch }
        | TK::BufPtr { buf: ch }
        | TK::MapLen { map: ch }
        | TK::MapClear { map: ch }
        | TK::FieldGet { base: ch, .. } => f(ch),
        TK::BufNew { count, .. } => f(count),
        TK::BufGet { buf: a, idx: b }
        | TK::MapFind { map: a, key: b }
        | TK::MapDel { map: a, key: b }
        | TK::MapNext { map: a, from: b }
        | TK::MapKeyAt { map: a, idx: b }
        | TK::MapValAt { map: a, idx: b }
        | TK::Binary { lhs: a, rhs: b, .. } => {
            f(a);
            f(b);
        }
        TK::BufSet { buf: a, idx: b, val: c } | TK::MapSet { map: a, key: b, val: c } => {
            f(a);
            f(b);
            f(c);
        }
        TK::Slice { base, from, to } => {
            f(base);
            f(from);
            if let Some(t) = to {
                f(t);
            }
        }
        TK::If { cond, then, els } => {
            f(cond);
            f(then);
            f(els);
        }
        TK::Match { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        TK::Select { arms, default } => {
            for a in arms {
                match &a.op {
                    TSelectOp::Recv { chan, .. } => f(chan),
                    TSelectOp::Send { chan, value } => {
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
        TK::Block(stmts) => stmts.iter().for_each(|s| match s {
            TStmt::Bind { value, .. } => f(value),
            TStmt::Expr(x) => f(x),
        }),
    }
}

pub fn impl_key(ty: &Ty) -> Option<ImplKey> {
    match ty {
        Ty::Int => Some(ImplKey::Int),
        Ty::Float => Some(ImplKey::Float),
        Ty::Bool => Some(ImplKey::Bool),
        Ty::Str => Some(ImplKey::Str),
        Ty::Adt(i, _) => Some(ImplKey::Adt(*i)),
        _ => None,
    }
}

/// The type arguments an impl instance needs for a receiver type.
pub fn impl_targs(ty: &Ty) -> Vec<Ty> {
    match ty {
        Ty::Adt(_, args) => args.clone(),
        _ => Vec::new(),
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(name, "Int" | "Float" | "Bool" | "Str" | "StrBuf" | "Unit" | "Ptr")
}

/// How many arguments each map builtin takes, and by being `Some` at all,
/// that the name is one.
fn map_builtin_arity(name: &str) -> Option<usize> {
    Some(match name {
        "map_new" => 0,
        "map_len" | "map_clear" => 1,
        "map_find" | "map_del" | "map_next" | "map_key_at" | "map_val_at" => 2,
        "map_set" => 3,
        _ => return None,
    })
}

fn is_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "println"
            | "print"
            | "channel"
            | "send"
            | "recv"
            | "strbuf"
            | "strbuf_add"
            | "strbuf_byte"
            | "strbuf_str"
            | "str_len"
            | "int_to_str"
            | "float_to_str"
            | "bool_to_str"
            | "show"
            | "str_get"
            | "str_sub"
            | "str_find"
            | "str_find_from"
            | "str_cmp"
            | "str_to_int"
            | "str_to_float"
            | "chr"
            | "now_nanos"
            | "sleep_ms"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bit_not"
            | "shl"
            | "shr"
            | "to_float"
            | "to_int"
            | "arg_count"
            | "arg"
            | "read_file_raw"
            | "io_ok"
            | "write_file"
            | "read_line"
            | "scan_int"
            | "exit"
            | "to_cstr"
            | "from_cstr"
            | "free_ptr"
            | "null_ptr"
            | "is_null"
            | "alloc_bytes"
            | "ptr_offset"
            | "poke_i8"
            | "poke_i16"
            | "poke_i32"
            | "poke_i64"
            | "poke_f32"
            | "poke_f64"
            | "poke_ptr"
            | "peek_ptr"
            | "buf_i8"
            | "buf_u8"
            | "buf_i16"
            | "buf_u16"
            | "buf_i32"
            | "buf_u32"
            | "buf_int"
            | "buf_f32"
            | "buf_float"
            | "buf_ptr"
            | "buf_get"
            | "buf_set"
            | "buf_len"
            | "map_new"
            | "map_set"
            | "map_find"
            | "map_del"
            | "map_len"
            | "map_clear"
            | "map_next"
            | "map_key_at"
            | "map_val_at"
            | "peek_i8"
            | "peek_u8"
            | "peek_i16"
            | "peek_u16"
            | "peek_i32"
            | "peek_u32"
            | "peek_i64"
            | "peek_f32"
            | "peek_f64"
            | "tcp_listen"
            | "tcp_accept"
            | "tcp_connect"
            | "sock_read"
            | "sock_wait"
            | "sock_peer"
            | "sock_write"
            | "sock_close"
    )
}

fn resolve_type_ref(
    tr: &TypeRef,
    adt_index: &HashMap<String, usize>,
    adts: &[AdtDef],
    params: Option<&HashMap<&str, u32>>,
) -> Result<Ty, TypeError> {
    // Lowercase: a type parameter (only valid inside a generic `type` def).
    if tr.name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        if !tr.args.is_empty() {
            return Err(TypeError::new(
                tr.line,
                format!("type parameter `{}` cannot take arguments", tr.name),
            ));
        }
        return match params.and_then(|p| p.get(tr.name.as_str())) {
            Some(&i) => Ok(Ty::Gen(i)),
            None => Err(TypeError::new(tr.line, format!("unknown type `{}`", tr.name))),
        };
    }
    if tr.name == "Buf" {
        if tr.args.len() != 1 {
            return Err(TypeError::new(
                tr.line,
                "`Buf` takes exactly one element width".to_string(),
            ));
        }
        let w = resolve_c_type(&tr.args[0], adt_index, adts, "buffer element")?;
        if matches!(w, CTy::Fn(..) | CTy::Unit) {
            return Err(TypeError::new(
                tr.args[0].line,
                format!("a buffer cannot hold `{}`", w.name()),
            ));
        }
        // A buffer is memory with a layout, and an index into it is elements
        // times a width the program has to know. `USize` is whatever the
        // target says, so a buffer of them would change shape underneath a C
        // library that was told how wide its array is. Name the width.
        if w.is_word_sized() {
            return Err(TypeError::new(
                tr.args[0].line,
                format!(
                    "a buffer cannot hold `{}`: its width is the target's, and a buffer's \
                     layout is the program's. Use `I64`/`U64` or `I32`/`U32`",
                    w.name()
                ),
            ));
        }
        return Ok(Ty::Buf(w));
    }
    if tr.name == "Chan" {
        if tr.args.len() != 1 {
            return Err(TypeError::new(
                tr.line,
                "`Chan` takes exactly one type argument".to_string(),
            ));
        }
        let elem = resolve_type_ref(&tr.args[0], adt_index, adts, params)?;
        return Ok(Ty::Chan(Box::new(elem)));
    }
    if tr.name == "Map" {
        if tr.args.len() != 2 {
            return Err(TypeError::new(
                tr.line,
                "`Map` takes a key type and a value type".to_string(),
            ));
        }
        let k = resolve_type_ref(&tr.args[0], adt_index, adts, params)?;
        let v = resolve_type_ref(&tr.args[1], adt_index, adts, params)?;
        return Ok(Ty::Map(Box::new(k), Box::new(v)));
    }
    if is_builtin_type(&tr.name) {
        if !tr.args.is_empty() {
            return Err(TypeError::new(
                tr.line,
                format!("`{}` does not take type arguments", tr.name),
            ));
        }
        return Ok(match tr.name.as_str() {
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "Bool" => Ty::Bool,
            "Str" => Ty::Str,
            "StrBuf" => Ty::StrBuf,
            "Ptr" => Ty::Ptr,
            _ => Ty::Unit,
        });
    }
    let &i = adt_index
        .get(&tr.name)
        .ok_or_else(|| TypeError::new(tr.line, format!("unknown type `{}`", tr.name)))?;
    if tr.args.len() != adts[i].nparams {
        return Err(TypeError::new(
            tr.line,
            format!(
                "type `{}` takes {} type argument(s), but {} given",
                tr.name,
                adts[i].nparams,
                tr.args.len()
            ),
        ));
    }
    let mut args = Vec::new();
    for a in &tr.args {
        args.push(resolve_type_ref(a, adt_index, adts, params)?);
    }
    Ok(Ty::Adt(i, args))
}

/// Collects free variable names of an expression in first-use order.
fn free_vars(e: &Expr, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    match e {
        Expr::Var(n, _) => {
            if seen.insert(n.clone()) {
                out.push(n.clone());
            }
        }
        Expr::Call { name, args, .. } => {
            if seen.insert(name.clone()) {
                out.push(name.clone());
            }
            for a in args {
                free_vars(a, out, seen);
            }
        }
        Expr::Unary { expr, .. } => free_vars(expr, out, seen),
        Expr::Binary { lhs, rhs, .. } => {
            free_vars(lhs, out, seen);
            free_vars(rhs, out, seen);
        }
        Expr::If { cond, then, els } => {
            free_vars(cond, out, seen);
            free_vars(then, out, seen);
            free_vars(els, out, seen);
        }
        Expr::Match { scrutinee, arms, .. } => {
            free_vars(scrutinee, out, seen);
            for arm in arms {
                // Pattern bindings shadow, but over-capturing is harmless:
                // the capture list is filtered against the enclosing env.
                if let Some(g) = &arm.guard {
                    free_vars(g, out, seen);
                }
                free_vars(&arm.body, out, seen);
            }
        }
        Expr::Len { expr, .. } | Expr::Try { expr, .. } => free_vars(expr, out, seen),
        Expr::Field { base, .. } => free_vars(base, out, seen),
        Expr::Slice { base, from, to, .. } => {
            free_vars(base, out, seen);
            if let Some(f) = from {
                free_vars(f, out, seen);
            }
            if let Some(t) = to {
                free_vars(t, out, seen);
            }
        }
        Expr::Index { base, index, .. } => {
            free_vars(base, out, seen);
            free_vars(index, out, seen);
        }
        Expr::IndexSet { base, index, value, .. }
        | Expr::IndexOp { base, index, value, .. } => {
            free_vars(base, out, seen);
            free_vars(index, out, seen);
            free_vars(value, out, seen);
        }
        Expr::Append { target, value, .. } => {
            free_vars(target, out, seen);
            free_vars(value, out, seen);
        }
        Expr::For { from, to, acc, body, .. } => {
            free_vars(from, out, seen);
            free_vars(to, out, seen);
            if let Some((_, init)) = acc {
                free_vars(init, out, seen);
            }
            free_vars(body, out, seen);
        }
        Expr::Spawn { body, .. } => free_vars(body, out, seen),
        Expr::Select { arms, default, .. } => {
            for a in arms {
                match &a.op {
                    SelectOp::Recv { chan, .. } => free_vars(chan, out, seen),
                    SelectOp::Send { chan, value } => {
                        free_vars(chan, out, seen);
                        free_vars(value, out, seen);
                    }
                }
                free_vars(&a.body, out, seen);
            }
            if let Some(d) = default {
                free_vars(d, out, seen);
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut inner_out = Vec::new();
            let mut inner_seen = seen.clone();
            for p in params {
                inner_seen.insert(p.clone());
            }
            free_vars(body, &mut inner_out, &mut inner_seen);
            for n in inner_out {
                if !params.contains(&n) && seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                match s {
                    Stmt::Bind { value, .. } | Stmt::PatBind { value, .. } => {
                        free_vars(value, out, seen)
                    }
                    Stmt::Fn(f) => free_vars(&f.body, out, seen),
                    Stmt::Expr(v) => free_vars(v, out, seen),
                }
            }
        }
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => {}
    }
}

trait FnDefExt {
    fn name_line(&self) -> usize;
}

impl FnDefExt for FnDef {
    fn name_line(&self) -> usize {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_src(src: &str) -> Result<TModule, TypeError> {
        check(&rill_syntax::parse(src).unwrap())
    }

    /// Like `check_src`, but with the prelude in scope (`List`, `Option`, ...).
    fn check_std(src: &str) -> Result<TModule, TypeError> {
        check(&rill_syntax::parse_program(src).unwrap())
    }

    fn sig<'a>(t: &'a TModule, name: &str) -> &'a TFn {
        &t.fns[t.fn_index[name]]
    }

    #[test]
    fn infers_int_fn() {
        let t = check_src("fn double(x) = x * 2\n\nfn main() = println(double(21))\n").unwrap();
        let f = sig(&t, "double");
        assert_eq!(f.params[0].1, Ty::Int);
        assert_eq!(f.ret, Ty::Int);
        assert_eq!(f.ngen, 0);
    }

    #[test]
    fn generalizes_identity() {
        let t = check_src(
            "fn id(x) = x\n\nfn main() =\n  println(id(1))\n  println(id(\"s\"))\n",
        )
        .unwrap();
        let f = sig(&t, "id");
        assert_eq!(f.ngen, 1);
        assert_eq!(f.params[0].1, Ty::Gen(0));
        assert_eq!(f.ret, Ty::Gen(0));
    }

    #[test]
    fn generic_adt_list() {
        let src = "type List(a)\n  Nil\n  Cons(head: a, tail: List(a))\n\nfn len(l) = match l\n  Nil -> 0\n  Cons(_, rest) -> 1 + len(rest)\n\nfn main() =\n  println(len(Cons(1, Nil)))\n  println(len(Cons(\"a\", Nil)))\n";
        let t = check_src(src).unwrap();
        let f = sig(&t, "len");
        assert_eq!(f.ngen, 1);
        assert_eq!(f.ret, Ty::Int);
    }

    #[test]
    fn lambda_and_closure_capture() {
        let src = "fn main() =\n  n = 10\n  f = \\x -> x + n\n  println(f(5))\n";
        let t = check_src(src).unwrap();
        let main = sig(&t, "main");
        // find the lambda
        fn find_lambda(e: &TExpr) -> Option<&TExpr> {
            match &e.kind {
                TK::Lambda { .. } => Some(e),
                TK::Block(stmts) => stmts.iter().find_map(|s| match s {
                    TStmt::Bind { value, .. } => find_lambda(value),
                    TStmt::Expr(v) => find_lambda(v),
                }),
                _ => None,
            }
        }
        let lam = find_lambda(&main.body).expect("lambda not found");
        match &lam.kind {
            TK::Lambda { captures, .. } => {
                assert_eq!(captures.len(), 1);
                assert_eq!(captures[0].0, "n");
                assert_eq!(captures[0].1, Ty::Int);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn higher_order_fn() {
        let src = "fn apply(f, x) = f(x)\n\nfn main() = println(apply(\\x -> x * 2, 21))\n";
        let t = check_src(src).unwrap();
        let f = sig(&t, "apply");
        match &f.params[0].1 {
            Ty::Fn(p, _) => assert_eq!(p.len(), 1),
            other => panic!("expected fn type, got {other:?}"),
        }
    }

    #[test]
    fn named_fn_as_value() {
        let src = "fn double(x) = x * 2\n\nfn apply(f, x) = f(x)\n\nfn main() = println(apply(double, 21))\n";
        check_src(src).unwrap();
    }

    #[test]
    fn rejects_int_float_mix() {
        assert!(check_src("fn main() = println(1 + 2.0)\n").is_err());
    }

    #[test]
    fn rejects_arity_mismatch_closure() {
        let src = "fn main() =\n  f = \\x -> x\n  println(f(1, 2))\n";
        assert!(check_src(src).is_err());
    }

    #[test]
    fn rejects_non_exhaustive_match() {
        let src = "type Color\n  Red\n  Green\n\nfn f(c) = match c\n  Red -> 1\n\nfn main() = println(f(Red))\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("Green"), "{}", err.msg);
    }

    #[test]
    fn rejects_printing_closure() {
        assert!(check_src("fn main() = println(\\x -> x)\n").is_err());
    }

    #[test]
    fn use_before_definition_pins_types() {
        let src = "fn main() = println(twice(1))\n\nfn twice(x) = x * 2\n";
        let t = check_src(src).unwrap();
        assert_eq!(sig(&t, "twice").ngen, 0);
    }

    #[test]
    fn type_settled_by_a_later_definition() {
        // `step` is generalized only after `stop` and `again`, which are what
        // decide it returns Unit. Checking in source order used to quantify
        // the return type into a variable nothing could satisfy.
        let src = "fn step(n) = if n == 0 then stop(n) else again(n)\n\nfn again(n) = step(n - 1)\n\nfn stop(n) = println(n)\n\nfn main() = step(3)\n";
        let t = check_src(src).unwrap();
        assert_eq!(sig(&t, "step").ngen, 0);
        assert_eq!(sig(&t, "step").ret, Ty::Unit);
    }

    #[test]
    fn constraints_ride_along_in_schemes() {
        // `<` used to pin a parameter to Int; now it travels as a
        // requirement, so the function stays generic.
        let t = check_std("fn biggest(a, b) = if a > b then a else b\n\nfn main() =\n  println(biggest(1, 2))\n  println(biggest(\"a\", \"b\"))\n").unwrap();
        let f = &t.fns[t.fn_index["biggest"]];
        assert_eq!(f.ngen, 1);
        assert_eq!(f.constraints.len(), 1);
        assert_eq!(f.constraints[0].1, Constraint::Ordered);
    }

    #[test]
    fn println_is_polymorphic() {
        let t = check_std("fn announce(x) = println(x)\n\nfn main() =\n  announce(1)\n  announce(\"s\")\n").unwrap();
        assert_eq!(t.fns[t.fn_index["announce"]].ngen, 1);
    }

    #[test]
    fn local_requirements_still_default_to_int() {
        let t = check_std("fn f() = 1 + 2\n\nfn main() = println(f())\n").unwrap();
        assert_eq!(t.fns[t.fn_index["f"]].ret, Ty::Int);
    }

    #[test]
    fn ord_impl_enables_comparison() {
        let src = "type M\n  M(Int)\n\nimpl Ord(M)\n  fn compare(a, b) = match a\n    M(x) -> match b\n      M(y) -> x - y\n\nfn main() = println(M(1) < M(2))\n";
        check_std(src).unwrap();
    }

    #[test]
    fn comparison_without_ord_is_rejected() {
        let src = "type C\n  Red\n\nfn main() = println(Red < Red)\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("Ord"), "{}", err.msg);
    }

    #[test]
    fn redefining_a_builtin_trait_is_rejected() {
        let src = "trait Ord(a)\n  cmp(a) -> Int\n\nfn main() = println(1)\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("already defined"), "{}", err.msg);
    }

    #[test]
    fn nested_patterns_type_check() {
        let src = "fn f(l) = match l\n  Nil -> 0\n  Cons(Some(0), _) -> 1\n  Cons(Some(n), _) -> n\n  Cons(None, _) -> 2\n\nfn main() = println(f(Nil))\n";
        check_std(src).unwrap();
    }

    #[test]
    fn nested_exhaustiveness_names_the_missing_case() {
        let src = "fn f(l) = match l\n  Nil -> 0\n  Cons(Some(x), _) -> x\n  Cons(None, Nil) -> 1\n\nfn main() = println(f(Nil))\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("Cons(None, Cons(_, _))"), "{}", err.msg);
    }

    #[test]
    fn int_exhaustiveness_names_a_missing_literal() {
        let src = "fn f(n) = match n\n  0 -> 1\n  1 -> 2\n\nfn main() = println(f(0))\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("`2`"), "{}", err.msg);
    }

    #[test]
    fn nested_unreachable_arm_rejected() {
        let src = "fn f(o) = match o\n  Some(0) -> 1\n  Some(_) -> 2\n  None -> 3\n  Some(1) -> 4\n\nfn main() = println(f(None))\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("unreachable"), "{}", err.msg);
    }

    #[test]
    fn nested_pattern_binds_at_depth() {
        let src = "fn f(o) = match o\n  Some(Cons(x, _)) -> x\n  Some(Nil) -> 0\n  None -> -1\n\nfn main() = println(f(None))\n";
        check_std(src).unwrap();
    }

    #[test]
    fn trait_method_generalizes() {
        let src = "trait D(a)\n  d(a) -> Str\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n\nimpl D(Str)\n  fn d(s) = s\n\nfn announce(x) = println(d(x))\n\nfn main() =\n  announce(1)\n  announce(\"a\")\n";
        let t = check_src(src).unwrap();
        assert_eq!(sig(&t, "announce").ngen, 1);
        assert_eq!(t.impl_methods.len(), 2);
    }

    #[test]
    fn impl_missing_method_rejected() {
        let src = "trait D(a)\n  d(a) -> Str\n  e(a) -> Int\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("missing method `e`"), "{}", err.msg);
    }

    #[test]
    fn impl_for_unknown_trait_rejected() {
        let src = "impl Nope(Int)\n  fn f(n) = 1\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("unknown trait"), "{}", err.msg);
    }

    #[test]
    fn fn_clashing_with_method_rejected() {
        let src = "trait D(a)\n  d(a) -> Str\n\nfn d(x) = \"no\"\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("trait method"), "{}", err.msg);
    }

    #[test]
    fn generic_impl_covers_every_instantiation() {
        let src = "type Box(a)\n  B(a)\n\ntrait D(a)\n  d(a) -> Str\n\nimpl D(Box(a))\n  fn d(b) = \"box\"\n\nfn main() = println(1)\n";
        let t = check_src(src).unwrap();
        // The method is generic over the target's parameter.
        assert_eq!(t.fns[t.fn_index["D.Box.d"]].ngen, 1);
    }

    #[test]
    fn impl_target_must_name_its_parameters() {
        let src = "type Box(a)\n  B(a)\n\ntrait D(a)\n  d(a) -> Str\n\nimpl D(Box)\n  fn d(b) = \"box\"\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("type parameter"), "{}", err.msg);
    }

    #[test]
    fn impl_pinned_to_one_instantiation_is_rejected() {
        let src = "type Box(a)\n  B(a)\n\ntrait D(a)\n  d(a) -> Str\n\nimpl D(Box(a))\n  fn d(b) = match b\n    B(x) -> int_to_str(x)\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("not general enough"), "{}", err.msg);
    }

    /// A width in a foreign signature records the C side of the call; the
    /// Rill side of it still speaks in `Int` and `Float`.
    #[test]
    fn foreign_widths_narrow_only_the_c_side() {
        let src = "extern fn sqrtf(x: F32) -> F32\n\nextern fn abs(n: I32) -> I32\n\nfn main() =\n  println(sqrtf(2.0))\n  println(abs(-7))\n";
        let t = check_src(src).unwrap();
        let f = &t.externs["sqrtf"];
        assert_eq!(f.params[0], CTy::F32);
        assert_eq!(f.ret, CTy::F32);
        assert_eq!(f.ret.rill_ty(), Ty::Float);
        assert_eq!(t.externs["abs"].params[0].rill_ty(), Ty::Int);
    }

    /// Unadorned, they still mean C's 64-bit forms.
    #[test]
    fn foreign_int_and_float_stay_sixty_four_bit() {
        let src = "extern fn f(a: Int, b: Float) -> Int\n\nfn main() = println(f(1, 2.0))\n";
        let t = check_src(src).unwrap();
        assert_eq!(t.externs["f"].params, vec![CTy::I64, CTy::F64]);
    }

    /// The one width a portable signature cannot write as a number. The Rill
    /// side of the call still deals in `Int`, exactly as `I32` does.
    #[test]
    fn foreign_word_width_is_the_targets() {
        let src = "extern fn strlen(s: Ptr) -> USize\n\nfn main() = println(strlen(to_cstr(\"ab\")))\n";
        let t = check_src(src).unwrap();
        assert_eq!(t.externs["strlen"].ret, CTy::USize);
        assert_eq!(t.externs["strlen"].ret.rill_ty(), Ty::Int);
        assert!(CTy::USize.is_word_sized() && CTy::ISize.is_signed());
        assert_eq!(CTy::USize.int_bits(), None);
    }

    /// A buffer is memory the program lays out, so its element width has to be
    /// one the program can state — not one the target decides.
    #[test]
    fn a_buffer_cannot_be_a_word_wide() {
        let src = "fn f(v: Buf(USize), i) = buf_get(v, i)\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("the target's"), "{}", err.msg);
    }

    /// Codegen has no LLVM type to pass here, and used to panic on it.
    #[test]
    fn foreign_unit_parameter_is_rejected() {
        let src = "extern fn f(x: Unit)\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("not a value C can be passed"), "{}", err.msg);
    }

    /// A buffer's width is part of its type, and reading one gives back the
    /// Rill type that width stands for.
    #[test]
    fn buffer_width_decides_the_element_type() {
        let src = "fn main() =\n  v = buf_f32(4)\n  buf_set(v, 0, 1.5)\n  println(buf_get(v, 0))\n";
        let t = check_src(src).unwrap();
        let _ = t;
        let u = check_src("fn total(v: Buf(I32), i) = buf_get(v, i)\n\nfn main() = println(total(buf_i32(2), 0))\n").unwrap();
        assert_eq!(sig(&u, "total").ret, Ty::Int);
        assert_eq!(sig(&u, "total").params[0].1, Ty::Buf(CTy::I32));
    }

    /// `b[i]` is `buf_get` and `b[i] = v` is `buf_set` — same types, same
    /// errors, so the notation carries no meaning of its own.
    #[test]
    fn indexing_is_the_buffer_builtins() {
        let t = check_src(
            "fn f(v: Buf(F32), i) =\n  v[i] = 1.5\n  v[i]\n\nfn main() = println(f(buf_f32(2), 0))\n",
        )
        .unwrap();
        assert_eq!(sig(&t, "f").ret, Ty::Float);
        let s = check_src("fn main() = println(\"ab\"[1])\n").unwrap();
        let _ = s;
        // An unannotated base cannot be a buffer, so it settles as a string
        // and drives the rest of inference the way `str_get` would.
        let t = check_src("fn f(s, i) = s[i] + 1\n\nfn main() = println(f(\"ab\", 0))\n").unwrap();
        assert_eq!(sig(&t, "f").params[0].1, Ty::Str);
        let err = check_src("fn main() =\n  x = 1\n  println(x[0])\n").unwrap_err();
        assert!(err.msg.contains("indexes a buffer, a map or a string"), "{}", err.msg);
        let err = check_src("fn main() =\n  s = \"ab\"\n  s[0] = 65\n").unwrap_err();
        assert!(err.msg.contains("not storage"), "{}", err.msg);
        let err = check_src("fn main() =\n  v = buf_i32(2)\n  v[0] = 1.5\n").unwrap_err();
        assert!(err.msg.contains("buffer element"), "{}", err.msg);
    }

    /// A map's two type arguments are inferred like any others — unlike a
    /// buffer's width, which is not a type argument at all — so a table needs
    /// no annotation anywhere it is written or read.
    #[test]
    fn map_types_are_inferred() {
        let t = check_std(
            "fn main() =\n  m = map_new()\n  m[\"a\"] = 1\n  println(map_or(m, \"a\", 0))\n",
        )
        .unwrap();
        let _ = t;
        // The base of a write cannot be anything else: a buffer would have
        // been annotated and a string is not storage.
        let t = check_std("fn put(m, k, v) =\n  m[k] = v\n\nfn main() =\n  m = map_new()\n  put(m, 1, \"x\")\n")
            .unwrap();
        assert!(
            matches!(sig(&t, "put").params[0].1, Ty::Map(..)),
            "{:?}",
            sig(&t, "put").params[0].1
        );
        // Reading answers with an `Option`, which is what a key that may not
        // be there is worth. An open base still settles as a string, as it
        // always did, so a map one is said out loud — by an annotation here,
        // or by anything else that has already fixed the type.
        let t = check_std(
            "fn get(m: Map(Int, Str)) = m[1]\n\nfn main() =\n  m = map_new()\n  m[1] = \"x\"\n  println(unwrap_or(get(m), \"?\"))\n",
        )
        .unwrap();
        assert!(matches!(sig(&t, "get").ret, Ty::Adt(..)), "{:?}", sig(&t, "get").ret);
        // `#m` is how many entries, not how many slots.
        check_std("fn main() =\n  m = map_new()\n  m[1] = 2\n  println(#m)\n").unwrap();
    }

    /// What may be a key is what has structural equality, and what may not
    /// says so where the key is written rather than somewhere later.
    #[test]
    fn map_keys_must_be_value_types() {
        let err = check_std(
            "fn main() =\n  m = map_new()\n  m[\\x -> x + 1] = 1\n  println(#m)\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("a map key requires"), "{}", err.msg);
        // A table is storage, so it neither prints nor compares — and the
        // error says what to ask instead.
        let err =
            check_std("fn main() =\n  m = map_new()\n  m[1] = 2\n  println(m)\n").unwrap_err();
        assert!(err.msg.contains("map_items"), "{}", err.msg);
        // `m[k] += 1` expands to `m[k] = m[k] + 1`, where the `Option` a read
        // gives back meets a number. Saying which is which is the difference
        // between a puzzle and a fix.
        let err = check_std(
            "fn main() =\n  m = map_new()\n  m[\"a\"] = 1\n  m[\"a\"] += 1\n  println(#m)\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("unwrap_or(m[k], 0)"), "{}", err.msg);
    }

    /// A guard is the rest of what a pattern cannot say, and an arm with one
    /// covers nothing: its condition may be false, and the value falls
    /// through to the arms below.
    #[test]
    fn a_guarded_arm_covers_nothing() {
        let t = check_std(
            "fn sign(n) = match n\n  x if x > 0 -> 1\n  x if x < 0 -> -1\n  _ -> 0\n\nfn main() = println(sign(3))\n",
        )
        .unwrap();
        assert_eq!(sig(&t, "sign").ret, Ty::Int);
        // Guarded arms leave the case they are guarding open.
        let err = check_std(
            "fn f(o) = match o\n  Some(n) if n > 0 -> \"pos\"\n  None -> \"none\"\n\nfn main() = println(f(None))\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("not exhaustive"), "{}", err.msg);
        // And they do not make the arms below them unreachable — but an
        // unguarded arm above still does.
        let err = check_std(
            "fn f(n) = match n\n  _ -> 0\n  x if x > 0 -> 1\n\nfn main() = println(f(1))\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("unreachable match arm"), "{}", err.msg);
        // A guard is a condition, so it is a `Bool`.
        let err = check_std(
            "fn f(n) = match n\n  x if x + 1 -> 0\n  _ -> 1\n\nfn main() = println(f(1))\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("a match guard"), "{}", err.msg);
    }

    /// A field is read by the name its constructor gave it, from a type with
    /// one constructor — the same rule a binding that takes a value apart
    /// already followed.
    #[test]
    fn fields_are_read_by_name() {
        let point = "type Point\n  P(x: Int, y: Int)\n\n";
        let t = check_src(&format!(
            "{point}fn get(p: Point) = p.x + p.y\n\nfn main() = println(get(P(1, 2)))\n"
        ))
        .unwrap();
        assert_eq!(sig(&t, "get").ret, Ty::Int);
        // A dot after something that is not a name is the same read.
        check_src(&format!(
            "{point}fn mk(n) = P(n, n)\n\nfn main() = println(mk(1).y)\n"
        ))
        .unwrap();
        // More than one constructor: which fields there are depends on which
        // one it is, so `match` is the way in and the error says so.
        let err = check_src(
            "type Shape\n  Circle(r: Float)\n  Rect(w: Float, h: Float)\n\nfn f(s: Shape) = s.r\n\nfn main() = println(1)\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("more than one constructor"), "{}", err.msg);
        // A field that is not there is named against the ones that are.
        let err = check_src(&format!("{point}fn f(p: Point) = p.z\n\nfn main() = println(1)\n"))
            .unwrap_err();
        assert!(err.msg.contains("no field `z`"), "{}", err.msg);
        // And a base whose type nothing has settled says what to do about it.
        let err = check_src(&format!("{point}fn f(p) = p.x\n\nfn main() = println(f(P(1, 2)))\n"))
            .unwrap_err();
        assert!(err.msg.contains("annotate it"), "{}", err.msg);
    }

    /// `?` is a `match` whose `Ok` arm is the rest of the block, so what it
    /// asks of the program is that the block be what the function returns.
    #[test]
    fn question_mark_carries_an_error_out() {
        let half = "fn half(n) = if n % 2 == 0 then Ok(n / 2) else Err(\"odd\")\n\n";
        let t = check_std(&format!(
            "{half}fn quarter(n) =\n  h = half(n)?\n  half(h)\n\nfn main() = println(1)\n"
        ))
        .unwrap();
        // `Result(Int, Str)` either way: the error type travels with it.
        assert_eq!(sig(&t, "quarter").ret, sig(&t, "half").ret);
        // Written where the block is not the function's value, it says so —
        // and says which value the block is worth instead.
        let err = check_std(&format!(
            "{half}fn bad(n) =\n  h = half(n)?\n  h + 1\n\nfn main() = println(1)\n"
        ))
        .unwrap_err();
        assert!(err.msg.contains("has to be what the function returns"), "{}", err.msg);
        // And on anything but a `Result`, it says what to write instead.
        let err = check_std(
            "fn first(l) =\n  x = nth(l, 0)?\n  Some(x)\n\nfn main() = println(1)\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("takes the value out of a `Result`"), "{}", err.msg);
    }

    /// Where a `Ptr` is wanted, a buffer is its first element's address —
    /// at a foreign parameter, at a builtin, and at a Rill parameter that
    /// says `Ptr`. Everywhere else a buffer is a buffer.
    #[test]
    fn a_buffer_decays_where_a_pointer_is_wanted() {
        check_src(
            "extern fn strlen(s: Ptr) -> USize\n\nfn main() =\n  b = buf_u8(4)\n  println(strlen(b))\n",
        )
        .unwrap();
        // A builtin position, and a Rill parameter annotated `Ptr`.
        check_src(
            "fn first(p: Ptr) = peek_u8(p, 0)\n\nfn main() =\n  b = buf_u8(4)\n  println(first(b) + peek_u8(b, 1))\n",
        )
        .unwrap();
        // Not a coercion in general: a buffer is a buffer anywhere else.
        let err = check_src(
            "fn main() =\n  b = buf_u8(4)\n  p = null_ptr()\n  println(is_null(if true then b else p))\n",
        )
        .unwrap_err();
        assert!(err.msg.contains("Buf(U8)") || err.msg.contains("Ptr"), "{}", err.msg);
    }

    /// `s[i:j]` takes bytes rather than one, and either end may be left out.
    #[test]
    fn slices_take_a_range_of_a_string() {
        let t = check_src("fn mid(s) = s[1:#s - 1]\n\nfn main() = println(mid(\"abc\"))\n").unwrap();
        assert_eq!(sig(&t, "mid").params[0].1, Ty::Str);
        assert_eq!(sig(&t, "mid").ret, Ty::Str);
        check_src("fn main() = println(\"ab\"[:] + \"c\"[1:])\n").unwrap();
        let err = check_src("fn main() =\n  v = buf_int(2)\n  println(v[0:1])\n").unwrap_err();
        assert!(err.msg.contains("slices a string"), "{}", err.msg);
        let err = check_src("fn main() = println(\"ab\"[0:\"x\"])\n").unwrap_err();
        assert!(err.msg.contains("the end of a slice"), "{}", err.msg);
    }

    /// `#x` measures the two things `[]` indexes, and settles an open operand
    /// as a string for the same reason.
    #[test]
    fn hash_measures_strings_and_buffers() {
        let t = check_src("fn f(s, v: Buf(I32)) = #s + #v\n\nfn main() = println(f(\"a\", buf_i32(1)))\n")
            .unwrap();
        assert_eq!(sig(&t, "f").params[0].1, Ty::Str);
        assert_eq!(sig(&t, "f").ret, Ty::Int);
        let err = check_src("fn main() = println(#1)\n").unwrap_err();
        assert!(err.msg.contains("measures a string, a buffer, a map or a builder"), "{}", err.msg);
    }

    /// Storing the wrong shape in one is a type error, not a silent
    /// reinterpretation.
    #[test]
    fn buffer_rejects_the_wrong_element() {
        let err = check_src("fn main() =\n  v = buf_i32(4)\n  buf_set(v, 0, 1.5)\n").unwrap_err();
        assert!(err.msg.contains("buffer element"), "{}", err.msg);
    }

    /// The width is never inferred, so a function taking a buffer says which.
    #[test]
    fn buffer_parameter_must_be_annotated() {
        let err = check_src("fn f(v, i) = buf_get(v, i)\n\nfn main() = println(f(buf_f32(1), 0))\n").unwrap_err();
        assert!(err.msg.contains("annotate the parameter"), "{}", err.msg);
    }

    /// The numeric types never mix on their own, so crossing between them is
    /// written out — and each way is a single instruction.
    #[test]
    fn numeric_conversions_are_explicit() {
        let t = check_src("fn main() = println(to_float(3) / 2.0)\n").unwrap();
        assert!(check_src("fn main() = println(1 + 2.0)\n").is_err());
        let _ = t;
        let u = check_src("fn half(n) = to_int(to_float(n) * 0.5)\n\nfn main() = println(half(7))\n").unwrap();
        assert_eq!(sig(&u, "half").ret, Ty::Int);
    }

    /// A callback's Rill type is an ordinary function type, so passing one
    /// type-checks like any other argument.
    #[test]
    fn callback_parameters_are_function_types() {
        let src = "extern fn signal(sig: I32, handler: Fn(I32)) -> Ptr\n\nfn beep(n) = println(n)\n\nfn main() =\n  signal(30, beep)\n  println(1)\n";
        let t = check_src(src).unwrap();
        let sig = &t.externs["signal"];
        assert_eq!(sig.params[1], CTy::Fn(vec![CTy::I32], Box::new(CTy::Unit)));
        assert_eq!(sig.params[1].rill_ty(), Ty::Fn(vec![Ty::Int], Box::new(Ty::Unit)));
    }

    /// `Fn(..) -> T` gives the callback a result, and it reaches the Rill side
    /// as an ordinary function type the argument has to match.
    #[test]
    fn callback_result_types_are_checked() {
        let src = "extern fn each(f: Fn(I32) -> I32) -> Unit\n\nfn double(n) = n * 2\n\nfn main() =\n  each(double)\n  println(1)\n";
        let t = check_src(src).unwrap();
        let sig = &t.externs["each"];
        assert_eq!(sig.params[0], CTy::Fn(vec![CTy::I32], Box::new(CTy::I32)));
        assert_eq!(sig.params[0].rill_ty(), Ty::Fn(vec![Ty::Int], Box::new(Ty::Int)));

        // A callback whose result does not match the declared one is caught
        // where any other argument mismatch is.
        let bad = "extern fn each(f: Fn(I32) -> I32) -> Unit\n\nfn shout(n) = println(n)\n\nfn main() =\n  each(shout)\n  println(1)\n";
        assert!(check_src(bad).is_err());
    }

    /// A callback that answers with another callback would need a trampoline
    /// built from a value C chose, at run time.
    #[test]
    fn callback_returning_callback_is_rejected() {
        let src = "extern fn odd(f: Fn(I32) -> Fn(I32)) -> Unit\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("cannot return `Fn`"), "{}", err.msg);
    }

    /// A callback hands nothing back, so it cannot be one's result either.
    #[test]
    fn callback_return_type_is_rejected() {
        let src = "extern fn installer() -> Fn(I32)\n\nfn main() = println(1)\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("declare it `Ptr`"), "{}", err.msg);
    }

    /// Widths are FFI-only spellings, not types the language gained.
    #[test]
    fn width_names_are_not_types_outside_an_extern() {
        let src = "fn f(x: I32) = x\n\nfn main() = println(f(1))\n";
        let err = check_src(src).unwrap_err();
        assert!(err.msg.contains("unknown type `I32`"), "{}", err.msg);
    }

    /// A local function that closes over nothing is an ordinary top-level
    /// function once it has been lifted, and generalizes like one.
    #[test]
    fn a_local_function_that_captures_nothing_is_polymorphic() {
        let src = "fn main() =\n  fn id(x) = x\n\n  println(id(1))\n  println(id(\"a\"))\n";
        check_std(src).unwrap();
    }

    /// One that closes over something is checked with the function it was
    /// written in — which is what lets a captured buffer keep its width — and
    /// so has one type. The error has to say that, not just report a mismatch.
    #[test]
    fn a_capturing_local_function_says_why_it_has_one_type() {
        let src = "fn main() =\n  k = 3\n\n  fn pair(x) = (k, x)\n\n  \
                   println(show(pair(1)))\n  println(show(pair(\"a\")))\n";
        let err = check_std(src).unwrap_err();
        assert!(err.msg.contains("uses names from `main`"), "{}", err.msg);
        assert!(err.msg.contains("captures nothing does generalize"), "{}", err.msg);
    }

    #[test]
    fn option_map() {
        let src = "type Option(a)\n  None\n  Some(a)\n\nfn omap(o, f) = match o\n  None -> None\n  Some(x) -> Some(f(x))\n\nfn unwrap_or(o, d) = match o\n  None -> d\n  Some(x) -> x\n\nfn main() =\n  r = omap(Some(20), \\x -> x + 1)\n  println(unwrap_or(r, 0))\n";
        let t = check_src(src).unwrap();
        assert_eq!(sig(&t, "omap").ngen, 2);
    }
}

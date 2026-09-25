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

mod sandbox;
mod shared;
pub use shared::{check_rings, check_shared};
pub use sandbox::{capability_of, check_sandbox, Entries, Sandbox};

use std::collections::{HashMap, HashSet};

use rill_syntax::{
    Arm, BinOp, Expr, FnDef, Module, Param, Pattern, SelectOp, Stmt, TypeRef, UnOp,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Float,
    /// Two `Float`s that travel together, in one 128-bit register: what a
    /// kernel that has been written with lanes in mind is written in. The
    /// arithmetic operators work on both lanes at once; `float2`, `splat2`,
    /// `lane0`, `lane1`, `f2_load`, `f2_store` and `fma2` are the rest of
    /// it.
    Float2,
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
    /// Where a callback's environment travels. An `extern` with an `Env`
    /// parameter has one callback whose `Fn(...)` has an `Env` too: the
    /// Rill call leaves the `Env` out, the compiler hands C a pointer that
    /// carries the closure, and C hands it back into the callback's `Env`
    /// slot — `qsort_r`'s `arg`, `pthread_create`'s, every library's "user
    /// data". That is what lets a lambda cross.
    Env,
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
            CTy::Ptr | CTy::Env => Ty::Ptr,
            CTy::Unit => Ty::Unit,
            // The `Env` slot is the compiler's, not the callback's: a Rill
            // function passed for `Fn(Ptr, Env) -> I32` takes one `Ptr`.
            CTy::Fn(ps, r) => Ty::Fn(
                ps.iter().filter(|p| !matches!(p, CTy::Env)).map(|p| p.rill_ty()).collect(),
                Box::new(r.rill_ty()),
            ),
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
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Fn(..) | CTy::Env => 8,
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
            CTy::Env => "Env",
        }
    }
}

/// A foreign function's signature, in C's own types.
#[derive(Debug, Clone)]
pub struct ExternSig {
    pub params: Vec<CTy>,
    pub ret: CTy,
    /// `extern blocking fn`: the call may wait, so it is made on a helper
    /// thread while the strand is parked, and the worker goes on.
    pub blocking: bool,
}

/// `asm fn` — a run of instructions rather than a symbol. Same shapes cross as
/// for a C call, because the question is the same one: what fits in a register
/// the way the machine expects to find it.
///
/// `constraints` is settled here rather than in codegen: written out by the
/// author, or the ordinary `=r`/`r` shape when they left it off. LLVM aborts
/// the process on a constraint string that does not match the signature, so
/// the count is checked while there is still a line number to report it on.
#[derive(Debug, Clone)]
pub struct AsmSig {
    pub params: Vec<CTy>,
    pub ret: CTy,
    pub template: String,
    pub constraints: String,
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

/// A field asked of a type that was not known when it was asked: the type,
/// the field's name, the variable standing for what was read from it (or
/// written into it), where, and how an error should call the value.
struct FieldObligation {
    base: Ty,
    name: String,
    result: Ty,
    line: usize,
    what: String,
    ctx: &'static str,
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
    /// What the checker had to say short of refusing, by line.
    pub warnings: Vec<(usize, String)>,
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
    /// Instructions the program spells out, by the name it calls them.
    pub asms: HashMap<String, AsmSig>,
    /// `naked asm fn` — whole functions, by symbol name, holding the
    /// instructions that are all there is of them. Nothing in Rill calls one:
    /// they are reached by an interrupt table, a reset vector or a linker
    /// script, and what Rill has to do is put the name where those can find it.
    pub naked_asms: HashMap<String, String>,
    /// Libraries those symbols come from, for the linker.
    pub libs: Vec<String>,
    /// How many leading entries of `fns` came from the prelude. Trait impl
    /// methods are appended after the module's own functions, so everything
    /// from here on is code the program's author wrote.
    pub prelude_fns: usize,
    /// The functions that came in through `import`, by name — the other
    /// part of what is not the author's.
    pub imported_fns: HashSet<String>,
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
    /// `fast fn`: see `FnDef::fast`.
    pub fast: bool,
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
    /// `machine_show(x)`: shown the way `show` shows, with what `show`
    /// refuses said instead of refused — a channel as `#n`, the number
    /// the trace gives it, a function as `<fn>`, storage as `<buf>` or
    /// `<map>`. What the trace says of a state's value; not for programs.
    Inspect(Box<TExpr>),
    /// `channel()` or `channel(capacity)` — a new channel of the
    /// expression's `Chan(elem)` type.
    ChanNew(Option<Box<TExpr>>),
    /// `send(ch, v)` — transfers ownership of `v` into the channel.
    ChanSend { ch: Box<TExpr>, val: Box<TExpr> },
    /// `recv(ch)` — receives an owned value.
    ChanRecv { ch: Box<TExpr> },
    /// The channel calls that take a channel and answer something which is not
    /// its element type: `close`, `closed` and `chan_wait`.
    ChanOp { ch: Box<TExpr>, op: ChanOp },
    /// `chan_taken(ch)` — the value the `chan_wait` before it took. Typed like
    /// `recv` because it hands back the same thing, and lowered like it for
    /// the same reason: two words to be unpacked into whatever the element is.
    ChanTaken { ch: Box<TExpr> },
    /// `buf_f32(n)` and friends: a counted buffer of `n` elements.
    BufNew { width: CTy, count: Box<TExpr> },
    /// Reading and writing one element, bounds-checked.
    /// `s[i:j]` — the bytes from `i` up to but not `j`, either end the
    /// string's own when it was left out.
    Slice { base: Box<TExpr>, from: Box<TExpr>, to: Option<Box<TExpr>> },
    /// `p.x` where the type of `p` was not yet known where it was written.
    /// Which field that is cannot be said until it is, so the name is carried
    /// until `zonk`, by which point inference has finished and the answer is
    /// there. Nothing past the checker ever sees one.
    FieldNamed { base: Box<TExpr>, name: String },
    /// `{ p | x = 1 }` where the type of `p` was not yet known where it was
    /// written. Which constructor it is a call to cannot be said until it
    /// is, so the base and the new values are carried until `zonk`, which
    /// builds the call. Nothing past the checker ever sees one.
    UpdateNamed { base: Box<TExpr>, fields: Vec<(String, TExpr, usize)> },
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
    /// `unchecked`: the index was proved in range by a `for.check` in front
    /// of the loop, so no bounds test is generated.
    BufGet { buf: Box<TExpr>, idx: Box<TExpr>, unchecked: bool },
    BufSet { buf: Box<TExpr>, idx: Box<TExpr>, val: Box<TExpr>, unchecked: bool },
    BufLen { buf: Box<TExpr> },
    /// `unique(x)`: whether this frame holds the only reference to `x`.
    Unique { val: Box<TExpr> },
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

/// Which of the three a `TK::ChanOp` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChanOp {
    /// `close(ch)` — nothing more may be sent, and everything parked wakes.
    Close,
    /// `closed(ch)` — whether it has been.
    Closed,
    /// `chan_wait(ch)` — receive into the strand's slot, false once the
    /// channel is closed and drained.
    Wait,
    /// `chan_id(ch)` — a small number naming the channel, for a trace.
    Id,
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
    /// `timeout(ms) ->` — ready when nothing else has been for that long.
    /// There is no channel, so this arm queues on nothing and is answered by
    /// the poller instead.
    Timeout { ms: TExpr },
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
    /// A type with an implementation of this trait: what a `where` says.
    Impl(usize),
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
        Ok(c) => {
            let t = c.run(module)?;
            check_rings(&t)?;
            check_systems(module, &t)?;
            Ok(t)
        }
        Err(e) => Err(vec![e]),
    }
}

/// A `system` against its machines: every route names machines of the
/// file or `env` and signals that exist; every signal a machine sends —
/// `output`, `reply` — is on some route out of it; every signal a machine
/// answers is on some route into it. A signal is named by its constructor,
/// so this is a check of the block diagram against the code, as SDL's tools
/// made it, and it says which signal and which machine.
pub fn check_systems(module: &Module, t: &TModule) -> Result<(), Vec<TypeError>> {
    let mut errors = Vec::new();
    for sd in &module.systems {
        // the file's machines, and the imports' — a machine of a library
        // is routed to by name, or `alias.name` when the import has one
        let machine_names: Vec<&str> = module.machines.iter().chain(module.imported_machines.iter()).map(|m| m.name.as_str()).collect();
        // a signal list names signals, each once, and no list
        for l in &sd.lists {
            if sd.lists.iter().filter(|o| o.name == l.name).count() > 1 {
                errors.push(TypeError::new(l.line, format!("system `{}`: two signal lists named `{}`", sd.name, l.name)));
            }
            for s in &l.signals {
                if sd.lists.iter().any(|o| o.name == *s) {
                    errors.push(TypeError::new(l.line, format!("system `{}`: a signal list names signals, not lists: `{s}`", sd.name)));
                } else if !t.ctors.contains_key(s) {
                    errors.push(TypeError::new(l.line, format!("system `{}`: there is no signal `{s}`", sd.name)));
                }
            }
        }
        // a route is a route of the system wherever it is written
        let routes = sd.all_routes();
        for r in &routes {
            for end in [&r.from, &r.to] {
                if end != "env" && !machine_names.contains(&end.as_str()) {
                    errors.push(TypeError::new(r.line, format!("system `{}`: `{end}` is not a machine of this file or its imports, nor `env`", sd.name)));
                }
            }
            for s in &r.signals {
                if !t.ctors.contains_key(s) && !sd.lists.iter().any(|l| l.name == *s) {
                    errors.push(TypeError::new(r.line, format!("system `{}`: there is no signal `{s}`", sd.name)));
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        // what may leave and enter each machine, by the routes, their lists opened up
        let out_of = |m: &str| -> Vec<String> {
            let mut v = Vec::new();
            for r in &routes {
                if r.from == m || (r.both && r.to == m) {
                    v.extend(sd.carried(r));
                }
            }
            v
        };
        let into = |m: &str| -> Vec<String> {
            let mut v = Vec::new();
            for r in &routes {
                if r.to == m || (r.both && r.from == m) {
                    v.extend(sd.carried(r));
                }
            }
            v
        };
        for m in &module.machines {
            if !routes.iter().any(|r| r.from == m.name || r.to == m.name) {
                continue;
            }
            let leaving = out_of(&m.name);
            let mut entering = into(&m.name);
            // a timer's signal comes from the machine itself, on no route
            let states = m.all_states();
            for st in &states {
                for a in &st.arms {
                    collect_timers(&a.body, &mut entering);
                }
                if let Some((_, body, _)) = &st.after {
                    collect_timers(body, &mut entering);
                }
                for (_, body, _) in &st.whens {
                    collect_timers(body, &mut entering);
                }
            }
            for st in &states {
                let mut sent: Vec<(String, usize)> = Vec::new();
                for a in &st.arms {
                    collect_sent(&a.body, a.line, &mut sent);
                    let mut named = Vec::new();
                    ctor_names(&a.pattern, &mut named);
                    for n in named {
                        if !entering.contains(&n) {
                            errors.push(TypeError::new(a.line, format!("system `{}`: `{}` answers `{n}`, which no route into `{}` carries", sd.name, m.name, m.name)));
                        }
                    }
                }
                if let Some((_, body, l)) = &st.after {
                    collect_sent(body, *l, &mut sent);
                }
                for (_, body, l) in &st.whens {
                    collect_sent(body, *l, &mut sent);
                }
                for (sig, line) in sent {
                    if !leaving.contains(&sig) {
                        errors.push(TypeError::new(line, format!("system `{}`: `{}` sends `{sig}`, which no route out of `{}` carries", sd.name, m.name, m.name)));
                    }
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The signals a body sets as timers, by constructor.
fn collect_timers(e: &Expr, out: &mut Vec<String>) {
    if let Expr::Call { name, args, .. } = e {
        if name == "set" && args.len() == 2 {
            let ctor = match &args[0] {
                Expr::Var(n, _) => Some(n.clone()),
                Expr::Call { name, .. } => Some(name.clone()),
                _ => None,
            };
            if let Some(c) = ctor {
                if c.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) && !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    rill_syntax::lift::each_child_all(e, &mut |c| collect_timers(c, out));
}

/// The signals a body sends with `output` or `reply`, by constructor.
fn collect_sent(e: &Expr, line: usize, out: &mut Vec<(String, usize)>) {
    if let Expr::Call { name, args, line: l, .. } = e {
        let sig = if name == "output" && args.len() == 2 { Some(&args[1]) } else if name == "reply" && args.len() == 1 { Some(&args[0]) } else { None };
        if let Some(sig) = sig {
            let ctor = match sig {
                Expr::Var(n, _) => Some(n.clone()),
                Expr::Call { name, .. } => Some(name.clone()),
                _ => None,
            };
            if let Some(c) = ctor {
                if c.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
                    out.push((c, if *l == 0 { line } else { *l }));
                }
            }
        }
    }
    rill_syntax::lift::each_child_all(e, &mut |c| collect_sent(c, line, out));
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
    /// `describe(x: a) -> Str = ...`: the parameter names and the body an
    /// impl gets where it leaves the method out.
    default: Option<(Vec<String>, Expr)>,
}

struct Checker {
    adts: Vec<AdtDef>,
    ctors: HashMap<String, (usize, usize)>,
    /// The state of a `machine` whose arms are being inferred — the
    /// machine's name and the state's pattern — so the match on its
    /// signal can say which state drops what.
    machine_state: Option<(String, String)>,
    /// What the compiler has to say short of refusing: a machine's state
    /// that drops signals silently. Lines and text, for the CLI to print.
    pub warnings: Vec<(usize, String)>,
    /// Each state of each machine as its signal match was checked: the
    /// machine, the line, the state, the signals its arms answer, and
    /// every signal of the type — for the report made once all are in.
    machine_states: Vec<(String, usize, String, Vec<String>, Vec<String>, bool)>,
    fn_index: HashMap<String, usize>,
    sigs: Vec<SigState>,
    subst: Vec<Option<Ty>>,
    /// Numbers the bindings `{ p | x = 9 }` needs for the value it is updating.
    /// A dot keeps them out of reach of anything a program could name. A
    /// cell, since `zonk` expands the deferred ones through `&self`.
    updates: std::cell::Cell<usize>,
    /// `p.x` read, or `{ p | x = 1 }` written, before the type of `p` was
    /// known. They are settled once the whole dependency group is inferred,
    /// which is the first moment the answer can exist.
    field_obligations: Vec<FieldObligation>,
    /// Problems found in `zonk`, which walks bodies after every signature is
    /// settled and has no way to return.
    late_errors: std::cell::RefCell<Vec<TypeError>>,
    pending: Vec<(u32, Constraint, usize, &'static str)>,
    traits: Vec<(String, Vec<MethodInfo>)>,
    /// Trait name -> the type of its objects: a record of the trait's
    /// methods, each a closure over one value. Only the traits whose every
    /// method takes the trait's type once, and first, are here; the others
    /// say why in `object_refusals`.
    trait_objects: HashMap<String, usize>,
    object_refusals: HashMap<String, String>,
    /// For each trait, the traits a type must implement as well.
    trait_supers: Vec<Vec<usize>>,
    externs: HashMap<String, ExternSig>,
    asms: HashMap<String, AsmSig>,
    naked_asms: HashMap<String, String>,
    libs: Vec<String>,
    /// Method name -> (trait index, method index): the qualified names,
    /// `Describe.describe`, and the bare ones only one trait uses. A bare
    /// name two traits share is in `method_names` instead, and a call by
    /// it is settled by the type it is called on.
    method_index: HashMap<String, (usize, usize)>,
    /// Every bare method name, with each trait that has it.
    method_names: HashMap<String, Vec<(usize, usize)>>,
    impl_methods: HashMap<(usize, usize, ImplKey), usize>,
    /// Impl functions get their signature from the trait, not inference:
    /// parameter types, return type, and how many type parameters the target
    /// type constructor has (zero for a plain type).
    forced_sigs: HashMap<String, (Vec<Ty>, Ty, usize)>,
    /// A function annotated on every parameter and its return, with type
    /// variables among them: the scheme the annotation declares, by index.
    /// A use from inside its own dependency group instantiates this rather
    /// than sharing the provisional signature, which is what lets a function
    /// call itself, or a group-mate, at another type. The body is held to
    /// the declaration once the group is generalized.
    declared: HashMap<usize, (Vec<Ty>, Ty, usize)>,
    /// What a function's `where` requires of its type variables, by index:
    /// the inference variable standing for each, the trait, and the line.
    /// Asked of the body's group when the body is inferred, so that it
    /// travels with the scheme like a requirement the body implied.
    bounds: HashMap<usize, Vec<(Ty, Constraint, usize)>>,
    /// All functions to check: user functions plus mangled impl methods.
    expanded_fns: Vec<FnDef>,
    /// For each function index, the name it was written inside if it was
    /// lifted out of one. Only used to say something useful when a local
    /// function is asked to be two types at once — see below.
    lifted: Vec<Option<String>>,
    /// Carried through to `TModule`; see the field there.
    prelude_fns: usize,
    /// The function whose body is being inferred.
    cur_fn_name: String,
    /// What the body being inferred is to answer, innermost last: the
    /// function's declared or provisional return type, or a fresh variable
    /// for a lambda's. A `?` looks here for the error type it carries out
    /// to, and wraps what it met in the one constructor that takes it.
    expected_ret: Vec<Ty>,
    /// `(loop function, buffer)` pairs a `for.check` has vouched for: the
    /// buffer was known to be one when the check was typed, so `for.at` and
    /// `for.set` inside that loop may skip their bounds tests. Typed the
    /// other way round — loop first — they stay checked, which is safe.
    hoisted: HashSet<(String, String)>,
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
            machine_state: None,
            warnings: Vec::new(),
            machine_states: Vec::new(),
            fn_index: HashMap::new(),
            updates: std::cell::Cell::new(0),
            field_obligations: Vec::new(),
            late_errors: std::cell::RefCell::new(Vec::new()),
            sigs: Vec::new(),
            subst: Vec::new(),
            pending: Vec::new(),
            traits: Vec::new(),
            externs: HashMap::new(),
            asms: HashMap::new(),
            naked_asms: HashMap::new(),
            libs: Vec::new(),
            method_index: HashMap::new(),
            method_names: HashMap::new(),
            impl_methods: HashMap::new(),
            forced_sigs: HashMap::new(),
            declared: HashMap::new(),
            bounds: HashMap::new(),
            expanded_fns: Vec::new(),
            lifted: Vec::new(),
            prelude_fns: 0,
            cur_fn_name: String::new(),
            trait_objects: HashMap::new(),
            object_refusals: HashMap::new(),
            trait_supers: Vec::new(),
            expected_ret: Vec::new(),
            hoisted: HashSet::new(),
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

        // A type that holds itself at a bigger type — `Nest(a)` holding a
        // `Nest((a, a))` — nests without end, and a compiler that makes one
        // layout per type cannot make infinitely many. Refused here, where
        // it is written, rather than in the code generator that would
        // otherwise walk it forever.
        for i in 0..c.adts.len() {
            c.regular(i)?;
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
                default: None,
            }],
        ));
        c.traits.push((
            "Ord".to_string(),
            vec![MethodInfo {
                name: "compare".to_string(),
                params: vec![Ty::Gen(0), Ty::Gen(0)],
                ret: Ty::Int,
                default: None,
            }],
        ));
        // `compare` is callable directly; `show` stays the built-in function
        // and consults the impl table when the concrete type is known.
        c.method_index.insert("compare".to_string(), (ORD_TRAIT, 0));

        // Traits: method signatures with Gen(0) as the trait parameter.
        let mut supers_named: Vec<(usize, Vec<String>, usize)> = Vec::new();
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
                if methods.iter().any(|x: &MethodInfo| x.name == m.name) {
                    return Err(TypeError::new(
                        m.line,
                        format!("method `{}` is already in trait `{}`", m.name, td.name),
                    ));
                }
                let mut params = Vec::new();
                for tr in &m.params {
                    params.push(resolve_type_ref(tr, &adt_index, &c.adts, Some(&scope))?);
                }
                let ret = resolve_type_ref(&m.ret, &adt_index, &c.adts, Some(&scope))?;
                c.add_method(&td.name, &m.name, (tid, methods.len()));
                let default =
                    m.default.clone().map(|b| (m.names.iter().flatten().cloned().collect(), b));
                methods.push(MethodInfo { name: m.name.clone(), params, ret, default });
            }
            supers_named.push((tid, td.supers.clone(), td.line));
            // The type of the trait's objects: a record with a field per
            // method, holding the method as a function of everything but
            // the value, which the object has closed over. `infer_call`
            // makes one from `Speak(x)`; the record's own constructor is
            // kept under a name a program cannot write.
            match object_fields(&methods) {
                Ok(fields) => {
                    if adt_index.contains_key(&td.name) {
                        return Err(TypeError::new(
                            td.line,
                            format!(
                                "trait `{}` shares its name with a type, and a trait's name is \
                                 the type of its objects",
                                td.name
                            ),
                        ));
                    }
                    let adt = c.adts.len();
                    adt_index.insert(td.name.clone(), adt);
                    c.adts.push(AdtDef {
                        name: td.name.clone(),
                        nparams: 0,
                        ctors: vec![CtorSig {
                            name: td.name.clone(),
                            fields,
                            names: methods.iter().map(|m| Some(m.name.clone())).collect(),
                            tag: 0,
                            line: td.line,
                        }],
                    });
                    c.ctors.insert(format!("{}.obj", td.name), (adt, 0));
                    c.trait_objects.insert(td.name.clone(), adt);
                }
                Err(why) => {
                    c.object_refusals.insert(td.name.clone(), why);
                }
            }
            c.traits.push((td.name.clone(), methods));
        }

        // What each trait requires of a type as well, resolved once every
        // trait has a number.
        c.trait_supers = vec![Vec::new(); c.traits.len()];
        for (tid, names, line) in supers_named {
            for n in names {
                let Some(s) = c.traits.iter().position(|(t, _)| *t == n) else {
                    return Err(TypeError::new(line, format!("unknown trait `{n}`")));
                };
                if s == tid {
                    return Err(TypeError::new(line, format!("trait `{n}` cannot require itself")));
                }
                c.trait_supers[tid].push(s);
            }
        }

        // Impls: each method becomes a mangled top-level function whose
        // signature is forced by the trait (v1 targets: builtins and
        // non-generic ADTs).
        let mut impl_targets: Vec<(usize, Ty, String, usize)> = Vec::new();
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
            impl_targets.push((tid, target_ty.clone(), im.target.name.clone(), im.line));
            // A method the impl leaves out is the trait's default, written
            // here as if the impl had written it — and checked at this
            // impl's type, so a default that calls another method of the
            // trait dispatches to this impl's.
            let mut fns: Vec<FnDef> = im.fns.clone();
            for m in &trait_methods {
                if fns.iter().all(|f| f.name != m.name) {
                    if let Some((names, body)) = &m.default {
                        fns.push(FnDef {
                            name: m.name.clone(),
                            exported: false,
                            params: names.iter().map(|n| Param { name: n.clone(), ann: None }).collect(),
                            ret_ann: None,
                            body: body.clone(),
                            line: im.line,
                            lifted_from: None,
                            wheres: Vec::new(),
                            private: false,
                            fast: false,
                        });
                    }
                }
            }
            let mut implemented = vec![false; trait_methods.len()];
            for f in &fns {
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
                if f.params.iter().any(|p| p.ann.is_some()) || f.ret_ann.is_some() || !f.wheres.is_empty() {
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

        // A trait that requires others: every impl of it has the others too.
        for (tid, target_ty, target_name, line) in impl_targets {
            for &s in &c.trait_supers[tid] {
                let met = match s {
                    ORD_TRAIT => c.check_need(&target_ty, Constraint::Ordered, line, "").is_ok(),
                    SHOW_TRAIT => c.check_need(&target_ty, Constraint::Basic, line, "").is_ok(),
                    s => c.has_impl(s, &target_ty),
                };
                if !met {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "impl of `{}` for `{}` needs `{}` implemented for `{}` as well: `{}` \
                             requires it",
                            c.traits[tid].0, target_name, c.traits[s].0, target_name, c.traits[tid].0
                        ),
                    ));
                }
            }
        }

        // Foreign declarations. Their types are written out, never inferred,
        // and only shapes the C ABI can carry are allowed.
        for e in &module.externs {
            if is_builtin_fn(&e.name) || c.is_method(&e.name) {
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
            // `Env` comes in pairs: the parameter the pointer goes out by,
            // and the slot of the one callback it comes back into.
            let envs = params.iter().filter(|p| matches!(p, CTy::Env)).count();
            let carried = params
                .iter()
                .filter(|p| matches!(p, CTy::Fn(ps, _) if ps.iter().any(|q| matches!(q, CTy::Env))))
                .count();
            if envs > 1 || carried > 1 || envs != carried {
                return Err(TypeError::new(
                    e.line,
                    format!(
                        "`{}`: `Env` names where a callback's environment travels — an `extern` \
                         with an `Env` parameter has exactly one callback whose `Fn(...)` has an \
                         `Env` slot, and the other way round; this one has {envs} `Env` \
                         parameter(s) and {carried} such callback(s)",
                        e.name
                    ),
                ));
            }
            let ret = match &e.ret {
                Some(tr) => {
                    let r = resolve_c_type(tr, &adt_index, &c.adts, "return type")?;
                    if matches!(r, CTy::Env) {
                        return Err(TypeError::new(tr.line, "return type: `Env` is a parameter's to carry".to_string()));
                    }
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
            if e.blocking && params.iter().any(|p| matches!(p, CTy::Fn(..) | CTy::Env)) {
                return Err(TypeError::new(
                    e.line,
                    format!(
                        "`{}`: a blocking call is made on a helper thread, where a callback \
                         could not run Rill code — a call that calls back is not `blocking`",
                        e.name
                    ),
                ));
            }
            c.externs.insert(e.name.clone(), ExternSig { params, ret, blocking: e.blocking });
            if let Some(lib) = &e.lib {
                if !c.libs.contains(lib) {
                    c.libs.push(lib.clone());
                }
            }
        }

        for a in &module.asms {
            if is_builtin_fn(&a.name) || c.is_method(&a.name) {
                return Err(TypeError::new(
                    a.line,
                    format!("`{}` is already a built-in or a trait method", a.name),
                ));
            }
            if c.externs.contains_key(&a.name) {
                return Err(TypeError::new(
                    a.line,
                    format!("`{}` is already declared as a foreign function", a.name),
                ));
            }
            if c.naked_asms.contains_key(&a.name) {
                return Err(TypeError::new(
                    a.line,
                    format!("`{}` is already declared as a naked function", a.name),
                ));
            }
            if a.naked {
                // A naked function is entered without a prologue, so there is
                // no place an argument could have been left and nothing to
                // return into. Saying otherwise would compile and then be
                // wrong at the one moment it runs.
                if !a.params.is_empty() {
                    return Err(TypeError::new(
                        a.line,
                        format!(
                            "`{}`: a naked function takes no parameters — it is entered \
                             without a prologue, so there is nowhere for one to arrive",
                            a.name
                        ),
                    ));
                }
                if a.ret.is_some() {
                    return Err(TypeError::new(
                        a.line,
                        format!(
                            "`{}`: a naked function answers with nothing — it returns the \
                             way its instructions say to, if it returns at all",
                            a.name
                        ),
                    ));
                }
                if a.constraints.is_some() {
                    return Err(TypeError::new(
                        a.line,
                        format!(
                            "`{}`: a naked function has no operands to constrain; the \
                             instructions name the registers they use",
                            a.name
                        ),
                    ));
                }
                c.naked_asms.insert(a.name.clone(), a.template.clone());
                continue;
            }
            let mut params = Vec::new();
            for (pname, tr) in &a.params {
                let what = format!("parameter `{pname}`");
                let cty = resolve_c_type(tr, &adt_index, &c.adts, &what)?;
                if cty == CTy::Unit {
                    return Err(TypeError::new(
                        tr.line,
                        format!("{what}: `Unit` is not a value an instruction can be given"),
                    ));
                }
                if matches!(cty, CTy::Fn(..)) {
                    return Err(TypeError::new(
                        tr.line,
                        format!(
                            "{what}: a callback has nowhere to go in an instruction; \
                             declare it `Ptr` if the address is what is wanted"
                        ),
                    ));
                }
                params.push(cty);
            }
            let ret = match &a.ret {
                Some(tr) => {
                    let r = resolve_c_type(tr, &adt_index, &c.adts, "return type")?;
                    if matches!(r, CTy::Fn(..)) {
                        return Err(TypeError::new(
                            tr.line,
                            "return type: an instruction cannot answer with a callback; \
                             declare it `Ptr`"
                                .to_string(),
                        ));
                    }
                    r
                }
                None => CTy::Unit,
            };
            let outs = usize::from(ret != CTy::Unit);
            let constraints = match &a.constraints {
                Some(text) => {
                    check_constraints(text, outs, params.len(), a.line, &a.name)?;
                    text.clone()
                }
                None => {
                    let mut parts: Vec<&str> = Vec::with_capacity(outs + params.len());
                    if outs == 1 {
                        parts.push("=r");
                    }
                    parts.extend(core::iter::repeat("r").take(params.len()));
                    parts.join(",")
                }
            };
            c.asms.insert(
                a.name.clone(),
                AsmSig { params, ret, template: a.template.clone(), constraints },
            );
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
            if c.asms.contains_key(&f.name) {
                return Err(TypeError::new(
                    f.line,
                    format!("`{}` is already declared as an instruction", f.name),
                ));
            }
            if c.naked_asms.contains_key(&f.name) {
                return Err(TypeError::new(
                    f.line,
                    format!("`{}` is already declared as a naked function", f.name),
                ));
            }
            if c.is_method(&f.name) {
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
            // A lowercase name in an annotation is a type variable, and the
            // same name is the same type throughout the signature.
            let mut vars: Vec<String> = Vec::new();
            for Param { ann, .. } in &f.params {
                if let Some(tr) = ann {
                    collect_type_vars(tr, &mut vars);
                }
            }
            if let Some(tr) = &f.ret_ann {
                collect_type_vars(tr, &mut vars);
            }
            let names: HashMap<&str, u32> =
                vars.iter().enumerate().map(|(i, n)| (n.as_str(), i as u32)).collect();
            let mut params = Vec::new();
            let mut whole = f.ret_ann.is_some();
            for Param { ann, .. } in &f.params {
                let ty = match ann {
                    Some(tr) => {
                        // A `Unit` parameter is a byte nobody reads, as an
                        // inferred one always was.
                        resolve_type_ref(tr, &adt_index, &c.adts, Some(&names))?
                    }
                    None => {
                        whole = false;
                        c.fresh()
                    }
                };
                params.push(ty);
            }
            let ret = match &f.ret_ann {
                Some(tr) => resolve_type_ref(tr, &adt_index, &c.adts, Some(&names))?,
                None => c.fresh(),
            };
            let idx = c.sigs.len();
            // A whole signature with variables in it is a scheme declared up
            // front, and a use inside the function's own group may take it
            // at any type. The body is still checked against inference
            // variables standing for the declared ones, and held to the
            // declaration at the end.
            if whole && !vars.is_empty() {
                c.declared.insert(idx, (params.clone(), ret.clone(), vars.len()));
            }
            let inst: Vec<Ty> = (0..vars.len()).map(|_| c.fresh()).collect();
            // `where Describe(a)`: `a` has to be one of the signature's
            // variables, and the trait one there is.
            let mut required = Vec::new();
            for b in &f.wheres {
                let Some(&i) = names.get(b.var.as_str()) else {
                    return Err(TypeError::new(
                        b.line,
                        format!(
                            "`where {}({})`: `{}` is not a type variable of this signature",
                            b.trait_name, b.var, b.var
                        ),
                    ));
                };
                let Some(tid) = c.traits.iter().position(|(n, _)| *n == b.trait_name) else {
                    return Err(TypeError::new(b.line, format!("unknown trait `{}`", b.trait_name)));
                };
                // `Ord` and `Show` are the built-in notions, which `Int` and
                // `Str` meet without an impl.
                let need = match tid {
                    ORD_TRAIT => Constraint::Ordered,
                    SHOW_TRAIT => Constraint::Basic,
                    t => Constraint::Impl(t),
                };
                required.push((inst[i as usize].clone(), need, b.line));
            }
            if !required.is_empty() {
                c.bounds.insert(idx, required);
            }
            let params = params.iter().map(|t| subst_gen(t, &inst)).collect();
            let ret = subst_gen(&ret, &inst);
            c.fn_index.insert(f.name.clone(), idx);
            c.sigs.push(SigState::Provisional { params, ret });
        }
        // A program has a `main`; a library says `module name` at its head
        // and has none. A file with neither is asked which it is.
        if !c.fn_index.contains_key("main") && module.name.is_none() {
            return Err(TypeError::new(
                0,
                "no `main` function defined — a program has one, and a library says `module <name>` \
                 on its first line",
            ));
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

        // A field whose type inference never learned. This is the annotation
        // the old rule demanded up front, asked for now only where it is
        // genuinely needed.
        let late = std::mem::take(&mut *self.late_errors.borrow_mut());
        if !late.is_empty() {
            let mut late = late;
            late.sort_by_key(|e| e.line);
            late.dedup_by(|a, b| a.line == b.line && a.msg == b.msg);
            return Err(late);
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

        // A function that declared its signature was used at that scheme
        // inside its own group, so its body has to have earned it: the
        // scheme inference found must be the one written. Where the body
        // pinned a variable, the one written was a promise the function
        // does not keep.
        let mut declared: Vec<_> = self.declared.iter().collect();
        declared.sort_by_key(|(idx, _)| **idx);
        for (&idx, (params, ret, _)) in declared {
            let f = &tfns[idx];
            let same = f.params.iter().zip(params).all(|((_, a), b)| a == b) && f.ret == *ret;
            if !same {
                let written: Vec<String> = params.iter().map(|t| self.type_name(t)).collect();
                let found: Vec<String> = f.params.iter().map(|(_, t)| self.type_name(t)).collect();
                return Err(vec![TypeError::new(
                    f.line,
                    format!(
                        "`{}` is declared `({}) -> {}`, but its body only works as `({}) -> {}`",
                        f.name,
                        written.join(", "),
                        self.type_name(ret),
                        found.join(", "),
                        self.type_name(&f.ret)
                    ),
                )]);
            }
        }

        self.report_machines();
        Ok(TModule {
            warnings: self.warnings,
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
            asms: self.asms,
            naked_asms: self.naked_asms,
            libs: self.libs,
            prelude_fns: self.prelude_fns,
            imported_fns: module.imported_fns.iter().cloned().collect(),
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
            Ty::Float2 => "Float2".into(),
            Ty::Bool => "Bool".into(),
            Ty::Str => "Str".into(),
            Ty::Unit => "Unit".into(),
            Ty::Ptr => "Ptr".into(),
            Ty::Adt(i, args) => {
                let name = &self.adts[i].name;
                let args: Vec<String> = args.iter().map(|a| self.type_name(a)).collect();
                // A tuple is written `(a, b)`, not by the prelude's name for it.
                let tuple = name.strip_prefix("Tuple").is_some_and(|n| n.parse::<usize>().is_ok());
                if tuple {
                    format!("({})", args.join(", "))
                } else if args.is_empty() {
                    name.clone()
                } else {
                    format!("{name}({})", args.join(", "))
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
            Constraint::Numeric => matches!(ty, Ty::Int | Ty::Float | Ty::Float2),
            Constraint::Addable => matches!(ty, Ty::Int | Ty::Float | Ty::Float2 | Ty::Str),
            Constraint::Ordered => {
                matches!(ty, Ty::Int | Ty::Float | Ty::Str) || self.has_impl(ORD_TRAIT, ty)
            }
            Constraint::Basic => {
                matches!(ty, Ty::Int | Ty::Float | Ty::Float2 | Ty::Bool | Ty::Str | Ty::Adt(..))
            }
            Constraint::Impl(t) => self.has_impl(t, ty),
        };
        if ok {
            Ok(())
        } else {
            let allowed = match need {
                Constraint::Numeric => "Int or Float".to_string(),
                Constraint::Addable => "Int, Float or Str".to_string(),
                Constraint::Ordered => {
                    "Int, Float, Str or a type with an `Ord` implementation".to_string()
                }
                Constraint::Basic => "a value type (not a function, channel or storage)".to_string(),
                Constraint::Impl(t) => format!("a type with a `{}` implementation", self.traits[t].0),
            };
            // Storage is the interesting refusal: a map or a buffer is a
            // thing that changes, so printing or comparing one is a question
            // about a moment rather than about a value. Both can be asked as
            // one, and the answer says how.
            let hint = match ty {
                Ty::Map(..) => " — `map_items(m)` is a map as a value",
                Ty::Buf(_) => " — a buffer is storage; measure it or read an element",
                Ty::Unit => " — `()` is nothing, and there is nothing of it to use",
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

    /// Refuses a type whose definition reaches itself at a growing
    /// instantiation. The field types are expanded through every type they
    /// name, starting from the type at its own parameters and keeping the
    /// instantiations seen; a regular type runs out of new ones, and one
    /// bigger than anything a program writes has been grown.
    fn regular(&self, adt: usize) -> Result<(), TypeError> {
        let identity: Vec<Ty> = (0..self.adts[adt].nparams as u32).map(Ty::Gen).collect();
        let mut seen: HashSet<Ty> = HashSet::new();
        let mut work = vec![Ty::Adt(adt, identity.clone())];
        // The first time the type is met at other than its own parameters,
        // which is the thing to show; the cap catches growth by other roads.
        let mut witness: Option<Ty> = None;
        while let Some(t) = work.pop() {
            if !seen.insert(t.clone()) {
                continue;
            }
            let Ty::Adt(i, args) = &t else { unreachable!("only types are queued") };
            for ctor in &self.adts[*i].ctors {
                for field in &ctor.fields {
                    let mut found = Vec::new();
                    adts_in(&subst_gen(field, args), &mut found);
                    for u in found {
                        if witness.is_none() && matches!(&u, Ty::Adt(j, a) if *j == adt && *a != identity) {
                            witness = Some(u.clone());
                        }
                        if ty_size(&u) > 64 {
                            let shown = witness.clone().unwrap_or(u);
                            let line = self.adts[adt].ctors.first().map(|c| c.line).unwrap_or(0);
                            return Err(TypeError::new(
                                line,
                                format!(
                                    "`{}` holds a `{}` in its own definition, so the type nests \
                                     without end: a value of it would need a layout for every \
                                     type it grows to, and one is made per type. Keep the \
                                     growing part in a value — a list of them, say",
                                    self.adts[adt].name,
                                    self.type_name(&shown)
                                ),
                            ));
                        }
                        work.push(u);
                    }
                }
            }
        }
        Ok(())
    }

    /// Records a method under its qualified name, and under its bare one
    /// while that is one trait's alone; a second trait with the same bare
    /// name takes it out of the direct table, and calls by it are settled
    /// by the receiver from then on.
    fn add_method(&mut self, trait_name: &str, method: &str, at: (usize, usize)) {
        self.method_index.insert(format!("{trait_name}.{method}"), at);
        let bare = self.method_names.entry(method.to_string()).or_default();
        bare.push(at);
        if bare.len() == 1 {
            self.method_index.insert(method.to_string(), at);
        } else {
            self.method_index.remove(method);
        }
    }

    /// Whether a name is a method's, qualified or bare, of any trait.
    fn is_method(&self, name: &str) -> bool {
        self.method_index.contains_key(name) || self.method_names.contains_key(name)
    }

    /// Which of several traits' methods a bare name means here: the one
    /// whose trait is implemented for the type the receiver has. Where the
    /// receiver's type is not known yet, or more than one fits, the call
    /// has to say — `Describe.describe(x)`.
    fn pick_method(
        &self,
        name: &str,
        fitting: &[(usize, usize)],
        targs: &[TExpr],
        line: usize,
    ) -> Result<(usize, usize), TypeError> {
        let traits: Vec<String> = fitting.iter().map(|&(t, _)| format!("`{}`", self.traits[t].0)).collect();
        let named = traits.join(" and ");
        let say: Vec<String> =
            fitting.iter().map(|&(t, _)| format!("`{}.{name}(..)`", self.traits[t].0)).collect();
        let say = say.join(" or ");
        let mut by_impl = Vec::new();
        let mut recv_ty = None;
        let mut unknown = false;
        for &(t, m) in fitting {
            let sig = &self.traits[t].1[m];
            let Some(pos) = sig.params.iter().position(|p| *p == Ty::Gen(0)) else { continue };
            match self.resolve(&targs[pos].ty) {
                Ty::Var(_) => unknown = true,
                ty => {
                    if impl_key(&ty).is_some_and(|k| self.impl_methods.contains_key(&(t, m, k))) {
                        by_impl.push((t, m));
                    }
                    recv_ty = Some(ty);
                }
            }
        }
        match by_impl.as_slice() {
            [one] => Ok(*one),
            [] if unknown => Err(TypeError::new(
                line,
                format!(
                    "`{name}` is a method of {named}, and the type it is called on is not known \
                     here, so it cannot say which — write {say}"
                ),
            )),
            [] => Err(TypeError::new(
                line,
                format!(
                    "`{name}` is a method of {named}, and neither is implemented for `{}`",
                    recv_ty.map(|t| self.type_name(&t)).unwrap_or_default()
                ),
            )),
            _ => Err(TypeError::new(
                line,
                format!(
                    "`{name}` is a method of {named}, both implemented for `{}` — write {say}",
                    recv_ty.map(|t| self.type_name(&t)).unwrap_or_default()
                ),
            )),
        }
    }

    /// "unknown function" — unless a file has it as a `private fn`, which
    /// is worth saying, since the name is right and the file is keeping it.
    fn unknown_name(&self, name: &str, what: &str) -> String {
        // `m.helper` is `helper` asked of the file imported as `m`.
        let base = name.rsplit('.').next().unwrap_or(name);
        let suffix = format!("..{base}");
        match self.fn_index.keys().find(|k| k.ends_with(&suffix)) {
            Some(k) => {
                let file = &k[..k.len() - suffix.len()];
                let file = file.rsplit('.').next().unwrap_or(file);
                format!(
                    "`{base}` is private to `{file}.rill`: a function that file keeps to itself. \
                     Call one the file offers, or drop the `private`"
                )
            }
            None => format!("unknown {what} `{name}`"),
        }
    }

    /// The signature a use of a function sees. Once generalized it is
    /// instantiated afresh. While its group is still being inferred it is
    /// the provisional one, shared with every other use — unless the
    /// function declared its whole signature, in which case the declared
    /// scheme is instantiated instead, and the function can be used at
    /// another type inside its own group: polymorphic recursion, at the
    /// price of writing the signature down. A local function that captures
    /// is tied to its parent and keeps the shared one.
    fn sig_for_use(&mut self, idx: usize) -> (Vec<Ty>, Ty, Vec<Ty>) {
        match self.sigs[idx].clone() {
            SigState::Generalized { params, ret, ngen } => self.instantiate(&params, &ret, ngen),
            SigState::Provisional { params, ret } => {
                let captures = self.lifted.get(idx).is_some_and(|l| l.is_some());
                match self.declared.get(&idx).cloned() {
                    Some((p, r, n)) if !captures => self.instantiate(&p, &r, n),
                    _ => (params, ret, vec![]),
                }
            }
        }
    }

    // ---- per-function check + finalize ----------------------------------

    /// Infers a function body against its provisional signature, leaving
    /// generalization to the caller.
    /// The three calls `expand_for` writes for a `v[i]` indexed by the loop
    /// variable. `for.check(v, lo, hi, go)` is one bounds test before the
    /// loop when `v` is a buffer, and nothing otherwise; `for.at(v, i, go)`
    /// and `for.set(v, i, x, go)` are the accesses inside loop function `go`,
    /// unchecked when the check vouched for exactly this buffer in exactly
    /// this loop, and the ordinary checked (or map) access otherwise.
    fn infer_for_index(
        &mut self,
        name: &str,
        args: &[Expr],
        line: usize,
        env: &mut HashMap<String, Ty>,
    ) -> Result<TExpr, TypeError> {
        // for.check(base, lo, hi, go, exact); for.at(base, i, go, origin);
        // for.set(base, i, value, go, origin). `origin` is the dotted field
        // name a `for.base` temp stands for, or "" for a plain name.
        let go = match args.iter().find(|a| matches!(a, Expr::Str(_))) {
            Some(Expr::Str(g)) => g.clone(),
            _ => unreachable!("expand_for names the loop"),
        };
        let origin = match args.last() {
            Some(Expr::Var(n, _)) if name != "for.check" => n.clone(),
            _ => String::new(),
        };
        let exact = matches!(args.last(), Some(Expr::Bool(true)));
        let base_name = match &args[0] {
            Expr::Var(n, _) => n.clone(),
            _ => unreachable!("expand_for hoists only named buffers"),
        };
        // A field read into a temp before the record's type is settled has a
        // type nothing can look inside yet; indexing it would be guessed as a
        // string or a map. The field itself is deferred until the type is
        // known, so it is what gets indexed then — checked, as before.
        let base_expr = if name != "for.check" && !origin.is_empty() {
            let t = self.infer(&args[0], env)?;
            if matches!(self.resolve(&t.ty), Ty::Var(_)) {
                Expr::Var(origin.clone(), line)
            } else {
                args[0].clone()
            }
        } else {
            args[0].clone()
        };
        if name == "for.check" {
            let b = self.infer(&args[0], env)?;
            let lo = self.infer(&args[1], env)?;
            let hi = self.infer(&args[2], env)?;
            self.unify(&Ty::Int, &lo.ty, line, "a loop bound")?;
            self.unify(&Ty::Int, &hi.ty, line, "a loop bound")?;
            // `v[i + e]`: the loop's own range and the offset apart, so the
            // check can add them where nothing wraps.
            let off = match args.get(5) {
                Some(o) => {
                    let o = self.infer(o, env)?;
                    self.unify(&Ty::Int, &o.ty, line, "an index offset")?;
                    Some(o)
                }
                None => None,
            };
            match self.resolve(&b.ty) {
                Ty::Buf(_) => {
                    self.hoisted.insert((go, base_name));
                    let kind = match off {
                        Some(o) => TK::Builtin { rt: "rill_buf_check_off", args: vec![b, lo, hi, o] },
                        None => TK::Builtin { rt: "rill_buf_check", args: vec![b, lo, hi] },
                    };
                    return Ok(TExpr { ty: Ty::Unit, line, kind });
                }
                // A string index past the end is `-1` by contract, and a loop
                // may lean on that, so a string is only vouched for when the
                // range is exactly `0..#s` — in range by construction, no
                // test needed in front either.
                Ty::Str if exact => {
                    self.hoisted.insert((go, base_name));
                }
                _ => {}
            }
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Block(Vec::new()) });
        }
        // The loop function is lifted under its parent's name — `for.go`
        // becomes `main.for.go` — and the names `pick_name` hands out are
        // unique across the module, so the suffix identifies the loop.
        let vouched = self.hoisted.iter().any(|(g, b)| {
            *b == base_name && (self.cur_fn_name == *g || self.cur_fn_name.ends_with(&format!(".{g}")))
        });
        if name == "for.at" {
            let plain = Expr::Index {
                base: Box::new(base_expr),
                index: Box::new(args[1].clone()),
                line,
            };
            let mut t = self.infer(&plain, env)?;
            if vouched {
                match &mut t.kind {
                    TK::BufGet { unchecked, .. } => *unchecked = true,
                    TK::Builtin { rt, .. } if *rt == "rill_str_get" => *rt = "rill_str_at",
                    _ => {}
                }
            }
            return Ok(t);
        }
        let plain = Expr::IndexSet {
            base: Box::new(base_expr),
            index: Box::new(args[1].clone()),
            value: Box::new(args[2].clone()),
            line,
        };
        let mut t = self.infer(&plain, env)?;
        if vouched {
            if let TK::BufSet { unchecked, .. } = &mut t.kind {
                *unchecked = true;
            }
        }
        Ok(t)
    }

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
        let saved_name = std::mem::replace(&mut self.cur_fn_name, f.name.clone());
        // What the `where` clause requires, asked now so that it is settled
        // with the body's own requirements and travels with the scheme.
        for (ty, need, line) in self.bounds.get(&idx).cloned().unwrap_or_default() {
            self.require(&ty, need, line, "the `where` clause")?;
        }
        self.expected_ret.push(sig_ret.clone());
        let body = self.infer(&f.body, &mut env);
        self.expected_ret.pop();
        self.cur_fn_name = saved_name;
        let body = body?;
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
        // 0. Fields read before their type was known. The whole group has been
        //    inferred by now, so this is the first moment the answer can
        //    exist — and it has to happen before the requirements below, since
        //    `a = p.x` followed by `a + 1` leaves a requirement on whatever
        //    `a` is, and only this says what that is.
        let obligations = std::mem::take(&mut self.field_obligations);
        let mut unsaid: Vec<FieldObligation> = Vec::new();
        for ob in obligations {
            match self.resolve(&ob.base) {
                Ty::Var(_) => unsaid.push(ob),
                _ => self.settle_field(&ob)?,
            }
        }
        //    The ones nothing said the type of are answered by the fields
        //    themselves: a value read by `.x` and `.y` is of the type that
        //    has an `x` and a `y`, when there is exactly one such type. Two
        //    that both have them is the case an annotation is for, and the
        //    error names them; none is a field that does not exist.
        let mut groups: Vec<(u32, Vec<FieldObligation>)> = Vec::new();
        for ob in unsaid {
            let Ty::Var(v) = self.resolve(&ob.base) else { unreachable!("sorted above") };
            match groups.iter_mut().find(|(g, _)| *g == v) {
                Some((_, obs)) => obs.push(ob),
                None => groups.push((v, vec![ob])),
            }
        }
        //    Two passes: first the groups one type answers outright, since
        //    naming one can settle another — `if p.x > 0 then p else q.pos`
        //    says what `p` is once `q` is known — then what is left, which
        //    is either settled by now or has no answer.
        let mut done = vec![false; groups.len()];
        for pass in 0..2 {
            for (i, (v, obs)) in groups.iter().enumerate() {
                if done[i] {
                    continue;
                }
                if !matches!(self.resolve(&Ty::Var(*v)), Ty::Var(_)) {
                    for ob in obs {
                        self.settle_field(ob)?;
                    }
                    done[i] = true;
                } else if pass == 1 || self.types_with_fields(&fields_asked(obs)).len() == 1 {
                    self.name_type_by_fields(*v, obs)?;
                    done[i] = true;
                }
            }
        }

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
        Ok(TFn { name: f.name.clone(), exported: f.exported, params, ret, ngen, constraints, body, line: f.line, fast: f.fast })
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
            TK::BufLen { buf } | TK::BufPtr { buf } | TK::Unique { val: buf } => {
                self.zonk_expr(buf, genmap, defaults)
            }
            TK::Slice { base, from, to } => {
                self.zonk_expr(base, genmap, defaults);
                self.zonk_expr(from, genmap, defaults);
                if let Some(t) = to {
                    self.zonk_expr(t, genmap, defaults);
                }
            }
            TK::BufGet { buf, idx, .. } => {
                self.zonk_expr(buf, genmap, defaults);
                self.zonk_expr(idx, genmap, defaults);
            }
            TK::BufSet { buf, idx, val, .. } => {
                self.zonk_expr(buf, genmap, defaults);
                self.zonk_expr(idx, genmap, defaults);
                self.zonk_expr(val, genmap, defaults);
            }
            TK::FieldGet { base, .. } => self.zonk_expr(base, genmap, defaults),
            // The field can be named now. Everything the group learned is in
            // the substitution, and the base has just been zonked with it, so
            // if the type is ever going to be known it is known here.
            TK::FieldNamed { base, name } => {
                self.zonk_expr(base, genmap, defaults);
                let name = name.clone();
                let what = describe(base);
                match self.settled_field(&base.ty, &name, e.line, &what) {
                    Ok((adt, per)) => {
                        let base = std::mem::replace(
                            base,
                            Box::new(TExpr { ty: Ty::Unit, line: e.line, kind: TK::MapNew }),
                        );
                        e.kind = self.lower_field(*base, adt, &per, e.line).kind;
                    }
                    Err(err) => self.late_errors.borrow_mut().push(err),
                }
            }
            // The call can be built now, for the same reason.
            TK::UpdateNamed { base, fields } => {
                self.zonk_expr(base, genmap, defaults);
                for (_, v, _) in fields.iter_mut() {
                    self.zonk_expr(v, genmap, defaults);
                }
                let what = describe(base);
                let mut wheres: Vec<Vec<usize>> = Vec::new();
                for (name, _, fline) in fields.iter() {
                    match self.settled_field(&base.ty, name, *fline, &what) {
                        Ok((_, per)) => wheres.push(per),
                        Err(err) => {
                            self.late_errors.borrow_mut().push(err);
                            return;
                        }
                    }
                }
                let Ty::Adt(adt, targs) = self.resolve(&base.ty) else { unreachable!("settled") };
                let base = std::mem::replace(
                    base,
                    Box::new(TExpr { ty: Ty::Unit, line: e.line, kind: TK::MapNew }),
                );
                let sets = wheres.into_iter().zip(std::mem::take(fields)).map(|(per, (_, v, _))| (per, v)).collect();
                e.kind = self.lower_update(adt, targs, *base, sets, e.line).kind;
            }
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
            TK::Println(a) | TK::Print(a) | TK::Show(a) | TK::Inspect(a) | TK::Neg(a) => {
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
            TK::ChanOp { ch, .. } => self.zonk_expr(ch, genmap, defaults),
            TK::ChanTaken { ch } => self.zonk_expr(ch, genmap, defaults),
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
                        TSelectOp::Timeout { ms } => self.zonk_expr(ms, genmap, defaults),
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
            Expr::Unit => Ok(TExpr { ty: Ty::Unit, line: 0, kind: TK::Block(Vec::new()) }),
            Expr::Var(name, line) => self.infer_var(name, *line, env),
            Expr::Field { base, name, line } => {
                let b = self.infer(base, env)?;
                self.field_of(b, name, *line)
            }
            Expr::Update { base, fields, line } => self.infer_update(base, fields, *line, env),
            Expr::Call { name, args, line, .. }
                if name == "for.check" || name == "for.at" || name == "for.set" =>
            {
                self.infer_for_index(name, args, *line, env)
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
            Expr::Binary { op, lhs, rhs, line, .. } => {
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
            Expr::If { cond, then, els, .. } => {
                let c = self.infer(cond, env)?;
                // Expected first: the message had the two the wrong way
                // round, and `!5` — which is this `if` — blamed an `if` the
                // program never wrote (2026-09-25).
                let ctx = if matches!((then.as_ref(), els.as_ref()), (Expr::Bool(false), Expr::Bool(true))) {
                    "`!` takes a Bool"
                } else {
                    "`if` condition"
                };
                self.unify(&Ty::Bool, &c.ty, c.line, ctx)?;
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
                let unknown = self.fresh();
                self.expected_ret.push(unknown);
                let tbody = self.infer(body, &mut inner);
                self.expected_ret.pop();
                let tbody = tbody?;
                let mut free = Vec::new();
                let mut seen = HashSet::new();
                free_vars(body, &mut free, &mut seen);
                call_heads(body, &mut free, &mut seen);
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
                // Where the first `timeout` arm was written, for the
                // complaint below about one beside a `default`.
                let mut timeout_line = None;
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
                        SelectOp::Timeout { ms } => {
                            timeout_line.get_or_insert(arm.line);
                            // A `default` arm means "do not wait", so a
                            // deadline beside one could never be reached. The
                            // check is below, where both have been seen, since
                            // `default` may be written after this arm.
                            let n = self.infer(ms, env)?;
                            self.unify(&Ty::Int, &n.ty, arm.line, "`timeout` in select")?;
                            (TSelectOp::Timeout { ms: n }, None)
                        }
                    };
                    let body = self.infer(&arm.body, &mut arm_env)?;
                    self.unify(&result_ty, &body.ty, arm.line, "select arms")?;
                    tarms.push(TSelectArm { op, bind, body });
                }
                // A `default` arm means "do not wait", so a deadline beside
                // one could never be reached. Written either way round, that
                // is a program meaning to wait and saying both.
                if default.is_some() {
                    if let Some(at) = timeout_line {
                        return Err(TypeError::new(
                            at,
                            "a `select` with `default` never waits, so a `timeout` arm could not be reached"
                                .to_string(),
                        ));
                    }
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
                let unknown = self.fresh();
                self.expected_ret.push(unknown);
                let tbody = self.infer(body, &mut inner);
                self.expected_ret.pop();
                let tbody = tbody?;

                // Captures: free variables of the body that refer to locals of
                // the enclosing scope (not the lambda's own parameters).
                let mut free = Vec::new();
                let mut seen = HashSet::new();
                free_vars(body, &mut free, &mut seen);
                call_heads(body, &mut free, &mut seen);
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
                        stacked: false,
                    };
                    return self.infer(&call, env);
                }
                let idx = self.infer(index, env)?;
                self.unify(&Ty::Int, &idx.ty, *line, "index")?;
                match self.resolve(&b.ty) {
                    Ty::Buf(w) => Ok(TExpr {
                        ty: w.rill_ty(),
                        line: *line,
                        kind: TK::BufGet { buf: Box::new(b), idx: Box::new(idx), unchecked: false },
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
                        unchecked: false,
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
            Expr::While { line, .. } => {
                Err(TypeError::new(*line, "internal: `while` was not expanded".to_string()))
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
                // What a block binds is the block's: a name bound in one
                // branch of an `if` is not there after it, and one that
                // shadows an outer name shadows it for the block's length.
                let mut scoped = env.clone();
                let env = &mut scoped;
                let mut out = Vec::new();
                let mut ty = Ty::Unit;
                for (i, stmt) in stmts.iter().enumerate() {
                    let last = i + 1 == stmts.len();
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
                        Stmt::Bind { name, value, line } => {
                            // A block's value is what it ends with, and a
                            // binding is not a value: nothing after this one
                            // can read the name, because there is nothing
                            // after it. The usual cause is a loop meant to
                            // carry something — `acc = acc + v[i]` written in
                            // the body rather than `with acc = 0` on the
                            // loop — which used to compile and answer with
                            // whatever the accumulator started as.
                            if last {
                                let mut names = Vec::new();
                                let mut seen = HashSet::new();
                                free_vars(value, &mut names, &mut seen);
                                return Err(TypeError::new(
                                    *line,
                                    if names.contains(name) {
                                        format!(
                                            "`{name}` is bound to something that reads `{name}`, \
                                             as the last thing in this block — so nothing ever \
                                             reads what it holds, and the binding does nothing. \
                                             A loop that carries a value says so on the loop: \
                                             `for i in .. with {name} = <start>`, whose body's \
                                             value is the next `{name}`."
                                        )
                                    } else {
                                        format!(
                                            "`{name}` is bound as the last thing in this block, \
                                             so nothing can read it: a block's value is the \
                                             expression it ends with, and a binding has none. \
                                             Write the expression on its own, or move the \
                                             binding above what uses it."
                                        )
                                    },
                                ));
                            }
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
    /// `p.x`.
    ///
    /// When the type of `p` is known here, this is a lookup and the answer is
    /// a `FieldGet`. When it is not, the answer cannot exist yet — and used to
    /// be an error demanding an annotation, which made `.x` depend on the
    /// order the statements were written in: `a = uzak(p)` then `p.x` compiled,
    /// and the same two lines the other way round did not, though the second
    /// says exactly as much about `p` as the first.
    ///
    /// So it is deferred. The name is carried on the node and an obligation is
    /// recorded against the type, both settled once the whole dependency group
    /// is inferred. The annotation is still needed when inference genuinely
    /// never learns the type — a function polymorphic in that parameter — and
    /// that is what the error says now, at the end, having actually looked.
    fn field_of(&mut self, base: TExpr, field: &str, line: usize) -> Result<TExpr, TypeError> {
        if let Ty::Var(_) = self.resolve(&base.ty) {
            let ty = self.fresh();
            self.field_obligations.push(FieldObligation {
                base: base.ty.clone(),
                name: field.to_string(),
                result: ty.clone(),
                line,
                what: describe(&base),
                ctx: "a field",
            });
            return Ok(TExpr {
                ty,
                line,
                kind: TK::FieldNamed { base: Box::new(base), name: field.to_string() },
            });
        }
        let (adt, per) = self.settled_field(&base.ty, field, line, &describe(&base))?;
        Ok(self.lower_field(base, adt, &per, line))
    }

    /// `p.x` once the type is known: one load when there is one constructor,
    /// and a `match` naming the field in each when there are several — which
    /// is what a program would have written, and what codegen already knows
    /// how to take apart.
    fn lower_field(&self, base: TExpr, adt: usize, per: &[usize], line: usize) -> TExpr {
        let targs = match self.resolve(&base.ty) {
            Ty::Adt(_, targs) => targs,
            _ => Vec::new(),
        };
        let ty = subst_gen(&self.adts[adt].ctors[0].fields[per[0]], &targs);
        if per.len() == 1 {
            let index = per[0];
            return TExpr { ty, line, kind: TK::FieldGet { base: Box::new(base), adt, ctor: 0, index } };
        }
        let held = self.held_name("field");
        let arms = self.adts[adt]
            .ctors
            .iter()
            .enumerate()
            .map(|(ci, ctor)| {
                let args = (0..ctor.fields.len())
                    .map(|k| if k == per[ci] { Pattern::Var(held.clone()) } else { Pattern::Wildcard })
                    .collect();
                TArm {
                    pattern: Pattern::Ctor { name: ctor.name.clone(), args, line },
                    guard: None,
                    body: TExpr { ty: ty.clone(), line, kind: TK::Local(held.clone()) },
                    line,
                }
            })
            .collect();
        TExpr { ty, line, kind: TK::Match { scrutinee: Box::new(base), arms } }
    }

    /// A name for a binding the checker makes: a dot keeps it out of reach
    /// of anything a program could write.
    fn held_name(&self, what: &str) -> String {
        let n = self.updates.get();
        self.updates.set(n + 1);
        format!("{what}.{n}")
    }

    /// Which field `.x` is, for a type that is known: the type, and the
    /// field's position in each of its constructors. With one constructor
    /// the field is simply there or not. With several it has to be in every
    /// one — a field some constructor lacks is not certainly there, which is
    /// what `match` is for — and of one type across them.
    fn settled_field(
        &self,
        base_ty: &Ty,
        field: &str,
        line: usize,
        what: &str,
    ) -> Result<(usize, Vec<usize>), TypeError> {
        let adt = match self.resolve(base_ty) {
            Ty::Adt(i, _) => i,
            // Inference finished and never found out. Two types may both have
            // an `x`, and nothing here can work backwards from the name, so
            // this is one of the few places an annotation is not optional.
            Ty::Var(_) | Ty::Gen(_) => {
                let name = what.trim_matches('`');
                let name = if name == "this" { "x" } else { name };
                return Err(TypeError::new(
                    line,
                    format!(
                        "the type of {what} is never known, so `.{field}` cannot say which \
                         field it is — annotate it (`{name}: SomeType`)"
                    ),
                ))
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
        fn named(c: &CtorSig) -> Vec<&str> {
            c.names.iter().filter_map(|n| n.as_deref()).collect()
        }
        let mut per = Vec::with_capacity(def.ctors.len());
        for ctor in &def.ctors {
            if let Some(i) = ctor.names.iter().position(|n| n.as_deref() == Some(field)) {
                per.push(i);
                continue;
            }
            if def.ctors.len() == 1 {
                let known = named(ctor);
                let has = if known.is_empty() {
                    "its fields are not named, and a field is read by its name".to_string()
                } else {
                    format!("it has {}", known.join(", "))
                };
                return Err(TypeError::new(
                    line,
                    format!("`{}` has no field `{field}` — {has}", def.name),
                ));
            }
            let in_some: Vec<String> = def
                .ctors
                .iter()
                .filter(|c| c.names.iter().any(|n| n.as_deref() == Some(field)))
                .map(|c| format!("`{}`", c.name))
                .collect();
            if in_some.is_empty() {
                let each: Vec<String> = def
                    .ctors
                    .iter()
                    .map(|c| {
                        let fs = named(c);
                        let has = if fs.is_empty() { "no named fields".to_string() } else { fs.join(", ") };
                        format!("`{}` has {has}", c.name)
                    })
                    .collect();
                return Err(TypeError::new(
                    line,
                    format!("`{}` has no field `{field}` — {}", def.name, each.join("; ")),
                ));
            }
            return Err(TypeError::new(
                line,
                format!(
                    "`{}` has more than one constructor, and `{field}` is a field of {} but not \
                     of `{}` — on such a type a field has to be in every constructor to be \
                     read by name; `match` is how the others are read",
                    def.name,
                    in_some.join(" and "),
                    ctor.name
                ),
            ));
        }
        let first = &def.ctors[0].fields[per[0]];
        for (ci, ctor) in def.ctors.iter().enumerate().skip(1) {
            let this = &ctor.fields[per[ci]];
            if this != first {
                return Err(TypeError::new(
                    line,
                    format!(
                        "`{field}` is `{}` in `{}` and `{}` in `{}`, so `.{field}` on a `{}` \
                         has no one type",
                        self.type_name(first),
                        def.ctors[0].name,
                        self.type_name(this),
                        ctor.name,
                        def.name
                    ),
                ));
            }
        }
        Ok((adt, per))
    }

    /// `{ p | x = 9 }` — every field `p` had, with those named replaced.
    ///
    /// It becomes an ordinary call to the constructor: the named fields take
    /// the new expressions, and every other one is read off the base. Which
    /// means the base has to be read several times, so it is bound first —
    /// `{ f(a) | x = 9 }` must call `f` once, not once per field it did not
    /// mention.
    fn infer_update(
        &mut self,
        base: &Expr,
        fields: &[(String, Expr, usize)],
        line: usize,
        env: &mut HashMap<String, Ty>,
    ) -> Result<TExpr, TypeError> {
        let b = self.infer(base, env)?;
        // The same rule `.x` has, and the same reasons, so the same words: a
        // field name only says which field when there is one constructor for
        // it to belong to.
        let (adt, targs) = match self.resolve(&b.ty) {
            Ty::Adt(i, targs) => (i, targs),
            // Not known here. The names are carried to `zonk`, which builds
            // the call once inference has said what the base is — and each
            // is an obligation on its type, exactly as `p.x` is, so the
            // fields an update sets can say the type the way the fields a
            // read asks can.
            Ty::Var(_) => {
                let what = describe(&b);
                let mut out: Vec<(String, TExpr, usize)> = Vec::new();
                for (name, value, fline) in fields {
                    if out.iter().any(|(n, _, _)| n == name) {
                        return Err(TypeError::new(
                            *fline,
                            format!("field `{name}` is set twice in this update"),
                        ));
                    }
                    let v = self.infer(value, env)?;
                    self.field_obligations.push(FieldObligation {
                        base: b.ty.clone(),
                        name: name.clone(),
                        result: v.ty.clone(),
                        line: *fline,
                        what: what.clone(),
                        ctx: "the new value for a field",
                    });
                    out.push((name.clone(), v, *fline));
                }
                let ty = b.ty.clone();
                return Ok(TExpr { ty, line, kind: TK::UpdateNamed { base: Box::new(b), fields: out } });
            }
            other => {
                return Err(TypeError::new(
                    line,
                    format!("`{}` has no fields to update", self.type_name(&other)),
                ))
            }
        };
        // Which field each name is, checked before anything is inferred so
        // that a misspelling is reported as one.
        let what = describe(&b);
        let mut wheres: Vec<Vec<usize>> = Vec::new();
        for (i, (name, _, fline)) in fields.iter().enumerate() {
            if fields[..i].iter().any(|(n, _, _)| n == name) {
                return Err(TypeError::new(
                    *fline,
                    format!("field `{name}` is set twice in this update"),
                ));
            }
            wheres.push(self.settled_field(&b.ty, name, *fline, &what)?.1);
        }
        let mut sets = Vec::new();
        for (per, (_, value, fline)) in wheres.into_iter().zip(fields) {
            let v = self.infer(value, env)?;
            let want = subst_gen(&self.adts[adt].ctors[0].fields[per[0]], &targs);
            self.unify(&want, &v.ty, *fline, "the new value for a field")?;
            sets.push((per, v));
        }
        Ok(self.lower_update(adt, targs, b, sets, line))
    }

    /// The call an update is: the constructor, given the new values where
    /// there are any and the base's own fields everywhere else. The base is
    /// bound first, so it is evaluated once however many fields it fills in.
    /// Each new value comes with the field's position in every constructor.
    ///
    /// With several constructors, which one is being rebuilt is only known
    /// at run time, so it is a `match` with an arm per constructor, each
    /// naming the fields it keeps and calling itself again with the new
    /// values in their places. Those are bound before the match, since one
    /// expression cannot stand in every arm.
    fn lower_update(
        &self,
        adt: usize,
        targs: Vec<Ty>,
        b: TExpr,
        sets: Vec<(Vec<usize>, TExpr)>,
        line: usize,
    ) -> TExpr {
        let held = self.held_name("update");
        let ty = Ty::Adt(adt, targs.clone());
        let def = &self.adts[adt];
        let local = |name: String, ty: Ty| TExpr { ty, line, kind: TK::Local(name) };
        let field_ty = |ci: usize, k: usize| subst_gen(&def.ctors[ci].fields[k], &targs);
        let mut stmts = vec![TStmt::Bind { name: held.clone(), value: b }];

        if def.ctors.len() == 1 {
            let mut slot: Vec<Option<TExpr>> = (0..def.ctors[0].fields.len()).map(|_| None).collect();
            // The new values run in the order they are written. Put straight
            // into the constructor's arguments they ran in the fields' order:
            // `{ r | y = g(), x = h() }` called `h` first (2026-09-25). Where
            // the two orders differ they are named first, in the written one.
            let in_order = sets.windows(2).all(|w| w[0].0[0] < w[1].0[0]);
            for (j, (per, v)) in sets.into_iter().enumerate() {
                if in_order {
                    slot[per[0]] = Some(v);
                } else {
                    let name = format!("{held}.{j}");
                    let vty = v.ty.clone();
                    stmts.push(TStmt::Bind { name: name.clone(), value: v });
                    slot[per[0]] = Some(local(name, vty));
                }
            }
            let args = (0..slot.len())
                .map(|i| match slot[i].take() {
                    Some(v) => v,
                    None => TExpr {
                        ty: field_ty(0, i),
                        line,
                        kind: TK::FieldGet {
                            base: Box::new(local(held.clone(), ty.clone())),
                            adt,
                            ctor: 0,
                            index: i,
                        },
                    },
                })
                .collect();
            stmts.push(TStmt::Expr(TExpr {
                ty: ty.clone(),
                line,
                kind: TK::CallCtor { adt, ctor: 0, targs, args },
            }));
            return TExpr { ty, line, kind: TK::Block(stmts) };
        }

        let news: Vec<(Vec<usize>, String)> = sets
            .into_iter()
            .enumerate()
            .map(|(j, (per, v))| {
                let name = format!("{held}.{j}");
                stmts.push(TStmt::Bind { name: name.clone(), value: v });
                (per, name)
            })
            .collect();
        let arms = def
            .ctors
            .iter()
            .enumerate()
            .map(|(ci, ctor)| {
                let mut pats = Vec::new();
                let mut args = Vec::new();
                for k in 0..ctor.fields.len() {
                    match news.iter().find(|(per, _)| per[ci] == k) {
                        Some((_, name)) => {
                            pats.push(Pattern::Wildcard);
                            args.push(local(name.clone(), field_ty(ci, k)));
                        }
                        None => {
                            let name = format!("{held}.{ci}.{k}");
                            pats.push(Pattern::Var(name.clone()));
                            args.push(local(name, field_ty(ci, k)));
                        }
                    }
                }
                TArm {
                    pattern: Pattern::Ctor { name: ctor.name.clone(), args: pats, line },
                    guard: None,
                    body: TExpr {
                        ty: ty.clone(),
                        line,
                        kind: TK::CallCtor { adt, ctor: ci, targs: targs.clone(), args },
                    },
                    line,
                }
            })
            .collect();
        stmts.push(TStmt::Expr(TExpr {
            ty: ty.clone(),
            line,
            kind: TK::Match { scrutinee: Box::new(local(held, ty.clone())), arms },
        }));
        TExpr { ty, line, kind: TK::Block(stmts) }
    }

    /// Settles one obligation whose type is known    /// Settles one obligation whose type is known: the field's type is what
    /// was read from it. One the type does not have is left for `zonk`,
    /// which reports it with the right words.
    fn settle_field(&mut self, ob: &FieldObligation) -> Result<(), TypeError> {
        if let Ty::Adt(adt, targs) = self.resolve(&ob.base) {
            if let Ok((_, per)) = self.settled_field(&ob.base, &ob.name, ob.line, "it") {
                let ty = subst_gen(&self.adts[adt].ctors[0].fields[per[0]], &targs);
                self.unify(&ty, &ob.result, ob.line, ob.ctx)?;
            }
        }
        Ok(())
    }

    /// The types every constructor of which has every one of these fields.
    fn types_with_fields(&self, names: &[&str]) -> Vec<usize> {
        (0..self.adts.len())
            .filter(|&i| names.iter().all(|n| self.has_field(i, n)))
            .collect()
    }

    /// Whether every constructor of a type has a field of this name — the
    /// condition for reading it without a `match`.
    fn has_field(&self, adt: usize, field: &str) -> bool {
        let ctors = &self.adts[adt].ctors;
        !ctors.is_empty() && ctors.iter().all(|c| c.names.iter().any(|n| n.as_deref() == Some(field)))
    }

    /// The field names every constructor of a type has.
    fn common_fields(&self, adt: usize) -> Vec<&str> {
        let ctors = &self.adts[adt].ctors;
        ctors[0].names.iter().filter_map(|n| n.as_deref()).filter(|n| self.has_field(adt, n)).collect()
    }

    /// The type of a value nothing named, from the fields asked of it: the
    /// one type with one constructor that has them all. With none there is
    /// no such type; with two, the names cannot choose, and that is the one
    /// case an annotation is still for.
    fn name_type_by_fields(&mut self, v: u32, obs: &[FieldObligation]) -> Result<(), TypeError> {
        let names = fields_asked(obs);
        let candidates = self.types_with_fields(&names);
        let first = &obs[0];
        let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
        let which = if quoted.len() == 1 {
            format!("a field {}", quoted[0])
        } else {
            format!("fields {} together", quoted.join(", "))
        };
        match candidates.as_slice() {
            [adt] => {
                let targs: Vec<Ty> = (0..self.adts[*adt].nparams).map(|_| self.fresh()).collect();
                self.unify(&Ty::Var(v), &Ty::Adt(*adt, targs), first.line, "a field")?;
                for ob in obs {
                    self.settle_field(ob)?;
                }
                Ok(())
            }
            [] => {
                // A misspelling, most likely: say what the near misses have.
                let near: Vec<String> = (0..self.adts.len())
                    .filter(|&i| names.iter().any(|n| self.has_field(i, n)))
                    .map(|i| format!("`{}` has {}", self.adts[i].name, self.common_fields(i).join(", ")))
                    .collect();
                let hint = if near.is_empty() { String::new() } else { format!(" ({})", near.join("; ")) };
                Err(TypeError::new(
                    first.line,
                    format!(
                        "nothing says what {} is, and no type has {which}{hint}",
                        first.what
                    ),
                ))
            }
            many => {
                let types: Vec<String> = many.iter().map(|&i| format!("`{}`", self.adts[i].name)).collect();
                let name = first.what.trim_matches('`');
                let name = if name == "this" { "x" } else { name };
                Err(TypeError::new(
                    first.line,
                    format!(
                        "{} and {} alike have {which}, so the field names cannot say which {} is — \
                         annotate it (`{name}: {}`)",
                        types[..types.len() - 1].join(", "),
                        types[types.len() - 1],
                        first.what,
                        self.adts[many[0]].name,
                    ),
                ))
            }
        }
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
            let (params, ret, subst) = self.sig_for_use(idx);
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
        if name == "_" {
            // It got past `lift`, so it was not in an argument list.
            return Err(TypeError::new(
                line,
                "`_` stands for an argument left to be given later, so it only means something where an argument would go — `add(2, _)`"
                    .to_string(),
            ));
        }
        Err(TypeError::new(line, self.unknown_name(name, "variable")))
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

        // `s.speak(x)`: a field that holds a function, called. A module
        // alias is not a local, so `geo.area(x)` does not come this way —
        // and where a program has both an alias `g` and a local `g`, the
        // alias wins, as it does for every dotted name.
        if let Some((head, _)) = name.split_once('.') {
            let global = self.fn_index.contains_key(name)
                || self.ctors.contains_key(name)
                || self.is_method(name)
                || self.externs.contains_key(name);
            if env.contains_key(head) && !global {
                let callee = self.infer_var(name, line, env)?;
                let mut targs = Vec::new();
                for a in args {
                    targs.push(self.infer(a, env)?);
                }
                let ret = self.fresh();
                let want =
                    Ty::Fn(targs.iter().map(|a| a.ty.clone()).collect(), Box::new(ret.clone()));
                self.unify(&callee.ty, &want, line, &format!("calling `{name}`"))?;
                return Ok(TExpr {
                    ty: ret,
                    line,
                    kind: TK::CallClosure { callee: Box::new(callee), args: targs },
                });
            }
        }
        // `Speak(x)`: `x` as an object of the trait. Written out it is
        // `Speak.obj(\ -> speak(x), \o -> greet(x, o))` — a record whose
        // fields hold the methods with the value closed over — and that is
        // what is written here, then inferred as any expression is. Each
        // call inside dispatches on the type of `x` as every method call
        // does; the record's type is the trait's name, so a `List(Speak)`
        // holds a dog and a robot alike.
        if self.trait_objects.contains_key(name) {
            if args.len() != 1 {
                return Err(TypeError::new(
                    line,
                    format!("`{name}(x)` makes an object of one value; {} given", args.len()),
                ));
            }
            let tid = self.traits.iter().position(|(n, _)| n == name).expect("a trait");
            let held = self.held_name("obj");
            let lambdas: Vec<Expr> = self.traits[tid]
                .1
                .iter()
                .map(|m| {
                    let params: Vec<String> =
                        (1..m.params.len()).map(|i| format!("{held}.{i}")).collect();
                    let mut call_args = vec![Expr::Var(held.clone(), line)];
                    call_args.extend(params.iter().map(|p| Expr::Var(p.clone(), line)));
                    Expr::Lambda {
                        params,
                        body: Box::new(Expr::Call {
                            name: m.name.clone(),
                            args: call_args,
                            line,
                            piped: false,
                            stacked: false,
                        }),
                        line,
                    }
                })
                .collect();
            let make = Expr::Block(vec![
                Stmt::Bind { name: held, value: args[0].clone(), line },
                Stmt::Expr(Expr::Call {
                    name: format!("{name}.obj"),
                    args: lambdas,
                    line,
                    piped: false,
                    stacked: false,
                }),
            ]);
            return self.infer(&make, env);
        }
        if let Some(why) = self.object_refusals.get(name) {
            return Err(TypeError::new(
                line,
                format!("`{name}` cannot be the type of an object: {why}"),
            ));
        }

        if name == "println" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`println` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`println`")?;
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Println(Box::new(a)) });
        }
        if name == "mailbox" {
            // `mailbox()`: a channel whose buffer grows rather than making a
            // sender wait — a machine's, so two machines sending each other
            // signals at once never hold each other up.
            if !args.is_empty() {
                return Err(TypeError::new(line, "`mailbox` takes nothing: it grows as it must".to_string()));
            }
            let elem = self.fresh();
            let cap = TExpr { ty: Ty::Int, line, kind: TK::Int(-1) };
            return Ok(TExpr { ty: Ty::Chan(Box::new(elem)), line, kind: TK::ChanNew(Some(Box::new(cap))) });
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
        // `buf_sort(b)`: the buffer's numbers into ascending order, in place,
        // by the runtime's pattern-defeating quicksort — what a program that
        // keeps its data in a buffer sorts it with, where it used to have to
        // write the sort out. Floats in their total order: NaN at the ends.
        // `buf_select_nth(b, lo, hi, k)`: the float that would be at `k` were
        // the range sorted, the range reordered around it.
        if name == "buf_select_nth" {
            if args.len() != 4 {
                return Err(TypeError::new(line, format!("`buf_select_nth` takes a buffer, a range and a place, but {} given", args.len())));
            }
            let b = self.infer(&args[0], env)?;
            self.unify(&Ty::Buf(CTy::F64), &b.ty, line, "what `buf_select_nth` looks in")?;
            let mut all = vec![b];
            for a in &args[1..] {
                let x = self.infer(a, env)?;
                self.unify(&Ty::Int, &x.ty, line, "a place in the buffer")?;
                all.push(x);
            }
            return Ok(TExpr { ty: Ty::Float, line, kind: TK::Builtin { rt: "rill_buf_select_nth", args: all } });
        }
        // `buf_argsort(src, dst)`: the order of a float buffer's elements,
        // stable, into an `Int` buffer.
        if name == "buf_argsort" {
            if args.len() != 2 {
                return Err(TypeError::new(line, format!("`buf_argsort` takes two buffers, but {} given", args.len())));
            }
            let src = self.infer(&args[0], env)?;
            let dst = self.infer(&args[1], env)?;
            self.unify(&Ty::Buf(CTy::F64), &src.ty, line, "what `buf_argsort` orders")?;
            self.unify(&Ty::Buf(CTy::I64), &dst.ty, line, "where `buf_argsort` writes the order")?;
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Builtin { rt: "rill_buf_argsort", args: vec![src, dst] } });
        }
        if name == "buf_sort" || name == "buf_sort_range" {
            let want = if name == "buf_sort" { 1 } else { 3 };
            if args.len() != want {
                return Err(TypeError::new(line, format!("`{name}` takes {want} argument(s), but {} given", args.len())));
            }
            let b = self.infer(&args[0], env)?;
            let code = match self.resolve(&b.ty) {
                Ty::Buf(w) => match w {
                    CTy::I8 => 1,
                    CTy::U8 => 2,
                    CTy::I16 => 3,
                    CTy::U16 => 4,
                    CTy::I32 => 5,
                    CTy::U32 => 6,
                    CTy::I64 | CTy::ISize => 7,
                    CTy::U64 | CTy::USize => 8,
                    CTy::F32 => 9,
                    CTy::F64 => 10,
                    _ => {
                        return Err(TypeError::new(line, "`buf_sort` sorts a buffer of numbers"));
                    }
                },
                other => {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "`buf_sort` needs a buffer, but this is `{}` — a `Buf` width is never inferred, so annotate the parameter",
                            self.type_name(&other)
                        ),
                    ))
                }
            };
            let code = TExpr { ty: Ty::Int, line, kind: TK::Int(code) };
            let mut all = vec![b, code];
            for a in &args[1..] {
                let x = self.infer(a, env)?;
                self.unify(&Ty::Int, &x.ty, line, "a buffer range")?;
                all.push(x);
            }
            let rt = if name == "buf_sort" { "rill_buf_sort" } else { "rill_buf_sort_range" };
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Builtin { rt, args: all } });
        }
        // The prelude's `sort_with`: a stable sort of a list's own cells in
        // the runtime (`rill_list_sort`). `(List(a), (a, a) -> Bool) -> List(a)`.
        if name == "list_sort" {
            if args.len() != 2 {
                return Err(TypeError::new(
                    line,
                    format!("`list_sort` takes a list and an ordering, but {} given", args.len()),
                ));
            }
            let Some(list) = self.adts.iter().position(|a| a.name == "List") else {
                return Err(TypeError::new(line, "`list_sort` needs the prelude's `List`"));
            };
            let a = self.fresh();
            let l = self.infer(&args[0], env)?;
            let lt = Ty::Adt(list, vec![a.clone()]);
            self.unify(&lt, &l.ty, line, "the list `list_sort` sorts")?;
            let f = self.infer(&args[1], env)?;
            self.unify(&Ty::Fn(vec![a.clone(), a], Box::new(Ty::Bool)), &f.ty, line, "the ordering `list_sort` sorts by")?;
            return Ok(TExpr { ty: lt, line, kind: TK::Builtin { rt: "rill_list_sort", args: vec![l, f] } });
        }
        if name == "unique" {
            if args.len() != 1 {
                return Err(TypeError::new(
                    line,
                    format!("`unique` takes 1 argument, but {} given", args.len()),
                ));
            }
            let v = self.infer(&args[0], env)?;
            match self.resolve(&v.ty) {
                Ty::Buf(_) | Ty::Str | Ty::StrBuf | Ty::Map(..) | Ty::Chan(..) => {}
                Ty::Adt(..) => {}
                other => {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "`unique` asks whether a counted value has one holder, and `{}` \
                             is not counted — a buffer, a string, a map or a data value is",
                            self.type_name(&other)
                        ),
                    ))
                }
            }
            return Ok(TExpr { ty: Ty::Bool, line, kind: TK::Unique { val: Box::new(v) } });
        }
        if name == "str_from_buf" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`str_from_buf` takes one byte buffer"));
            }
            let b = self.infer(&args[0], env)?;
            match self.resolve(&b.ty) {
                Ty::Buf(CTy::U8) => {}
                other => {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "`str_from_buf` needs a `Buf(U8)`, but this is `{}`",
                            self.type_name(&other)
                        ),
                    ))
                }
            }
            return Ok(TExpr { ty: Ty::Str, line, kind: TK::Builtin { rt: "rill_str_from_buf".into(), args: vec![b] } });
        }
        // `buf_copy(dst, to, src, from, n)` and `buf_cmp(a, at, b, bt, n)`:
        // a run of elements between two buffers of one width, or within
        // one, kept inside both — what `memmove` and `memcmp` on raw
        // pointers do unasked. The width goes to the runtime as a sixth
        // argument, since a buffer does not carry it.
        if name == "buf_copy" || name == "buf_cmp" {
            if args.len() != 5 {
                return Err(TypeError::new(
                    line,
                    format!("`{name}` takes 5 arguments (dst, to, src, from, n), but {} given", args.len()),
                ));
            }
            let a = self.infer(&args[0], env)?;
            let width = match self.resolve(&a.ty) {
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
            let at = self.infer(&args[1], env)?;
            self.unify(&Ty::Int, &at.ty, line, "buffer offset")?;
            let b = self.infer(&args[2], env)?;
            self.unify(&Ty::Buf(width.clone()), &b.ty, line, &format!("`{name}`: both buffers"))?;
            let bt = self.infer(&args[3], env)?;
            self.unify(&Ty::Int, &bt.ty, line, "buffer offset")?;
            let n = self.infer(&args[4], env)?;
            self.unify(&Ty::Int, &n.ty, line, "element count")?;
            let size = TExpr { ty: Ty::Int, line, kind: TK::Int(width.size_bytes() as i64) };
            let (rt, ty) = if name == "buf_copy" { ("rill_buf_copy", Ty::Unit) } else { ("rill_buf_cmp", Ty::Int) };
            return Ok(TExpr {
                ty,
                line,
                kind: TK::Builtin { rt: rt.into(), args: vec![a, at, b, bt, n, size] },
            });
        }
        // `buf_get_unchecked` / `buf_set_unchecked`: one access with no bounds
        // test, for a loop whose indexes come from its data — an interpreter's
        // registers — where no check in front can stand for them. What
        // `--unchecked` does to a whole program, said at the one place that
        // was measured to need it; a sandbox counts it with the pointers.
        let unchecked = name == "buf_get_unchecked" || name == "buf_set_unchecked";
        let name = match name {
            "buf_get_unchecked" => "buf_get",
            "buf_set_unchecked" => "buf_set",
            other => other,
        };
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
            // An access asked to go unchecked carries a mark a sandbox can
            // see; one a loop's check in front stands for does not.
            let marked = |e: TExpr| -> TExpr {
                if !unchecked {
                    return e;
                }
                let ty = e.ty.clone();
                let mark = TExpr { ty: Ty::Unit, line, kind: TK::Builtin { rt: "rill_unchecked", args: Vec::new() } };
                TExpr { ty, line, kind: TK::Block(vec![TStmt::Expr(mark), TStmt::Expr(e)]) }
            };
            if name == "buf_get" {
                return Ok(marked(TExpr {
                    ty: width.rill_ty(),
                    line,
                    kind: TK::BufGet { buf: Box::new(b), idx: Box::new(idx), unchecked },
                }));
            }
            let val = self.infer(&args[2], env)?;
            self.unify(&width.rill_ty(), &val.ty, line, "buffer element")?;
            return Ok(marked(TExpr {
                ty: Ty::Unit,
                line,
                kind: TK::BufSet {
                    buf: Box::new(b),
                    idx: Box::new(idx),
                    val: Box::new(val),
                    unchecked,
                },
            }));
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
        if name == "close" || name == "closed" || name == "chan_wait" || name == "chan_id" {
            if args.len() != 1 {
                return Err(TypeError::new(
                    line,
                    format!("`{name}` takes exactly one channel"),
                ));
            }
            let ch = self.infer(&args[0], env)?;
            let elem = self.fresh();
            self.unify(&ch.ty, &Ty::Chan(Box::new(elem)), line, &format!("`{name}`"))?;
            let (op, ty) = match name {
                "close" => (ChanOp::Close, Ty::Unit),
                "closed" => (ChanOp::Closed, Ty::Bool),
                "chan_id" => (ChanOp::Id, Ty::Int),
                _ => (ChanOp::Wait, Ty::Bool),
            };
            return Ok(TExpr { ty, line, kind: TK::ChanOp { ch: Box::new(ch), op } });
        }
        if name == "chan_taken" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`chan_taken` takes exactly one channel"));
            }
            let ch = self.infer(&args[0], env)?;
            let elem = self.fresh();
            self.unify(&ch.ty, &Ty::Chan(Box::new(elem.clone())), line, "`chan_taken`")?;
            return Ok(TExpr { ty: elem, line, kind: TK::ChanTaken { ch: Box::new(ch) } });
        }
        if name == "show" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`show` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`show`")?;
            return Ok(TExpr { ty: Ty::Str, line, kind: TK::Show(Box::new(a)) });
        }
        if name == "machine_show" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`machine_show` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            return Ok(TExpr { ty: Ty::Str, line, kind: TK::Inspect(Box::new(a)) });
        }
        if name == "print" {
            if args.len() != 1 {
                return Err(TypeError::new(line, "`print` takes exactly one argument"));
            }
            let a = self.infer(&args[0], env)?;
            self.require(&a.ty, Constraint::Basic, line, "`print`")?;
            return Ok(TExpr { ty: Ty::Unit, line, kind: TK::Print(Box::new(a)) });
        }
        // `assert(c)` is `if c then () else <die, saying the line>`, and
        // `assert_eq(a, b)` the same on `a == b`, saying both values as
        // `show` does. Nothing is caught: a test that fails is a program
        // that ends, and `rill test` runs each in a process of its own.
        if name == "assert" || name == "assert_eq" {
            let unit = |line: usize| TExpr { ty: Ty::Unit, line, kind: TK::Block(Vec::new()) };
            let int = |v: i64, line: usize| TExpr { ty: Ty::Int, line, kind: TK::Int(v) };
            if name == "assert" {
                if args.len() != 1 {
                    return Err(TypeError::new(line, "`assert` takes one condition"));
                }
                let c = self.infer(&args[0], env)?;
                self.unify(&Ty::Bool, &c.ty, line, "`assert`")?;
                let fail = TExpr {
                    ty: Ty::Unit,
                    line,
                    kind: TK::Builtin { rt: "rill_assert_failed", args: vec![int(line as i64, line)] },
                };
                return Ok(TExpr {
                    ty: Ty::Unit,
                    line,
                    kind: TK::If { cond: Box::new(c), then: Box::new(unit(line)), els: Box::new(fail) },
                });
            }
            if args.len() != 2 {
                return Err(TypeError::new(line, "`assert_eq` takes the value and what it should be"));
            }
            let a = self.infer(&args[0], env)?;
            let b = self.infer(&args[1], env)?;
            self.unify(&a.ty, &b.ty, line, "`assert_eq`")?;
            self.require(&a.ty, Constraint::Basic, line, "`assert_eq`")?;
            let held = self.held_name("assert");
            let (ha, hb) = (format!("{held}.got"), format!("{held}.want"));
            let ty = a.ty.clone();
            let local = |n: &str| TExpr { ty: ty.clone(), line, kind: TK::Local(n.to_string()) };
            let shown = |n: &str| TExpr { ty: Ty::Str, line, kind: TK::Show(Box::new(local(n))) };
            let same = TExpr {
                ty: Ty::Bool,
                line,
                kind: TK::Binary { op: BinOp::Eq, lhs: Box::new(local(&ha)), rhs: Box::new(local(&hb)) },
            };
            let fail = TExpr {
                ty: Ty::Unit,
                line,
                kind: TK::Builtin {
                    rt: "rill_assert_eq_failed",
                    args: vec![shown(&ha), shown(&hb), int(line as i64, line)],
                },
            };
            return Ok(TExpr {
                ty: Ty::Unit,
                line,
                kind: TK::Block(vec![
                    TStmt::Bind { name: ha.clone(), value: a },
                    TStmt::Bind { name: hb.clone(), value: b },
                    TStmt::Expr(TExpr {
                        ty: Ty::Unit,
                        line,
                        kind: TK::If { cond: Box::new(same), then: Box::new(unit(line)), els: Box::new(fail) },
                    }),
                ]),
            });
        }
        let simple_builtin: Option<(&'static str, Vec<Ty>, Ty)> = match name {
            // `yield()`: this strand's turn given up, if another is waiting
            // on the worker — for a loop that has no call to be checked at.
            "yield" => Some(("rill_yield", vec![], Ty::Unit)),
            "strbuf" => Some(("rill_strbuf_new", vec![], Ty::StrBuf)),
            "strbuf_add" => Some(("rill_strbuf_add", vec![Ty::StrBuf, Ty::Str], Ty::Unit)),
            "strbuf_reserve" => Some(("rill_strbuf_reserve", vec![Ty::StrBuf, Ty::Int], Ty::Unit)),
            "strbuf_add_sub" => {
                Some(("rill_strbuf_add_sub", vec![Ty::StrBuf, Ty::Str, Ty::Int, Ty::Int], Ty::Unit))
            }
            "strbuf_byte" => Some(("rill_strbuf_byte", vec![Ty::StrBuf, Ty::Int], Ty::Unit)),
            "strbuf_str" => Some(("rill_strbuf_str", vec![Ty::StrBuf], Ty::Str)),
            // Empties a builder and keeps its room, for text built over and
            // over into the same one.
            "strbuf_clear" => Some(("rill_strbuf_clear", vec![Ty::StrBuf], Ty::Unit)),
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
            "str_find_in" => {
                Some(("rill_str_find_in", vec![Ty::Str, Ty::Str, Ty::Int, Ty::Int], Ty::Int))
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
            // The bits of a float as the integer they are, and back: what a
            // file format or a sort key needs, and no instruction at all.
            "float_bits" => Some(("rill.f2i", vec![Ty::Float], Ty::Int)),
            "bits_float" => Some(("rill.i2f", vec![Ty::Int], Ty::Float)),
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
            // The wall clock, in milliseconds since 1970. `now_nanos` is for
            // measuring how long something took; this is for saying when it
            // happened, which is a different question and a different clock.
            "now_millis" => Some(("rill_now_millis", vec![], Ty::Int)),
            // How many bytes of memory this process is holding right now. A
            // server that reports its own weight can be watched without
            // anything else being installed to watch it.
            "rss_bytes" => Some(("rill_rss_bytes", vec![], Ty::Int)),
            // Whether `RILL_TRACE` is set: what a machine asks before it
            // says what it did, and what a program may ask too.
            "tracing" => Some(("rill_tracing", vec![], Ty::Int)),
            // One line of the trace, numbered by the runtime in the order
            // events happened across every strand.
            "trace_line" => Some(("rill_trace_line", vec![Ty::Str], Ty::Unit)),
            // The workers this run has, for deciding how many strands a big
            // piece of work is worth. 1 unless `RILL_THREADS` said otherwise.
            "workers" => Some(("rill_workers", vec![], Ty::Int)),
            "random_i64" => Some(("rill_random_i64", vec![], Ty::Int)),
            // Waiting, without a worker standing still to do it: the strand
            // goes into the same poller a socket would have parked it in.
            "sleep_ms" => Some(("rill_sleep_ms", vec![Ty::Int], Ty::Unit)),
            // Bridges to and from C strings; `to_cstr` allocates, so the
            // caller frees with `free_ptr`.
            "to_cstr" => Some(("rill_to_cstr", vec![Ty::Str], Ty::Ptr)),
            "from_cstr" => Some(("rill_from_cstr", vec![Ty::Ptr], Ty::Str)),
            // The same bridge for bytes rather than text: `from_cstr` stops at
            // the first zero, which is right for a C string and wrong for
            // everything a C library hands back by filling a buffer and saying
            // how many bytes it wrote.
            "str_from_ptr" => Some(("rill_str_from_ptr", vec![Ty::Ptr, Ty::Int], Ty::Str)),
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
            // The same on one address — `127.0.0.1` for this machine alone.
            "tcp_listen_at" => Some(("rill_tcp_listen_at", vec![Ty::Str, Ty::Int], Ty::Int)),
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
            // Which port this end is on: what a program asks after listening
            // on port 0, where the kernel chose the number.
            "sock_port" => Some(("rill_sock_port", vec![Ty::Int], Ty::Int)),
            // Nagle off, for a server whose clock is faster than the round
            // trip: without it every small write waits on the last one's
            // acknowledgement. 1 if the kernel took it.
            "sock_nodelay" => Some(("rill_sock_nodelay", vec![Ty::Int], Ty::Int)),
            "sock_buffered" => Some(("rill_sock_buffered", vec![Ty::Int], Ty::Unit)),
            // A buffer of floats whose elements are whatever the memory
            // held, for a caller about to write every one: `zeros` costs a
            // clearing pass an array operation does not need.
            "buf_float_uninit" => Some(("rill_buf_new_uninit_f64", vec![Ty::Int], Ty::Buf(CTy::F64))),
            // Two lanes at once. `f2_load(b, i)` is `(b[i], b[i + 1])` as one
            // register and `f2_store` puts one back; `fma2` is a fused
            // multiply-add on both lanes.
            "float2" => Some(("rill.f2", vec![Ty::Float, Ty::Float], Ty::Float2)),
            "splat2" => Some(("rill.f2splat", vec![Ty::Float], Ty::Float2)),
            "lane0" => Some(("rill.f2lane0", vec![Ty::Float2], Ty::Float)),
            "lane1" => Some(("rill.f2lane1", vec![Ty::Float2], Ty::Float)),
            "f2_load" => Some(("rill.f2load", vec![Ty::Buf(CTy::F64), Ty::Int], Ty::Float2)),
            "f2_store" => {
                Some(("rill.f2store", vec![Ty::Buf(CTy::F64), Ty::Int, Ty::Float2], Ty::Unit))
            }
            "fma2" => Some(("rill.f2fma", vec![Ty::Float2, Ty::Float2, Ty::Float2], Ty::Float2)),
            // Lane-wise smaller and larger. A NaN loses to a number, the way
            // it does under `<`, so a min or a max kept in lanes answers what
            // the scalar loop would.
            "fmin2" => Some(("rill.f2min", vec![Ty::Float2, Ty::Float2], Ty::Float2)),
            "fmax2" => Some(("rill.f2max", vec![Ty::Float2, Ty::Float2], Ty::Float2)),
            "sock_flush" => Some(("rill_sock_flush", vec![Ty::Int], Ty::Int)),
            "sock_write" => Some(("rill_sock_write", vec![Ty::Int, Ty::Str], Ty::Int)),
            "sock_close" => Some(("rill_sock_close", vec![Ty::Int], Ty::Unit)),
            // Ends a connection without closing the descriptor, which is the
            // only way one strand gets another out of a read: the socket
            // becomes readable-at-end-of-file at once, so the poller wakes
            // whoever was parked on it and their `sock_read` answers "".
            "sock_shutdown" => Some(("rill_sock_shutdown", vec![Ty::Int], Ty::Int)),
            // Parks until the program is told something: 1 for `SIGHUP`, 2
            // for `SIGINT`, 15 for `SIGTERM`. A handler may not run anything
            // a program has written, so it writes a byte to a pipe and the
            // strand parked on that pipe is what wakes.
            "wait_signal" => Some(("rill_wait_signal", vec![], Ty::Int)),
            // Files, as descriptors — the same `Int` a socket is, so that a
            // program holding both is not holding two ideas of what an open
            // thing is. `read_file` and `write_file` stay what they were, for
            // a file a program wants all of at once; these are for the other
            // kind, a file too big to hold or one being appended to for as
            // long as the program runs.
            "file_open" => Some(("rill_file_open", vec![Ty::Str, Ty::Str], Ty::Int)),
            "file_read" => Some(("rill_file_read", vec![Ty::Int, Ty::Int], Ty::Str)),
            // Reads at an offset without moving the descriptor, which is what
            // lets one open file answer readers that are at different places
            // in it while something else appends to the end.
            "file_pread" => Some(("rill_file_pread", vec![Ty::Int, Ty::Int, Ty::Int], Ty::Str)),
            "file_pread_into" => Some((
                "rill_file_pread_into",
                vec![Ty::Int, Ty::Int, Ty::Buf(CTy::U8), Ty::Int, Ty::Int],
                Ty::Int,
            )),
            "file_pwrite" => Some((
                "rill_file_pwrite",
                vec![Ty::Int, Ty::Int, Ty::Buf(CTy::U8), Ty::Int, Ty::Int],
                Ty::Int,
            )),
            "file_lock" => Some(("rill_file_lock", vec![Ty::Int, Ty::Bool], Ty::Bool)),
            "file_unlock" => Some(("rill_file_unlock", vec![Ty::Int], Ty::Bool)),
            "file_write" => Some(("rill_file_write", vec![Ty::Int, Ty::Str], Ty::Int)),
            "file_seek" => Some(("rill_file_seek", vec![Ty::Int, Ty::Int], Ty::Int)),
            "file_pos" => Some(("rill_file_pos", vec![Ty::Int], Ty::Int)),
            "file_size" => Some(("rill_file_size", vec![Ty::Int], Ty::Int)),
            "file_truncate" => Some(("rill_file_truncate", vec![Ty::Int, Ty::Int], Ty::Bool)),
            // The one file call slow enough that a worker must not stand
            // still for it: the strand parks and a helper thread does the
            // waiting, exactly as a strand waiting on a socket parks.
            "file_sync" => Some(("rill_file_sync", vec![Ty::Int], Ty::Bool)),
            "file_close" => Some(("rill_file_close", vec![Ty::Int], Ty::Unit)),
            "file_remove" => Some(("rill_file_remove", vec![Ty::Str], Ty::Bool)),
            "file_rename" => Some(("rill_file_rename", vec![Ty::Str, Ty::Str], Ty::Bool)),
            "file_exists" => Some(("rill_file_exists", vec![Ty::Str], Ty::Bool)),
            "dir_make" => Some(("rill_dir_make", vec![Ty::Str], Ty::Bool)),
            "dir_list" => Some(("rill_dir_list", vec![Ty::Str], Ty::Str)),
            // What makes a created or renamed file's *name* durable, which
            // syncing the file itself does not.
            "dir_sync" => Some(("rill_dir_sync", vec![Ty::Str], Ty::Bool)),
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
            // The `Env` parameter is the compiler's to fill: the call does
            // not write it.
            let visible: Vec<CTy> = sig.params.iter().filter(|p| !matches!(p, CTy::Env)).cloned().collect();
            let params = &visible;
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

        if let Some(sig) = self.asms.get(name).cloned() {
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
                targs.push(self.arg_for(a, &pt.rill_ty(), env, line, "assembly argument")?);
            }
            // The same node a C call makes: codegen tells them apart by which
            // table the name is in, and everything after that — narrowing an
            // argument, widening a result — is identical.
            return Ok(TExpr {
                ty: ret,
                line,
                kind: TK::CallExtern { name: name.to_string(), args: targs },
            });
        }

        // A method: by its qualified name, `Describe.describe`, or by its
        // bare one — which, where two traits have it, is settled by the type
        // it is called on, the argument standing where the trait's type does.
        let candidates: Vec<(usize, usize)> = match self.method_index.get(name) {
            Some(&m) => vec![m],
            None => self.method_names.get(name).cloned().unwrap_or_default(),
        };
        if !candidates.is_empty() {
            let fitting: Vec<(usize, usize)> = candidates
                .iter()
                .copied()
                .filter(|&(t, m)| self.traits[t].1[m].params.len() == args.len())
                .collect();
            if fitting.is_empty() {
                let (t, m) = candidates[0];
                return Err(TypeError::new(
                    line,
                    format!(
                        "method `{name}` of trait `{}` takes {} argument(s), but {} given",
                        self.traits[t].0,
                        self.traits[t].1[m].params.len(),
                        args.len()
                    ),
                ));
            }
            let mut targs = Vec::new();
            for a in args {
                targs.push(self.infer(a, env)?);
            }
            let (tid, mid) = if fitting.len() == 1 {
                fitting[0]
            } else {
                self.pick_method(name, &fitting, &targs, line)?
            };
            let sig = self.traits[tid].1[mid].clone();
            let recv = self.fresh();
            let inst = [recv.clone()];
            for (at, pt) in targs.iter().zip(&sig.params) {
                self.unify(&subst_gen(pt, &inst), &at.ty, line, "method argument")?;
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
            None if self.naked_asms.contains_key(name) => {
                return Err(TypeError::new(
                    line,
                    format!(
                        "`{name}` is a naked function and cannot be called: it is entered \
                         by an interrupt, a vector or a jump, not by a caller. Its address \
                         is what a program has use for."
                    ),
                ));
            }
            None => return Err(TypeError::new(line, self.unknown_name(name, "function"))),
        };
        let (params, ret, subst) = self.sig_for_use(idx);
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
        // `?` is written for a `Result`, and the two arms it becomes fit one.
        // On an `Option` the same two arms read `Some` and `None`, the `None`
        // going out as itself. Anything else gets told what it is rather
        // than being shown a pattern it never wrote.
        let try_match = is_try_match(arms);
        let on_option: Vec<Arm>;
        let arms: &[Arm] = if try_match {
            match self.resolve(&scrut.ty) {
                Ty::Adt(i, _) if self.adts[i].name == "Option" => {
                    on_option = option_try_arms(arms);
                    &on_option
                }
                Ty::Adt(i, _) if self.adts[i].name == "Result" => arms,
                Ty::Var(_) => arms,
                other => {
                    return Err(TypeError::new(
                        line,
                        format!(
                            "`?` takes the value out of a `Result` or an `Option`, and this \
                             is `{}` — `match` on it",
                            self.type_name(&other)
                        ),
                    ))
                }
            }
        } else {
            arms
        };
        let result_ty = self.fresh();
        let mut tarms = Vec::new();
        // Rows seen so far, for the reachability check.
        let mut matrix: Vec<Vec<Pattern>> = Vec::new();

        for arm in arms {
            let mut arm_env = env.clone();
            let mut bound = HashSet::new();
            self.check_pattern(&arm.pattern, &scrut.ty, arm.line, &mut arm_env, &mut bound)?;

            if !self.useful(&matrix, &[arm.pattern.clone()], &[scrut.ty.clone()]) {
                // A machine's "any other signal" arm is written for every
                // state; a state whose arms answer every signal has no
                // other, and the arm is simply left out.
                if matches!(&arm.pattern, Pattern::Var(n) if n == "machine.other") {
                    continue;
                }
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

            // The `Err` arm of a `?` in a function whose error type is
            // another: the error it met goes out inside the one constructor
            // of the function's that takes it.
            let wrapped: Expr;
            let arm_body: &Expr = if try_match && tarms.len() == 1 {
                match self.wrap_carried_error(&scrut.ty, &result_ty, arm.line)? {
                    Some(e) => {
                        wrapped = e;
                        &wrapped
                    }
                    None => &arm.body,
                }
            } else {
                &arm.body
            };
            // A `machine`'s match on its state: the arms are its states,
            // and the signal match inside each wants to know which.
            let state_here = match scrutinee {
                Expr::Var(n, _) if n == "machine.state" => Some((self.machine_name(), show_pattern(&arm.pattern))),
                _ => None,
            };
            let outer_state = if state_here.is_some() { std::mem::replace(&mut self.machine_state, state_here) } else { None };
            let body = self.infer(arm_body, &mut arm_env);
            if outer_state.is_some() || matches!(scrutinee, Expr::Var(n, _) if n == "machine.state") {
                self.machine_state = outer_state;
            }
            let body = body?;
            // The one `match` a program did not write is the one `?` becomes,
            // and its arms disagreeing means only one thing: the `?` was
            // written where the block's value is not the function's. Say that
            // rather than showing the match nobody typed.
            self.unify(&result_ty, &body.ty, arm.line, "match arms").map_err(|e| {
                let named = |t: &Ty| match self.resolve(t) {
                    Ty::Adt(i, _) => self.adts[i].name.clone(),
                    _ => String::new(),
                };
                if try_match && named(&scrut.ty) == "Option" && named(&result_ty) == "Result" {
                    // The one crossing `?` does not make: an absence is not
                    // an error until the program says which.
                    TypeError::new(
                        arm.line,
                        "`?` on an `Option` leaves the function with `None`, and this function \
                         answers a `Result`. Say what the absence is an error of: `ok_or(o, e)?`"
                            .to_string(),
                    )
                } else if try_match {
                    TypeError::new(
                        arm.line,
                        format!(
                            "`?` leaves the function with an `Err` — or a `None` — so the block \
                             it is in has to be what the function returns — and this one is \
                             worth `{}` instead. Take the `?` out to a block that is.",
                            self.type_name(&result_ty)
                        ),
                    )
                } else {
                    e
                }
            })?;
            tarms.push(TArm { pattern: arm.pattern.clone(), guard, body, line: arm.line });
        }

        // A machine's match on its signal: what its arms do not name is
        // dropped, and that is worth a word — the report SDL tools give of
        // a state's implicit consumption.
        if let (Expr::Var(n, _), Some((machine, state))) = (scrutinee, self.machine_state.clone()) {
            if n == "machine.sig" {
                // what the arms name, guarded or not — a `_ -> stay`
                // answers everything and names nothing, and is said so
                let all = self.ctor_names_of(&scrut.ty);
                let mut v = Vec::new();
                for a in arms.iter().filter(|a| !matches!(&a.pattern, Pattern::Var(n) if n == "machine.other")) {
                    ctor_names(&a.pattern, &mut v);
                }
                let catch_all = arms.iter().any(|a| a.guard.is_none() && matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)) && !matches!(&a.pattern, Pattern::Var(n) if n == "machine.other"));
                let named_ctors: Vec<String> = all.iter().filter(|c| v.contains(c)).cloned().collect();
                self.machine_states.push((machine, line, state, named_ctors, all, catch_all));
            }
        }

        if let Some(w) = self.witness(&matrix, &[scrut.ty.clone()]) {
            // a `machine` is a match on its state, and a state it does not
            // have is what the gap means
            let msg = match scrutinee {
                Expr::Var(n, _) if n == "machine.state" => {
                    format!("the machine has no `state {}`: every state of its type answers something", show_pattern(&w[0]))
                }
                _ => format!("match is not exhaustive; no arm matches `{}`", show_pattern(&w[0])),
            };
            return Err(TypeError::new(line, msg));
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
            // The name is the whole of what the inner pattern matched, so it
            // has that pattern's own type, and both are bound.
            Pattern::As(n, inner, aline) => {
                if !bound.insert(n.clone()) {
                    return Err(TypeError::new(
                        *aline,
                        format!("name `{n}` is bound twice in this pattern"),
                    ));
                }
                env.insert(n.clone(), ty.clone());
                self.check_pattern(inner, ty, line, env, bound)
            }
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
        match peel(p) {
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
            let mut new = match (peel(&row[0]), h) {
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
            .filter(|row| matches!(peel(&row[0]), Pattern::Wildcard | Pattern::Var(_)))
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
    /// The name of the machine being inferred: the function's, without
    /// the `.run` of a loop.
    fn machine_name(&self) -> String {
        self.cur_fn_name.trim_end_matches(".run").to_string()
    }

    /// The constructors of a type, by name.
    fn ctor_names_of(&self, ty: &Ty) -> Vec<String> {
        match self.resolve(ty) {
            Ty::Adt(adt, _) => self.adts[adt].ctors.iter().map(|c| c.name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// The report of what a machine's states drop, once every state has
    /// been checked: a signal some state answers and this one does not is
    /// said per state; one no state answers is said once, since it is
    /// either not an input of this machine or a mistake in every state.
    fn report_machines(&mut self) {
        let mut machines: Vec<String> = Vec::new();
        for (m, ..) in &self.machine_states {
            if !machines.contains(m) {
                machines.push(m.clone());
            }
        }
        for m in machines {
            let states: Vec<&(String, usize, String, Vec<String>, Vec<String>, bool)> =
                self.machine_states.iter().filter(|s| s.0 == m).collect();
            let all: Vec<String> = states.first().map(|s| s.4.clone()).unwrap_or_default();
            let mut anywhere: Vec<String> = Vec::new();
            for s in &states {
                for c in &s.3 {
                    if !anywhere.contains(c) {
                        anywhere.push(c.clone());
                    }
                }
            }
            let mut notes = Vec::new();
            for s in &states {
                let dropped: Vec<String> = if s.5 { Vec::new() } else { anywhere.iter().filter(|c| !s.3.contains(c)).cloned().collect() };
                if !dropped.is_empty() {
                    notes.push((s.1, format!(
                        "`{m}` in state `{}` drops {}: a signal no arm names is consumed and the state kept. Answer it, `save` it, or say so with `_ -> stay`",
                        s.2, backticked(&dropped)
                    )));
                }
            }
            let never: Vec<String> = all.iter().filter(|c| !anywhere.contains(c)).cloned().collect();
            if !never.is_empty() {
                let first = states.iter().map(|s| s.1).min().unwrap_or(0);
                notes.push((first, format!(
                    "`{m}` answers {} in no state: not an input of this machine, or a signal it was meant to handle",
                    backticked(&never)
                )));
            }
            self.warnings.extend(notes);
        }
    }

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

/// What a pattern tests for, with any `@` names taken off the front.
///
/// A name is a binding and not a test: `whole @ Circle(r)` matches exactly what
/// `Circle(r)` matches. Everything that asks a pattern what it tests for has to
/// ask through this, and the reason is sharp — `head_of` has a `_ => None`
/// arm, and an `As` reaching it would be read as "tests for nothing", which is
/// what a wildcard says. The arm would then appear to match everything, and a
/// `match` that is not exhaustive would compile.
fn peel(p: &Pattern) -> &Pattern {
    match p {
        Pattern::As(_, inner, _) => peel(inner),
        other => other,
    }
}

/// The constructors a pattern names at its top: one, or the alternatives
/// of an `|`.
fn ctor_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Ctor { name, .. } => out.push(name.clone()),
        Pattern::Or(alts, _) => alts.iter().for_each(|a| ctor_names(a, out)),
        Pattern::As(_, inner, _) => ctor_names(inner, out),
        _ => {}
    }
}

fn backticked(names: &[String]) -> String {
    names.iter().map(|d| format!("`{d}`")).collect::<Vec<_>>().join(", ")
}

fn show_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Or(..) => unreachable!("`lift` expands `|` patterns"),
        Pattern::As(n, inner, _) => format!("{n} @ {}", show_pattern(inner)),
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

impl Checker {
    /// What the `Err` arm of a `?` carries out when the error it met is not
    /// the one the function answers with: `Err(Io(try.e))` for an `IoErr`
    /// met in a function that answers `Result(_, AppErr)` and has a
    /// constructor `Io(IoErr)` — the match a program would have written.
    /// The function's error type is what the match is worth where that is
    /// already known, else what the function declared. `None` where the
    /// two agree, or nothing says what the function answers; an error where
    /// nothing of the function's takes what was met, or two things do.
    fn wrap_carried_error(
        &mut self,
        scrut_ty: &Ty,
        result_ty: &Ty,
        line: usize,
    ) -> Result<Option<Expr>, TypeError> {
        let err_of = |this: &Self, t: &Ty| match this.resolve(t) {
            Ty::Adt(i, args) if this.adts[i].name == "Result" && args.len() == 2 => {
                Some(this.resolve(&args[1]))
            }
            _ => None,
        };
        let Some(met) = err_of(self, scrut_ty) else { return Ok(None) };
        let declared = self.expected_ret.last().cloned().and_then(|r| err_of(self, &r));
        // What the function declared first: the match's own type can still
        // be open inside — `W(_)` for a declared `W(Int)` — and a
        // constructor of a generic error type then took nothing
        // (2026-09-25).
        let target = match &declared {
            Some(t @ Ty::Adt(..)) => t.clone(),
            _ => match err_of(self, result_ty) {
                Some(t @ Ty::Adt(..)) => t,
                _ => return Ok(None),
            },
        };
        // Where the function's return type is not written, the error type
        // is whatever the block settled on first, which is worth saying.
        let unwritten = if matches!(declared, Some(Ty::Adt(..))) {
            ""
        } else {
            " (the function's return type is not written, so this is the error type its \
             block settled on; `-> Result(_, T)` would say which to wrap in)"
        };
        if matches!(met, Ty::Var(_)) || met == target {
            return Ok(None);
        }
        let Ty::Adt(adt, targs) = &target else { unreachable!("matched above") };
        let takes: Vec<&CtorSig> = self.adts[*adt]
            .ctors
            .iter()
            .filter(|c| c.fields.len() == 1 && self.same_type(&subst_gen(&c.fields[0], targs), &met))
            .collect();
        match takes.as_slice() {
            [ctor] => {
                let e = Expr::Var("try.e".to_string(), line);
                let inner = Expr::Call {
                    name: ctor.name.clone(),
                    args: vec![e],
                    line,
                    piped: false,
                    stacked: false,
                };
                Ok(Some(Expr::Call {
                    name: "Err".to_string(),
                    args: vec![inner],
                    line,
                    piped: false,
                    stacked: false,
                }))
            }
            [] => Err(TypeError::new(
                line,
                format!(
                    "`?` met an `{}` in a function that answers `Result(_, {})`, and no \
                     constructor of `{}` takes one — `map_err(r, f)?` says how it crosses{unwritten}",
                    self.type_name(&met),
                    self.type_name(&target),
                    self.adts[*adt].name
                ),
            )),
            two => {
                let names: Vec<String> = two.iter().map(|c| format!("`{}`", c.name)).collect();
                Err(TypeError::new(
                    line,
                    format!(
                        "`?` met an `{}` in a function that answers `Result(_, {})`, and {} \
                         all take one, so it cannot say which — `map_err(r, f)?` does",
                        self.type_name(&met),
                        self.type_name(&target),
                        names.join(", ")
                    ),
                ))
            }
        }
    }

    /// Whether two types are the same once resolved, variables included —
    /// a comparison, not a unification: nothing is bound by asking.
    fn same_type(&self, a: &Ty, b: &Ty) -> bool {
        match (self.resolve(a), self.resolve(b)) {
            (Ty::Adt(i, x), Ty::Adt(j, y)) => {
                i == j && x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| self.same_type(p, q))
            }
            (Ty::Fn(p, r), Ty::Fn(q, s)) => {
                p.len() == q.len()
                    && p.iter().zip(&q).all(|(a, b)| self.same_type(a, b))
                    && self.same_type(&r, &s)
            }
            (Ty::Chan(a), Ty::Chan(b)) => self.same_type(&a, &b),
            (Ty::Map(k, v), Ty::Map(l, w)) => self.same_type(&k, &l) && self.same_type(&v, &w),
            (a, b) => a == b,
        }
    }
}

/// The fields of a trait's object type: each method as a function of
/// everything but the value. A method that does not take the trait's type
/// first, takes it again, or answers with it cannot be one — an object does
/// not know what another one of the type is, nor how to be one.
fn object_fields(methods: &[MethodInfo]) -> Result<Vec<Ty>, String> {
    let mut fields = Vec::new();
    for m in methods {
        if m.params.first() != Some(&Ty::Gen(0)) {
            return Err(format!("`{}` does not take the type as its first argument", m.name));
        }
        if m.params[1..].iter().any(mentions_gen) {
            return Err(format!(
                "`{}` takes a second one of the type, and an object does not know what the other is",
                m.name
            ));
        }
        if mentions_gen(&m.ret) {
            return Err(format!("`{}` answers with the type, which an object cannot be", m.name));
        }
        fields.push(Ty::Fn(m.params[1..].to_vec(), Box::new(m.ret.clone())));
    }
    Ok(fields)
}

/// Whether a type has a type parameter anywhere in it.
fn mentions_gen(t: &Ty) -> bool {
    match t {
        Ty::Gen(_) => true,
        Ty::Adt(_, args) => args.iter().any(mentions_gen),
        Ty::Fn(ps, r) => ps.iter().any(mentions_gen) || mentions_gen(r),
        Ty::Chan(e) => mentions_gen(e),
        Ty::Map(k, v) => mentions_gen(k) || mentions_gen(v),
        _ => false,
    }
}

/// The two arms of a `?` read for an `Option`: `Some` where `Ok` was, and
/// `None` carried out as itself where `Err(e)` was.
fn option_try_arms(arms: &[Arm]) -> Vec<Arm> {
    let mut out: Vec<Arm> = arms.to_vec();
    if let Pattern::Ctor { name, .. } = &mut out[0].pattern {
        *name = "Some".to_string();
    }
    let line = out[1].line;
    out[1].pattern = Pattern::Ctor { name: "None".to_string(), args: vec![], line };
    out[1].body = Expr::Var("None".to_string(), line);
    out
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
/// Checks an `asm fn`'s constraint string against its signature.
///
/// LLVM does not report a mismatch here — it asserts, and the compiler dies
/// with it, taking the line number with it. So the count is done first: an
/// output for a result and none for `Unit`, one input per parameter, and as
/// many clobbers as the author likes.
///
/// The grammar is LLVM's, not ours, and only the first character of an item is
/// read: `=` and `+` open an output, `~` a clobber, anything else an input.
/// What follows is passed through untouched, which is what lets a register be
/// pinned (`{dx}`) or an operand tied to an earlier one (`0`).
fn check_constraints(
    text: &str,
    outs: usize,
    ins: usize,
    line: usize,
    name: &str,
) -> Result<(), TypeError> {
    let (mut saw_out, mut saw_in) = (0usize, 0usize);
    if !text.trim().is_empty() {
        for item in text.split(',') {
            let item = item.trim();
            if item.is_empty() {
                return Err(TypeError::new(
                    line,
                    format!("`{name}`: an empty constraint between two commas"),
                ));
            }
            match item.as_bytes()[0] {
                b'=' | b'+' => saw_out += 1,
                b'~' => {}
                _ => saw_in += 1,
            }
        }
    }
    if saw_out != outs {
        let wanted = if outs == 1 {
            "one output constraint (`=r`, or a register to pin it to)"
        } else {
            "no output constraint"
        };
        return Err(TypeError::new(
            line,
            format!(
                "`{name}`: the constraint string has {saw_out} output(s), but a signature \
                 returning {} wants {wanted}",
                if outs == 1 { "a value" } else { "nothing" }
            ),
        ));
    }
    if saw_in != ins {
        return Err(TypeError::new(
            line,
            format!(
                "`{name}`: the constraint string has {saw_in} input(s), but the function \
                 takes {ins} parameter(s); every parameter needs one, in order"
            ),
        ));
    }
    Ok(())
}

fn resolve_c_type(
    tr: &TypeRef,
    adt_index: &HashMap<String, usize>,
    adts: &[AdtDef],
    what: &str,
) -> Result<CTy, TypeError> {
    if tr.name == "->" {
        return Err(TypeError::new(
            tr.line,
            format!(
                "{what}: a callback C calls is written `Fn(I32) -> I32`; `(A) -> B` is the type \
                 of a Rill function, which C cannot be handed"
            ),
        ));
    }
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
    if tr.name == "Env" {
        if !tr.args.is_empty() || tr.ret.is_some() {
            return Err(TypeError::new(tr.line, format!("{what}: `Env` takes nothing")));
        }
        return Ok(CTy::Env);
    }
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
        if params.iter().filter(|p| matches!(p, CTy::Env)).count() > 1 {
            return Err(TypeError::new(
                tr.line,
                format!("{what}: a callback carries its environment in one `Env` slot, not two"),
            ));
        }
        if matches!(ret, CTy::Env) {
            return Err(TypeError::new(tr.line, format!("{what}: a callback cannot answer with `Env`")));
        }
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
        Ty::Float2 => "Float2".into(),
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
                // a tuple is written as one, `(a, b)`, never as `Tuple2`
                if adts[*i].name.starts_with("Tuple") && adts[*i].name[5..].parse::<usize>().is_ok() {
                    format!("({})", args.join(", "))
                } else {
                    format!("{}({})", adts[*i].name, args.join(", "))
                }
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
pub fn show_signature(f: &TFn, adts: &[AdtDef], traits: &[TraitInfo]) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(n, t)| format!("{n}: {}", show_type(t, adts)))
        .collect();
    let mut s = format!("fn {}({}) -> {}", f.name, params.join(", "), show_type(&f.ret, adts));
    let mut needs: Vec<String> = Vec::new();
    for (g, c, _) in &f.constraints {
        let what = match c {
            Constraint::Numeric => "a number".to_string(),
            Constraint::Addable => "addable".to_string(),
            Constraint::Ordered => "ordered".to_string(),
            Constraint::Basic => "a value type".to_string(),
            Constraint::Impl(t) => format!("`{}`", traits[*t].name),
        };
        let name = show_type(&Ty::Gen(*g), adts);
        let one = match c {
            Constraint::Impl(_) => format!("{name} implements {what}"),
            _ => format!("{name} is {what}"),
        };
        if !needs.contains(&one) {
            needs.push(one);
        }
    }
    if !needs.is_empty() {
        s.push_str(&format!("    # where {}", needs.join(", ")));
    }
    s
}

/// How to speak about the thing a `.x` was read from, for an error that has to
/// tell somebody which annotation to write.
fn describe(e: &TExpr) -> String {
    match &e.kind {
        TK::Local(n) => format!("`{n}`"),
        _ => "this".to_string(),
    }
}

/// The distinct field names a group of obligations asks of one value, in the
/// order they were asked.
fn fields_asked(obs: &[FieldObligation]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for ob in obs {
        if !names.contains(&ob.name.as_str()) {
            names.push(&ob.name);
        }
    }
    names
}

/// Applies `f` to every immediate sub-expression of a checked expression.
/// Written here rather than beside its one user because more than one thing
/// wants to walk a body now: codegen, and whatever is answering an editor.
pub fn walk_texpr<'a>(e: &'a TExpr, f: &mut impl FnMut(&'a TExpr)) {
    match &e.kind {
        TK::FieldNamed { base, .. } => f(base),
        TK::UpdateNamed { base, fields } => {
            f(base);
            for (_, v, _) in fields {
                f(v);
            }
        }
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
        | TK::Inspect(x)
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
        | TK::ChanOp { ch, .. }
        | TK::ChanTaken { ch }
        | TK::BufLen { buf: ch }
        | TK::Unique { val: ch }
        | TK::BufPtr { buf: ch }
        | TK::MapLen { map: ch }
        | TK::MapClear { map: ch }
        | TK::FieldGet { base: ch, .. } => f(ch),
        TK::BufNew { count, .. } => f(count),
        TK::BufGet { buf: a, idx: b, .. }
        | TK::MapFind { map: a, key: b }
        | TK::MapDel { map: a, key: b }
        | TK::MapNext { map: a, from: b }
        | TK::MapKeyAt { map: a, idx: b }
        | TK::MapValAt { map: a, idx: b }
        | TK::Binary { lhs: a, rhs: b, .. } => {
            f(a);
            f(b);
        }
        TK::BufSet { buf: a, idx: b, val: c, .. } | TK::MapSet { map: a, key: b, val: c } => {
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
                    TSelectOp::Timeout { ms } => f(ms),
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
    matches!(name, "Int" | "Float" | "Float2" | "Bool" | "Str" | "StrBuf" | "Unit" | "Ptr")
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
            | "assert"
            | "assert_eq"
            | "yield"
            | "print"
            | "channel"
            | "mailbox"
            | "send"
            | "recv"
            | "close"
            | "closed"
            | "chan_wait"
            | "chan_taken"
            | "strbuf"
            | "strbuf_add"
            | "strbuf_byte"
            | "strbuf_str"
            | "strbuf_clear"
            | "strbuf_add_sub"
            | "strbuf_reserve"
            | "str_from_buf"
            | "str_len"
            | "int_to_str"
            | "float_to_str"
            | "bool_to_str"
            | "show"
            | "machine_show"
            | "str_get"
            | "str_sub"
            | "str_find"
            | "str_find_from"
            | "str_find_in"
            | "str_cmp"
            | "str_to_int"
            | "str_to_float"
            | "chr"
            | "now_nanos"
            | "now_millis"
            | "rss_bytes"
            | "tracing"
            | "trace_line"
            | "chan_id"
            | "workers"
            | "random_i64"
            | "sleep_ms"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bit_not"
            | "shl"
            | "shr"
            | "to_float"
            | "to_int"
            | "float_bits"
            | "bits_float"
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
            | "str_from_ptr"
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
            | "buf_get_unchecked"
            | "buf_set_unchecked"
            | "buf_set"
            | "buf_len"
            | "buf_copy"
            | "buf_cmp"
            | "buf_float_uninit"
            | "unique"
            | "list_sort"
            | "buf_sort"
            | "buf_sort_range"
            | "buf_argsort"
            | "buf_select_nth"
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
            | "tcp_listen_at"
            | "tcp_accept"
            | "tcp_connect"
            | "sock_read"
            | "sock_wait"
            | "sock_peer"
            | "sock_port"
            | "sock_nodelay"
            | "sock_buffered"
            | "sock_flush"
            | "sock_write"
            | "sock_close"
            | "sock_shutdown"
            | "wait_signal"
            | "file_open"
            | "file_read"
            | "file_pread"
            | "file_pread_into"
            | "file_pwrite"
            | "file_lock"
            | "file_unlock"
            | "file_write"
            | "file_seek"
            | "file_pos"
            | "file_size"
            | "file_truncate"
            | "file_sync"
            | "file_close"
            | "file_remove"
            | "file_rename"
            | "file_exists"
            | "dir_make"
            | "dir_list"
            | "dir_sync"
    )
}

/// Every type constructor application inside a type, the outer ones first.
fn adts_in(t: &Ty, out: &mut Vec<Ty>) {
    match t {
        Ty::Adt(_, args) => {
            out.push(t.clone());
            for a in args {
                adts_in(a, out);
            }
        }
        Ty::Fn(params, ret) => {
            for p in params {
                adts_in(p, out);
            }
            adts_in(ret, out);
        }
        Ty::Chan(e) => adts_in(e, out),
        Ty::Map(k, v) => {
            adts_in(k, out);
            adts_in(v, out);
        }
        _ => {}
    }
}

/// How many nodes a type has: the measure of a type growing.
pub fn ty_size(t: &Ty) -> usize {
    1 + match t {
        Ty::Adt(_, args) => args.iter().map(ty_size).sum(),
        Ty::Fn(params, ret) => params.iter().map(ty_size).sum::<usize>() + ty_size(ret),
        Ty::Chan(e) => ty_size(e),
        Ty::Map(k, v) => ty_size(k) + ty_size(v),
        _ => 0,
    }
}

/// The lowercase names in an annotation, in order of first appearance: the
/// type variables of a signature. The order is the one `generalize` walks
/// a type in, so a declared scheme and the inferred one number them alike.
fn collect_type_vars(tr: &TypeRef, out: &mut Vec<String>) {
    let last = tr.name.rsplit('.').next().unwrap_or(&tr.name);
    if last.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && !out.contains(&tr.name) {
        out.push(tr.name.clone());
    }
    for a in &tr.args {
        collect_type_vars(a, out);
    }
    if let Some(r) = &tr.ret {
        collect_type_vars(r, out);
    }
}

fn resolve_type_ref(
    tr: &TypeRef,
    adt_index: &HashMap<String, usize>,
    adts: &[AdtDef],
    params: Option<&HashMap<&str, u32>>,
) -> Result<Ty, TypeError> {
    // Lowercase: a type parameter (only valid inside a generic `type` def).
    // A type from an aliased module is `alias.Name`, and the alias is
    // lowercase; it is the name after the dot that says what this is.
    let last = tr.name.rsplit('.').next().unwrap_or(&tr.name);
    if last.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
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
    // `(A, B) -> C`: a function's type, as a lambda has.
    if tr.name == "->" {
        let mut ps = Vec::new();
        for a in &tr.args {
            ps.push(resolve_type_ref(a, adt_index, adts, params)?);
        }
        let r = tr.ret.as_ref().expect("an arrow has a result");
        let ret = resolve_type_ref(r, adt_index, adts, params)?;
        return Ok(Ty::Fn(ps, Box::new(ret)));
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
            "Float2" => Ty::Float2,
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
/// The `o` of every `o.g()` in `e`: a call through a field of a value is a
/// use of the value, which `free_vars` leaves out so that a lifted
/// function's name (`main.go`) does not make its parent a dependency. What
/// a lambda or a strand captures has to have it, or `\ -> o.g()` closed over
/// nothing and the compiler died looking `o` up (2026-09-25). The list is
/// filtered against the enclosing scope, so a function's name that comes
/// out of here is dropped there.
fn call_heads(e: &Expr, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    if let Expr::Call { name, .. } = e {
        if let Some((head, _)) = name.split_once('.') {
            if seen.insert(head.to_string()) {
                out.push(head.to_string());
            }
        }
    }
    rill_syntax::lift::each_child_all(e, &mut |c| call_heads(c, out, seen));
}

fn free_vars(e: &Expr, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    let mut name_of = |n: &String, out: &mut Vec<String>| {
        if seen.insert(n.clone()) {
            out.push(n.clone());
        }
        // `p.x` arrives here as one name, because the lexer cannot tell a
        // field from a module — see `infer_var`. What a lambda captures is
        // `p`, and naming both is safe: the list is filtered against the
        // enclosing scope, so a `geo.area` adds a `geo` that is not there and
        // is dropped. Without this a lambda that reads a field of something
        // it closes over captured nothing at all.
        if let Some((head, _)) = n.split_once('.') {
            let head = head.to_string();
            if seen.insert(head.clone()) {
                out.push(head);
            }
        }
    };
    match e {
        Expr::Var(n, _) => name_of(n, out),
        // A call's name is the function and nothing else: `main.go` is the
        // loop lifted out of `main`, not a field of it, and naming `main`
        // here made every lifted function a dependency of its parent — so
        // the two were inferred as one group, parent first, and a buffer
        // the loop handed back was taken for a string by the code after it.
        Expr::Call { name, args, .. } => {
            drop(name_of);
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
        Expr::If { cond, then, els, .. } => {
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
        Expr::Update { base, fields, .. } => {
            free_vars(base, out, seen);
            for (_, v, _) in fields {
                free_vars(v, out, seen);
            }
        }
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
        Expr::While { cond, acc, body, .. } => {
            free_vars(cond, out, seen);
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
                    SelectOp::Timeout { ms } => free_vars(ms, out, seen),
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
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Unit => {}
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
        // A base whose type nothing else settles is named by the field: the
        // one type with an `x` is `Point`. Two with an `x` cannot be told
        // apart, and that is the one case an annotation is still for.
        let t = check_src(&format!("{point}fn f(p) = p.x\n\nfn main() = println(f(P(1, 2)))\n"))
            .unwrap();
        assert_eq!(sig(&t, "f").ret, Ty::Int);
        let err = check_src(&format!(
            "{point}type Vec3\n  V(x: Int, y: Int, z: Int)\n\nfn f(p) = p.x\n\nfn main() = println(1)\n"
        ))
        .unwrap_err();
        assert!(err.msg.contains("annotate it") && err.msg.contains("`Point` and `Vec3`"), "{}", err.msg);
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
        // On an `Option`, in a function that answers one, it is the same
        // two arms reading `Some` and `None`.
        let t = check_std("fn first(l) =\n  x = nth(l, 0)?\n  Some(x)\n\nfn main() = println(1)\n")
            .unwrap();
        assert!(matches!(&sig(&t, "first").ret, Ty::Adt(i, _) if t.adts[*i].name == "Option"));
        // And on anything else, it says what it takes the value out of.
        let err = check_std("fn first(n: Int) =\n  x = n?\n  Some(x)\n\nfn main() = println(1)\n")
            .unwrap_err();
        assert!(err.msg.contains("takes the value out of a `Result` or an `Option`"), "{}", err.msg);
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

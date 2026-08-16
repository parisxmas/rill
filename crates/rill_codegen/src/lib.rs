//! rill_codegen: compiles type-checked Rill to native object code via LLVM.
//!
//! Generic functions are **monomorphized**: starting from `main`, every
//! `CallFn` with a concrete instantiation requests an instance, and instances
//! are generated from a work queue. Nothing is boxed; `List(Int)` and
//! `List(Str)` get separate layouts and separate specialized functions.
//!
//! Value representation (all concrete types):
//! - Int -> i64, Float -> f64, Bool -> i1, Unit -> no value
//! - Str -> { ptr, i64 } by value
//! - ADT value -> pointer to heap box `{ i64 tag, fields... }`
//! - Function value -> closure pair `{ code ptr, env ptr }`; the code takes
//!   the env pointer as a hidden first parameter. Named functions used as
//!   values get a shim that ignores the env.
//!
//! Memory (M3): compile-time reference counting, Perceus-style ownership.
//! - Heap objects carry an `i64` refcount at offset 0. ADT boxes are
//!   `{ rc, tag, fields... }`; closure envs are `{ rc, drop_fn, captures... }`.
//! - Ownership convention: every expression yields an *owned* reference
//!   (locals are retained on use); call/constructor arguments are consumed;
//!   functions release their parameters and top-level bindings on exit;
//!   match scrutinees are released at the merge point; values discarded in
//!   statement position are released immediately.
//! - Releases go through compiler-generated, type-specific `drop$T` functions
//!   (ADTs, statically dispatched) or through the env's stored drop pointer
//!   (closures, dynamically dispatched).
//! - Strings are static data and are not counted.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use inkwell::basic_block::BasicBlock;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFlagsConstants, DISubprogram, DWARFEmissionKind,
    DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, StructType};
use inkwell::values::BasicValue;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, CallSiteValue, FunctionValue, IntValue, PointerValue,
};
use inkwell::{FloatPredicate, IntPredicate, OptimizationLevel};

use rill_syntax::{BinOp, Pattern};
use rill_types::{
    CTy, Constraint, TArm, TExpr, TFn, TK, TModule, TSelectArm, TSelectOp, TStmt, Ty,
};

#[derive(Debug, Clone)]
pub struct CodegenError {
    pub line: usize,
    pub msg: String,
}

impl CodegenError {
    fn new(line: usize, msg: impl Into<String>) -> Self {
        CodegenError { line, msg: msg.into() }
    }
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "error at line {}: {}", self.line, self.msg)
        } else {
            write!(f, "error: {}", self.msg)
        }
    }
}

impl std::error::Error for CodegenError {}

/// Compiles to an object file. With `debug` the O3 pass pipeline and
/// aggressive instruction selection are skipped, so the generated code stays
/// close to the source for stepping and inspection.
/// Compiles to an object file. `debug` skips optimization and emits DWARF
/// line information; `source` is the file the line numbers refer to.
pub fn compile_to_object(
    tmod: &TModule,
    out_path: &Path,
    debug: bool,
    parallel: bool,
    source: &Path,
    target: Option<&str>,
) -> Result<(), CodegenError> {
    let context = Context::create();
    check_exports(tmod, target)?;
    let cg = Codegen::build_module(&context, tmod, parallel, debug.then_some(source), target)?;
    if cg.wasm {
        cg.check_wasm_supported()?;
    }
    cg.write_object(out_path, !debug, target)
}

/// What `export fn` promises: a name the host can call, with arguments it can
/// produce. That constrains the signature to the types that cross the wasm
/// boundary as plain values — Int, Float, Bool, Ptr — and rules out type
/// parameters, because an export is one function, not a family waiting for a
/// call site to pick the types. And it is a wasm idea: on a native target
/// nothing calls in, so the word is refused rather than silently meaning
/// nothing.
fn check_exports(tmod: &TModule, target: Option<&str>) -> Result<(), CodegenError> {
    let exports: Vec<&TFn> = tmod.fns.iter().filter(|f| f.exported).collect();
    if exports.is_empty() {
        return Ok(());
    }
    if !target.is_some_and(|t| t.starts_with("wasm")) {
        return Err(CodegenError::new(
            exports[0].line,
            format!(
                "`export fn {}` needs a wasm target: exports are functions the \
                 embedding host calls, and a native executable has no host. \
                 Build with `--target wasm32-wasip1`.",
                exports[0].name
            ),
        ));
    }
    let scalar = |t: &Ty| matches!(t, Ty::Int | Ty::Float | Ty::Bool | Ty::Ptr);
    for f in exports {
        if f.ngen > 0 {
            return Err(CodegenError::new(
                f.line,
                format!(
                    "`export fn {}` is generic: an export is a single entry point, \
                     so every parameter type must be concrete",
                    f.name
                ),
            ));
        }
        for (pname, pty) in &f.params {
            if !scalar(pty) {
                return Err(CodegenError::new(
                    f.line,
                    format!(
                        "`export fn {}`: parameter `{pname}` is {} — only Int, Float, \
                         Bool and Ptr cross the wasm boundary",
                        f.name,
                        type_label(pty, &tmod.adts)
                    ),
                ));
            }
        }
        if !(scalar(&f.ret) || f.ret == Ty::Unit) {
            return Err(CodegenError::new(
                f.line,
                format!(
                    "`export fn {}` returns {} — only Int, Float, Bool, Ptr and Unit \
                     cross the wasm boundary",
                    f.name,
                    type_label(&f.ret, &tmod.adts)
                ),
            ));
        }
    }
    Ok(())
}

/// The type's name as the author would write it, for a diagnostic.
fn type_label(t: &Ty, adts: &[rill_types::AdtDef]) -> String {
    mangle_friendly(t, adts)
}

/// The socket builtins, by the runtime symbol each becomes.
const SOCKET_RT: [&str; 6] = [
    "rill_tcp_listen",
    "rill_tcp_accept",
    "rill_tcp_connect",
    "rill_sock_read",
    "rill_sock_write",
    "rill_sock_close",
];

/// Whether any function `main` can reach has a strand in it.
///
/// Asked before a line of code is generated, because it decides whether tail
/// calls carry a preemption check, and that has to be settled while they are
/// being written. `needs_scheduler` answers the same question exactly but only
/// once the module exists, which is too late.
///
/// Erring towards "yes" costs a load and an untaken branch per tail call in a
/// program that did not need it; erring towards "no" would leave a strand able
/// to hold its worker forever. So an edge it cannot follow — a closure called
/// through a variable — is taken to reach everything a function value was ever
/// made of.
fn uses_scheduler(tmod: &TModule) -> bool {
    let Some(&entry) = tmod.fn_index.get("main") else {
        return false;
    };
    let graph = call_graph(tmod);
    let mut seen = vec![false; tmod.fns.len()];
    let mut queue = vec![entry];
    seen[entry] = true;
    while let Some(i) = queue.pop() {
        let mut found = false;
        let mut edges: Vec<usize> = Vec::new();
        let mut methods = false;
        collect_calls(&tmod.fns[i].body, &mut edges, &mut found, &mut methods);
        if found {
            return true;
        }
        for &e in &graph[i] {
            if !seen[e] {
                seen[e] = true;
                queue.push(e);
            }
        }
    }
    false
}

/// Who calls whom, by function index.
///
/// Trait dispatch picks its implementation from the receiver type at
/// monomorphization; rather than repeat that here, a method call is taken to
/// reach every implementation of anything. Erring wide is the safe direction
/// for both readers of this: a preemption check that was not needed costs a
/// branch, and a missing one costs a strand that never gives its worker back.
fn call_graph(tmod: &TModule) -> Vec<Vec<usize>> {
    let all_impls: Vec<usize> = tmod.impl_methods.values().copied().collect();
    tmod.fns
        .iter()
        .map(|f| {
            let (mut edges, mut found, mut methods) = (Vec::new(), false, false);
            collect_calls(&f.body, &mut edges, &mut found, &mut methods);
            if methods {
                edges.extend_from_slice(&all_impls);
            }
            edges.retain(|&e| e < tmod.fns.len());
            edges.sort_unstable();
            edges.dedup();
            edges
        })
        .collect()
}

/// Call targets out of one body, and whether the body itself has a strand in
/// it. `FnRef` counts as a call: a function named as a value is one that
/// something is going to call.
fn collect_calls(e: &TExpr, out: &mut Vec<usize>, found: &mut bool, methods: &mut bool) {
    match &e.kind {
        TK::Spawn { .. }
        | TK::ChanNew(_)
        | TK::ChanSend { .. }
        | TK::ChanRecv { .. }
        | TK::Select { .. } => *found = true,
        TK::Builtin { rt, .. } if SOCKET_RT.contains(rt) => *found = true,
        TK::CallFn { fn_idx, .. } => out.push(*fn_idx),
        TK::FnRef { fn_idx, .. } => out.push(*fn_idx),
        TK::MethodCall { .. } => *methods = true,
        _ => {}
    }
    walk_children(e, &mut |c| collect_calls(c, out, found, methods));
}

/// The first use of a scheduler feature in this expression, if there is one:
/// its line, and what to call it in the error.
fn find_unsupported(e: &TExpr) -> Option<(usize, &'static str)> {
    let here = match &e.kind {
        TK::Spawn { .. } => Some("spawn"),
        TK::ChanNew(_) | TK::ChanSend { .. } | TK::ChanRecv { .. } => Some("a channel"),
        TK::Select { .. } => Some("select"),
        TK::Builtin { rt, .. } if SOCKET_RT.contains(rt) => Some("a socket"),
        _ => None,
    };
    if let Some(what) = here {
        return Some((e.line, what));
    }
    let mut found = None;
    walk_children(e, &mut |c| {
        if found.is_none() {
            found = find_unsupported(c);
        }
    });
    found
}

/// Compile and return the (unoptimized) LLVM IR as text.
pub fn compile_to_ir(tmod: &TModule) -> Result<String, CodegenError> {
    let context = Context::create();
    let cg = Codegen::build_module(&context, tmod, false, None, None)?;
    Ok(cg.module.print_to_string().to_string())
}

/// Applies a substitution for `Ty::Gen` parameters.
fn subst_gen(ty: &Ty, args: &[Ty]) -> Ty {
    match ty {
        Ty::Gen(i) => args[*i as usize].clone(),
        Ty::Adt(a, ts) => Ty::Adt(*a, ts.iter().map(|t| subst_gen(t, args)).collect()),
        Ty::Fn(p, r) => Ty::Fn(
            p.iter().map(|t| subst_gen(t, args)).collect(),
            Box::new(subst_gen(r, args)),
        ),
        other => other.clone(),
    }
}

fn mangle(ty: &Ty, adts: &[rill_types::AdtDef]) -> String {
    match ty {
        Ty::Int => "Int".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "Bool".into(),
        Ty::Str => "Str".into(),
        Ty::StrBuf => "StrBuf".into(),
        Ty::Unit => "Unit".into(),
        Ty::Adt(i, args) => {
            let mut s = adts[*i].name.clone();
            for a in args {
                s.push('_');
                s.push_str(&mangle(a, adts));
            }
            s
        }
        Ty::Fn(p, r) => {
            let mut s = "fn".to_string();
            for a in p {
                s.push('_');
                s.push_str(&mangle(a, adts));
            }
            s.push_str("_to_");
            s.push_str(&mangle(r, adts));
            s
        }
        Ty::Chan(e) => format!("Chan_{}", mangle(e, adts)),
        Ty::Buf(w) => format!("Buf_{}", w.name()),
        Ty::Ptr => "Ptr".into(),
        Ty::Var(_) | Ty::Gen(_) => unreachable!("mangling requires concrete types"),
    }
}

/// `Tuple2`..`Tuple4` are what the tuple notation stands for.
fn is_tuple_ctor(name: &str, n: usize) -> bool {
    (2..=4).contains(&n) && name == format!("Tuple{n}")
}

/// A function value pinned at compile time: which top-level function, at
/// which instantiation. This is what a C callback ultimately needs, since the
/// trampoline it points at is emitted while compiling.
type FnVal = (usize, Vec<Ty>);

/// A function, its type arguments, and the callbacks bound to whichever
/// parameters have to be known while compiling. The third field is empty for
/// nearly every function; when it is not, the function is emitted once per
/// distinct callback handed to it.
type InstanceKey = (usize, Vec<Ty>, Vec<(usize, FnVal)>);

/// Which parameters of each function must be a function known at compile
/// time: the ones that end up in a C callback position, whether directly or
/// by being handed on to another function that puts them there.
///
/// This is what lets a callback travel as an ordinary parameter. C is given a
/// bare code pointer with nowhere to keep an environment, so the trampoline
/// has to be chosen while compiling; a helper that forwards a callback is
/// therefore emitted once per callback it is called with. A parameter that is
/// rebound in the body is never marked, so the name in a callback position is
/// certainly the parameter.
fn static_params(tmod: &TModule) -> Vec<Vec<bool>> {
    let mut need: Vec<Vec<bool>> = tmod.fns.iter().map(|f| vec![false; f.params.len()]).collect();
    // Only parameters whose name still means the parameter everywhere in the
    // body can be followed by name.
    let stable: Vec<Vec<Option<&str>>> = tmod
        .fns
        .iter()
        .map(|f| {
            let mut rebound = HashSet::new();
            collect_rebound(&f.body, &mut rebound);
            f.params
                .iter()
                .map(|(n, _)| if rebound.contains(n.as_str()) { None } else { Some(n.as_str()) })
                .collect()
        })
        .collect();

    loop {
        let mut changed = false;
        for fi in 0..tmod.fns.len() {
            let mut found: Vec<&str> = Vec::new();
            find_callback_locals(&tmod.fns[fi].body, tmod, &need, &mut found);
            for (pi, name) in stable[fi].iter().enumerate() {
                let Some(name) = name else { continue };
                if !need[fi][pi] && found.contains(name) {
                    need[fi][pi] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return need;
        }
    }
}

/// Every name the body binds, so a parameter shadowed by one is left alone.
fn collect_rebound(e: &TExpr, out: &mut HashSet<String>) {
    match &e.kind {
        TK::Block(stmts) => {
            for s in stmts {
                match s {
                    TStmt::Bind { name, value } => {
                        out.insert(name.clone());
                        collect_rebound(value, out);
                    }
                    TStmt::Expr(x) => collect_rebound(x, out),
                }
            }
        }
        TK::Match { scrutinee, arms } => {
            collect_rebound(scrutinee, out);
            for a in arms {
                collect_pattern_binds(&a.pattern, out);
                collect_rebound(&a.body, out);
            }
        }
        TK::Lambda { params, body, .. } => {
            for (n, _) in params {
                out.insert(n.clone());
            }
            collect_rebound(body, out);
        }
        TK::Select { arms, default } => {
            for a in arms {
                if let Some((n, _)) = &a.bind {
                    out.insert(n.clone());
                }
                match &a.op {
                    TSelectOp::Recv { chan, .. } => collect_rebound(chan, out),
                    TSelectOp::Send { chan, value } => {
                        collect_rebound(chan, out);
                        collect_rebound(value, out);
                    }
                }
                collect_rebound(&a.body, out);
            }
            if let Some(d) = default {
                collect_rebound(d, out);
            }
        }
        _ => walk_children(e, &mut |c| collect_rebound(c, out)),
    }
}

fn collect_pattern_binds(p: &Pattern, out: &mut HashSet<String>) {
    match p {
        Pattern::Var(n) => {
            out.insert(n.clone());
        }
        Pattern::Ctor { args, .. } => {
            for a in args {
                collect_pattern_binds(a, out);
            }
        }
        Pattern::Wildcard | Pattern::Int(_) => {}
    }
}

/// Names used in this body where a compile-time function is required: an
/// argument to a C callback parameter, or one handed to a function that
/// already needs it there.
fn find_callback_locals<'a>(
    e: &'a TExpr,
    tmod: &'a TModule,
    need: &[Vec<bool>],
    out: &mut Vec<&'a str>,
) {
    match &e.kind {
        TK::CallExtern { name, args } => {
            let sig = &tmod.externs[name];
            for (a, pt) in args.iter().zip(&sig.params) {
                if let (CTy::Fn(..), TK::Local(n)) = (pt, &a.kind) {
                    out.push(n);
                }
            }
        }
        TK::CallFn { fn_idx, args, .. } => {
            for (i, a) in args.iter().enumerate() {
                if need[*fn_idx].get(i) == Some(&true) {
                    if let TK::Local(n) = &a.kind {
                        out.push(n);
                    }
                }
            }
        }
        _ => {}
    }
    walk_children(e, &mut |c| find_callback_locals(c, tmod, need, out));
}

/// Applies `f` to every immediate sub-expression.
fn walk_children<'a>(e: &'a TExpr, f: &mut impl FnMut(&'a TExpr)) {
    match &e.kind {
        TK::Int(_) | TK::Float(_) | TK::Str(_) | TK::Bool(_) | TK::Local(_) | TK::FnRef { .. } => {}
        TK::Slice { base, from, to } => {
            f(base);
            f(from);
            if let Some(t) = to {
                f(t);
            }
        }
        TK::Builtin { args, .. }
        | TK::CallFn { args, .. }
        | TK::MethodCall { args, .. }
        | TK::CallExtern { args, .. }
        | TK::CallCtor { args, .. } => args.iter().for_each(f),
        TK::CallClosure { callee, args } => {
            f(callee);
            args.iter().for_each(f);
        }
        TK::Println(x) | TK::Print(x) | TK::Show(x) | TK::Neg(x) | TK::Spawn { body: x, .. } => f(x),
        TK::ChanNew(x) => {
            if let Some(x) = x {
                f(x)
            }
        }
        TK::ChanSend { ch, val } => {
            f(ch);
            f(val);
        }
        TK::ChanRecv { ch } | TK::BufLen { buf: ch } | TK::BufPtr { buf: ch } => f(ch),
        TK::BufNew { count, .. } => f(count),
        TK::BufGet { buf, idx } => {
            f(buf);
            f(idx);
        }
        TK::BufSet { buf, idx, val } => {
            f(buf);
            f(idx);
            f(val);
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
                f(d)
            }
        }
        TK::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        TK::If { cond, then, els } => {
            f(cond);
            f(then);
            f(els);
        }
        TK::Match { scrutinee, arms } => {
            f(scrutinee);
            arms.iter().for_each(|a| f(&a.body));
        }
        TK::Block(stmts) => stmts.iter().for_each(|s| match s {
            TStmt::Bind { value, .. } => f(value),
            TStmt::Expr(x) => f(x),
        }),
        TK::Lambda { body, .. } => f(body),
    }
}

// ---- pattern matrices and decision trees -----------------------------------

/// Where a value sits inside the scrutinee: the constructor tested and the
/// field taken, at each step down.
type Occ = Vec<(usize, u32)>;

#[derive(Clone)]
struct Row {
    pats: Vec<Pattern>,
    arm: usize,
}

/// The value shape a pattern tests for. Within one column every constructor
/// head belongs to the same ADT, so its index identifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    Ctor(usize),
    Int(i64),
}

enum Tree {
    /// Run this arm.
    Leaf(usize),
    /// Test the value at `occ` and take the matching branch.
    Switch { occ: Occ, on_int: bool, cases: Vec<(u64, Tree)>, default: Option<Box<Tree>> },
    /// No row can match — unreachable once the checker has run.
    Fail,
}

fn is_wild(p: &Pattern) -> bool {
    matches!(p, Pattern::Wildcard | Pattern::Var(_))
}

/// Rows that survive once the first column is known to be `head`, with the
/// head's fields spliced in where the column was.
fn specialize(
    rows: &[Row],
    head: Head,
    arity: usize,
    head_of: impl Fn(&Pattern) -> Option<Head>,
) -> Vec<Row> {
    let mut out = Vec::new();
    for r in rows {
        let mut pats = if is_wild(&r.pats[0]) {
            vec![Pattern::Wildcard; arity]
        } else if head_of(&r.pats[0]) == Some(head) {
            match &r.pats[0] {
                Pattern::Ctor { args, .. } => args.clone(),
                _ => Vec::new(),
            }
        } else {
            continue;
        };
        pats.extend_from_slice(&r.pats[1..]);
        out.push(Row { pats, arm: r.arm });
    }
    out
}

/// The variables a pattern binds, each with the path it sits at. The path
/// records the constructor taken at every step, which is exactly what the
/// field load needs to know the box layout.
fn collect_bindings(
    p: &Pattern,
    occ: Occ,
    ctors: &HashMap<String, (usize, usize)>,
    out: &mut Vec<(String, Occ)>,
) {
    match p {
        Pattern::Var(n) => out.push((n.clone(), occ)),
        Pattern::Ctor { name, args, .. } => {
            let ci = ctors[name].1;
            for (i, sub) in args.iter().enumerate() {
                let mut o = occ.clone();
                o.push((ci, i as u32));
                collect_bindings(sub, o, ctors, out);
            }
        }
        _ => {}
    }
}

fn mangle_friendly(ty: &Ty, adts: &[rill_types::AdtDef]) -> String {
    match ty {
        Ty::Fn(..) => "a function".into(),
        Ty::Chan(..) => "a channel".into(),
        Ty::Buf(w) => format!("Buf({})", w.name()),
        Ty::Unit => "Unit".into(),
        other => mangle(other, adts),
    }
}

struct Codegen<'ctx, 'a> {
    ctx: &'ctx Context,
    module: LlvmModule<'ctx>,
    builder: Builder<'ctx>,
    tmod: &'a TModule,
    instances: HashMap<InstanceKey, FunctionValue<'ctx>>,
    shims: HashMap<FnVal, FunctionValue<'ctx>>,
    /// Which parameters of each function have to be a compile-time function,
    /// because they reach a C callback position. See `static_params`.
    static_params: Vec<Vec<bool>>,
    /// Those parameters of the instance being generated, resolved: the name a
    /// callback travels under, and the function it actually is.
    bound_fns: HashMap<String, FnVal>,
    /// One C-ABI trampoline per (function, instantiation, C parameters, C
    /// result) — the same function passed at two widths needs two of them.
    callbacks: HashMap<(usize, Vec<Ty>, Vec<CTy>, CTy), FunctionValue<'ctx>>,
    queue: VecDeque<InstanceKey>,
    /// Per concrete ADT type: its generated `drop$T` function.
    adt_drops: HashMap<Ty, FunctionValue<'ctx>>,
    /// Per concrete ADT type: its generated structural `show$T` function.
    adt_shows: HashMap<Ty, FunctionValue<'ctx>>,
    /// Per concrete ADT type: its generated structural `eq$T` function.
    adt_eqs: HashMap<Ty, FunctionValue<'ctx>>,
    /// Immortal singleton globals for nullary constructors — `Leaf`, `Nil`,
    /// `None`... never allocate.
    nullary_singletons: HashMap<(Ty, usize), PointerValue<'ctx>>,
    /// Owned (value, type) pairs of the function currently being generated,
    /// released before its return.
    cleanup: Vec<(BasicValueEnum<'ctx>, Ty)>,
    str_count: usize,
    lambda_count: usize,
    /// Built with `--parallel`: refcount updates check `rill_mt` and go
    /// atomic when the program asks for more than one worker.
    parallel: bool,
    /// Present in debug builds: emits DWARF so a debugger can map machine
    /// code back to Rill source lines.
    debug: Option<DebugCtx<'ctx>>,
    /// Line most recently attached to generated instructions.
    cur_line: u32,
    /// The function being generated, for spotting a tail call that goes round.
    cur_fn: usize,
    /// Who calls whom, and the memoised answer to "what can this one reach".
    /// Both empty in a program with no strands, which never asks.
    callgraph: Vec<Vec<usize>>,
    reaches: HashMap<usize, HashSet<usize>>,
    /// The program has strands in it, so tail calls carry a preemption check.
    /// Decided up front by `uses_scheduler`; see `emit_preempt_check`.
    preempt: bool,
    /// Building for wasm, which spells the entry point differently. Decided
    /// here rather than at object-writing time because the name has to be
    /// chosen while the function is being created.
    wasm: bool,
}

struct DebugCtx<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    unit: DICompileUnit<'ctx>,
    /// The subprogram for the function being generated.
    scope: Option<DISubprogram<'ctx>>,
}

/// Local scope: name -> value (`None` for Unit-typed locals).
type Scope<'ctx> = HashMap<String, Option<BasicValueEnum<'ctx>>>;

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    fn build_module(
        ctx: &'ctx Context,
        tmod: &'a TModule,
        parallel: bool,
        source: Option<&Path>,
        target: Option<&str>,
    ) -> Result<Codegen<'ctx, 'a>, CodegenError> {
        let module = ctx.create_module("rill");
        let builder = ctx.create_builder();
        let preempt = uses_scheduler(tmod);
        let mut cg = Codegen {
            ctx,
            module,
            builder,
            tmod,
            instances: HashMap::new(),
            shims: HashMap::new(),
            static_params: static_params(tmod),
            bound_fns: HashMap::new(),
            callbacks: HashMap::new(),
            queue: VecDeque::new(),
            adt_drops: HashMap::new(),
            adt_shows: HashMap::new(),
            adt_eqs: HashMap::new(),
            nullary_singletons: HashMap::new(),
            cleanup: Vec::new(),
            str_count: 0,
            lambda_count: 0,
            parallel,
            debug: None,
            cur_line: 0,
            cur_fn: usize::MAX,
            callgraph: if preempt { call_graph(tmod) } else { Vec::new() },
            reaches: HashMap::new(),
            preempt,
            wasm: target.is_some_and(|t| t.starts_with("wasm")),
        };
        if let Some(src) = source {
            let file = src.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let dir = src
                .parent()
                .map(|d| d.to_string_lossy().to_string())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| ".".to_string());
            cg.module.add_basic_value_flag(
                "Debug Info Version",
                inkwell::module::FlagBehavior::Warning,
                ctx.i32_type().const_int(3, false),
            );
            let (builder, unit) = cg.module.create_debug_info_builder(
                true,
                DWARFSourceLanguage::C,
                &file,
                &dir,
                "rill",
                false,
                "",
                0,
                "",
                DWARFEmissionKind::Full,
                0,
                false,
                false,
                "",
                "",
            );
            cg.debug = Some(DebugCtx { builder, unit, scope: None });
        }
        cg.declare_runtime();
        // Tells the runtime whether this binary's reference counting is safe
        // to run on more than one worker.
        let flag = cg.module.add_global(cg.ctx.i8_type(), None, "rill_parallel_build");
        flag.set_initializer(&cg.ctx.i8_type().const_int(parallel as u64, false));

        let main_idx = tmod.fn_index["main"];
        cg.request_instance(main_idx, vec![], vec![]);
        // Exports are entry points in their own right: emitted whether or not
        // anything in `main` reaches them.
        for (i, f) in tmod.fns.iter().enumerate() {
            if f.exported {
                cg.request_instance(i, vec![], vec![]);
            }
        }
        while let Some(key) = cg.queue.pop_front() {
            cg.gen_instance(&key)?;
        }
        cg.gen_main_wrapper();

        if let Err(msg) = cg.module.verify() {
            return Err(CodegenError::new(
                0,
                format!("internal error: invalid LLVM module generated:\n{}", msg.to_string()),
            ));
        }
        Ok(cg)
    }

    // ---- types -----------------------------------------------------------

    /// Immortal refcount for string literals: never reaches zero.
    const IMMORTAL: u64 = (i64::MAX / 2) as u64;
    /// Anything counted at or above this is one of those constants. The test
    /// has room to spare in both directions: a real count is bounded by how
    /// many references can fit in memory.
    const IMMORTAL_MIN: u64 = (i64::MAX / 4) as u64;

    fn closure_type(&self) -> StructType<'ctx> {
        let ptr = self.ctx.ptr_type(Default::default());
        self.ctx.struct_type(&[ptr.into(), ptr.into()], false)
    }

    /// LLVM type of a concrete Rill type; `None` for Unit.
    fn llvm_type(&self, ty: &Ty) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            Ty::Int => Some(self.ctx.i64_type().into()),
            Ty::Float => Some(self.ctx.f64_type().into()),
            Ty::Bool => Some(self.ctx.bool_type().into()),
            Ty::Str | Ty::StrBuf | Ty::Adt(..) | Ty::Chan(..) | Ty::Ptr | Ty::Buf(_) => {
                Some(self.ctx.ptr_type(Default::default()).into())
            }
            Ty::Fn(..) => Some(self.closure_type().into()),
            Ty::Unit => None,
            Ty::Var(_) | Ty::Gen(_) => unreachable!("codegen requires concrete types"),
        }
    }

    /// LLVM type of a foreign signature position; `None` for Unit (a void
    /// return). C takes a bool as a byte.
    fn c_type(&self, c: &CTy) -> Option<BasicTypeEnum<'ctx>> {
        Some(match c {
            CTy::I8 | CTy::U8 | CTy::Bool => self.ctx.i8_type().into(),
            CTy::I16 | CTy::U16 => self.ctx.i16_type().into(),
            CTy::I32 | CTy::U32 => self.ctx.i32_type().into(),
            CTy::I64 | CTy::U64 => self.ctx.i64_type().into(),
            CTy::F32 => self.ctx.f32_type().into(),
            CTy::F64 => self.ctx.f64_type().into(),
            CTy::Ptr | CTy::Fn(..) => self.ctx.ptr_type(Default::default()).into(),
            CTy::Unit => return None,
        })
    }

    /// Narrow a Rill value (`i64`, `f64`, `i1`, pointer) to what the C symbol
    /// actually declared.
    fn to_c(&self, v: BasicValueEnum<'ctx>, c: &CTy) -> BasicValueEnum<'ctx> {
        let b = &self.builder;
        match c {
            CTy::Bool => b
                .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "cb")
                .expect("LLVM builder error")
                .into(),
            CTy::F32 => b
                .build_float_trunc(v.into_float_value(), self.ctx.f32_type(), "cf")
                .expect("LLVM builder error")
                .into(),
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Unit | CTy::Fn(..) => v,
            _ => {
                let t = self.c_type(c).expect("not Unit").into_int_type();
                b.build_int_truncate(v.into_int_value(), t, "ci")
                    .expect("LLVM builder error")
                    .into()
            }
        }
    }

    /// Widen a C result back to the Rill type the checker gave the call site.
    /// Signedness decides the extension: `U32` fills with zeroes so a large
    /// unsigned result does not come back negative.
    fn from_c(&self, v: BasicValueEnum<'ctx>, c: &CTy) -> BasicValueEnum<'ctx> {
        let b = &self.builder;
        let i64t = self.ctx.i64_type();
        match c {
            CTy::Bool => b
                .build_int_truncate(v.into_int_value(), self.ctx.bool_type(), "rb")
                .expect("LLVM builder error")
                .into(),
            CTy::F32 => b
                .build_float_ext(v.into_float_value(), self.ctx.f64_type(), "rf")
                .expect("LLVM builder error")
                .into(),
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Unit | CTy::Fn(..) => v,
            _ if c.is_signed() => b
                .build_int_s_extend(v.into_int_value(), i64t, "rs")
                .expect("LLVM builder error")
                .into(),
            _ => b
                .build_int_z_extend(v.into_int_value(), i64t, "ru")
                .expect("LLVM builder error")
                .into(),
        }
    }

    /// `{ i64 rc, i64 len, elements... }` — the length sits where a string
    /// keeps its byte count.
    fn buf_len(&self, p: PointerValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let slot = unsafe {
            self.builder
                .build_gep(i64t, p, &[i64t.const_int(1, false)], "len.ptr")
                .expect("LLVM builder error")
        };
        let load = self.builder.build_load(i64t, slot, "len").expect("LLVM builder error");
        // A buffer's length is fixed when it is made, so this load may be
        // hoisted out of a loop and shared between the bounds checks of one.
        // Without saying so, every indexed access reads the header again,
        // since a write to an element is not obviously disjoint from it.
        self.mark_invariant(load);
        load.into_int_value()
    }

    /// Marks a load as reading memory that never changes after it is written.
    fn mark_invariant(&self, load: BasicValueEnum<'ctx>) {
        if let Ok(inst) = load.as_instruction_value().ok_or(()) {
            let empty = self.ctx.metadata_node(&[]);
            let kind = self.ctx.get_kind_id("invariant.load");
            let _ = inst.set_metadata(empty, kind);
        }
    }

    /// The first element's address, which is also what C is handed.
    fn buf_data(&self, p: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let i8t = self.ctx.i8_type();
        unsafe {
            self.builder
                .build_gep(i8t, p, &[self.ctx.i64_type().const_int(16, false)], "data")
                .expect("LLVM builder error")
        }
    }

    /// The address of element `i`, after checking it is one. The comparison is
    /// unsigned, so a negative index fails the same test as one off the end.
    fn buf_slot(
        &mut self,
        p: PointerValue<'ctx>,
        i: IntValue<'ctx>,
        w: &CTy,
        func: FunctionValue<'ctx>,
    ) -> PointerValue<'ctx> {
        let len = self.buf_len(p);
        let ok = self
            .builder
            .build_int_compare(IntPredicate::ULT, i, len, "inrange")
            .expect("LLVM builder error");
        let good = self.ctx.append_basic_block(func, "buf.ok");
        let bad = self.ctx.append_basic_block(func, "buf.oob");
        self.builder.build_conditional_branch(ok, good, bad).expect("LLVM builder error");

        self.builder.position_at_end(bad);
        let oob = self.module.get_function("rill_buf_oob").unwrap();
        self.builder.build_call(oob, &[i.into(), len.into()], "").expect("LLVM builder error");
        self.builder.build_unreachable().expect("LLVM builder error");

        self.builder.position_at_end(good);
        let data = self.buf_data(p);
        unsafe {
            self.builder
                .build_gep(self.c_type(w).expect("a width"), data, &[i], "slot")
                .expect("LLVM builder error")
        }
    }

    /// The heap layout of one constructor instance:
    /// `{ i64 rc, i64 tag, fields... }`.
    fn ctor_struct(&self, adt: usize, ctor: usize, targs: &[Ty]) -> StructType<'ctx> {
        let mut members: Vec<BasicTypeEnum> =
            vec![self.ctx.i64_type().into(), self.ctx.i64_type().into()];
        for f in &self.tmod.adts[adt].ctors[ctor].fields {
            let cf = subst_gen(f, targs);
            members.push(self.llvm_type(&cf).expect("ADT fields are never Unit"));
        }
        self.ctx.struct_type(&members, false)
    }

    /// Allocates a heap object of the given layout with refcount 1.
    fn alloc_object(&mut self, struct_ty: StructType<'ctx>, name: &str) -> PointerValue<'ctx> {
        let size = struct_ty.size_of().expect("sized struct");
        let alloc = self.module.get_function("rill_alloc").unwrap();
        let call = self
            .builder
            .build_call(alloc, &[size.into()], name)
            .expect("LLVM builder error");
        let ptr = match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_pointer_value(),
            _ => unreachable!(),
        };
        let rc_ptr = self
            .builder
            .build_struct_gep(struct_ty, ptr, 0, "rc.init")
            .expect("LLVM builder error");
        self.builder
            .build_store(rc_ptr, self.ctx.i64_type().const_int(1, false))
            .expect("LLVM builder error");
        ptr
    }

    fn fn_llvm_type(&self, params: &[Ty], ret: &Ty, with_env: bool) -> FunctionType<'ctx> {
        let mut ps: Vec<BasicMetadataTypeEnum> = Vec::new();
        if with_env {
            ps.push(self.ctx.ptr_type(Default::default()).into());
        }
        for p in params {
            ps.push(self.llvm_type(p).expect("parameters are never Unit").into());
        }
        match self.llvm_type(ret) {
            None => self.ctx.void_type().fn_type(&ps, false),
            Some(t) => t.fn_type(&ps, false),
        }
    }

    // ---- runtime ---------------------------------------------------------

    fn declare_runtime(&mut self) {
        let i64t = self.ctx.i64_type();
        let f64t = self.ctx.f64_type();
        let i8t = self.ctx.i8_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let void = self.ctx.void_type();
        self.module.add_function(
            "rill_println_str",
            void.fn_type(&[ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_println_int", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_println_float", void.fn_type(&[f64t.into()], false), None);
        self.module.add_function("rill_println_bool", void.fn_type(&[i8t.into()], false), None);
        self.module.add_function(
            "rill_str_eq",
            i8t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_str_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_str_concat",
            ptrt.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_str_len", i64t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_int_to_str", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_float_to_str", ptrt.fn_type(&[f64t.into()], false), None);
        self.module.add_function("rill_bool_to_str", ptrt.fn_type(&[i8t.into()], false), None);
        self.module.add_function("rill_print_str", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_print_int", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_print_float", void.fn_type(&[f64t.into()], false), None);
        self.module.add_function("rill_print_bool", void.fn_type(&[i8t.into()], false), None);
        self.module.add_function(
            "rill_str_get",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_str_sub",
            ptrt.fn_type(&[ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_str_find",
            i64t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_str_cmp",
            i64t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_str_to_int", i64t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_str_to_float",
            f64t.fn_type(&[ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_chr", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_set_args",
            void.fn_type(&[i64t.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_arg_count", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_arg", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_io_ok", i8t.fn_type(&[], false), None);
        self.module.add_function("rill_read_file", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_write_file",
            i8t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_read_line", ptrt.fn_type(&[], false), None);
        self.module.add_function("rill_scan_int", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_exit", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_to_cstr", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_from_cstr", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_free_ptr", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_null_ptr", ptrt.fn_type(&[], false), None);
        self.module.add_function("rill_is_null", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_buf_new",
            ptrt.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_buf_release",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_strbuf_new", ptrt.fn_type(&[], false), None);
        self.module.add_function(
            "rill_strbuf_add",
            void.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_strbuf_byte",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_strbuf_str", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_strbuf_len", i64t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_strbuf_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_buf_oob",
            void.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_alloc_bytes", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_ptr_offset",
            ptrt.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_poke_ptr",
            void.fn_type(&[ptrt.into(), i64t.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_peek_ptr",
            ptrt.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        // Buffer access. The runtime narrows, so every `poke` takes Rill's own
        // i64/f64 and every `peek` hands one back.
        for w in ["i8", "i16", "i32", "i64"] {
            self.module.add_function(
                &format!("rill_poke_{w}"),
                void.fn_type(&[ptrt.into(), i64t.into(), i64t.into()], false),
                None,
            );
        }
        for w in ["f32", "f64"] {
            self.module.add_function(
                &format!("rill_poke_{w}"),
                void.fn_type(&[ptrt.into(), i64t.into(), f64t.into()], false),
                None,
            );
        }
        for w in ["i8", "u8", "i16", "u16", "i32", "u32", "i64"] {
            self.module.add_function(
                &format!("rill_peek_{w}"),
                i64t.fn_type(&[ptrt.into(), i64t.into()], false),
                None,
            );
        }
        for w in ["f32", "f64"] {
            self.module.add_function(
                &format!("rill_peek_{w}"),
                f64t.fn_type(&[ptrt.into(), i64t.into()], false),
                None,
            );
        }
        self.module.add_function("rill_preempt_yield", void.fn_type(&[], false), None);
        self.module.add_function("rill_tcp_listen", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_tcp_accept", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_tcp_connect",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_sock_read", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_sock_write",
            i64t.fn_type(&[i64t.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_sock_close", void.fn_type(&[i64t.into()], false), None);

        // Foreign functions, declared at the widths the C symbol was compiled
        // with rather than at Rill's own — the conversion happens per call.
        for (name, sig) in &self.tmod.externs {
            let ps: Vec<BasicMetadataTypeEnum> = sig
                .params
                .iter()
                .map(|c| self.c_type(c).expect("a parameter is never Unit").into())
                .collect();
            let fn_ty = match self.c_type(&sig.ret) {
                Some(t) => t.fn_type(&ps, false),
                None => void.fn_type(&ps, false),
            };
            self.module.add_function(name, fn_ty, None);
        }
        self.module.add_function("rill_alloc", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_free_sized",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_retain", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_env_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_alloc_report", void.fn_type(&[], false), None);
        self.module.add_function(
            "rill_rt_run_direct",
            void.fn_type(&[ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_chan_new", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_chan_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_chan_send",
            void.fn_type(&[ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        let words = self.ctx.struct_type(&[i64t.into(), i64t.into()], false);
        self.module.add_function("rill_chan_recv", words.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_spawn",
            void.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_rt_run", void.fn_type(&[ptrt.into()], false), None);
        // i64 rill_select(cases*, n, has_default, out*) — the received value
        // goes through `out` rather than an oversized return value.
        self.module.add_function(
            "rill_select",
            i64t.fn_type(&[ptrt.into(), i64t.into(), i8t.into(), ptrt.into()], false),
            None,
        );
    }

    // ---- channel value packing (two raw words fit every Rill value) ------

    fn pack_words(
        &mut self,
        v: BasicValueEnum<'ctx>,
        ty: &Ty,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let zero = i64t.const_zero();
        let b = &self.builder;
        match ty {
            Ty::Int => (v.into_int_value(), zero),
            Ty::Bool => (
                b.build_int_z_extend(v.into_int_value(), i64t, "w").unwrap(),
                zero,
            ),
            Ty::Float => (
                b.build_bit_cast(v.into_float_value(), i64t, "w")
                    .unwrap()
                    .into_int_value(),
                zero,
            ),
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Ptr | Ty::Buf(_) => (
                b.build_ptr_to_int(v.into_pointer_value(), i64t, "w").unwrap(),
                zero,
            ),
            Ty::Fn(..) => {
                let agg = v.into_struct_value();
                let f0 = b.build_extract_value(agg, 0, "w0").unwrap();
                let f1 = b.build_extract_value(agg, 1, "w1").unwrap();
                let w0 = b
                    .build_ptr_to_int(f0.into_pointer_value(), i64t, "w0i")
                    .unwrap();
                let w1 = b.build_ptr_to_int(f1.into_pointer_value(), i64t, "w1i").unwrap();
                (w0, w1)
            }
            Ty::Unit | Ty::Var(_) | Ty::Gen(_) => unreachable!("not sendable"),
        }
    }

    fn unpack_words(
        &mut self,
        a: IntValue<'ctx>,
        bw: IntValue<'ctx>,
        ty: &Ty,
    ) -> BasicValueEnum<'ctx> {
        let b = &self.builder;
        let ptrt = self.ctx.ptr_type(Default::default());
        match ty {
            Ty::Int => a.into(),
            Ty::Bool => b
                .build_int_truncate(a, self.ctx.bool_type(), "b")
                .unwrap()
                .into(),
            Ty::Float => b
                .build_bit_cast(a, self.ctx.f64_type(), "f")
                .unwrap(),
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Ptr | Ty::Buf(_) => {
                let _ = bw;
                b.build_int_to_ptr(a, ptrt, "p").unwrap().into()
            }
            Ty::Fn(..) => {
                let p0 = b.build_int_to_ptr(a, ptrt, "p0").unwrap();
                let p1 = b.build_int_to_ptr(bw, ptrt, "p1").unwrap();
                let s0 = b
                    .build_insert_value(self.closure_type().get_undef(), p0, 0, "c")
                    .unwrap();
                let s1 = b.build_insert_value(s0, p1, 1, "c").unwrap();
                s1.into_struct_value().into()
            }
            Ty::Unit | Ty::Var(_) | Ty::Gen(_) => unreachable!("not receivable"),
        }
    }

    // ---- reference counting ---------------------------------------------

    fn is_heap(ty: &Ty) -> bool {
        matches!(ty, Ty::Adt(..) | Ty::Fn(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Buf(_))
    }

    /// The `{ i64 rc, i64 tag }` header shared by all ADT boxes.
    fn header_struct(&self) -> StructType<'ctx> {
        self.ctx.struct_type(&[self.ctx.i64_type().into(), self.ctx.i64_type().into()], false)
    }

    fn call_rt(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> CallSiteValue<'ctx> {
        let f = self.module.get_function(name).unwrap();
        self.builder.build_call(f, args, "").expect("LLVM builder error")
    }

    /// +1 on an owned heap reference (no-op for non-heap types).
    /// +1 on a pointer that is known non-null. A single-worker build does it
    /// inline; a `--parallel` build goes through the runtime, which picks the
    /// atomic form when the program is running on several workers.
    /// Stores an updated count, unless the object is a compile-time constant
    /// living in read-only memory. A branch rather than a redirected store:
    /// storing through a selected pointer costs the optimizer its knowledge
    /// of what the store touches, and reference counting is hot enough that
    /// this showed up as a third of the running time on tree-building code.
    /// Taken as written, the common path is exactly what it was before.
    fn store_rc(
        &mut self,
        rc_ptr: PointerValue<'ctx>,
        rc: IntValue<'ctx>,
        new: IntValue<'ctx>,
    ) {
        let i64t = self.ctx.i64_type();
        let func = self
            .builder
            .get_insert_block()
            .and_then(|b| b.get_parent())
            .expect("emitting outside a function");
        let live = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                rc,
                i64t.const_int(Self::IMMORTAL_MIN, false),
                "live",
            )
            .expect("LLVM builder error");
        let write_bb = self.ctx.append_basic_block(func, "rc.write");
        let skip_bb = self.ctx.append_basic_block(func, "rc.skip");
        self.builder
            .build_conditional_branch(live, write_bb, skip_bb)
            .expect("LLVM builder error");
        self.builder.position_at_end(write_bb);
        self.builder.build_store(rc_ptr, new).expect("LLVM builder error");
        self.builder.build_unconditional_branch(skip_bb).expect("LLVM builder error");
        self.builder.position_at_end(skip_bb);
    }

    fn emit_rc_inc(&mut self, ptr: BasicValueEnum<'ctx>) {
        if self.parallel {
            self.call_rt("rill_retain", &[ptr.into()]);
            return;
        }
        let i64t = self.ctx.i64_type();
        let p = ptr.into_pointer_value();
        let rc = self
            .builder
            .build_load(i64t, p, "rc")
            .expect("LLVM builder error")
            .into_int_value();
        let inc = self
            .builder
            .build_int_add(rc, i64t.const_int(1, false), "inc")
            .expect("LLVM builder error");
        self.store_rc(p, rc, inc);
    }

    fn emit_retain(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty) {
        match ty {
            // Boxes, channels and strings are never null; closure
            // environments can be, so those keep the null-safe runtime call.
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Buf(_) => self.emit_rc_inc(v),
            Ty::Fn(..) => {
                let env = self
                    .builder
                    .build_extract_value(v.into_struct_value(), 1, "env")
                    .expect("LLVM builder error");
                self.call_rt("rill_retain", &[env.into()]);
            }
            _ => {}
        }
    }

    /// An operand the operation only borrows. A local can be handed over
    /// where it stands: whatever owns it outlives this expression, so the
    /// retain that a use would normally take, and the release that would undo
    /// it, are both nothing but counter traffic. Anything else is generated as
    /// usual and comes back owned, for the caller to release.
    fn gen_borrowed(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<(BasicValueEnum<'ctx>, bool), CodegenError> {
        if let TK::Local(name) = &e.kind {
            if let Some(Some(v)) = scope.get(name) {
                return Ok((*v, false));
            }
        }
        Ok((self.gen_expr(e, scope, subst, func)?.expect("value"), true))
    }

    /// -1 on an owned heap reference, freeing (recursively) at zero.
    fn emit_release(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty) {
        self.emit_release_call(v, ty, false);
    }

    /// The same, marked `tail`. Only for a release that is the last thing its
    /// function does: the promise is that the callee touches nothing of this
    /// frame, which lets the backend reuse it instead of stacking another.
    fn emit_release_tail(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty) {
        self.emit_release_call(v, ty, true);
    }

    fn emit_release_call(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty, tail: bool) {
        let call = match ty {
            Ty::Adt(..) => {
                let drop_fn = self.adt_drop(ty);
                Some(self.builder.build_call(drop_fn, &[v.into()], "").expect("LLVM builder error"))
            }
            Ty::Chan(..) => Some(self.call_rt("rill_chan_release", &[v.into()])),
            Ty::Buf(w) => {
                let size = self.ctx.i64_type().const_int(w.size_bytes(), false);
                Some(self.call_rt("rill_buf_release", &[v.into(), size.into()]))
            }
            Ty::Str => Some(self.call_rt("rill_str_release", &[v.into()])),
            Ty::StrBuf => Some(self.call_rt("rill_strbuf_release", &[v.into()])),
            Ty::Fn(..) => {
                let env = self
                    .builder
                    .build_extract_value(v.into_struct_value(), 1, "env")
                    .expect("LLVM builder error");
                Some(self.call_rt("rill_env_release", &[env.into()]))
            }
            _ => None,
        };
        if tail {
            if let Some(call) = call {
                call.set_tail_call(true);
            }
        }
    }

    /// Decrements a refcount in place and yields the new value. Single-worker
    /// programs take the plain path; once the runtime has been asked for more
    /// workers (`rill_mt`, fixed before any thread starts) the same code takes
    /// the atomic one. The branch is perfectly predicted either way.
    fn emit_rc_dec(
        &mut self,
        rc_ptr: PointerValue<'ctx>,
        func: FunctionValue<'ctx>,
    ) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let one = i64t.const_int(1, false);
        if !self.parallel {
            // Single-worker build: a plain load, subtract and store, with no
            // check to pay for.
            let rc = self
                .builder
                .build_load(i64t, rc_ptr, "rc")
                .expect("LLVM builder error")
                .into_int_value();
            let dec = self.builder.build_int_sub(rc, one, "dec").expect("LLVM builder error");
            self.store_rc(rc_ptr, rc, dec);
            return dec;
        }
        let mt = self.module.get_global("rill_mt").unwrap_or_else(|| {
            let g = self.module.add_global(self.ctx.i8_type(), None, "rill_mt");
            g.set_linkage(inkwell::module::Linkage::External);
            g
        });
        let flag = self
            .builder
            .build_load(self.ctx.i8_type(), mt.as_pointer_value(), "mt")
            .expect("LLVM builder error")
            .into_int_value();
        let shared = self
            .builder
            .build_int_compare(IntPredicate::NE, flag, self.ctx.i8_type().const_zero(), "shared")
            .expect("LLVM builder error");

        let atomic_bb = self.ctx.append_basic_block(func, "rc.atomic");
        let plain_bb = self.ctx.append_basic_block(func, "rc.plain");
        let cont_bb = self.ctx.append_basic_block(func, "rc.cont");
        self.builder
            .build_conditional_branch(shared, atomic_bb, plain_bb)
            .expect("LLVM builder error");

        self.builder.position_at_end(atomic_bb);
        // Same guard as the plain path: a constant's count lives in read-only
        // memory, and an atomic subtract would fault on it.
        let seen = self
            .builder
            .build_load(i64t, rc_ptr, "rc.seen")
            .expect("LLVM builder error")
            .into_int_value();
        let live = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                seen,
                i64t.const_int(Self::IMMORTAL_MIN, false),
                "live",
            )
            .expect("LLVM builder error");
        let do_bb = self.ctx.append_basic_block(func, "rc.atomic.do");
        self.builder
            .build_conditional_branch(live, do_bb, cont_bb)
            .expect("LLVM builder error");
        self.builder.position_at_end(do_bb);
        let old = self
            .builder
            .build_atomicrmw(
                inkwell::AtomicRMWBinOp::Sub,
                rc_ptr,
                one,
                inkwell::AtomicOrdering::AcquireRelease,
            )
            .expect("LLVM builder error");
        let dec_atomic =
            self.builder.build_int_sub(old, one, "dec").expect("LLVM builder error");
        self.builder.build_unconditional_branch(cont_bb).expect("LLVM builder error");

        self.builder.position_at_end(plain_bb);
        let rc = self
            .builder
            .build_load(i64t, rc_ptr, "rc")
            .expect("LLVM builder error")
            .into_int_value();
        let dec_plain = self.builder.build_int_sub(rc, one, "dec").expect("LLVM builder error");
        self.store_rc(rc_ptr, rc, dec_plain);
        // `store_rc` branches, so the block that reaches `cont` is the one it
        // left the builder in rather than `plain_bb` itself.
        let plain_end = self.builder.get_insert_block().expect("in a block");
        self.builder.build_unconditional_branch(cont_bb).expect("LLVM builder error");

        self.builder.position_at_end(cont_bb);
        let phi = self.builder.build_phi(i64t, "rc.new").expect("LLVM builder error");
        phi.add_incoming(&[
            (&dec_atomic, do_bb),
            (&seen, atomic_bb),
            (&dec_plain, plain_end),
        ]);
        phi.as_basic_value().into_int_value()
    }

    /// The type-specific drop function for a concrete ADT type:
    /// decrement, and at zero release heap fields and free the box.
    fn adt_drop(&mut self, ty: &Ty) -> FunctionValue<'ctx> {
        if let Some(&f) = self.adt_drops.get(ty) {
            return f;
        }
        let (adt, targs) = match ty {
            Ty::Adt(i, targs) => (*i, targs.clone()),
            _ => unreachable!("adt_drop on non-ADT"),
        };
        let ptrt = self.ctx.ptr_type(Default::default());
        let fn_ty = self.ctx.void_type().fn_type(&[ptrt.into()], false);
        let name = format!("drop${}", mangle(ty, &self.tmod.adts));
        let f = self.module.add_function(&name, fn_ty, None);
        // Cache before generating the body so recursive types terminate.
        self.adt_drops.insert(ty.clone(), f);

        let saved = self.builder.get_insert_block();
        let i64t = self.ctx.i64_type();
        let ptr = f.get_nth_param(0).unwrap().into_pointer_value();

        let entry = self.ctx.append_basic_block(f, "entry");
        let free_bb = self.ctx.append_basic_block(f, "free");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let hdr = self.header_struct();
        let rc_ptr =
            self.builder.build_struct_gep(hdr, ptr, 0, "rc.ptr").expect("LLVM builder error");
        let dec = self.emit_rc_dec(rc_ptr, f);
        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, dec, i64t.const_zero(), "dead")
            .expect("LLVM builder error");
        self.builder.build_conditional_branch(is_zero, free_bb, done_bb).expect("builder");

        // At zero: dispatch on the tag; each constructor releases its heap
        // fields and frees with its exact size (pool allocator fast path).
        // Nullary constructors are immortal singletons and never reach here.
        self.builder.position_at_end(free_bb);
        let ctors = self.tmod.adts[adt].ctors.clone();
        let tag_ptr =
            self.builder.build_struct_gep(hdr, ptr, 1, "tag.ptr").expect("LLVM builder error");
        let tag = self
            .builder
            .build_load(i64t, tag_ptr, "tag")
            .expect("LLVM builder error")
            .into_int_value();
        let mut cases = Vec::new();
        for (ci, c) in ctors.iter().enumerate() {
            if c.fields.is_empty() {
                continue;
            }
            let bb = self.ctx.append_basic_block(f, &format!("drop.{ci}"));
            cases.push((i64t.const_int(c.tag, false), bb, ci));
        }
        self.builder
            .build_switch(tag, done_bb, &cases.iter().map(|(v, b, _)| (*v, *b)).collect::<Vec<_>>())
            .expect("LLVM builder error");
        for (_, bb, ci) in cases {
            self.builder.position_at_end(bb);
            let struct_ty = self.ctor_struct(adt, ci, &targs);
            let fields = ctors[ci].fields.clone();
            // Every heap field is read out before the box goes, and the last
            // of them is released *after* it, by a tail call — so a list
            // releases its tail from the frame it is already in rather than
            // stacking one per link. Freeing first is what makes the call the
            // last thing this function does; nothing here touches the box
            // again, and Rill data cannot point back at it. Only one field
            // can have the last word: a chain running through some earlier
            // field of a multi-field constructor still recurses, and a deep
            // enough one still overflows.
            let mut heap = Vec::new();
            for (fi, ft) in fields.iter().enumerate() {
                let cf = subst_gen(ft, &targs);
                if Self::is_heap(&cf) {
                    let fp = self
                        .builder
                        .build_struct_gep(struct_ty, ptr, (fi + 2) as u32, "f")
                        .expect("LLVM builder error");
                    let fv = self
                        .builder
                        .build_load(self.llvm_type(&cf).unwrap(), fp, "fv")
                        .expect("LLVM builder error");
                    heap.push((fv, cf));
                }
            }
            let last = heap.pop();
            for (fv, cf) in heap {
                self.emit_release(fv, &cf);
            }
            let size = struct_ty.size_of().expect("sized struct");
            self.call_rt("rill_free_sized", &[ptr.into(), size.into()]);
            match last {
                Some((fv, cf)) => {
                    self.emit_release_tail(fv, &cf);
                    self.builder.build_return(None).expect("LLVM builder error");
                }
                None => {
                    self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
                }
            }
        }

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).expect("LLVM builder error");

        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        f
    }

    // ---- derived show / eq (M6) ------------------------------------------

    fn call_concat(&mut self, a: BasicValueEnum<'ctx>, b: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let f = self.module.get_function("rill_str_concat").unwrap();
        let call = self.builder.build_call(f, &[a.into(), b.into()], "cat").expect("builder");
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v,
            _ => unreachable!(),
        }
    }

    /// Renders an owned value of concrete type `ty` to an owned string,
    /// consuming the value.
    fn gen_show_of(
        &mut self,
        v: BasicValueEnum<'ctx>,
        ty: &Ty,
        line: usize,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // A user `impl Show(T)` replaces the derived rendering.
        if let Some(fn_idx) = self.user_impl(rill_types::SHOW_TRAIT, 0, ty) {
            let call = self.call_impl(fn_idx, ty, &[v.into()], "shown");
            return Ok(match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(s) => s,
                _ => unreachable!("`show` returns a string"),
            });
        }
        let rt = match ty {
            Ty::Int => Some("rill_int_to_str"),
            Ty::Float => Some("rill_float_to_str"),
            Ty::Bool => Some("rill_bool_to_str"),
            _ => None,
        };
        if let Some(rt) = rt {
            let arg: BasicMetadataValueEnum = if *ty == Ty::Bool {
                self.builder
                    .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "b")
                    .expect("LLVM builder error")
                    .into()
            } else {
                v.into()
            };
            let f = self.module.get_function(rt).unwrap();
            let call = self.builder.build_call(f, &[arg], "shown").expect("builder");
            return Ok(match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(s) => s,
                _ => unreachable!(),
            });
        }
        match ty {
            Ty::Str => Ok(v),
            Ty::Adt(..) => {
                let f = self.adt_show(ty, line)?;
                let call = self.builder.build_call(f, &[v.into()], "shown").expect("builder");
                let s = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(s) => s,
                    _ => unreachable!(),
                };
                self.emit_release(v, ty);
                Ok(s)
            }
            other => Err(CodegenError::new(
                line,
                format!("cannot show a value of type {}", mangle_friendly(other, &self.tmod.adts)),
            )),
        }
    }

    /// The generated structural renderer for one concrete ADT type:
    /// borrows the box, returns an owned string like `Cons(1, Nil)`.
    fn adt_show(&mut self, ty: &Ty, line: usize) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(&f) = self.adt_shows.get(ty) {
            return Ok(f);
        }
        let (adt, targs) = match ty {
            Ty::Adt(i, targs) => (*i, targs.clone()),
            _ => unreachable!("adt_show on non-ADT"),
        };
        let ptrt = self.ctx.ptr_type(Default::default());
        let fn_ty = ptrt.fn_type(&[ptrt.into()], false);
        let name = format!("show${}", mangle(ty, &self.tmod.adts));
        let f = self.module.add_function(&name, fn_ty, None);
        self.adt_shows.insert(ty.clone(), f);

        let saved = self.builder.get_insert_block();
        let i64t = self.ctx.i64_type();
        let ptr = f.get_nth_param(0).unwrap().into_pointer_value();
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let tag_ptr = self
            .builder
            .build_struct_gep(self.header_struct(), ptr, 1, "tag.ptr")
            .expect("LLVM builder error");
        let tag = self
            .builder
            .build_load(i64t, tag_ptr, "tag")
            .expect("LLVM builder error")
            .into_int_value();

        let default_bb = self.ctx.append_basic_block(f, "bad");
        let ctors = self.tmod.adts[adt].ctors.clone();
        let mut cases = Vec::new();
        for (ci, c) in ctors.iter().enumerate() {
            let bb = self.ctx.append_basic_block(f, &format!("show.{}", c.name));
            cases.push((i64t.const_int(c.tag, false), bb, ci));
        }
        self.builder
            .build_switch(tag, default_bb, &cases.iter().map(|(v, b, _)| (*v, *b)).collect::<Vec<_>>())
            .expect("LLVM builder error");
        self.builder.position_at_end(default_bb);
        self.builder.build_unreachable().expect("LLVM builder error");

        for (_, bb, ci) in cases {
            self.builder.position_at_end(bb);
            let c = &ctors[ci];
            if c.fields.is_empty() {
                let s = self.const_str(&c.name);
                self.builder.build_return(Some(&s)).expect("LLVM builder error");
                continue;
            }
            let struct_ty = self.ctor_struct(adt, ci, &targs);
            // A tuple shows the way it is written, without the name standing
            // in for the notation.
            let head = if is_tuple_ctor(&c.name, c.fields.len()) {
                "(".to_string()
            } else {
                format!("{}(", c.name)
            };
            let mut acc: BasicValueEnum<'ctx> = self.const_str(&head).into();
            let fields = c.fields.clone();
            for (fi, fty) in fields.iter().enumerate() {
                if fi > 0 {
                    let sep: BasicValueEnum<'ctx> = self.const_str(", ").into();
                    let next = self.call_concat(acc, sep);
                    self.emit_release(acc, &Ty::Str);
                    self.emit_release(sep, &Ty::Str);
                    acc = next;
                }
                let cf = subst_gen(fty, &targs);
                let fp = self
                    .builder
                    .build_struct_gep(struct_ty, ptr, (fi + 2) as u32, "f")
                    .expect("LLVM builder error");
                let fv = self
                    .builder
                    .build_load(self.llvm_type(&cf).unwrap(), fp, "fv")
                    .expect("LLVM builder error");
                if Self::is_heap(&cf) {
                    self.emit_retain(fv, &cf);
                }
                let fs = self.gen_show_of(fv, &cf, line)?;
                let next = self.call_concat(acc, fs);
                self.emit_release(acc, &Ty::Str);
                self.emit_release(fs, &Ty::Str);
                acc = next;
            }
            let close: BasicValueEnum<'ctx> = self.const_str(")").into();
            let out = self.call_concat(acc, close);
            self.emit_release(acc, &Ty::Str);
            self.emit_release(close, &Ty::Str);
            self.builder.build_return(Some(&out)).expect("LLVM builder error");
        }

        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }

    /// Structural equality for values of one concrete type; both operands
    /// are borrowed.
    fn gen_eq_of(
        &mut self,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
        ty: &Ty,
        line: usize,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match ty {
            Ty::Int | Ty::Bool => Ok(self
                .builder
                .build_int_compare(IntPredicate::EQ, a.into_int_value(), b.into_int_value(), "eq")
                .expect("LLVM builder error")),
            Ty::Float => Ok(self
                .builder
                .build_float_compare(
                    FloatPredicate::OEQ,
                    a.into_float_value(),
                    b.into_float_value(),
                    "eq",
                )
                .expect("LLVM builder error")),
            Ty::Str => {
                let f = self.module.get_function("rill_str_eq").unwrap();
                let call =
                    self.builder.build_call(f, &[a.into(), b.into()], "streq").expect("builder");
                let raw = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                    _ => unreachable!(),
                };
                Ok(self
                    .builder
                    .build_int_compare(
                        IntPredicate::NE,
                        raw,
                        self.ctx.i8_type().const_zero(),
                        "eq",
                    )
                    .expect("LLVM builder error"))
            }
            Ty::Adt(..) => {
                let f = self.adt_eq(ty, line)?;
                let call =
                    self.builder.build_call(f, &[a.into(), b.into()], "eq").expect("builder");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                    _ => unreachable!(),
                })
            }
            other => Err(CodegenError::new(
                line,
                format!(
                    "cannot compare values of type {}",
                    mangle_friendly(other, &self.tmod.adts)
                ),
            )),
        }
    }

    /// The generated structural equality for one concrete ADT type.
    fn adt_eq(&mut self, ty: &Ty, line: usize) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(&f) = self.adt_eqs.get(ty) {
            return Ok(f);
        }
        let (adt, targs) = match ty {
            Ty::Adt(i, targs) => (*i, targs.clone()),
            _ => unreachable!("adt_eq on non-ADT"),
        };
        let ptrt = self.ctx.ptr_type(Default::default());
        let fn_ty = self.ctx.bool_type().fn_type(&[ptrt.into(), ptrt.into()], false);
        let name = format!("eq${}", mangle(ty, &self.tmod.adts));
        let f = self.module.add_function(&name, fn_ty, None);
        self.adt_eqs.insert(ty.clone(), f);

        let saved = self.builder.get_insert_block();
        let i64t = self.ctx.i64_type();
        let pa = f.get_nth_param(0).unwrap().into_pointer_value();
        let pb = f.get_nth_param(1).unwrap().into_pointer_value();
        let entry = self.ctx.append_basic_block(f, "entry");
        let neq_bb = self.ctx.append_basic_block(f, "neq");
        let same_tag_bb = self.ctx.append_basic_block(f, "same.tag");

        self.builder.position_at_end(entry);
        let hdr = self.header_struct();
        let ta_ptr =
            self.builder.build_struct_gep(hdr, pa, 1, "ta.ptr").expect("LLVM builder error");
        let tb_ptr =
            self.builder.build_struct_gep(hdr, pb, 1, "tb.ptr").expect("LLVM builder error");
        let ta = self.builder.build_load(i64t, ta_ptr, "ta").expect("builder").into_int_value();
        let tb = self.builder.build_load(i64t, tb_ptr, "tb").expect("builder").into_int_value();
        let same =
            self.builder.build_int_compare(IntPredicate::EQ, ta, tb, "same").expect("builder");
        self.builder.build_conditional_branch(same, same_tag_bb, neq_bb).expect("builder");

        self.builder.position_at_end(neq_bb);
        self.builder
            .build_return(Some(&self.ctx.bool_type().const_zero()))
            .expect("LLVM builder error");

        self.builder.position_at_end(same_tag_bb);
        let ctors = self.tmod.adts[adt].ctors.clone();
        let mut cases = Vec::new();
        for (ci, c) in ctors.iter().enumerate() {
            let bb = self.ctx.append_basic_block(f, &format!("eq.{}", c.name));
            cases.push((i64t.const_int(c.tag, false), bb, ci));
        }
        self.builder
            .build_switch(ta, neq_bb, &cases.iter().map(|(v, b, _)| (*v, *b)).collect::<Vec<_>>())
            .expect("LLVM builder error");

        for (_, bb, ci) in cases {
            self.builder.position_at_end(bb);
            let c = &ctors[ci];
            let mut acc = self.ctx.bool_type().const_int(1, false);
            if !c.fields.is_empty() {
                let struct_ty = self.ctor_struct(adt, ci, &targs);
                let fields = c.fields.clone();
                for (fi, fty) in fields.iter().enumerate() {
                    let cf = subst_gen(fty, &targs);
                    let fap = self
                        .builder
                        .build_struct_gep(struct_ty, pa, (fi + 2) as u32, "fa")
                        .expect("LLVM builder error");
                    let fbp = self
                        .builder
                        .build_struct_gep(struct_ty, pb, (fi + 2) as u32, "fb")
                        .expect("LLVM builder error");
                    let ll = self.llvm_type(&cf).unwrap();
                    let fa = self.builder.build_load(ll, fap, "fav").expect("builder");
                    let fb = self.builder.build_load(ll, fbp, "fbv").expect("builder");
                    let c_eq = self.gen_eq_of(fa, fb, &cf, line)?;
                    acc = self.builder.build_and(acc, c_eq, "and").expect("LLVM builder error");
                }
            }
            self.builder.build_return(Some(&acc)).expect("LLVM builder error");
        }

        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }

    // ---- instances -------------------------------------------------------

    fn instance_name(&self, fn_idx: usize, subst: &[Ty]) -> String {
        let f = &self.tmod.fns[fn_idx];
        if f.name == "main" {
            return "rill_main".into();
        }
        if subst.is_empty() {
            f.name.clone()
        } else {
            let mut s = f.name.clone();
            for t in subst {
                s.push('$');
                s.push_str(&mangle(t, &self.tmod.adts));
            }
            s
        }
    }

    /// The symbol of one instance. A helper specialized on the callbacks it
    /// forwards gets one name per callback, since each is a different body.
    fn key_name(&self, key: &InstanceKey) -> String {
        let mut s = self.instance_name(key.0, &key.1);
        for (pi, (cb, cbsubst)) in &key.2 {
            s.push_str(&format!("$p{pi}${}", self.instance_name(*cb, cbsubst)));
        }
        s
    }

    fn request_instance(
        &mut self,
        fn_idx: usize,
        subst: Vec<Ty>,
        bound: Vec<(usize, FnVal)>,
    ) -> FunctionValue<'ctx> {
        let f = &self.tmod.fns[fn_idx];
        assert_eq!(
            subst.len(),
            f.ngen,
            "internal error: instantiation arity mismatch for `{}`",
            f.name
        );
        let key = (fn_idx, subst, bound);
        if let Some(&v) = self.instances.get(&key) {
            return v;
        }
        let params: Vec<Ty> = f.params.iter().map(|(_, t)| subst_gen(t, &key.1)).collect();
        let ret = subst_gen(&f.ret, &key.1);
        let fn_type = self.fn_llvm_type(&params, &ret, false);
        let name = self.key_name(&key);
        let v = self.module.add_function(&name, fn_type, None);
        self.instances.insert(key.clone(), v);
        self.queue.push_back(key);
        v
    }

    /// The compile-time function an expression stands for, if it is one: a
    /// named function, or a parameter a caller has already pinned to one.
    fn known_fn(&mut self, e: &TExpr, subst: &[Ty]) -> Option<FnVal> {
        match &e.kind {
            TK::FnRef { fn_idx, subst: inner } => {
                Some((*fn_idx, inner.iter().map(|t| self.concrete(t, subst)).collect()))
            }
            TK::Local(name) => self.bound_fns.get(name).cloned(),
            _ => None,
        }
    }

    /// Records what a binding names, so `cmp = up` may be handed to C just as
    /// `up` could. A binding of anything else shadows whatever the name meant
    /// before, and has to clear it.
    fn rebind_known_fn(&mut self, name: &str, value: &TExpr, subst: &[Ty]) {
        match self.known_fn(value, subst) {
            Some(v) => {
                self.bound_fns.insert(name.to_string(), v);
            }
            None => {
                self.bound_fns.remove(name);
            }
        }
    }

    /// Resolves the arguments a callee needs pinned, for its instance key.
    fn bind_static_args(
        &mut self,
        fn_idx: usize,
        args: &[TExpr],
        subst: &[Ty],
        line: usize,
    ) -> Result<Vec<(usize, FnVal)>, CodegenError> {
        let mut bound = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if self.static_params[fn_idx].get(i) != Some(&true) {
                continue;
            }
            match self.known_fn(a, subst) {
                Some(v) => bound.push((i, v)),
                None => {
                    let f = &self.tmod.fns[fn_idx];
                    return Err(CodegenError::new(
                        line,
                        format!(
                            "`{}` hands its parameter `{}` to C as a callback, so it has \
                             to be a function known while compiling — pass a top-level \
                             function by name, not a lambda or a value chosen at run time",
                            f.name, f.params[i].0
                        ),
                    ));
                }
            }
        }
        Ok(bound)
    }

    /// Checks the requirements a generic function's body imposed on its type
    /// parameters, now that the instantiation is known.
    fn check_constraints(&self, key: &InstanceKey) -> Result<(), CodegenError> {
        let f = &self.tmod.fns[key.0];
        for (g, c, line) in &f.constraints {
            let ty = &key.1[*g as usize];
            let ok = match c {
                Constraint::Numeric => matches!(ty, Ty::Int | Ty::Float),
                Constraint::Addable => matches!(ty, Ty::Int | Ty::Float | Ty::Str),
                Constraint::Ordered => {
                    matches!(ty, Ty::Int | Ty::Float | Ty::Str)
                        || self.user_impl(rill_types::ORD_TRAIT, 0, ty).is_some()
                }
                Constraint::Basic => {
                    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Adt(..))
                }
            };
            if !ok {
                let what = match c {
                    Constraint::Numeric => "be Int or Float",
                    Constraint::Addable => "support `+`",
                    Constraint::Ordered => "be ordered (Int, Float, Str or an `Ord` impl)",
                    Constraint::Basic => "be printable",
                };
                return Err(CodegenError::new(
                    *line,
                    format!(
                        "`{}` cannot be used at {}: this type parameter must {what}",
                        f.name,
                        mangle_friendly(ty, &self.tmod.adts)
                    ),
                ));
            }
        }
        Ok(())
    }

    fn gen_instance(&mut self, key: &InstanceKey) -> Result<(), CodegenError> {
        self.check_constraints(key)?;
        let (fn_idx, subst, bound) = key;
        self.cur_fn = *fn_idx;
        let fn_val = self.instances[key];
        let f = &self.tmod.fns[*fn_idx];
        // What the caller pinned, under the names the body knows them by. The
        // parameter is still passed as an ordinary closure value, so calling
        // it normally works as before; this only records what it must be.
        let outer_bound = std::mem::take(&mut self.bound_fns);
        for (pi, v) in bound {
            self.bound_fns.insert(f.params[*pi].0.clone(), v.clone());
        }
        let entry = self.ctx.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);
        let (dbg_name, dbg_line) = (f.name.clone(), f.line as u32);
        self.begin_debug_fn(&dbg_name, dbg_line, fn_val);

        let outer_cleanup = std::mem::take(&mut self.cleanup);
        let mut scope: Scope<'ctx> = HashMap::new();
        for (i, (pname, pty)) in f.params.iter().enumerate() {
            let raw = fn_val.get_nth_param(i as u32).unwrap();
            raw.set_name(pname);
            scope.insert(pname.clone(), Some(raw));
            let cty = subst_gen(pty, subst);
            if Self::is_heap(&cty) {
                self.cleanup.push((raw, cty));
            }
        }

        let ret_ty = subst_gen(&f.ret, subst);
        let body = &self.tmod.fns[*fn_idx].body;
        self.gen_tail(body, &mut scope, subst, fn_val, &ret_ty)?;
        self.cleanup = outer_cleanup;
        self.bound_fns = outer_bound;
        self.end_debug_fn();
        Ok(())
    }

    /// Generates a function body in tail position: every control-flow leaf
    /// emits the pending releases (parameters, bindings, live scrutinees) and
    /// its own `ret`. Direct/indirect calls in tail position are released
    /// *before* the call and marked `tail`, so tail recursion — including
    /// functions with heap parameters — compiles to a loop in constant stack.
    fn gen_tail(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
        ret_ty: &Ty,
    ) -> Result<(), CodegenError> {
        match &e.kind {
            TK::Block(stmts) => {
                let (last, init) = match stmts.split_last() {
                    Some((TStmt::Expr(last), init)) => (Some(last), init),
                    _ => (None, &stmts[..]),
                };
                // What the block's own bindings name is forgotten with them,
                // so a name that meant one function inside cannot be taken
                // for it outside.
                let saved_bound = self.bound_fns.clone();
                for stmt in init {
                    match stmt {
                        TStmt::Bind { name, value } => {
                            let v = self.gen_expr(value, scope, subst, func)?;
                            scope.insert(name.clone(), v);
                            self.rebind_known_fn(name, value, subst);
                            if let Some(v) = v {
                                let ty = self.concrete(&value.ty, subst);
                                if Self::is_heap(&ty) {
                                    self.cleanup.push((v, ty));
                                }
                            }
                        }
                        TStmt::Expr(v) => {
                            let out = self.gen_expr(v, scope, subst, func)?;
                            if let Some(out) = out {
                                let ty = self.concrete(&v.ty, subst);
                                if Self::is_heap(&ty) {
                                    self.emit_release(out, &ty);
                                }
                            }
                        }
                    }
                }
                let out = match last {
                    Some(last) => self.gen_tail(last, scope, subst, func, ret_ty),
                    None => self.finish_leaf(None, &Ty::Unit, ret_ty),
                };
                self.bound_fns = saved_bound;
                out
            }
            TK::If { cond, then, els } => {
                let cv = self.gen_expr(cond, scope, subst, func)?.expect("value").into_int_value();
                let then_bb = self.ctx.append_basic_block(func, "then");
                let else_bb = self.ctx.append_basic_block(func, "else");
                self.builder
                    .build_conditional_branch(cv, then_bb, else_bb)
                    .expect("LLVM builder error");
                let saved_cleanup = self.cleanup.clone();
                self.builder.position_at_end(then_bb);
                let mut then_scope = scope.clone();
                self.gen_tail(then, &mut then_scope, subst, func, ret_ty)?;
                self.cleanup = saved_cleanup;
                self.builder.position_at_end(else_bb);
                let mut else_scope = scope.clone();
                self.gen_tail(els, &mut else_scope, subst, func, ret_ty)
            }
            TK::Match { scrutinee, arms } => {
                self.gen_tail_match(scrutinee, arms, scope, subst, func, ret_ty)
            }
            TK::CallFn { fn_idx, subst: inner, args } => {
                // Before the arguments, so there is as little live across the
                // yield as possible.
                if self.preempt && self.call_goes_round(*fn_idx) {
                    self.emit_preempt_check(func);
                }
                let out_ty = self.concrete(&e.ty, subst);
                let inner: Vec<Ty> = inner.iter().map(|t| self.concrete(t, subst)).collect();
                let bound = self.bind_static_args(*fn_idx, args, subst, e.line)?;
                let callee = self.request_instance(*fn_idx, inner, bound);
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for a in args {
                    // A binding this frame owns and hands to the call is
                    // moved, not copied: the retain a use would take and the
                    // release that follows it cancel exactly, and every loop
                    // written as a tail call passes its whole working set.
                    let v = match self.take_owned(a, scope) {
                        Some(v) => v,
                        None => self.gen_expr(a, scope, subst, func)?.expect("arguments are never Unit"),
                    };
                    argv.push(v.into());
                }
                // Everything this frame still owns dies before the call, which
                // is what makes it a genuine tail call.
                self.flush_cleanup();
                let call =
                    self.builder.build_call(callee, &argv, "tcall").expect("LLVM builder error");
                call.set_tail_call(true);
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                self.ret_leaf(out, &out_ty, ret_ty)
            }
            _ => {
                let ty = self.concrete(&e.ty, subst);
                let out = self.gen_expr(e, scope, subst, func)?;
                self.finish_leaf(out, &ty, ret_ty)
            }
        }
    }

    /// Emits every pending release (reverse order) without clearing the list
    /// permanently — each control-flow leaf flushes its own copy.
    /// A local whose one reference this frame owns and is about to give away.
    /// Taking it out of the cleanup list is the whole move: nothing retains it
    /// and nothing releases it here.
    fn take_owned(
        &mut self,
        e: &TExpr,
        scope: &Scope<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let TK::Local(name) = &e.kind else { return None };
        let v = (*scope.get(name)?)?;
        let i = self.cleanup.iter().position(|(c, _)| *c == v)?;
        self.cleanup.remove(i);
        Some(v)
    }

    fn flush_cleanup(&mut self) {
        let items = self.cleanup.clone();
        for (v, ty) in items.into_iter().rev() {
            self.emit_release(v, &ty);
        }
    }

    /// Leaf that still owns its frame: release everything, then return.
    fn finish_leaf(
        &mut self,
        out: Option<BasicValueEnum<'ctx>>,
        out_ty: &Ty,
        ret_ty: &Ty,
    ) -> Result<(), CodegenError> {
        self.flush_cleanup();
        self.ret_leaf(out, out_ty, ret_ty)
    }

    /// Lets the scheduler have this worker back if it has asked for it.
    ///
    /// Emitted before a tail call, because that is where a Rill loop goes
    /// round: the language has no `while`, so a strand that computes forever
    /// is a function calling itself in tail position, and without this it
    /// would hold its worker until the program ended. Everything already
    /// waiting on that worker would wait for good — a started strand is
    /// pinned to the stack its frames live on and cannot be picked up
    /// elsewhere.
    ///
    /// The load has to be volatile or it would be lifted out of the loop it
    /// exists to interrupt, which is the whole cost: one byte read and a
    /// branch that is not taken. Programs with no strands in them do not get
    /// this at all, so nothing that never spawns pays for it.
    /// Whether a tail call from the function being generated to `callee` is a
    /// loop going round — which is true exactly when `callee` can get back
    /// here. Direct recursion is the common one; two functions calling each
    /// other is the same loop written differently, and a check on only the
    /// first would leave the second able to spin forever.
    ///
    /// Answered by walking out from the callee, memoised per callee, and only
    /// ever asked in a program that has strands in it.
    fn call_goes_round(&mut self, callee: usize) -> bool {
        let caller = self.cur_fn;
        if callee == caller {
            return true;
        }
        if let Some(reached) = self.reaches.get(&callee) {
            return reached.contains(&caller);
        }
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue = vec![callee];
        while let Some(i) = queue.pop() {
            for &e in &self.callgraph[i] {
                if seen.insert(e) {
                    queue.push(e);
                }
            }
        }
        let answer = seen.contains(&caller);
        self.reaches.insert(callee, seen);
        answer
    }

    fn emit_preempt_check(&mut self, func: FunctionValue<'ctx>) {
        if !self.preempt {
            return;
        }
        let i8t = self.ctx.i8_type();
        let flag = self.module.get_global("rill_preempt").unwrap_or_else(|| {
            let g = self.module.add_global(i8t, None, "rill_preempt");
            g.set_linkage(inkwell::module::Linkage::External);
            g
        });
        let load = self
            .builder
            .build_load(i8t, flag.as_pointer_value(), "preempt")
            .expect("LLVM builder error");
        load.as_instruction_value()
            .expect("a load is an instruction")
            .set_volatile(true)
            .expect("a load can be volatile");
        let asked = self
            .builder
            .build_int_compare(IntPredicate::NE, load.into_int_value(), i8t.const_zero(), "asked")
            .expect("LLVM builder error");
        let yield_bb = self.ctx.append_basic_block(func, "preempt.yield");
        let cont_bb = self.ctx.append_basic_block(func, "preempt.cont");
        self.builder
            .build_conditional_branch(asked, yield_bb, cont_bb)
            .expect("LLVM builder error");
        self.builder.position_at_end(yield_bb);
        self.call_rt("rill_preempt_yield", &[]);
        self.builder.build_unconditional_branch(cont_bb).expect("LLVM builder error");
        self.builder.position_at_end(cont_bb);
    }

    /// Emits the final `ret`, discarding (and releasing) the value when the
    /// function returns Unit.
    fn ret_leaf(
        &mut self,
        out: Option<BasicValueEnum<'ctx>>,
        out_ty: &Ty,
        ret_ty: &Ty,
    ) -> Result<(), CodegenError> {
        match self.llvm_type(ret_ty) {
            None => {
                if let Some(v) = out {
                    if Self::is_heap(out_ty) {
                        self.emit_release(v, out_ty);
                    }
                }
                self.builder.build_return(None).expect("LLVM builder error");
            }
            Some(_) => {
                let v = out.expect("checker guarantees a value for non-Unit returns");
                self.builder.build_return(Some(&v)).expect("LLVM builder error");
            }
        }
        Ok(())
    }

    /// Match in tail position: each arm returns on its own, so there is no
    /// merge block; the scrutinee joins the pending releases.
    fn gen_tail_match(
        &mut self,
        scrutinee: &TExpr,
        arms: &[TArm],
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
        ret_ty: &Ty,
    ) -> Result<(), CodegenError> {
        self.compile_match(&Ty::Unit, scrutinee, arms, scope, subst, func, Some(ret_ty))?;
        Ok(())
    }

    /// A shim adapting a named function to the closure calling convention
    /// (ignored env pointer first).
    fn request_shim(&mut self, fn_idx: usize, subst: Vec<Ty>) -> FunctionValue<'ctx> {
        let key = (fn_idx, subst);
        if let Some(&v) = self.shims.get(&key) {
            return v;
        }
        let target = self.request_instance(fn_idx, key.1.clone(), vec![]);
        let f = &self.tmod.fns[key.0];
        let params: Vec<Ty> = f.params.iter().map(|(_, t)| subst_gen(t, &key.1)).collect();
        let ret = subst_gen(&f.ret, &key.1);
        let shim_ty = self.fn_llvm_type(&params, &ret, true);
        let name = format!("{}$shim", self.instance_name(key.0, &key.1));
        let shim = self.module.add_function(&name, shim_ty, None);

        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(shim, "entry");
        self.builder.position_at_end(entry);
        let args: Vec<BasicMetadataValueEnum> =
            (1..shim.count_params()).map(|i| shim.get_nth_param(i).unwrap().into()).collect();
        let call = self.builder.build_call(target, &args, "").expect("LLVM builder error");
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => {
                self.builder.build_return(Some(&v)).expect("LLVM builder error");
            }
            _ => {
                self.builder.build_return(None).expect("LLVM builder error");
            }
        }
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        self.shims.insert(key, shim);
        shim
    }

    /// A C-callable trampoline around a Rill function, so a library can call
    /// back into the program. C hands the arguments at its own widths and has
    /// nowhere to put an environment, which is why the callback has to be a
    /// named top-level function rather than a closure: what crosses is a bare
    /// code pointer. A result travels back the same way an `extern` call's
    /// does, narrowed to the width the signature declared.
    ///
    /// The function may be named here or have arrived as a parameter, which
    /// the caller pinned to a particular function when it asked for this
    /// instance — either way it is settled before any code is emitted.
    fn gen_callback(
        &mut self,
        arg: &TExpr,
        cparams: &[CTy],
        cret: &CTy,
        subst: &[Ty],
    ) -> Result<FunctionValue<'ctx>, CodegenError> {
        let Some((fn_idx, inner)) = self.known_fn(arg, subst) else {
            return Err(CodegenError::new(
                arg.line,
                "a callback must be a function known while compiling — name a \
                 top-level function, or take one as a parameter and let the \
                 callers name it. A lambda or a closure carries an environment, \
                 and C has nowhere to put it"
                    .to_string(),
            ));
        };
        let key = (fn_idx, inner.clone(), cparams.to_vec(), cret.clone());
        if let Some(&v) = self.callbacks.get(&key) {
            return Ok(v);
        }

        let f = &self.tmod.fns[fn_idx];
        let ret = subst_gen(&f.ret, &inner);
        if Self::is_heap(&ret) {
            return Err(CodegenError::new(
                arg.line,
                format!(
                    "callback `{}` returns `{}`, which C discards and nothing \
                     would ever release — return Unit or a number",
                    f.name,
                    mangle_friendly(&ret, &self.tmod.adts)
                ),
            ));
        }
        let target = self.request_instance(fn_idx, inner.clone(), vec![]);

        let ps: Vec<BasicMetadataTypeEnum> = cparams
            .iter()
            .map(|c| self.c_type(c).expect("a callback parameter is never Unit").into())
            .collect();
        let fty = match self.c_type(cret) {
            Some(r) => r.fn_type(&ps, false),
            None => self.ctx.void_type().fn_type(&ps, false),
        };
        let name = format!("{}$c", self.instance_name(fn_idx, &inner));
        let tramp = self.module.add_function(&name, fty, None);

        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(tramp, "entry");
        self.builder.position_at_end(entry);
        let args: Vec<BasicMetadataValueEnum> = cparams
            .iter()
            .enumerate()
            .map(|(i, c)| self.from_c(tramp.get_nth_param(i as u32).unwrap(), c).into())
            .collect();
        let call = self.builder.build_call(target, &args, "").expect("LLVM builder error");
        match call.try_as_basic_value() {
            // The checker has already unified the function's result with the
            // declared one, so what comes back only needs narrowing.
            inkwell::values::ValueKind::Basic(v) if self.c_type(cret).is_some() => {
                let narrowed = self.to_c(v, cret);
                self.builder.build_return(Some(&narrowed)).expect("LLVM builder error");
            }
            _ => {
                self.builder.build_return(None).expect("LLVM builder error");
            }
        }
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }

        self.callbacks.insert(key, tramp);
        Ok(tramp)
    }

    /// Whether anything in the program can end up waiting: a strand, a
    /// channel, or a socket. Asked of the finished module rather than of the
    /// source, so it counts what was actually emitted — a channel in a
    /// function nothing reaches does not drag a scheduler in.
    fn needs_scheduler(&self) -> bool {
        const WAITERS: [&str; 12] = [
            "rill_spawn",
            "rill_chan_new",
            "rill_chan_send",
            "rill_chan_recv",
            "rill_chan_release",
            "rill_select",
            "rill_tcp_listen",
            "rill_tcp_accept",
            "rill_tcp_connect",
            "rill_sock_read",
            "rill_sock_write",
            "rill_sock_close",
        ];
        WAITERS.iter().any(|n| {
            self.module
                .get_function(n)
                .is_some_and(|f| f.as_global_value().get_first_use().is_some())
        })
    }

    /// Refuses what a wasm build cannot honour, rather than emitting a module
    /// that links and then aborts on its first spawn — which is what the
    /// runtime's stubs would otherwise do.
    ///
    /// Strands need a context switch, and wasm has nothing to switch: the call
    /// stack belongs to the engine and is not memory the module can address.
    /// This is a property of the target, not a gap to be filled in later.
    fn check_wasm_supported(&self) -> Result<(), CodegenError> {
        // A named library is a shared object to be found at link time, and a
        // wasm module has no such thing — it imports from its host instead. A
        // bare `extern fn` is different: it resolves against wasi-libc like
        // any other C symbol, so it is left alone.
        if let Some(lib) = self.tmod.libs.iter().find(|l| !l.starts_with('-')) {
            let name = lib.strip_prefix("framework:").unwrap_or(lib);
            return Err(CodegenError::new(
                0,
                format!(
                    "cannot link the library `{name}` into a wasm module: a wasm module \
                     imports from its host rather than linking a shared object"
                ),
            ));
        }
        if !self.needs_scheduler() {
            return Ok(());
        }
        // `needs_scheduler` has already said there is one; this is only to
        // find where, and it looks at the functions that were instantiated so
        // that a channel in a function nothing reaches is not blamed for it.
        let mut reached: Vec<usize> = self.instances.keys().map(|(i, ..)| *i).collect();
        reached.sort_unstable();
        reached.dedup();
        // Only the program's own functions can be pointed at. Reaching a
        // channel through a prelude function — `listen_on` opening a socket,
        // say — is still worth refusing, but citing a line of the prelude
        // under the program's file name would send the author somewhere that
        // does not exist.
        let (line, what) = reached
            .iter()
            .filter(|i| **i >= self.tmod.prelude_fns)
            .find_map(|i| find_unsupported(&self.tmod.fns[*i].body))
            .or_else(|| {
                reached
                    .iter()
                    .find_map(|i| find_unsupported(&self.tmod.fns[*i].body))
                    .map(|(_, what)| (0, what))
            })
            .unwrap_or((0, "this program"));
        Err(CodegenError::new(
            line,
            format!(
                "{what} needs the scheduler, which wasm cannot run: a context switch means \
                 saving the stack pointer, and a wasm module's call stack belongs to the \
                 engine. Build this program for a native target."
            ),
        ))
    }

    /// The real C `main`: runs `rill_main` as the main strand under the
    /// scheduler, then reports allocations and returns 0.
    fn gen_main_wrapper(&mut self) {
        // A module with exports is a reactor: it stays instantiated and the
        // host calls in, so there is no `_start`, no argv, and no moment at
        // which an allocation report would mean anything. `main` becomes the
        // module's `init` — the host calls it once before anything else, and
        // what it sets up stays live for the exports to use.
        if self.wasm && self.tmod.fns.iter().any(|f| f.exported) {
            let void = self.ctx.void_type();
            let init = self.module.add_function("rill_init", void.fn_type(&[], false), None);
            let entry = self.ctx.append_basic_block(init, "entry");
            self.builder.position_at_end(entry);
            let user_main = self.instances[&(self.tmod.fn_index["main"], vec![], vec![])];
            let rt_run = self.module.get_function("rill_rt_run_direct").unwrap();
            self.builder
                .build_call(rt_run, &[user_main.as_global_value().as_pointer_value().into()], "")
                .expect("LLVM builder error");
            self.builder.build_return(None).expect("LLVM builder error");
            return;
        }
        let i32t = self.ctx.i32_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        // What wasi-libc's startup code calls. Its `_start` calls `__main_void`,
        // which calls this; a function actually named `main` is never looked
        // for and the program traps on an unresolved weak symbol instead.
        let name = if self.wasm { "__main_argc_argv" } else { "main" };
        let main =
            self.module.add_function(name, i32t.fn_type(&[i32t.into(), ptrt.into()], false), None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        let argc = self
            .builder
            .build_int_s_extend(
                main.get_nth_param(0).unwrap().into_int_value(),
                self.ctx.i64_type(),
                "argc",
            )
            .expect("LLVM builder error");
        let argv = main.get_nth_param(1).unwrap();
        self.call_rt("rill_set_args", &[argc.into(), argv.into()]);
        let user_main = self.instances[&(self.tmod.fn_index["main"], vec![], vec![])];
        // A program with no concurrency in it does not need a scheduler, and
        // saying so here is what lets the linker leave one out of the binary.
        let entry =
            if self.needs_scheduler() { "rill_rt_run" } else { "rill_rt_run_direct" };
        let rt_run = self.module.get_function(entry).unwrap();
        self.builder
            .build_call(
                rt_run,
                &[user_main.as_global_value().as_pointer_value().into()],
                "",
            )
            .expect("LLVM builder error");
        self.call_rt("rill_alloc_report", &[]);
        self.builder.build_return(Some(&i32t.const_int(0, false))).expect("LLVM builder error");
    }

    // ---- expressions -----------------------------------------------------

    fn concrete(&self, ty: &Ty, subst: &[Ty]) -> Ty {
        subst_gen(ty, subst)
    }

    /// The user's implementation of a built-in trait for a concrete type, if
    /// they wrote one.
    fn user_impl(&self, trait_id: usize, method: usize, ty: &Ty) -> Option<usize> {
        rill_types::impl_key(ty)
            .and_then(|k| self.tmod.impl_methods.get(&(trait_id, method, k)))
            .copied()
    }

    /// Calls a trait method implementation on a receiver of concrete type
    /// `recv_ty`; a generic impl is instantiated at the receiver's arguments.
    fn call_impl(
        &mut self,
        fn_idx: usize,
        recv_ty: &Ty,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> inkwell::values::CallSiteValue<'ctx> {
        let targs = rill_types::impl_targs(recv_ty);
        let callee = self.request_instance(fn_idx, targs, vec![]);
        self.builder.build_call(callee, args, name).expect("LLVM builder error")
    }

    /// Static trait dispatch: the concrete receiver type selects the impl.
    fn resolve_impl(
        &mut self,
        trait_id: usize,
        method: usize,
        recv: &Ty,
        subst: &[Ty],
        line: usize,
    ) -> Result<usize, CodegenError> {
        let recv_c = self.concrete(recv, subst);
        rill_types::impl_key(&recv_c)
            .and_then(|k| self.tmod.impl_methods.get(&(trait_id, method, k)))
            .copied()
            .ok_or_else(|| {
                CodegenError::new(
                    line,
                    format!(
                        "no implementation of trait `{}` for {}",
                        self.tmod.traits[trait_id].name,
                        mangle_friendly(&recv_c, &self.tmod.adts)
                    ),
                )
            })
    }

    fn gen_expr(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        self.at_line(e.line);
        match &e.kind {
            TK::Int(v) => Ok(Some(self.ctx.i64_type().const_int(*v as u64, true).into())),
            TK::Float(v) => Ok(Some(self.ctx.f64_type().const_float(*v).into())),
            TK::Str(s) => Ok(Some(self.const_str(s).into())),
            TK::Bool(v) => Ok(Some(self.ctx.bool_type().const_int(*v as u64, false).into())),
            // Bit operations are instructions, not calls.
            // Converting between the numeric types: the only intrinsics whose
            // operand is not an integer, so they come first.
            // `s[i:j]`: the runtime takes a start and a length, so the end is
            // turned into one here. Each part is evaluated once, which is why
            // this is a node of its own rather than a rewrite.
            TK::Slice { base, from, to } => {
                let (b, owned) = self.gen_borrowed(base, scope, subst, func)?;
                let f = self
                    .gen_expr(from, scope, subst, func)?
                    .expect("value")
                    .into_int_value();
                let end = match to {
                    Some(t) => {
                        self.gen_expr(t, scope, subst, func)?.expect("value").into_int_value()
                    }
                    None => {
                        let call = self.call_rt("rill_str_len", &[b.into()]);
                        match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                            _ => unreachable!("rill_str_len returns a length"),
                        }
                    }
                };
                let len = self.builder.build_int_sub(end, f, "slicelen").expect("builder");
                let call = self.call_rt("rill_str_sub", &[b.into(), f.into(), len.into()]);
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => unreachable!("rill_str_sub returns a string"),
                };
                // The runtime borrows what it is handed; a string this
                // expression made is then its to release.
                if owned {
                    self.emit_release(b, &Ty::Str);
                }
                Ok(Some(out))
            }
            TK::Builtin { rt, args } if *rt == "rill.sitofp" || *rt == "rill.fptosi" => {
                let v = self.gen_expr(&args[0], scope, subst, func)?.expect("value");
                let out = if *rt == "rill.sitofp" {
                    self.builder
                        .build_signed_int_to_float(
                            v.into_int_value(),
                            self.ctx.f64_type(),
                            "tofloat",
                        )
                        .expect("LLVM builder error")
                        .into()
                } else {
                    // Truncates toward zero, like C.
                    self.builder
                        .build_float_to_signed_int(
                            v.into_float_value(),
                            self.ctx.i64_type(),
                            "toint",
                        )
                        .expect("LLVM builder error")
                        .into()
                };
                Ok(Some(out))
            }
            TK::Builtin { rt, args } if rt.starts_with("rill.") => {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(
                        self.gen_expr(a, scope, subst, func)?.expect("value").into_int_value(),
                    );
                }
                let b = &self.builder;
                let out = match *rt {
                    "rill.and" => b.build_and(vals[0], vals[1], "and"),
                    "rill.or" => b.build_or(vals[0], vals[1], "or"),
                    "rill.xor" => b.build_xor(vals[0], vals[1], "xor"),
                    "rill.not" => b.build_not(vals[0], "not"),
                    "rill.shl" => b.build_left_shift(vals[0], vals[1], "shl"),
                    // Logical: protocol code works with bit patterns, not
                    // signed magnitudes.
                    "rill.shr" => b.build_right_shift(vals[0], vals[1], false, "shr"),
                    other => unreachable!("unknown intrinsic {other}"),
                };
                Ok(Some(out.expect("LLVM builder error").into()))
            }
            TK::Builtin { rt, args } => {
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::new();
                let mut owned: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
                for a in args {
                    let ty = self.concrete(&a.ty, subst);
                    // The runtime borrows what it is passed, so a local goes
                    // in as it stands.
                    let (v, held) = self.gen_borrowed(a, scope, subst, func)?;
                    let passed = if ty == Ty::Bool {
                        self.builder
                            .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "b")
                            .expect("LLVM builder error")
                            .into()
                    } else {
                        v
                    };
                    argv.push(passed.into());
                    if held && Self::is_heap(&ty) {
                        owned.push((v, ty));
                    }
                }
                let f = self.module.get_function(rt).unwrap();
                let call = self.builder.build_call(f, &argv, rt).expect("LLVM builder error");
                let mut out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                // Bool-returning runtime functions use i8 at the ABI level.
                if self.concrete(&e.ty, subst) == Ty::Bool {
                    if let Some(v) = out {
                        out = Some(
                            self.builder
                                .build_int_truncate(
                                    v.into_int_value(),
                                    self.ctx.bool_type(),
                                    "b",
                                )
                                .expect("LLVM builder error")
                                .into(),
                        );
                    }
                }
                for (v, ty) in owned {
                    self.emit_release(v, &ty);
                }
                Ok(out)
            }
            TK::Local(name) => {
                let v = scope[name];
                if let Some(v) = v {
                    let ty = self.concrete(&e.ty, subst);
                    self.emit_retain(v, &ty);
                }
                Ok(v)
            }
            TK::FnRef { fn_idx, subst: inner } => {
                // A helper specialized on the callbacks it forwards exists in
                // as many copies as it has callers, so there is no one address
                // to hand out for it.
                if self.static_params[*fn_idx].iter().any(|b| *b) {
                    return Err(CodegenError::new(
                        e.line,
                        format!(
                            "`{}` cannot be used as a value: it hands a parameter to C \
                             as a callback, so it is compiled once per callback it is \
                             called with — call it by name instead",
                            self.tmod.fns[*fn_idx].name
                        ),
                    ));
                }
                let inner: Vec<Ty> = inner.iter().map(|t| self.concrete(t, subst)).collect();
                let shim = self.request_shim(*fn_idx, inner);
                let null = self.ctx.ptr_type(Default::default()).const_null();
                let agg = self.closure_type().const_named_struct(&[
                    shim.as_global_value().as_pointer_value().into(),
                    null.into(),
                ]);
                Ok(Some(agg.into()))
            }
            TK::CallFn { fn_idx, subst: inner, args } => {
                let inner: Vec<Ty> = inner.iter().map(|t| self.concrete(t, subst)).collect();
                let bound = self.bind_static_args(*fn_idx, args, subst, e.line)?;
                let callee = self.request_instance(*fn_idx, inner, bound);
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.gen_expr(a, scope, subst, func)?;
                    argv.push(v.expect("arguments are never Unit").into());
                }
                let call =
                    self.builder.build_call(callee, &argv, "call").expect("LLVM builder error");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                })
            }
            TK::BufNew { width, count } => {
                let n = self.gen_expr(count, scope, subst, func)?.expect("value");
                let size = self.ctx.i64_type().const_int(width.size_bytes(), false);
                let f = self.module.get_function("rill_buf_new").unwrap();
                let call = self
                    .builder
                    .build_call(f, &[n.into(), size.into()], "buf")
                    .expect("LLVM builder error");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => unreachable!("rill_buf_new returns a pointer"),
                })
            }
            TK::BufLen { buf } => {
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let len = self.buf_len(b.into_pointer_value());
                if owned {
                    self.emit_release(b, &self.concrete(&buf.ty, subst));
                }
                Ok(Some(len.into()))
            }
            TK::BufPtr { buf } => {
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let data = self.buf_data(b.into_pointer_value());
                if owned {
                    self.emit_release(b, &self.concrete(&buf.ty, subst));
                }
                Ok(Some(data.into()))
            }
            TK::BufGet { buf, idx } => {
                let bty = self.concrete(&buf.ty, subst);
                let w = match &bty {
                    Ty::Buf(w) => w.clone(),
                    _ => unreachable!("checked as a buffer"),
                };
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let i = self.gen_expr(idx, scope, subst, func)?.expect("value").into_int_value();
                let slot = self.buf_slot(b.into_pointer_value(), i, &w, func);
                let loaded = self
                    .builder
                    .build_load(self.c_type(&w).expect("a width"), slot, "bget")
                    .expect("LLVM builder error");
                let out = self.from_c(loaded, &w);
                if owned {
                    self.emit_release(b, &bty);
                }
                Ok(Some(out))
            }
            TK::BufSet { buf, idx, val } => {
                let bty = self.concrete(&buf.ty, subst);
                let w = match &bty {
                    Ty::Buf(w) => w.clone(),
                    _ => unreachable!("checked as a buffer"),
                };
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let i = self.gen_expr(idx, scope, subst, func)?.expect("value").into_int_value();
                let v = self.gen_expr(val, scope, subst, func)?.expect("value");
                let slot = self.buf_slot(b.into_pointer_value(), i, &w, func);
                let narrowed = self.to_c(v, &w);
                self.builder.build_store(slot, narrowed).expect("LLVM builder error");
                if owned {
                    self.emit_release(b, &bty);
                }
                Ok(None)
            }
            TK::CallExtern { name, args } => {
                let sig = self.tmod.externs[name].clone();
                let callee = self.module.get_function(name).expect("declared above");
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for (a, pt) in args.iter().zip(&sig.params) {
                    // A callback does not evaluate to a value C can hold: the
                    // named function is wrapped in a C-ABI trampoline and it
                    // is the trampoline's address that crosses.
                    if let CTy::Fn(cps, cret) = pt {
                        let trampoline = self.gen_callback(a, cps, cret, subst)?;
                        argv.push(trampoline.as_global_value().as_pointer_value().into());
                        continue;
                    }
                    let v = self.gen_expr(a, scope, subst, func)?.expect("value");
                    argv.push(self.to_c(v, pt).into());
                }
                let call =
                    self.builder.build_call(callee, &argv, "cffi").expect("LLVM builder error");
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                Ok(out.map(|v| self.from_c(v, &sig.ret)))
            }
            TK::MethodCall { trait_id, method, recv, args } => {
                let recv_ty = self.concrete(recv, subst);
                let fn_idx = self.resolve_impl(*trait_id, *method, recv, subst, e.line)?;
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for a in args {
                    let v = self.gen_expr(a, scope, subst, func)?;
                    argv.push(v.expect("arguments are never Unit").into());
                }
                let call = self.call_impl(fn_idx, &recv_ty, &argv, "mcall");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                })
            }
            TK::CallCtor { adt, ctor, targs, args } => {
                let targs: Vec<Ty> = targs.iter().map(|t| self.concrete(t, subst)).collect();
                // Nullary constructors are immortal singletons: no allocation,
                // and retains/releases on them are harmless counter churn.
                if args.is_empty() {
                    let key = (Ty::Adt(*adt, targs.clone()), *ctor);
                    if let Some(&p) = self.nullary_singletons.get(&key) {
                        return Ok(Some(p.into()));
                    }
                    let i64t = self.ctx.i64_type();
                    let ty = self.ctx.struct_type(&[i64t.into(), i64t.into()], false);
                    let sig = &self.tmod.adts[*adt].ctors[*ctor];
                    let init = ty.const_named_struct(&[
                        i64t.const_int(Self::IMMORTAL, false).into(),
                        i64t.const_int(sig.tag, false).into(),
                    ]);
                    let name =
                        format!("nul${}${}", mangle(&key.0, &self.tmod.adts), sig.name);
                    let g = self.module.add_global(ty, None, &name);
                    g.set_initializer(&init);
                    g.set_linkage(inkwell::module::Linkage::Private);
                    g.set_constant(true);
                    let p = g.as_pointer_value();
                    self.nullary_singletons.insert(key, p);
                    return Ok(Some(p.into()));
                }
                // Evaluate fields before allocating (an argument may allocate).
                let mut field_vals = Vec::with_capacity(args.len());
                for a in args {
                    field_vals.push(self.gen_expr(a, scope, subst, func)?);
                }
                let struct_ty = self.ctor_struct(*adt, *ctor, &targs);
                let sig = &self.tmod.adts[*adt].ctors[*ctor];
                let name = format!("{}.box", sig.name);
                let tag = sig.tag;
                let ptr = self.alloc_object(struct_ty, &name);
                let tag_ptr = self
                    .builder
                    .build_struct_gep(struct_ty, ptr, 1, "tag")
                    .expect("LLVM builder error");
                self.builder
                    .build_store(tag_ptr, self.ctx.i64_type().const_int(tag, false))
                    .expect("LLVM builder error");
                for (i, v) in field_vals.into_iter().enumerate() {
                    let fp = self
                        .builder
                        .build_struct_gep(struct_ty, ptr, (i + 2) as u32, "field")
                        .expect("LLVM builder error");
                    self.builder
                        .build_store(fp, v.expect("ADT fields are never Unit"))
                        .expect("LLVM builder error");
                }
                Ok(Some(ptr.into()))
            }
            TK::CallClosure { callee, args } => {
                let callee_ty = self.concrete(&callee.ty, subst);
                let (params, ret) = match &callee_ty {
                    Ty::Fn(p, r) => (p.clone(), (**r).clone()),
                    other => unreachable!("closure call on non-fn type {other:?}"),
                };
                let cv = self
                    .gen_expr(callee, scope, subst, func)?
                    .expect("closures are values")
                    .into_struct_value();
                let code = self
                    .builder
                    .build_extract_value(cv, 0, "code")
                    .expect("LLVM builder error")
                    .into_pointer_value();
                let env = self
                    .builder
                    .build_extract_value(cv, 1, "env")
                    .expect("LLVM builder error");
                let mut argv: Vec<BasicMetadataValueEnum> = vec![env.into()];
                for a in args {
                    let v = self.gen_expr(a, scope, subst, func)?;
                    argv.push(v.expect("arguments are never Unit").into());
                }
                let fn_ty = self.fn_llvm_type(&params, &ret, true);
                let call = self
                    .builder
                    .build_indirect_call(fn_ty, code, &argv, "ccall")
                    .expect("LLVM builder error");
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                // The callee expression produced an owned closure; the call
                // itself only borrows it.
                self.emit_release(cv.into(), &callee_ty);
                Ok(out)
            }
            TK::Println(arg) => {
                let ty = self.concrete(&arg.ty, subst);
                let v = self.gen_expr(arg, scope, subst, func)?;
                let v = v.expect("println argument is never Unit");
                if matches!(ty, Ty::Adt(..)) {
                    // Data values print through their derived show function.
                    let s = self.gen_show_of(v, &ty, e.line)?;
                    let rt = self.module.get_function("rill_println_str").unwrap();
                    self.builder.build_call(rt, &[s.into()], "").expect("LLVM builder error");
                    self.emit_release(s, &Ty::Str);
                    return Ok(None);
                }
                let (rt_name, rt_args): (&str, Vec<BasicMetadataValueEnum>) = match ty {
                    Ty::Int => ("rill_println_int", vec![v.into()]),
                    Ty::Float => ("rill_println_float", vec![v.into()]),
                    Ty::Bool => {
                        let ext = self
                            .builder
                            .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "b")
                            .unwrap();
                        ("rill_println_bool", vec![ext.into()])
                    }
                    Ty::Str => ("rill_println_str", vec![v.into()]),
                    other => {
                        return Err(CodegenError::new(
                            e.line,
                            format!(
                                "cannot print {}",
                                mangle_friendly(&other, &self.tmod.adts)
                            ),
                        ));
                    }
                };
                let rt = self.module.get_function(rt_name).unwrap();
                self.builder.build_call(rt, &rt_args, "").expect("LLVM builder error");
                if Self::is_heap(&ty) {
                    self.emit_release(v, &ty);
                }
                Ok(None)
            }
            TK::Print(arg) => {
                let ty = self.concrete(&arg.ty, subst);
                let v = self.gen_expr(arg, scope, subst, func)?;
                let v = v.expect("print argument is never Unit");
                if matches!(ty, Ty::Adt(..)) {
                    let s = self.gen_show_of(v, &ty, e.line)?;
                    let rt = self.module.get_function("rill_print_str").unwrap();
                    self.builder.build_call(rt, &[s.into()], "").expect("LLVM builder error");
                    self.emit_release(s, &Ty::Str);
                    return Ok(None);
                }
                let (rt_name, rt_args): (&str, Vec<BasicMetadataValueEnum>) = match ty {
                    Ty::Int => ("rill_print_int", vec![v.into()]),
                    Ty::Float => ("rill_print_float", vec![v.into()]),
                    Ty::Bool => {
                        let ext = self
                            .builder
                            .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "b")
                            .unwrap();
                        ("rill_print_bool", vec![ext.into()])
                    }
                    Ty::Str => ("rill_print_str", vec![v.into()]),
                    other => {
                        return Err(CodegenError::new(
                            e.line,
                            format!("cannot print {}", mangle_friendly(&other, &self.tmod.adts)),
                        ));
                    }
                };
                let rt = self.module.get_function(rt_name).unwrap();
                self.builder.build_call(rt, &rt_args, "").expect("LLVM builder error");
                if Self::is_heap(&ty) {
                    self.emit_release(v, &ty);
                }
                Ok(None)
            }
            TK::Show(arg) => {
                let ty = self.concrete(&arg.ty, subst);
                let v = self.gen_expr(arg, scope, subst, func)?;
                let v = v.expect("show argument is never Unit");
                Ok(Some(self.gen_show_of(v, &ty, e.line)?))
            }
            TK::Neg(inner) => {
                let v = self.gen_expr(inner, scope, subst, func)?.expect("value");
                Ok(Some(match self.concrete(&inner.ty, subst) {
                    Ty::Int => self
                        .builder
                        .build_int_neg(v.into_int_value(), "neg")
                        .expect("LLVM builder error")
                        .into(),
                    Ty::Float => self
                        .builder
                        .build_float_neg(v.into_float_value(), "neg")
                        .expect("LLVM builder error")
                        .into(),
                    other => unreachable!("negation of {other:?}"),
                }))
            }
            TK::Binary { op, lhs, rhs } => {
                let lt = self.concrete(&lhs.ty, subst);
                let l = self.gen_expr(lhs, scope, subst, func)?.expect("value");
                let r = self.gen_expr(rhs, scope, subst, func)?.expect("value");
                Ok(Some(self.gen_binary(*op, &lt, l, r, e.line)?))
            }
            TK::If { cond, then, els } => {
                let cv = self.gen_expr(cond, scope, subst, func)?.expect("value").into_int_value();
                let then_bb = self.ctx.append_basic_block(func, "then");
                let else_bb = self.ctx.append_basic_block(func, "else");
                self.builder
                    .build_conditional_branch(cv, then_bb, else_bb)
                    .expect("LLVM builder error");

                self.builder.position_at_end(then_bb);
                let tv = self.gen_expr(then, scope, subst, func)?;
                let t_end = self.builder.get_insert_block().unwrap();

                self.builder.position_at_end(else_bb);
                let ev = self.gen_expr(els, scope, subst, func)?;
                let e_end = self.builder.get_insert_block().unwrap();

                let ty = self.concrete(&e.ty, subst);
                self.merge_branches(func, &ty, vec![(tv, t_end), (ev, e_end)])
            }
            TK::Match { scrutinee, arms } => {
                self.gen_match(e, scrutinee, arms, scope, subst, func)
            }
            TK::Block(stmts) => {
                let mut last = None;
                // As in tail position: the block's bindings do not outlive it.
                let saved_bound = self.bound_fns.clone();
                for (i, stmt) in stmts.iter().enumerate() {
                    match stmt {
                        TStmt::Bind { name, value } => {
                            let v = self.gen_expr(value, scope, subst, func)?;
                            scope.insert(name.clone(), v);
                            self.rebind_known_fn(name, value, subst);
                            // The binding owns this reference; released at
                            // function exit.
                            if let Some(v) = v {
                                let ty = self.concrete(&value.ty, subst);
                                if Self::is_heap(&ty) {
                                    self.cleanup.push((v, ty));
                                }
                            }
                            last = None;
                        }
                        TStmt::Expr(v) => {
                            let out = self.gen_expr(v, scope, subst, func)?;
                            if i + 1 < stmts.len() {
                                // Discarded statement value: release now.
                                if let Some(out) = out {
                                    let ty = self.concrete(&v.ty, subst);
                                    if Self::is_heap(&ty) {
                                        self.emit_release(out, &ty);
                                    }
                                }
                                last = None;
                            } else {
                                last = out;
                            }
                        }
                    }
                }
                self.bound_fns = saved_bound;
                Ok(last)
            }
            TK::Lambda { params, captures, body } => {
                self.gen_lambda(params, captures, body, scope, subst)
            }
            TK::ChanNew(cap) => {
                let n = match cap {
                    Some(c) => self.gen_expr(c, scope, subst, func)?.expect("value"),
                    None => self.ctx.i64_type().const_zero().into(),
                };
                let f = self.module.get_function("rill_chan_new").unwrap();
                let call =
                    self.builder.build_call(f, &[n.into()], "chan").expect("LLVM builder error");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => unreachable!(),
                })
            }
            TK::ChanSend { ch, val } => {
                let ch_ty = self.concrete(&ch.ty, subst);
                let val_ty = self.concrete(&val.ty, subst);
                let cv = self.gen_expr(ch, scope, subst, func)?.expect("value");
                let vv = self.gen_expr(val, scope, subst, func)?.expect("value");
                let (a, b) = self.pack_words(vv, &val_ty);
                let f = self.module.get_function("rill_chan_send").unwrap();
                self.builder
                    .build_call(f, &[cv.into(), a.into(), b.into()], "")
                    .expect("LLVM builder error");
                // The value's ownership moved into the channel; the channel
                // reference itself was only borrowed by the call.
                self.emit_release(cv, &ch_ty);
                Ok(None)
            }
            TK::ChanRecv { ch } => {
                let ch_ty = self.concrete(&ch.ty, subst);
                let elem_ty = self.concrete(&e.ty, subst);
                let cv = self.gen_expr(ch, scope, subst, func)?.expect("value");
                let f = self.module.get_function("rill_chan_recv").unwrap();
                let call = self.builder.build_call(f, &[cv.into()], "recv").expect("builder");
                let words = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v.into_struct_value(),
                    _ => unreachable!(),
                };
                let a = self
                    .builder
                    .build_extract_value(words, 0, "w0")
                    .expect("LLVM builder error")
                    .into_int_value();
                let b = self
                    .builder
                    .build_extract_value(words, 1, "w1")
                    .expect("LLVM builder error")
                    .into_int_value();
                let out = self.unpack_words(a, b, &elem_ty);
                self.emit_release(cv, &ch_ty);
                Ok(Some(out))
            }
            TK::Select { arms, default } => {
                self.gen_select(e, arms, default.as_deref(), scope, subst, func)
            }
            TK::Spawn { captures, body } => {
                let (lifted, env_ptr) =
                    self.build_closure(&[], captures, body, scope, subst, true)?;
                let f = self.module.get_function("rill_spawn").unwrap();
                // env ownership transfers to the strand (released after
                // the body runs).
                self.builder
                    .build_call(
                        f,
                        &[lifted.as_global_value().as_pointer_value().into(), env_ptr.into()],
                        "",
                    )
                    .expect("LLVM builder error");
                Ok(None)
            }
        }
    }

    /// An immortal string literal: a writable global `{ rc, len, bytes }`
    /// whose refcount never reaches zero.
    fn const_str(&mut self, s: &str) -> PointerValue<'ctx> {
        let name = format!(".str{}", self.str_count);
        self.str_count += 1;
        let i64t = self.ctx.i64_type();
        let bytes = self.ctx.const_string(s.as_bytes(), false);
        let ty = self
            .ctx
            .struct_type(&[i64t.into(), i64t.into(), bytes.get_type().into()], false);
        let init = ty.const_named_struct(&[
            i64t.const_int(Self::IMMORTAL, false).into(),
            i64t.const_int(s.len() as u64, false).into(),
            bytes.into(),
        ]);
        let g = self.module.add_global(ty, None, &name);
        g.set_initializer(&init);
        g.set_linkage(inkwell::module::Linkage::Private);
        g.set_constant(true);
        g.as_pointer_value()
    }

    fn gen_binary(
        &mut self,
        op: BinOp,
        operand_ty: &Ty,
        l: BasicValueEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
        line: usize,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        // Ordering comparisons on strings and on types with an `Ord` impl
        // both reduce to a three-way compare against zero.
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
            && !matches!(operand_ty, Ty::Int | Ty::Float)
        {
            let ordering = match operand_ty {
                Ty::Str => {
                    let f = self.module.get_function("rill_str_cmp").unwrap();
                    let call = self
                        .builder
                        .build_call(f, &[l.into(), r.into()], "strcmp")
                        .expect("LLVM builder error");
                    self.emit_release(l, &Ty::Str);
                    self.emit_release(r, &Ty::Str);
                    call
                }
                _ => {
                    let fn_idx =
                        self.user_impl(rill_types::ORD_TRAIT, 0, operand_ty).ok_or_else(|| {
                            CodegenError::new(
                                line,
                                format!(
                                    "no implementation of trait `Ord` for {}",
                                    mangle_friendly(operand_ty, &self.tmod.adts)
                                ),
                            )
                        })?;
                    self.call_impl(fn_idx, operand_ty, &[l.into(), r.into()], "cmp")
                }
            };
            let ord = match ordering.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                _ => unreachable!("compare returns a value"),
            };
            let pred = match op {
                BinOp::Lt => IntPredicate::SLT,
                BinOp::Le => IntPredicate::SLE,
                BinOp::Gt => IntPredicate::SGT,
                BinOp::Ge => IntPredicate::SGE,
                _ => unreachable!(),
            };
            let zero = self.ctx.i64_type().const_zero();
            return Ok(self
                .builder
                .build_int_compare(pred, ord, zero, "ord")
                .expect("LLVM builder error")
                .into());
        }

        if matches!(operand_ty, Ty::Adt(..)) {
            let eq_fn = self.adt_eq(operand_ty, line)?;
            let call = self
                .builder
                .build_call(eq_fn, &[l.into(), r.into()], "eq")
                .expect("LLVM builder error");
            let raw = match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                _ => unreachable!(),
            };
            let out: BasicValueEnum<'ctx> = match op {
                BinOp::Eq => raw.into(),
                BinOp::Ne => self.builder.build_not(raw, "ne").expect("builder").into(),
                _ => unreachable!("checker restricts data-type operators"),
            };
            self.emit_release(l, operand_ty);
            self.emit_release(r, operand_ty);
            return Ok(out);
        }
        let b = &self.builder;
        Ok(match operand_ty {
            Ty::Int => {
                let (l, r) = (l.into_int_value(), r.into_int_value());
                match op {
                    BinOp::Add => b.build_int_add(l, r, "add").unwrap().into(),
                    BinOp::Sub => b.build_int_sub(l, r, "sub").unwrap().into(),
                    BinOp::Mul => b.build_int_mul(l, r, "mul").unwrap().into(),
                    BinOp::Div => b.build_int_signed_div(l, r, "div").unwrap().into(),
                    BinOp::Mod => b.build_int_signed_rem(l, r, "rem").unwrap().into(),
                    cmp => {
                        let pred = match cmp {
                            BinOp::Eq => IntPredicate::EQ,
                            BinOp::Ne => IntPredicate::NE,
                            BinOp::Lt => IntPredicate::SLT,
                            BinOp::Le => IntPredicate::SLE,
                            BinOp::Gt => IntPredicate::SGT,
                            BinOp::Ge => IntPredicate::SGE,
                            _ => unreachable!(),
                        };
                        b.build_int_compare(pred, l, r, "cmp").unwrap().into()
                    }
                }
            }
            Ty::Float => {
                let (l, r) = (l.into_float_value(), r.into_float_value());
                match op {
                    BinOp::Add => b.build_float_add(l, r, "add").unwrap().into(),
                    BinOp::Sub => b.build_float_sub(l, r, "sub").unwrap().into(),
                    BinOp::Mul => b.build_float_mul(l, r, "mul").unwrap().into(),
                    BinOp::Div => b.build_float_div(l, r, "div").unwrap().into(),
                    cmp => {
                        let pred = match cmp {
                            BinOp::Eq => FloatPredicate::OEQ,
                            BinOp::Ne => FloatPredicate::ONE,
                            BinOp::Lt => FloatPredicate::OLT,
                            BinOp::Le => FloatPredicate::OLE,
                            BinOp::Gt => FloatPredicate::OGT,
                            BinOp::Ge => FloatPredicate::OGE,
                            _ => unreachable!("checker rejects `%` on Float"),
                        };
                        b.build_float_compare(pred, l, r, "cmp").unwrap().into()
                    }
                }
            }
            Ty::Bool => {
                let (l, r) = (l.into_int_value(), r.into_int_value());
                let pred = match op {
                    BinOp::Eq => IntPredicate::EQ,
                    BinOp::Ne => IntPredicate::NE,
                    _ => unreachable!("checker restricts Bool operators"),
                };
                b.build_int_compare(pred, l, r, "cmp").unwrap().into()
            }
            Ty::Str => {
                let out: BasicValueEnum<'ctx> = match op {
                    BinOp::Add => {
                        let f = self.module.get_function("rill_str_concat").unwrap();
                        let call = self
                            .builder
                            .build_call(f, &[l.into(), r.into()], "concat")
                            .unwrap();
                        match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => v,
                            _ => unreachable!(),
                        }
                    }
                    BinOp::Eq | BinOp::Ne => {
                        let f = self.module.get_function("rill_str_eq").unwrap();
                        let call = self
                            .builder
                            .build_call(f, &[l.into(), r.into()], "streq")
                            .unwrap();
                        let raw = match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                            _ => unreachable!(),
                        };
                        let is_eq = self
                            .builder
                            .build_int_compare(
                                IntPredicate::NE,
                                raw,
                                self.ctx.i8_type().const_zero(),
                                "streq.b",
                            )
                            .unwrap();
                        if matches!(op, BinOp::Eq) {
                            is_eq.into()
                        } else {
                            self.builder.build_not(is_eq, "strne").unwrap().into()
                        }
                    }
                    _ => unreachable!("checker restricts Str operators"),
                };
                self.emit_release(l, &Ty::Str);
                self.emit_release(r, &Ty::Str);
                out
            }
            other => unreachable!("binary op on {other:?}"),
        })
    }

    fn gen_lambda(
        &mut self,
        params: &[(String, Ty)],
        captures: &[(String, Ty)],
        body: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let (lifted, env_ptr) = self.build_closure(params, captures, body, scope, subst, false)?;
        let undef = self.closure_type().get_undef();
        let s0 = self
            .builder
            .build_insert_value(undef, lifted.as_global_value().as_pointer_value(), 0, "clo")
            .expect("LLVM builder error");
        let s1 = self
            .builder
            .build_insert_value(s0, env_ptr, 1, "clo")
            .expect("LLVM builder error");
        Ok(Some(s1.into_struct_value().into()))
    }

    /// Lifts a closure body into a function taking `(env, params...)` and
    /// allocates its capture environment. With `discard_result` the lifted
    /// function returns void and releases any heap result (used by `spawn`).
    fn build_closure(
        &mut self,
        params: &[(String, Ty)],
        captures: &[(String, Ty)],
        body: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        discard_result: bool,
    ) -> Result<(FunctionValue<'ctx>, PointerValue<'ctx>), CodegenError> {
        let param_tys: Vec<Ty> = params.iter().map(|(_, t)| self.concrete(t, subst)).collect();
        let ret_ty =
            if discard_result { Ty::Unit } else { self.concrete(&body.ty, subst) };
        let body_ty = self.concrete(&body.ty, subst);
        let cap_tys: Vec<Ty> = captures.iter().map(|(_, t)| self.concrete(t, subst)).collect();

        // Environment layout: { i64 rc, ptr drop_fn, captures... }.
        let ptrt = self.ctx.ptr_type(Default::default());
        let mut env_members: Vec<BasicTypeEnum> =
            vec![self.ctx.i64_type().into(), ptrt.into()];
        for t in &cap_tys {
            env_members.push(self.llvm_type(t).expect("captures are never Unit"));
        }
        let env_ty = self.ctx.struct_type(&env_members, false);

        // The lifted function.
        let id = self.lambda_count;
        self.lambda_count += 1;
        let lifted_ty = self.fn_llvm_type(&param_tys, &ret_ty, true);
        let lifted = self.module.add_function(&format!("lambda${id}"), lifted_ty, None);

        let saved = self.builder.get_insert_block();
        let outer_cleanup = std::mem::take(&mut self.cleanup);
        let entry = self.ctx.append_basic_block(lifted, "entry");
        self.builder.position_at_end(entry);
        let mut inner_scope: Scope<'ctx> = HashMap::new();
        let env_param = lifted.get_nth_param(0).unwrap().into_pointer_value();
        for (i, (name, _)) in captures.iter().enumerate() {
            let fp = self
                .builder
                .build_struct_gep(env_ty, env_param, (i + 2) as u32, name)
                .expect("LLVM builder error");
            let raw = self
                .builder
                .build_load(env_members[i + 2], fp, name)
                .expect("LLVM builder error");
            // Borrowed from the env, which outlives the call.
            inner_scope.insert(name.clone(), Some(raw));
        }
        for (i, (name, _)) in params.iter().enumerate() {
            let raw = lifted.get_nth_param((i + 1) as u32).unwrap();
            raw.set_name(name);
            inner_scope.insert(name.clone(), Some(raw));
            if Self::is_heap(&param_tys[i]) {
                self.cleanup.push((raw, param_tys[i].clone()));
            }
        }
        let _ = body_ty;
        // Captures arrive through the environment as ordinary runtime values,
        // so no name in here is a function settled at compile time.
        let outer_bound = std::mem::take(&mut self.bound_fns);
        self.gen_tail(body, &mut inner_scope, subst, lifted, &ret_ty)?;
        self.bound_fns = outer_bound;
        self.cleanup = outer_cleanup;

        // The environment's drop function: decrement; at zero release heap
        // captures and free.
        let env_drop = if captures.is_empty() {
            None
        } else {
            let drop_ty = self.ctx.void_type().fn_type(&[ptrt.into()], false);
            let d = self.module.add_function(&format!("dropenv${id}"), drop_ty, None);
            let entry = self.ctx.append_basic_block(d, "entry");
            let free_bb = self.ctx.append_basic_block(d, "free");
            let done_bb = self.ctx.append_basic_block(d, "done");
            let i64t = self.ctx.i64_type();
            let p = d.get_nth_param(0).unwrap().into_pointer_value();
            self.builder.position_at_end(entry);
            let rc_ptr =
                self.builder.build_struct_gep(env_ty, p, 0, "rc.ptr").expect("builder");
            let dec = self.emit_rc_dec(rc_ptr, d);
            let is_zero = self
                .builder
                .build_int_compare(IntPredicate::EQ, dec, i64t.const_zero(), "dead")
                .expect("LLVM builder error");
            self.builder
                .build_conditional_branch(is_zero, free_bb, done_bb)
                .expect("LLVM builder error");
            self.builder.position_at_end(free_bb);
            for (i, cty) in cap_tys.iter().enumerate() {
                if Self::is_heap(cty) {
                    let fp = self
                        .builder
                        .build_struct_gep(env_ty, p, (i + 2) as u32, "cap")
                        .expect("LLVM builder error");
                    let cv = self
                        .builder
                        .build_load(env_members[i + 2], fp, "capv")
                        .expect("LLVM builder error");
                    self.emit_release(cv, cty);
                }
            }
            let env_size = env_ty.size_of().expect("sized struct");
            self.call_rt("rill_free_sized", &[p.into(), env_size.into()]);
            self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
            self.builder.position_at_end(done_bb);
            self.builder.build_return(None).expect("LLVM builder error");
            Some(d)
        };

        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }

        // Allocate and fill the environment at the definition site; the env
        // owns its captures, so each heap capture is retained.
        let env_ptr = if captures.is_empty() {
            self.ctx.ptr_type(Default::default()).const_null()
        } else {
            let p = self.alloc_object(env_ty, "env");
            let dp = self
                .builder
                .build_struct_gep(env_ty, p, 1, "drop.ptr")
                .expect("LLVM builder error");
            self.builder
                .build_store(dp, env_drop.unwrap().as_global_value().as_pointer_value())
                .expect("LLVM builder error");
            for (i, (name, _)) in captures.iter().enumerate() {
                let v = scope[name].expect("captures are never Unit");
                self.emit_retain(v, &cap_tys[i]);
                let fp = self
                    .builder
                    .build_struct_gep(env_ty, p, (i + 2) as u32, name)
                    .expect("LLVM builder error");
                self.builder.build_store(fp, v).expect("LLVM builder error");
            }
            p
        };

        Ok((lifted, env_ptr))
    }

    // ---- debug info ------------------------------------------------------

    /// Starts a debug scope for one generated function, so its instructions
    /// can be attributed to source lines.
    fn begin_debug_fn(&mut self, name: &str, line: u32, fn_val: FunctionValue<'ctx>) {
        let Some(dbg) = self.debug.as_mut() else { return };
        let file = dbg.unit.get_file();
        // Rill's types are not described yet; an unspecified signature is
        // enough for line tables, which is what stepping needs.
        let ty = dbg.builder.create_subroutine_type(file, None, &[], DIFlagsConstants::PUBLIC);
        let sp = dbg.builder.create_function(
            file.as_debug_info_scope(),
            name,
            Some(name),
            file,
            line,
            ty,
            false,
            true,
            line,
            DIFlagsConstants::PUBLIC,
            false,
        );
        fn_val.set_subprogram(sp);
        dbg.scope = Some(sp);
        self.cur_line = line;
        self.set_debug_line(line);
    }

    fn end_debug_fn(&mut self) {
        if let Some(dbg) = self.debug.as_mut() {
            dbg.scope = None;
        }
        self.builder.unset_current_debug_location();
    }

    fn set_debug_line(&mut self, line: u32) {
        let Some(dbg) = self.debug.as_ref() else { return };
        let Some(scope) = dbg.scope else { return };
        let loc = dbg.builder.create_debug_location(
            self.ctx,
            line.max(1),
            1,
            scope.as_debug_info_scope(),
            None,
        );
        self.builder.set_current_debug_location(loc);
    }

    /// Attributes the instructions generated next to a source line, when the
    /// node carries one.
    fn at_line(&mut self, line: usize) {
        if self.debug.is_none() || line == 0 {
            return;
        }
        if line as u32 != self.cur_line {
            self.cur_line = line as u32;
            self.set_debug_line(line as u32);
        }
    }

    // ---- select --------------------------------------------------------

    /// Builds the case table the runtime scans, calls it, then branches to the
    /// arm it picked. Channels are borrowed for the call; a send value is
    /// consumed only by the arm that actually ran, so the others release theirs.
    fn gen_select(
        &mut self,
        whole: &TExpr,
        arms: &[TSelectArm],
        default: Option<&TExpr>,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let case_ty = self.ctx.struct_type(&[i64t.into(), ptrt.into(), i64t.into(), i64t.into()], false);
        let n = arms.len();
        let table = self
            .builder
            .build_array_alloca(case_ty, i64t.const_int(n as u64, false), "cases")
            .expect("LLVM builder error");

        // Evaluate every channel (and send value) once, in source order.
        let mut chans: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
        let mut sends: Vec<Option<(BasicValueEnum<'ctx>, Ty)>> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let (kind, ch_expr, val) = match &arm.op {
                TSelectOp::Recv { chan, .. } => (0u64, chan, None),
                TSelectOp::Send { chan, value } => (1u64, chan, Some(value)),
            };
            let ch_ty = self.concrete(&ch_expr.ty, subst);
            let ch = self.gen_expr(ch_expr, scope, subst, func)?.expect("channel value");
            chans.push((ch, ch_ty));

            let (wa, wb, sent) = match val {
                Some(v) => {
                    let vty = self.concrete(&v.ty, subst);
                    let vv = self.gen_expr(v, scope, subst, func)?.expect("value");
                    let (a, b) = self.pack_words(vv, &vty);
                    (a, b, Some((vv, vty)))
                }
                None => (i64t.const_zero(), i64t.const_zero(), None),
            };
            sends.push(sent);

            let slot = unsafe {
                self.builder
                    .build_in_bounds_gep(case_ty, table, &[i64t.const_int(i as u64, false)], "case")
                    .expect("LLVM builder error")
            };
            for (field, value) in [
                (0u32, i64t.const_int(kind, false).into()),
                (1, ch),
                (2, wa.into()),
                (3, wb.into()),
            ] {
                let fp = self
                    .builder
                    .build_struct_gep(case_ty, slot, field, "cf")
                    .expect("LLVM builder error");
                self.builder.build_store(fp, value).expect("LLVM builder error");
            }
        }

        let out = self
            .builder
            .build_array_alloca(i64t, i64t.const_int(2, false), "sel.out")
            .expect("LLVM builder error");
        let sel = self.module.get_function("rill_select").unwrap();
        let has_default = self.ctx.i8_type().const_int(default.is_some() as u64, false);
        let call = self
            .builder
            .build_call(
                sel,
                &[
                    table.into(),
                    i64t.const_int(n as u64, false).into(),
                    has_default.into(),
                    out.into(),
                ],
                "sel",
            )
            .expect("LLVM builder error");
        let idx = match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            _ => unreachable!("rill_select returns an index"),
        };
        let got_a = self
            .builder
            .build_load(i64t, out, "a")
            .expect("LLVM builder error")
            .into_int_value();
        let out_b = unsafe {
            self.builder
                .build_in_bounds_gep(i64t, out, &[i64t.const_int(1, false)], "out.b")
                .expect("LLVM builder error")
        };
        let got_b = self
            .builder
            .build_load(i64t, out_b, "b")
            .expect("LLVM builder error")
            .into_int_value();

        // The channels were only borrowed, and every send value except the
        // one that was handed over is still ours.
        for (v, ty) in chans.clone() {
            self.emit_release(v, &ty);
        }
        for (i, sent) in sends.iter().enumerate() {
            let Some((v, ty)) = sent else { continue };
            if !Self::is_heap(ty) {
                continue;
            }
            let taken = self
                .builder
                .build_int_compare(IntPredicate::EQ, idx, i64t.const_int(i as u64, false), "taken")
                .expect("LLVM builder error");
            let keep_bb = self.ctx.append_basic_block(func, "sel.kept");
            let drop_bb = self.ctx.append_basic_block(func, "sel.drop");
            self.builder
                .build_conditional_branch(taken, keep_bb, drop_bb)
                .expect("LLVM builder error");
            self.builder.position_at_end(drop_bb);
            self.emit_release(*v, ty);
            self.builder.build_unconditional_branch(keep_bb).expect("LLVM builder error");
            self.builder.position_at_end(keep_bb);
        }

        // Branch to the winning arm.
        let default_bb = self.ctx.append_basic_block(func, "sel.default");
        let arm_blocks: Vec<BasicBlock<'ctx>> = (0..n)
            .map(|i| self.ctx.append_basic_block(func, &format!("sel.arm{i}")))
            .collect();
        let table_cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = arm_blocks
            .iter()
            .enumerate()
            .map(|(i, bb)| (i64t.const_int(i as u64, false), *bb))
            .collect();
        self.builder
            .build_switch(idx, default_bb, &table_cases)
            .expect("LLVM builder error");

        let result_ty = self.concrete(&whole.ty, subst);
        let mut incoming: Vec<(Option<BasicValueEnum<'ctx>>, BasicBlock<'ctx>)> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            self.builder.position_at_end(arm_blocks[i]);
            let mut arm_scope = scope.clone();
            let saved_bound = self.bound_fns.clone();
            if let Some((name, ty)) = &arm.bind {
                let ty = self.concrete(ty, subst);
                let v = self.unpack_words(got_a, got_b, &ty);
                arm_scope.insert(name.clone(), Some(v));
                self.bound_fns.remove(name);
            }
            let v = self.gen_expr(&arm.body, &mut arm_scope, subst, func)?;
            self.bound_fns = saved_bound;
            incoming.push((v, self.builder.get_insert_block().unwrap()));
        }

        self.builder.position_at_end(default_bb);
        match default {
            Some(d) => {
                let v = self.gen_expr(d, scope, subst, func)?;
                incoming.push((v, self.builder.get_insert_block().unwrap()));
            }
            None => {
                self.builder.build_unreachable().expect("LLVM builder error");
            }
        }
        self.merge_branches(func, &result_ty, incoming)
    }

    // ---- match: decision trees ---------------------------------------------
    //
    // Patterns nest freely, so a match compiles to a decision tree (Maranget's
    // scheme): repeatedly pick a column that is actually tested, switch on it,
    // and specialize the remaining rows per constructor. Each arm gets one
    // basic block — leaves branch to it rather than duplicating the body — and
    // the values a pattern binds are loaded there by walking their occurrence
    // path from the scrutinee, so no phi nodes are needed for bindings.

    fn gen_match(
        &mut self,
        whole: &TExpr,
        scrutinee: &TExpr,
        arms: &[TArm],
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let result_ty = self.concrete(&whole.ty, subst);
        self.compile_match(&result_ty, scrutinee, arms, scope, subst, func, None)
    }

    /// Compiles a match in either position: `tail` carries the function's
    /// return type when each arm should return instead of merging.
    #[allow(clippy::too_many_arguments)]
    fn compile_match(
        &mut self,
        result_ty: &Ty,
        scrutinee: &TExpr,
        arms: &[TArm],
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
        tail: Option<&Ty>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let scrut_ty = self.concrete(&scrutinee.ty, subst);
        let scrut = self.gen_expr(scrutinee, scope, subst, func)?;
        if tail.is_some() {
            // Each arm returns, so the scrutinee is released with the rest of
            // the frame just before the return.
            if let Some(sv) = scrut {
                if Self::is_heap(&scrut_ty) {
                    self.cleanup.push((sv, scrut_ty.clone()));
                }
            }
        }

        let rows: Vec<Row> = arms
            .iter()
            .enumerate()
            .map(|(i, a)| Row { pats: vec![a.pattern.clone()], arm: i })
            .collect();
        let tree = self.build_tree(vec![Vec::new()], vec![scrut_ty.clone()], rows);

        let arm_blocks: Vec<BasicBlock<'ctx>> = (0..arms.len())
            .map(|i| self.ctx.append_basic_block(func, &format!("arm{i}")))
            .collect();
        self.emit_tree(&tree, scrut, &scrut_ty, &arm_blocks, func);

        let saved_cleanup = self.cleanup.clone();
        let mut incoming: Vec<(Option<BasicValueEnum<'ctx>>, BasicBlock<'ctx>)> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            self.builder.position_at_end(arm_blocks[i]);
            self.cleanup = saved_cleanup.clone();
            let mut arm_scope = scope.clone();
            let mut binds = Vec::new();
            let tmod = self.tmod;
            collect_bindings(&arm.pattern, Vec::new(), &tmod.ctors, &mut binds);
            let saved_bound = self.bound_fns.clone();
            for (name, occ) in &binds {
                // Borrowed out of the box, which outlives the arm; every use
                // of the binding retains it in the usual way.
                let (v, _) = self.value_at(scrut.expect("value"), &scrut_ty, occ);
                arm_scope.insert(name.clone(), Some(v));
                self.bound_fns.remove(name);
            }
            match tail {
                Some(ret_ty) => self.gen_tail(&arm.body, &mut arm_scope, subst, func, ret_ty)?,
                None => {
                    let v = self.gen_expr(&arm.body, &mut arm_scope, subst, func)?;
                    incoming.push((v, self.builder.get_insert_block().unwrap()));
                }
            }
            self.bound_fns = saved_bound;
        }
        self.cleanup = saved_cleanup;

        if tail.is_some() {
            return Ok(None);
        }
        let out = self.merge_branches(func, result_ty, incoming)?;
        if let Some(sv) = scrut {
            if Self::is_heap(&scrut_ty) {
                self.emit_release(sv, &scrut_ty);
            }
        }
        Ok(out)
    }

    /// Field types a constructor exposes, at this column's instantiation.
    fn ctor_field_types(&self, ci: usize, col_ty: &Ty) -> Vec<Ty> {
        match col_ty {
            Ty::Adt(adt, targs) => self.tmod.adts[*adt].ctors[ci]
                .fields
                .iter()
                .map(|f| subst_gen(f, targs))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn pat_head(&self, p: &Pattern) -> Option<Head> {
        match p {
            Pattern::Int(v) => Some(Head::Int(*v)),
            Pattern::Ctor { name, .. } => self.tmod.ctors.get(name).map(|&(_, ci)| Head::Ctor(ci)),
            _ => None,
        }
    }

    fn build_tree(&self, occs: Vec<Occ>, tys: Vec<Ty>, rows: Vec<Row>) -> Tree {
        if rows.is_empty() {
            return Tree::Fail;
        }
        if rows[0].pats.iter().all(is_wild) {
            return Tree::Leaf(rows[0].arm);
        }
        // Test the leftmost column the first row actually constrains.
        let col = rows[0].pats.iter().position(|p| !is_wild(p)).expect("row is not all wildcards");
        let (occs, tys, rows) = if col == 0 {
            (occs, tys, rows)
        } else {
            let mut occs = occs;
            let mut tys = tys;
            occs.swap(0, col);
            tys.swap(0, col);
            let rows = rows
                .into_iter()
                .map(|mut r| {
                    r.pats.swap(0, col);
                    r
                })
                .collect();
            (occs, tys, rows)
        };

        let col_ty = tys[0].clone();
        let occ = occs[0].clone();
        let on_int = matches!(col_ty, Ty::Int);

        let mut heads: Vec<Head> = Vec::new();
        for r in &rows {
            if let Some(h) = self.pat_head(&r.pats[0]) {
                if !heads.contains(&h) {
                    heads.push(h);
                }
            }
        }

        let mut cases = Vec::new();
        for &h in &heads {
            let (tag, sub_tys, sub_occs) = match h {
                Head::Ctor(ci) => {
                    let fields = self.ctor_field_types(ci, &col_ty);
                    let occs: Vec<Occ> = (0..fields.len())
                        .map(|k| {
                            let mut o = occ.clone();
                            o.push((ci, k as u32));
                            o
                        })
                        .collect();
                    let tag = match &col_ty {
                        Ty::Adt(adt, _) => self.tmod.adts[*adt].ctors[ci].tag,
                        _ => unreachable!("constructor pattern on a non-ADT"),
                    };
                    (tag, fields, occs)
                }
                Head::Int(v) => (v as u64, Vec::new(), Vec::new()),
            };
            let arity = sub_tys.len();
            let mut next_occs = sub_occs;
            next_occs.extend_from_slice(&occs[1..]);
            let mut next_tys = sub_tys;
            next_tys.extend_from_slice(&tys[1..]);
            let next_rows = specialize(&rows, h, arity, |p| self.pat_head(p));
            cases.push((tag, self.build_tree(next_occs, next_tys, next_rows)));
        }

        let complete = match &col_ty {
            Ty::Adt(adt, _) => {
                let n = self.tmod.adts[*adt].ctors.len();
                (0..n).all(|ci| heads.contains(&Head::Ctor(ci)))
            }
            _ => false,
        };
        let default = if complete {
            None
        } else {
            let rest_rows: Vec<Row> = rows
                .iter()
                .filter(|r| is_wild(&r.pats[0]))
                .map(|r| Row { pats: r.pats[1..].to_vec(), arm: r.arm })
                .collect();
            Some(Box::new(self.build_tree(occs[1..].to_vec(), tys[1..].to_vec(), rest_rows)))
        };

        Tree::Switch { occ, on_int, cases, default }
    }

    /// Loads the value at an occurrence by walking constructor fields down
    /// from the scrutinee.
    fn value_at(
        &mut self,
        root: BasicValueEnum<'ctx>,
        root_ty: &Ty,
        occ: &Occ,
    ) -> (BasicValueEnum<'ctx>, Ty) {
        let mut val = root;
        let mut ty = root_ty.clone();
        for &(ci, fi) in occ {
            let (adt, targs) = match &ty {
                Ty::Adt(adt, targs) => (*adt, targs.clone()),
                other => unreachable!("occurrence walks into {other:?}"),
            };
            let struct_ty = self.ctor_struct(adt, ci, &targs);
            let field_ty = subst_gen(&self.tmod.adts[adt].ctors[ci].fields[fi as usize], &targs);
            let ptr = val.into_pointer_value();
            let fp = self
                .builder
                .build_struct_gep(struct_ty, ptr, fi + 2, "field")
                .expect("LLVM builder error");
            val = self
                .builder
                .build_load(self.llvm_type(&field_ty).expect("fields are never Unit"), fp, "fv")
                .expect("LLVM builder error");
            ty = field_ty;
        }
        (val, ty)
    }

    fn emit_tree(
        &mut self,
        tree: &Tree,
        root: Option<BasicValueEnum<'ctx>>,
        root_ty: &Ty,
        arm_blocks: &[BasicBlock<'ctx>],
        func: FunctionValue<'ctx>,
    ) {
        match tree {
            Tree::Leaf(k) => {
                self.builder
                    .build_unconditional_branch(arm_blocks[*k])
                    .expect("LLVM builder error");
            }
            Tree::Fail => {
                self.builder.build_unreachable().expect("LLVM builder error");
            }
            Tree::Switch { occ, on_int, cases, default } => {
                let (val, _) = self.value_at(root.expect("value"), root_ty, occ);
                let switch_val = if *on_int {
                    val.into_int_value()
                } else {
                    let tag_ptr = self
                        .builder
                        .build_struct_gep(self.header_struct(), val.into_pointer_value(), 1, "tag.p")
                        .expect("LLVM builder error");
                    self.builder
                        .build_load(self.ctx.i64_type(), tag_ptr, "tag")
                        .expect("LLVM builder error")
                        .into_int_value()
                };
                let default_bb = self.ctx.append_basic_block(func, "m.default");
                let blocks: Vec<BasicBlock<'ctx>> = (0..cases.len())
                    .map(|i| self.ctx.append_basic_block(func, &format!("m.case{i}")))
                    .collect();
                let table: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = cases
                    .iter()
                    .zip(&blocks)
                    .map(|((tag, _), bb)| {
                        (self.ctx.i64_type().const_int(*tag, *on_int), *bb)
                    })
                    .collect();
                self.builder
                    .build_switch(switch_val, default_bb, &table)
                    .expect("LLVM builder error");
                for ((_, sub), bb) in cases.iter().zip(&blocks) {
                    self.builder.position_at_end(*bb);
                    self.emit_tree(sub, root, root_ty, arm_blocks, func);
                }
                self.builder.position_at_end(default_bb);
                match default {
                    Some(d) => self.emit_tree(d, root, root_ty, arm_blocks, func),
                    None => {
                        self.builder.build_unreachable().expect("LLVM builder error");
                    }
                }
            }
        }
    }

    /// Joins N (value, predecessor-block) pairs with a merge block and a phi.
    /// Incoming blocks must currently lack a terminator.
    fn merge_branches(
        &mut self,
        func: FunctionValue<'ctx>,
        result_ty: &Ty,
        incoming: Vec<(Option<BasicValueEnum<'ctx>>, BasicBlock<'ctx>)>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let merge_bb = self.ctx.append_basic_block(func, "merge");
        for (_, bb) in &incoming {
            self.builder.position_at_end(*bb);
            self.builder.build_unconditional_branch(merge_bb).expect("LLVM builder error");
        }
        self.builder.position_at_end(merge_bb);

        match self.llvm_type(result_ty) {
            None => Ok(None),
            Some(ll_ty) => {
                let phi = self.builder.build_phi(ll_ty, "merge").unwrap();
                for (v, bb) in &incoming {
                    let v = v.expect("checker unifies branch types");
                    phi.add_incoming(&[(&v, *bb)]);
                }
                Ok(Some(phi.as_basic_value()))
            }
        }
    }

    // ---- output ----------------------------------------------------------

    fn write_object(
        &self,
        out_path: &Path,
        optimize: bool,
        want: Option<&str>,
    ) -> Result<(), CodegenError> {
        if let Some(dbg) = self.debug.as_ref() {
            dbg.builder.finalize();
        }
        // Only the host's backend is initialised for a host build, which is
        // the common case and the cheap one; asking for another target brings
        // the rest of LLVM's backends in.
        match want {
            None => Target::initialize_native(&InitializationConfig::default())
                .map_err(|e| CodegenError::new(0, format!("failed to initialize LLVM target: {e}")))?,
            Some(_) => Target::initialize_all(&InitializationConfig::default()),
        }
        let triple = match want {
            None => TargetMachine::get_default_triple(),
            Some(name) => TargetTriple::create(name),
        };
        let target = Target::from_triple(&triple)
            .map_err(|e| CodegenError::new(0, format!("bad target triple: {e}")))?;
        // The host's own processor is only the right answer when the host is
        // what is being built for. Cross-compiling, take the generic model of
        // the architecture: the alternative is emitting instructions the other
        // machine may not have.
        let (cpu, features) = match want {
            None => (
                TargetMachine::get_host_cpu_name().to_string(),
                TargetMachine::get_host_cpu_features().to_string(),
            ),
            Some(_) => (String::new(), String::new()),
        };
        let opt_level =
            if optimize { OptimizationLevel::Aggressive } else { OptimizationLevel::None };
        let machine = target
            .create_target_machine(
                &triple,
                &cpu,
                &features,
                opt_level,
                // Windows objects are not position-independent in the ELF
                // sense and the COFF writer rejects the request outright.
                if triple.as_str().to_string_lossy().contains("windows") {
                    RelocMode::Default
                } else {
                    RelocMode::PIC
                },
                CodeModel::Default,
            )
            .ok_or_else(|| CodegenError::new(0, "failed to create target machine"))?;
        self.module.set_triple(&triple);
        self.module.set_data_layout(&machine.get_target_data().get_data_layout());
        if optimize {
            // Full O3 pipeline: inlining, tail-call elimination, SROA, etc.
            self.module
                .run_passes("default<O3>", &machine, PassBuilderOptions::create())
                .map_err(|e| {
                    CodegenError::new(0, format!("optimization pipeline failed: {e}"))
                })?;
        }
        machine
            .write_to_file(&self.module, FileType::Object, out_path)
            .map_err(|e| CodegenError::new(0, format!("failed to write object file: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ir(src: &str) -> String {
        let m = rill_syntax::parse(src).unwrap();
        let tmod = rill_types::check(&m).unwrap();
        compile_to_ir(&tmod).unwrap()
    }

    #[test]
    fn export_needs_wasm_and_scalars() {
        let check = |src: &str, target| {
            let m = rill_syntax::parse(src).unwrap();
            let tmod = rill_types::check(&m).unwrap();
            check_exports(&tmod, target)
        };
        let e = check("export fn f(x: Int) -> Int = x\nfn main() = 0\n", None).unwrap_err();
        assert!(e.msg.contains("needs a wasm target"), "{e}");
        let e = check("export fn id(x) = x\nfn main() = 0\n", Some("wasm32-wasip1")).unwrap_err();
        assert!(e.msg.contains("generic"), "{e}");
        let e = check(
            "export fn shout(s: Str) -> Str = s\nfn main() = 0\n",
            Some("wasm32-wasip1"),
        )
        .unwrap_err();
        assert!(e.msg.contains("Str"), "{e}");
        check("export fn f(x: Int) -> Float = to_float(x)\nfn main() = 0\n", Some("wasm32-wasip1"))
            .unwrap();
    }

    #[test]
    fn hello_world_ir() {
        let out = ir("fn main() = println(\"hello, world\")\n");
        assert!(out.contains("rill_println_str"));
        assert!(out.contains("hello, world"));
    }

    #[test]
    fn monomorphizes_identity() {
        let out = ir("fn id(x) = x\n\nfn main() =\n  println(id(1))\n  println(id(\"s\"))\n");
        assert!(out.contains("id$Int"), "{out}");
        assert!(out.contains("id$Str"), "{out}");
    }

    #[test]
    fn lambda_closure_ir() {
        let out = ir("fn main() =\n  n = 10\n  f = \\x -> x + n\n  println(f(5))\n");
        assert!(out.contains("lambda$0"), "{out}");
    }

    #[test]
    fn generic_list_ir() {
        let src = "type List(a)\n  Nil\n  Cons(head: a, tail: List(a))\n\nfn len(l) = match l\n  Nil -> 0\n  Cons(_, rest) -> 1 + len(rest)\n\nfn main() = println(len(Cons(1, Nil)))\n";
        let out = ir(src);
        assert!(out.contains("len$Int"), "{out}");
        assert!(out.contains("switch i64"), "{out}");
    }

    #[test]
    fn missing_impl_is_a_codegen_error() {
        let src = "trait D(a)\n  d(a) -> Str\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n\nfn announce(x) = println(d(x))\n\nfn main() = announce(1.5)\n";
        let m = rill_syntax::parse(src).unwrap();
        let tmod = rill_types::check(&m).unwrap();
        let err = compile_to_ir(&tmod).unwrap_err();
        assert!(err.msg.contains("no implementation of trait `D`"), "{}", err.msg);
    }

    #[test]
    fn static_dispatch_ir() {
        let src = "trait D(a)\n  d(a) -> Str\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n\nfn main() = println(d(7))\n";
        let m = rill_syntax::parse(src).unwrap();
        let tmod = rill_types::check(&m).unwrap();
        let ir = compile_to_ir(&tmod).unwrap();
        assert!(ir.contains("D.Int.d"), "{ir}");
    }

    #[test]
    fn named_fn_as_value_ir() {
        let src = "fn double(x) = x * 2\n\nfn apply(f, x) = f(x)\n\nfn main() = println(apply(double, 21))\n";
        let out = ir(src);
        assert!(out.contains("$shim"), "{out}");
    }
}

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

use inkwell::attributes::{Attribute, AttributeLoc};
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
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, IntType, StructType,
};
use inkwell::values::BasicValue;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, CallSiteValue, FunctionValue, InstructionValue,
    IntValue, MetadataValue, PointerValue,
};
use inkwell::{FloatPredicate, IntPredicate, OptimizationLevel};

use rill_syntax::{BinOp, Pattern};
use rill_types::{
    CTy, Constraint, ExternSig, TArm, TExpr, TFn, TK, TModule, TSelectArm, TSelectOp, TStmt, Ty,
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
    unchecked: bool,
    shared: bool,
) -> Result<(), CodegenError> {
    let context = Context::create();
    check_exports(tmod, target, shared)?;
    let cg = Codegen::build_module(
        &context,
        tmod,
        parallel,
        debug.then_some(source),
        target,
        unchecked,
        shared,
        false,
    )?;
    if cg.wasm {
        cg.check_wasm_supported()?;
    }
    cg.write_object(out_path, !debug, target)
}

/// What `export fn` promises: a name the host can call, with arguments it can
/// produce. That constrains the signature to the types that cross the wasm
/// boundary as plain values — Int, Float, Bool, Ptr — and rules out type
/// parameters, because an export is one function, not a family waiting for a
/// call site to pick the types. It is a host's idea: on a native target the
/// host is whoever loads the shared library `--shared` makes, and there the
/// boundary is wider — a counted value crosses as a handle, lent to the
/// call by the host or handed to the host by a return. A native executable
/// has no host, so there the word is refused rather than silently meaning
/// nothing.
fn check_exports(tmod: &TModule, target: Option<&str>, shared: bool) -> Result<(), CodegenError> {
    let exports: Vec<&TFn> = tmod.fns.iter().filter(|f| f.exported).collect();
    if exports.is_empty() {
        return Ok(());
    }
    let wasm = target.is_some_and(|t| t.starts_with("wasm"));
    if !wasm && !shared {
        return Err(CodegenError::new(
            exports[0].line,
            format!(
                "`export fn {}` needs a host: exports are functions the embedding \
                 host calls, and a native executable has none. Build with \
                 `--target wasm32-wasip1` for a wasm host, or `--shared` for a \
                 library a C or Python program loads.",
                exports[0].name
            ),
        ));
    }
    let scalar = |t: &Ty| {
        matches!(t, Ty::Int | Ty::Float | Ty::Bool | Ty::Ptr)
            || (!wasm
                && matches!(t, Ty::Adt(..) | Ty::Buf(_) | Ty::Str | Ty::StrBuf | Ty::Map(..)))
    };
    let allowed = if wasm {
        "only Int, Float, Bool and Ptr cross the wasm boundary"
    } else {
        "only Int, Float, Bool, Ptr and counted values (data, buffers, strings, maps) \
         cross to a host"
    };
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
                        "`export fn {}`: parameter `{pname}` is {} — {allowed}",
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
                    "`export fn {}` returns {} — {allowed}, and Unit",
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
const SOCKET_RT: [&str; 11] = [
    "rill_sock_buffered",
    "rill_sock_flush",
    "rill_tcp_listen",
    "rill_tcp_accept",
    "rill_tcp_connect",
    "rill_sock_read",
    "rill_sock_wait",
    "rill_sock_peer",
    "rill_sock_nodelay",
    "rill_sock_write",
    "rill_sock_close",
];

/// The two file builtins that wait. Every other file call is a syscall that
/// returns; these two hand the work to a helper thread and park the strand,
/// so they need a scheduler in exactly the way a socket does.
const FILE_WAIT_RT: [&str; 3] =
    ["rill_file_sync", "rill_dir_sync", "rill_wait_signal"];

/// `sleep_ms`, which parks a strand in the same poller a socket would have.
/// It belongs beside the sockets in every judgement made here: it needs the
/// scheduler, and wasm cannot run one.
const TIMER_RT: &str = "rill_sleep_ms";

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
    strands_reachable_from(tmod, vec![entry])
}

/// A library's strands are reached from its exports, not from its `main`,
/// which is only its setup.
fn library_uses_scheduler(tmod: &TModule) -> bool {
    let mut roots: Vec<usize> = tmod.fn_index.get("main").copied().into_iter().collect();
    for (i, f) in tmod.fns.iter().enumerate() {
        if f.exported {
            roots.push(i);
        }
    }
    strands_reachable_from(tmod, roots)
}

fn strands_reachable_from(tmod: &TModule, roots: Vec<usize>) -> bool {
    let graph = call_graph(tmod);
    let mut seen = vec![false; tmod.fns.len()];
    let mut queue = roots;
    for &r in &queue {
        seen[r] = true;
    }
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
        | TK::ChanOp { .. }
        | TK::ChanTaken { .. }
        | TK::Select { .. } => *found = true,
        TK::Builtin { rt, .. }
            if SOCKET_RT.contains(rt) || FILE_WAIT_RT.contains(rt) || *rt == TIMER_RT =>
        {
            *found = true
        }
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
        TK::ChanNew(_)
        | TK::ChanSend { .. }
        | TK::ChanRecv { .. }
        | TK::ChanOp { .. }
        | TK::ChanTaken { .. } => Some("a channel"),
        TK::Select { .. } => Some("select"),
        TK::Builtin { rt, .. } if SOCKET_RT.contains(rt) => Some("a socket"),
        TK::Builtin { rt, .. } if *rt == TIMER_RT => Some("`sleep_ms`"),
        TK::Builtin { rt, .. } if FILE_WAIT_RT.contains(rt) => Some("`file_sync`"),

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
/// The memory decisions the code generator made for a program's own
/// functions, written out: which parameters are borrowed, which constructor
/// calls reuse a box and which allocate, which bindings are handed to a call
/// and which retained, which pipelines became loops, what a closure holds.
/// The report is the generator's own account, taken while it generates —
/// nothing is guessed after the fact.
pub fn explain(tmod: &TModule) -> Result<String, CodegenError> {
    let context = Context::create();
    let cg = Codegen::build_module(&context, tmod, false, None, None, false, false, true)?;
    let mut out = String::new();
    let mut last_fn = usize::MAX;
    for (fn_idx, line, text) in &cg.notes {
        let f = &tmod.fns[*fn_idx];
        if *fn_idx != last_fn {
            let params: Vec<&str> = f.params.iter().map(|(n, _)| n.as_str()).collect();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("fn {}({})   # line {}\n", f.name, params.join(", "), f.line));
            last_fn = *fn_idx;
        }
        if *line == f.line {
            out.push_str(&format!("  {text}\n"));
        } else {
            out.push_str(&format!("  line {line}: {text}\n"));
        }
    }
    Ok(out)
}

pub fn compile_to_ir(tmod: &TModule, parallel: bool, unchecked: bool) -> Result<String, CodegenError> {
    let context = Context::create();
    let cg = Codegen::build_module(&context, tmod, parallel, None, None, unchecked, false, false)?;
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
        Ty::Chan(e) => Ty::Chan(Box::new(subst_gen(e, args))),
        Ty::Map(k, v) => Ty::Map(Box::new(subst_gen(k, args)), Box::new(subst_gen(v, args))),
        other => other.clone(),
    }
}

/// Nodes in an instantiation's types past which its growth is called endless.
const TYPE_SIZE_CAP: usize = 512;

fn mangle(ty: &Ty, adts: &[rill_types::AdtDef]) -> String {
    match ty {
        Ty::Int => "Int".into(),
        Ty::Float => "Float".into(),
        Ty::Float2 => "Float2".into(),
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
        Ty::Map(k, v) => format!("Map_{}_{}", mangle(k, adts), mangle(v, adts)),
        Ty::Buf(w) => format!("Buf_{}", w.name()),
        Ty::Ptr => "Ptr".into(),
        Ty::Var(_) | Ty::Gen(_) => unreachable!("mangling requires concrete types"),
    }
}

/// `Tuple2`..`Tuple8` are what the tuple notation stands for.
fn is_tuple_ctor(name: &str, n: usize) -> bool {
    (2..=8).contains(&n) && name == format!("Tuple{n}")
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

/// Parameters a function only ever *reads* — indexes, measures, slices, hands
/// to a builtin or to another function's borrowed parameter — and never
/// binds, returns, boxes, captures or sends. Such a parameter is passed
/// without a reference of its own: the caller keeps the one it holds for the
/// length of the call and the callee releases nothing. The retain a call used
/// to take and the release its callee made were a load, a compare and a store
/// each on the same header, in the inner loop of every sort and every heap
/// (10% of quicksort, 15% of a Dijkstra).
///
/// Only storage is considered — buffers, strings, builders and maps —
/// because those are what hot loops pass, and none of them is taken apart by
/// a `match`. Impl methods stay owned: `call_impl` hands them owned arguments
/// from places that never consult this table. Solved as a fixpoint from
/// "everything borrowed" downward, since an argument in another function's
/// parameter position borrows only if that parameter does.
fn borrowed_params(tmod: &TModule) -> Vec<Vec<bool>> {
    let impls: HashSet<usize> = tmod.impl_methods.values().copied().collect();
    let mut borrowed: Vec<Vec<bool>> = tmod
        .fns
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            let mut rebound = HashSet::new();
            collect_rebound(&f.body, &mut rebound);
            f.params
                .iter()
                .map(|(n, t)| {
                    // A data value is a candidate too — a list walked by a
                    // `match` — unless the body builds values of that same
                    // type: then the box being taken apart may be the box the
                    // next one is built in, and that reuse needs ownership.
                    // `map_onto` keeps its list; `kick` walking the bodies
                    // does not.
                    let candidate = match t {
                        Ty::Buf(_) | Ty::Str | Ty::StrBuf | Ty::Map(..) => true,
                        Ty::Adt(adt, _) => !constructs_adt(&f.body, *adt),
                        _ => false,
                    };
                    // An impl method is reached from places that hand it
                    // owned arguments without consulting this table, so it
                    // stays owned — except for a builder, which is only ever
                    // written into and which every `render` in a page takes.
                    // The retain and the release around that call were a
                    // load, a compare and a store on the header in the middle
                    // of the tightest loop a page has.
                    let ok_impl = !impls.contains(&fi) || matches!(t, Ty::StrBuf);
                    candidate && ok_impl && !f.exported && !rebound.contains(n.as_str())
                })
                .collect()
        })
        .collect();
    let mut round = GoesRound::new(tmod);
    let nonesc = nonescaping_fn_params(tmod);
    // `RILL_BORROW_TRACE=name,name` says on stderr why a parameter of the
    // named functions is passed owned: which use, or which tail call, took
    // its borrow away.
    let trace: Vec<String> = std::env::var("RILL_BORROW_TRACE")
        .map(|v| v.split(',').map(|s| s.to_string()).collect())
        .unwrap_or_default();
    loop {
        let mut changed = false;
        for fi in 0..tmod.fns.len() {
            for pi in 0..tmod.fns[fi].params.len() {
                if borrowed[fi][pi]
                    && !borrows_only(&tmod.fns[fi].body, &tmod.fns[fi].params[pi].0, &borrowed, &nonesc)
                {
                    if trace.iter().any(|n| n == "*" || n == &tmod.fns[fi].name) {
                        let mut why = String::new();
                        borrow_breaker(&tmod.fns[fi].body, &tmod.fns[fi].params[pi].0, &borrowed, &nonesc, tmod, &mut why);
                        eprintln!(
                            "borrow: `{}` of `{}` is owned: {}",
                            tmod.fns[fi].params[pi].0, tmod.fns[fi].name, why
                        );
                    }
                    borrowed[fi][pi] = false;
                    changed = true;
                }
            }
            let held = HashSet::new();
            owned_at_tail(&tmod.fns[fi].body, fi, tmod, &mut borrowed, &mut changed, &held, &mut round);
        }
        if !changed {
            return borrowed;
        }
    }
}

/// Whether a call from `caller` to `callee` can come back around: the
/// callee reaches the caller in the call graph. What it reaches is found
/// once and kept.
struct GoesRound {
    graph: Vec<Vec<usize>>,
    reaches: HashMap<usize, HashSet<usize>>,
}

impl GoesRound {
    fn new(tmod: &TModule) -> GoesRound {
        GoesRound { graph: call_graph(tmod), reaches: HashMap::new() }
    }

    fn goes_round(&mut self, caller: usize, callee: usize) -> bool {
        if caller == callee {
            return true;
        }
        if let Some(reached) = self.reaches.get(&callee) {
            return reached.contains(&caller);
        }
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue = vec![callee];
        while let Some(i) = queue.pop() {
            for &e in &self.graph[i] {
                if seen.insert(e) {
                    queue.push(e);
                }
            }
        }
        let answer = seen.contains(&caller);
        self.reaches.insert(callee, seen);
        answer
    }
}

/// A call in tail position jumps: the frame releases everything it owns
/// first, so a borrowed argument must be something an *earlier* frame holds
/// — a borrowed parameter of the calling function — or the callee reads
/// freed memory. Any other argument in a borrowed position of a tail call
/// that can come back around makes that parameter owned, which keeps the
/// call a tail call: passing it by an ordinary call and releasing afterwards
/// would be safe, but it would also cost a stack frame per iteration of
/// every loop written that way (`join_onto` overflowed at half a million
/// links before this rule). A tail call that cannot come back around — the
/// callee never reaches the caller — is left alone: the code generator makes
/// it an ordinary call, which costs one frame and no loop, and the callee's
/// parameter stays borrowed for every other caller. Before this, one
/// `check(x, db.cache)` at the end of a function made `cache` owned in
/// `check` and in everything `check` handed it to, and a row filter paid a
/// retain and a release per call all the way down.
fn owned_at_tail(
    e: &TExpr,
    fi: usize,
    tmod: &TModule,
    borrowed: &mut Vec<Vec<bool>>,
    changed: &mut bool,
    held: &HashSet<String>,
    round: &mut GoesRound,
) {
    // A name some earlier frame holds: a borrowed parameter, or a binding a
    // `match` took out of one — the box it came from is held there too.
    let held_above = |n: &str, borrowed: &Vec<Vec<bool>>| -> bool {
        held.contains(n)
            || tmod.fns[fi]
                .params
                .iter()
                .position(|(p, _)| p == n)
                .is_some_and(|p| borrowed[fi][p])
    };
    match &e.kind {
        TK::Block(stmts) => {
            if let Some(TStmt::Expr(last)) = stmts.last() {
                owned_at_tail(last, fi, tmod, borrowed, changed, held, round);
            }
        }
        TK::If { then, els, .. } => {
            owned_at_tail(then, fi, tmod, borrowed, changed, held, round);
            owned_at_tail(els, fi, tmod, borrowed, changed, held, round);
        }
        TK::Match { scrutinee, arms } => {
            let from_held = match &scrutinee.kind {
                TK::Local(n) => held_above(n, borrowed),
                TK::FieldGet { .. } => root_local(scrutinee).is_some_and(|n| held_above(n, borrowed)),
                _ => false,
            };
            for a in arms {
                let mut inner = held.clone();
                if from_held {
                    collect_pattern_binds(&a.pattern, &mut inner);
                }
                owned_at_tail(&a.body, fi, tmod, borrowed, changed, &inner, round);
            }
        }
        TK::CallFn { fn_idx, args, .. } => {
            if !round.goes_round(fi, *fn_idx) {
                return;
            }
            for (i, a) in args.iter().enumerate() {
                if borrowed[*fn_idx].get(i) != Some(&true) {
                    continue;
                }
                // A field read out of a held box is held with it: the box
                // is immutable and outlives this frame, so `q.ctx` in a
                // tail call borrows as `ctx` would.
                let held_above = match &a.kind {
                    TK::Local(n) => held_above(n, borrowed),
                    TK::FieldGet { .. } => root_local(a).is_some_and(|n| held_above(n, borrowed)),
                    _ => false,
                };
                if !held_above {
                    if std::env::var("RILL_BORROW_TRACE")
                        .map(|v| v.split(',').any(|n| n == "*" || n == tmod.fns[*fn_idx].name))
                        .unwrap_or(false)
                    {
                        eprintln!(
                            "borrow: `{}` of `{}` is owned: a tail call from `{}` (line {}) that can come back around passes it something that frame owns",
                            tmod.fns[*fn_idx].params[i].0, tmod.fns[*fn_idx].name, tmod.fns[fi].name, e.line
                        );
                    }
                    borrowed[*fn_idx][i] = false;
                    *changed = true;
                }
            }
        }
        _ => {}
    }
}

/// Function-typed parameters that never leave the function they are passed
/// to: only ever called, or handed to another function's parameter of the
/// same kind. The prelude's `map`, `filter` and `fold` are the point — `f`
/// is called on each element and passed along the recursion, and that is
/// all. A lambda written in place for such a position is built with its
/// captures *borrowed*: the environment holds no reference of its own, since
/// the frame that made it outlives the call and holds every capture already.
/// That is also what lets `map(xs, \\x -> g(x, xs))` reuse the cells of `xs`
/// in place — the environment used to be a second holder of `xs`.
///
/// A fixpoint from "everything non-escaping" downward, like
/// `borrowed_params`; impl methods are excluded for the same reason.
fn nonescaping_fn_params(tmod: &TModule) -> Vec<Vec<bool>> {
    let impls: HashSet<usize> = tmod.impl_methods.values().copied().collect();
    let mut table: Vec<Vec<bool>> = tmod
        .fns
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            let mut rebound = HashSet::new();
            collect_rebound(&f.body, &mut rebound);
            f.params
                .iter()
                .map(|(n, t)| {
                    matches!(t, Ty::Fn(..))
                        && !impls.contains(&fi)
                        && !f.exported
                        && !rebound.contains(n.as_str())
                })
                .collect()
        })
        .collect();
    loop {
        let mut changed = false;
        for fi in 0..tmod.fns.len() {
            for pi in 0..tmod.fns[fi].params.len() {
                if table[fi][pi]
                    && !only_called(&tmod.fns[fi].body, &tmod.fns[fi].params[pi].0, &table)
                {
                    table[fi][pi] = false;
                    changed = true;
                }
            }
        }
        if !changed {
            return table;
        }
    }
}

/// Whether every appearance of `name` in `e` is as the callee of a call or
/// as an argument in a non-escaping position of one.
fn only_called(e: &TExpr, name: &str, table: &[Vec<bool>]) -> bool {
    let is_name = |x: &TExpr| matches!(&x.kind, TK::Local(n) if n == name);
    match &e.kind {
        TK::Local(n) => n != name,
        TK::CallClosure { callee, args } => {
            (is_name(callee) || only_called(callee, name, table))
                && args.iter().all(|a| only_called(a, name, table))
        }
        TK::CallFn { fn_idx, args, .. } => args.iter().enumerate().all(|(i, a)| {
            if is_name(a) {
                table[*fn_idx].get(i) == Some(&true)
            } else {
                only_called(a, name, table)
            }
        }),
        TK::Lambda { captures, .. } | TK::Spawn { captures, .. } => {
            !captures.iter().any(|(n, _)| n == name)
        }
        _ => {
            let mut ok = true;
            walk_children(e, &mut |c| {
                if ok {
                    ok = only_called(c, name, table);
                }
            });
            ok
        }
    }
}

/// Whether the body builds any value of the ADT `adt` with fields.
fn constructs_adt(e: &TExpr, adt: usize) -> bool {
    if matches!(&e.kind, TK::CallCtor { adt: a, args, .. } if *a == adt && !args.is_empty()) {
        return true;
    }
    let mut found = false;
    walk_children(e, &mut |c| {
        if !found {
            found = constructs_adt(c, adt);
        }
    });
    found
}

/// The variable a chain of field reads starts from, if it is one.
fn root_local(e: &TExpr) -> Option<&str> {
    match &e.kind {
        TK::Local(n) => Some(n),
        TK::FieldGet { base, .. } => root_local(base),
        _ => None,
    }
}

/// Whether every appearance of `name` in `e` is in a position that borrows
/// it — the positions `gen_borrowed` serves, or the borrowed parameter of a
/// call. A lambda or spawn that captures it keeps it, so that is an owned use.
fn borrows_only(e: &TExpr, name: &str, borrowed: &[Vec<bool>], nonesc: &[Vec<bool>]) -> bool {
    let operand = |x: &TExpr, borrowing: bool| -> bool {
        match &x.kind {
            TK::Local(n) if n == name => borrowing,
            _ => borrows_only(x, name, borrowed, nonesc),
        }
    };
    match &e.kind {
        TK::Local(n) => n != name,
        TK::Builtin { args, .. } => args.iter().all(|a| operand(a, true)),
        TK::BufGet { buf, idx, .. } => operand(buf, true) && operand(idx, false),
        TK::BufSet { buf, idx, val, .. } => {
            operand(buf, true) && operand(idx, false) && operand(val, false)
        }
        // Standing alone or as a `Ptr` argument, the buffer is held for the
        // call and released by whoever made it.
        TK::BufLen { buf } | TK::BufPtr { buf } => operand(buf, true),
        // `unique` reads the count as this frame holds it, which means
        // something only if this frame is the holder: the value it inspects
        // — through any chain of fields — is owned here, never borrowed.
        // A borrowed parameter's count of one would be the caller's
        // reference, and the caller may still be looking.
        TK::Unique { val } => root_local(val) != Some(name) && operand(val, true),
        TK::Slice { base, from, to } => {
            operand(base, true)
                && operand(from, false)
                && to.as_deref().map_or(true, |t| operand(t, false))
        }
        TK::MapFind { map, key: k } | TK::MapNext { map, from: k } => {
            operand(map, true) && operand(k, false)
        }
        // A slot's key or value is read through the map without a count
        // of its own (the code generator borrows the map there), so
        // `map_or` and `map_get` borrow theirs.
        TK::MapValAt { map, idx } | TK::MapKeyAt { map, idx } => {
            operand(map, true) && operand(idx, false)
        }
        TK::MapSet { map, key, val } => {
            operand(map, true) && operand(key, false) && operand(val, false)
        }
        TK::MapLen { map } => operand(map, true),
        // Taking a value apart, or reading one field, holds it only for the
        // look; what comes out is retained where it is kept.
        TK::FieldGet { base, .. } => operand(base, true),
        TK::Match { scrutinee, arms } => {
            operand(scrutinee, true)
                && arms.iter().all(|a| {
                    a.guard.as_ref().map_or(true, |g| borrows_only(g, name, borrowed, nonesc))
                        && borrows_only(&a.body, name, borrowed, nonesc)
                })
        }
        // A lambda written in place for a parameter that never escapes may
        // capture the name: its environment borrows the captures when the
        // call is not a tail call — this frame holds them for the call's
        // length, and the caller holds the parameter for this frame's — and
        // retains them when it is, which is a count on a value still alive.
        // `map(xs, \x -> f(x, ctx))` then leaves `ctx` borrowed.
        TK::CallFn { fn_idx, args, .. } => args.iter().enumerate().all(|(i, a)| {
            if matches!(a.kind, TK::Lambda { .. }) && nonesc[*fn_idx].get(i) == Some(&true) {
                return true;
            }
            operand(a, borrowed[*fn_idx].get(i) == Some(&true))
        }),
        TK::Lambda { captures, .. } | TK::Spawn { captures, .. } => {
            !captures.iter().any(|(n, _)| n == name)
        }
        _ => {
            let mut ok = true;
            walk_children(e, &mut |c| {
                if ok {
                    ok = borrows_only(c, name, borrowed, nonesc);
                }
            });
            ok
        }
    }
}

/// The first use of `name` that `borrows_only` refuses, described for the
/// trace. The same walk as `borrows_only`, saying where it stopped.
fn borrow_breaker(e: &TExpr, name: &str, borrowed: &[Vec<bool>], nonesc: &[Vec<bool>], tmod: &TModule, why: &mut String) {
    if !why.is_empty() {
        return;
    }
    let operand = |x: &TExpr, borrowing: bool, what: &str, why: &mut String| {
        match &x.kind {
            TK::Local(n) if n == name => {
                if !borrowing {
                    *why = format!("{what} (line {})", x.line);
                }
            }
            _ => borrow_breaker(x, name, borrowed, nonesc, tmod, why),
        }
    };
    match &e.kind {
        TK::Local(n) => {
            if n == name {
                *why = format!("used as a value (line {})", e.line);
            }
        }
        TK::Builtin { args, .. } => {
            for a in args {
                operand(a, true, "", why);
            }
        }
        TK::BufGet { buf, idx, .. } => {
            operand(buf, true, "", why);
            operand(idx, false, "an index", why);
        }
        TK::BufSet { buf, idx, val, .. } => {
            operand(buf, true, "", why);
            operand(idx, false, "an index", why);
            operand(val, false, "stored into a buffer", why);
        }
        TK::BufLen { buf } | TK::BufPtr { buf } => operand(buf, true, "", why),
        TK::Unique { val } => {
            if root_local(val) == Some(name) {
                *why = format!("asked `unique` (line {})", e.line);
            } else {
                operand(val, true, "", why);
            }
        }
        TK::Slice { base, from, to } => {
            operand(base, true, "", why);
            operand(from, false, "a slice bound", why);
            if let Some(t) = to {
                operand(t, false, "a slice bound", why);
            }
        }
        TK::MapFind { map, key: k } | TK::MapNext { map, from: k } => {
            operand(map, true, "", why);
            operand(k, false, "a map key", why);
        }
        TK::MapValAt { map, idx } | TK::MapKeyAt { map, idx } => {
            operand(map, true, "", why);
            operand(idx, false, "a slot number", why);
        }
        TK::MapSet { map, key, val } => {
            operand(map, true, "", why);
            operand(key, false, "a map key", why);
            operand(val, false, "stored into a map", why);
        }
        TK::MapLen { map } => operand(map, true, "", why),
        TK::FieldGet { base, .. } => operand(base, true, "", why),
        TK::Match { scrutinee, arms } => {
            operand(scrutinee, true, "", why);
            for a in arms {
                if let Some(g) = &a.guard {
                    borrow_breaker(g, name, borrowed, nonesc, tmod, why);
                }
                borrow_breaker(&a.body, name, borrowed, nonesc, tmod, why);
            }
        }
        TK::CallFn { fn_idx, args, .. } => {
            for (i, a) in args.iter().enumerate() {
                if matches!(a.kind, TK::Lambda { .. }) && nonesc[*fn_idx].get(i) == Some(&true) {
                    continue;
                }
                let b = borrowed[*fn_idx].get(i) == Some(&true);
                let what = format!(
                    "passed to `{}` as `{}`, which is owned",
                    tmod.fns[*fn_idx].name,
                    tmod.fns[*fn_idx].params.get(i).map(|p| p.0.as_str()).unwrap_or("?")
                );
                operand(a, b, &what, why);
            }
        }
        TK::Lambda { captures, .. } | TK::Spawn { captures, .. } => {
            if captures.iter().any(|(n, _)| n == name) {
                *why = format!("captured by a lambda (line {})", e.line);
            }
        }
        _ => {
            walk_children(e, &mut |c| borrow_breaker(c, name, borrowed, nonesc, tmod, why));
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
                if let Some(g) = &a.guard {
                    collect_rebound(g, out);
                }
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
                    TSelectOp::Timeout { ms } => collect_rebound(ms, out),
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
        Pattern::Or(..) => unreachable!("`lift` expands `|` patterns"),
        // The name stands at the position the pattern itself is at, and what
        // is under it is still matched and still binds.
        Pattern::As(n, inner, _) => {
            out.insert(n.clone());
            collect_pattern_binds(inner, out);
        }
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
            // An `asm fn` shares this node and is in no extern table; a
            // callback was refused at its declaration, so there is nothing
            // here to find.
            if let Some(sig) = tmod.externs.get(name) {
                // The arguments stand for the parameters that are not `Env`;
                // a callback whose `Fn` carries an `Env` may be any closure,
                // so it pins nothing.
                let visible = sig.params.iter().filter(|p| !matches!(p, CTy::Env));
                for (a, pt) in args.iter().zip(visible) {
                    if let (CTy::Fn(cps, _), TK::Local(n)) = (pt, &a.kind) {
                        if !cps.iter().any(|c| matches!(c, CTy::Env)) {
                            out.push(n);
                        }
                    }
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
/// The bindings a statement is the last to speak of, and speaks of once, as
/// a direct argument of a call on the statement's unconditional path — the
/// ones a call may be handed outright. `x = shift(t, 1.0)` with no `t`
/// after it hands `shift` the only reference to `t`, and `shift` may then
/// write its answer into `t`'s own storage. A name inside an `if`, a
/// `match`, a lambda or a loop body is never here: the move would take the
/// reference on one path and leave the release owed on the other.
/// Whether the expression calls through a closure value anywhere.
fn calls_closure(e: &TExpr) -> bool {
    if matches!(e.kind, TK::CallClosure { .. } | TK::Spawn { .. } | TK::Lambda { .. }) {
        return true;
    }
    let mut found = false;
    walk_children(e, &mut |c| {
        if !found {
            found = calls_closure(c);
        }
    });
    found
}

fn stmt_value(stmt: &TStmt) -> &TExpr {
    match stmt {
        TStmt::Bind { value, .. } | TStmt::Expr(value) => value,
    }
}

/// The names `value` speaks of exactly once and no statement in `rest`
/// speaks of at all.
fn free_once(value: &TExpr, rest: &[TStmt]) -> HashSet<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    count_locals(value, &mut counts);
    counts
        .into_iter()
        .filter(|(_, n)| *n == 1)
        .map(|(name, _)| name)
        .filter(|n| !rest.iter().any(|st| names_local(stmt_value(st), n)))
        .map(String::from)
        .collect()
}

fn count_locals<'e>(e: &'e TExpr, counts: &mut HashMap<&'e str, usize>) {
    if let TK::Local(n) = &e.kind {
        *counts.entry(n.as_str()).or_insert(0) += 1;
    }
    walk_children(e, &mut |c| count_locals(c, counts));
}

/// Names that are direct arguments of calls reached from `e` through
/// nothing but other calls' arguments — and through the one part of a
/// `match` or an `if` that runs whatever happens, its scrutinee or its
/// condition: `match (simp(a), simp(b))` hands `a` and `b` on as surely as
/// a call would.
fn spine_args<'e>(e: &'e TExpr, out: &mut Vec<&'e str>) {
    let args = match &e.kind {
        TK::CallFn { args, .. } | TK::CallCtor { args, .. } | TK::MethodCall { args, .. } => args,
        TK::Match { scrutinee, .. } => return spine_args(scrutinee, out),
        TK::If { cond, .. } => return spine_args(cond, out),
        _ => return,
    };
    for a in args {
        match &a.kind {
            TK::Local(n) => out.push(n),
            _ => spine_args(a, out),
        }
    }
}

/// Whether the name is still spoken anywhere in the expression. A box can
/// only be built over if nothing left can reach it by name; its fields have
/// already been loaded into registers by then, so those do not count.
fn names_local(e: &TExpr, name: &str) -> bool {
    if matches!(&e.kind, TK::Local(n) if n == name) {
        return true;
    }
    let mut found = false;
    walk_children(e, &mut |c| {
        if !found {
            found = names_local(c, name);
        }
    });
    found
}

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
        TK::ChanRecv { ch }
        | TK::ChanOp { ch, .. }
        | TK::ChanTaken { ch }
        | TK::BufLen { buf: ch }
        | TK::BufPtr { buf: ch } => f(ch),
        TK::Unique { val } => f(val),
        TK::FieldGet { base, .. } => f(base),
        TK::FieldNamed { .. } => unreachable!("`zonk` names every deferred field"),
        TK::UpdateNamed { .. } => unreachable!("`zonk` builds every deferred update"),
        TK::MapNew => {}
        TK::MapLen { map } | TK::MapClear { map } => f(map),
        TK::MapFind { map, key: other }
        | TK::MapDel { map, key: other }
        | TK::MapNext { map, from: other }
        | TK::MapKeyAt { map, idx: other }
        | TK::MapValAt { map, idx: other } => {
            f(map);
            f(other);
        }
        TK::MapSet { map, key, val } => {
            f(map);
            f(key);
            f(val);
        }
        TK::BufNew { count, .. } => f(count),
        TK::BufGet { buf, idx, .. } => {
            f(buf);
            f(idx);
        }
        TK::BufSet { buf, idx, val, .. } => {
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
                    TSelectOp::Timeout { ms } => f(ms),
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
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
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
    /// Run this arm. A guarded one carries what to do when the guard is
    /// false: the rest of the match, in the same value context — which is
    /// where the fall-through goes, and why a guard costs the arms below it
    /// being tested again rather than the whole match being linear.
    Leaf(usize, Option<Box<Tree>>),
    /// Test the value at `occ` and take the matching branch.
    Switch { occ: Occ, on_int: bool, cases: Vec<(u64, Tree)>, default: Option<Box<Tree>> },
    /// No row can match — unreachable once the checker has run.
    Fail,
}

fn is_wild(p: &Pattern) -> bool {
    matches!(peel(p), Pattern::Wildcard | Pattern::Var(_))
}

/// What a pattern tests for, with any `@` names taken off the front. A name is
/// a binding, not a test: `whole @ Circle(r)` tests exactly what `Circle(r)`
/// tests. The names are picked up separately, by `collect_bindings`, which
/// walks the pattern whole and knows the position each one stands at.
fn peel(p: &Pattern) -> &Pattern {
    match p {
        Pattern::As(_, inner, _) => peel(inner),
        other => other,
    }
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
            match peel(&r.pats[0]) {
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
        Pattern::As(n, inner, _) => {
            out.push((n.clone(), occ.clone()));
            collect_bindings(inner, occ, ctors, out);
        }
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
    /// The trampolines for callbacks that carry a closure, one per C
    /// signature. See `gen_closure_trampoline`.
    closure_tramps: HashMap<(Vec<CTy>, CTy), FunctionValue<'ctx>>,
    /// Which parameters of each function are borrowed: passed without a
    /// reference of their own and released by nobody. See `borrowed_params`.
    borrowed_params: Vec<Vec<bool>>,
    /// Which function-typed parameters never escape their function. See
    /// `nonescaping_fn_params`.
    nonescaping_fn_params: Vec<Vec<bool>>,
    /// Set for the one lambda about to be built for a non-escaping position:
    /// its environment borrows its captures rather than holding them.
    borrow_captures: bool,
    /// `--unchecked`: no bounds test on any buffer access. The trade Zig's
    /// ReleaseFast and Swift's -Ounchecked make; off by default.
    unchecked: bool,
    /// Names bound by a `match` on a borrowed parameter (or on one of these):
    /// held by an earlier frame, like the parameter itself. See `borrowed_here`.
    held_above: HashSet<String>,
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
    /// Which lifted loop functions are leaves (see `leaf_loop`).
    leaf_loops: HashMap<usize, bool>,
    /// Bindings this frame owns whose one remaining use is the statement
    /// being generated, on its unconditional path: a call that takes one
    /// in an owned position is handed the reference itself, with no retain
    /// before and no release after — the callee then holds the only one,
    /// and `unique` inside it says so. Computed per top-level statement of
    /// a function body, empty anywhere a branch could leave the release on
    /// one path only.
    movable: HashSet<String>,
    /// Every name the current top-level statement speaks of exactly once
    /// and no later statement speaks of at all: the pool `movable` is drawn
    /// from, for the statement itself and for any block nested in it.
    root_free: HashSet<String>,
    /// Whether the code being generated runs on every path through the
    /// current statement. False inside a branch of an `if` or `match` in
    /// value position — a move there would take the reference on one path
    /// and leave the release owed on the other.
    move_ok: bool,
    /// A box this frame owns, whose arm is being generated, and which the arm
    /// can no longer name: a constructor of exactly its shape may be built in
    /// it instead of in a fresh one. See `take_reuse`.
    reuse: Vec<(PointerValue<'ctx>, Ty, usize)>,
    /// Boxes this frame has emptied: a `match` arm took the fields out of a
    /// box the frame owned and found, at run time, that nothing else held
    /// it. What is left is memory of a known shape — null where the box
    /// turned out to be shared — to be built over by a constructor of that
    /// shape, or given back at the frame's exit. See `compile_match`.
    shells: Vec<(PointerValue<'ctx>, Ty, usize)>,
    /// The scrutinee of the `match` being compiled, when it is a tuple
    /// written in place — `match (simp(a), simp(b))` — and so was never
    /// boxed: an LLVM struct of its parts, which `value_at` takes apart
    /// with `extractvalue` where a box would be read through a pointer.
    unboxed_root: Option<BasicValueEnum<'ctx>>,
    /// Of the function being generated: has it a `musttail` call, and a
    /// call to itself that is not a tail call? Both together are the shape
    /// LLVM's tail-recursion elimination gets wrong — see `gen_instance`.
    saw_musttail: bool,
    saw_nontail_self: bool,
    str_count: usize,
    lambda_count: usize,
    /// Built with `--parallel`: refcount updates check `rill_mt` and go
    /// atomic when the program asks for more than one worker.
    parallel: bool,
    /// `RILL_RC_CHECK` in the compiler's environment: every count update
    /// first looks for the runtime's freed-block paint on the object, and
    /// a touch of a freed count stops the program there, in the toucher's
    /// own frames. Pairs with `RILL_ALLOC_CHECK=1` at run time.
    rc_check: bool,
    /// Present in debug builds: emits DWARF so a debugger can map machine
    /// code back to Rill source lines.
    debug: Option<DebugCtx<'ctx>>,
    /// Line most recently attached to generated instructions.
    cur_line: u32,
    /// The function being generated, for spotting a tail call that goes round.
    cur_fn: usize,
    /// `rill explain`: whether the decisions below are being written down,
    /// where the expression being generated was written, and what has been
    /// noted so far — (function, line, text), each once.
    explain: bool,
    note_line: usize,
    notes: std::collections::BTreeSet<(usize, usize, String)>,
    /// Who calls whom, and the memoised answer to "what can this one reach".
    /// Built for every program: preemption asks it only when there are
    /// strands, but the untailed-recursion warning asks it everywhere.
    callgraph: Vec<Vec<usize>>,
    reaches: HashMap<usize, HashSet<usize>>,
    /// Caller/callee pairs already warned about, so a generic instantiated
    /// five times complains once.
    warned_untailed: HashSet<(usize, usize)>,
    /// The program has strands in it, so tail calls carry a preemption check.
    /// Decided up front by `uses_scheduler`; see `emit_preempt_check`.
    preempt: bool,
    /// A library whose exports reach a spawn or a channel: run in host mode.
    host_strands: bool,
    /// Building for wasm, which spells the entry point differently. Decided
    /// here rather than at object-writing time because the name has to be
    /// chosen while the function is being created.
    wasm: bool,
    /// A `--shared` build: a library a host loads, with `export fn`s as its
    /// entry points and `rill_init` in place of `main`.
    shared: bool,
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
        unchecked: bool,
        shared: bool,
        explain: bool,
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
            closure_tramps: HashMap::new(),
            borrowed_params: borrowed_params(tmod),
            nonescaping_fn_params: nonescaping_fn_params(tmod),
            borrow_captures: false,
            unchecked,
            held_above: HashSet::new(),
            bound_fns: HashMap::new(),
            callbacks: HashMap::new(),
            queue: VecDeque::new(),
            adt_drops: HashMap::new(),
            adt_shows: HashMap::new(),
            adt_eqs: HashMap::new(),
            nullary_singletons: HashMap::new(),
            cleanup: Vec::new(),
            leaf_loops: HashMap::new(),
            movable: HashSet::new(),
            root_free: HashSet::new(),
            move_ok: false,
            reuse: Vec::new(),
            shells: Vec::new(),
            unboxed_root: None,
            saw_musttail: false,
            saw_nontail_self: false,
            str_count: 0,
            lambda_count: 0,
            parallel,
            rc_check: std::env::var_os("RILL_RC_CHECK").is_some(),
            debug: None,
            cur_line: 0,
            cur_fn: usize::MAX,
            explain,
            note_line: 0,
            notes: std::collections::BTreeSet::new(),
            callgraph: call_graph(tmod),
            reaches: HashMap::new(),
            warned_untailed: HashSet::new(),
            preempt,
            host_strands: shared && library_uses_scheduler(tmod),
            wasm: target.is_some_and(|t| t.starts_with("wasm")),
            shared,
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
            // A function whose signature was declared may call itself at
            // another type; one that does so at a bigger type each time has
            // no end in a compiler that makes one copy per type, and this
            // is where that would otherwise go on forever. No type a program
            // writes is anywhere near this big; one that is has been grown.
            if key.1.iter().map(rill_types::ty_size).sum::<usize>() > TYPE_SIZE_CAP {
                let f = &cg.tmod.fns[key.0];
                return Err(CodegenError::new(
                    f.line,
                    format!(
                        "`{}` is being specialized at ever bigger types: it calls itself at \
                         a bigger one each time, and a copy is made per type. Recursion over \
                         a type that nests without end cannot be compiled this way; keep the \
                         growing part in a value instead",
                        f.name
                    ),
                ));
            }
            cg.gen_instance(&key)?;
        }
        cg.gen_main_wrapper();
        cg.gen_sym_table();

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
            Ty::Float2 => Some(self.ctx.f64_type().vec_type(2).into()),
            Ty::Bool => Some(self.ctx.bool_type().into()),
            Ty::Adt(..) if self.is_flat(ty) => Some(self.flat_struct(ty).into()),
            Ty::Str | Ty::StrBuf | Ty::Adt(..) | Ty::Chan(..) | Ty::Ptr | Ty::Buf(_)
            | Ty::Map(..) => Some(self.ctx.ptr_type(Default::default()).into()),
            Ty::Fn(..) => Some(self.closure_type().into()),
            Ty::Unit => None,
            Ty::Var(_) | Ty::Gen(_) => unreachable!("codegen requires concrete types"),
        }
    }

    /// The integer C calls a word: `size_t` wide, which is pointer wide.
    fn word_type(&self) -> IntType<'ctx> {
        if self.wasm { self.ctx.i32_type() } else { self.ctx.i64_type() }
    }

    /// LLVM type of a foreign signature position; `None` for Unit (a void
    /// return). C takes a bool as a byte.
    fn c_type(&self, c: &CTy) -> Option<BasicTypeEnum<'ctx>> {
        Some(match c {
            CTy::I8 | CTy::U8 | CTy::Bool => self.ctx.i8_type().into(),
            CTy::I16 | CTy::U16 => self.ctx.i16_type().into(),
            CTy::I32 | CTy::U32 => self.ctx.i32_type().into(),
            CTy::I64 | CTy::U64 => self.ctx.i64_type().into(),
            // C's word: as wide as the target's pointer. Everywhere Rill links
            // natively that is 64 bits and this costs nothing; on wasm32 it is
            // 32, which is the whole reason the type exists.
            CTy::USize | CTy::ISize => self.word_type().into(),
            CTy::F32 => self.ctx.f32_type().into(),
            CTy::F64 => self.ctx.f64_type().into(),
            CTy::Ptr | CTy::Fn(..) | CTy::Env => self.ctx.ptr_type(Default::default()).into(),
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
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Unit | CTy::Fn(..) | CTy::Env => v,
            // A word is already 64 bits on a 64-bit target, and `trunc` to the
            // width a value already has is not an instruction.
            CTy::USize | CTy::ISize if !self.wasm => v,
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
            CTy::I64 | CTy::U64 | CTy::F64 | CTy::Ptr | CTy::Unit | CTy::Fn(..) | CTy::Env => v,
            CTy::USize | CTy::ISize if !self.wasm => v,
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
    /// A buffer value points at its first element; its count sits sixteen
    /// bytes before it and its length eight, so that `v[i]` is one
    /// register-indexed load with nothing added — the header's offset used
    /// to cost an `add` per element in every scan.
    fn buf_len(&self, p: PointerValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let slot = unsafe {
            self.builder
                .build_gep(i64t, p, &[i64t.const_int(-1i64 as u64, true)], "len.ptr")
                .expect("LLVM builder error")
        };
        let load = self.builder.build_load(i64t, slot, "len").expect("LLVM builder error");
        // Not marked `invariant.load`, though a buffer's length never
        // changes while the buffer lives: the mark lets LLVM move the load
        // anywhere the pointer is, which included past the buffer's own
        // release — `n + #make()` read the header of a freed block, and for
        // a block beyond the pool `free` had written over it. So the load
        // stays a plain one, ordered with the release, and a loop that
        // stores reads the header again on each bound it checks.
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

    /// Two alias scopes for a builder: its header — the count, the pointer,
    /// the length and the capacity — and the bytes that pointer leads to,
    /// which are a block of their own. A byte written into the text can never
    /// land on the header, and saying so is the whole difference between a
    /// run of appends that keeps the length and the capacity in registers and
    /// one that reads them back after every single write.
    fn sb_scopes(&self) -> (MetadataValue<'ctx>, MetadataValue<'ctx>) {
        let dom = self.ctx.metadata_node(&[self.ctx.metadata_string("rill.strbuf").into()]);
        let head = self
            .ctx
            .metadata_node(&[self.ctx.metadata_string("rill.strbuf.head").into(), dom.into()]);
        let data = self
            .ctx
            .metadata_node(&[self.ctx.metadata_string("rill.strbuf.data").into(), dom.into()]);
        (
            self.ctx.metadata_node(&[head.into()]),
            self.ctx.metadata_node(&[data.into()]),
        )
    }

    /// Says this instruction touches a builder's header and nothing else.
    fn mark_head(&self, inst: Option<InstructionValue<'ctx>>) {
        let Some(inst) = inst else { return };
        let (head, data) = self.sb_scopes();
        let _ = inst.set_metadata(head, self.ctx.get_kind_id("alias.scope"));
        let _ = inst.set_metadata(data, self.ctx.get_kind_id("noalias"));
    }

    /// Says this instruction touches the bytes and nothing else.
    fn mark_data(&self, inst: Option<InstructionValue<'ctx>>) {
        let Some(inst) = inst else { return };
        let (head, data) = self.sb_scopes();
        let _ = inst.set_metadata(data, self.ctx.get_kind_id("alias.scope"));
        let _ = inst.set_metadata(head, self.ctx.get_kind_id("noalias"));
    }

    /// The instruction the builder just laid down, for the two above.
    fn last_inst(&self) -> Option<InstructionValue<'ctx>> {
        self.builder.get_insert_block()?.get_last_instruction()
    }

    /// A builder's three fields, as addresses.
    fn sb_fields(
        &self,
        bp: PointerValue<'ctx>,
    ) -> (PointerValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let sb = self
            .ctx
            .struct_type(&[i64t.into(), ptrt.into(), i64t.into(), i64t.into()], false);
        let data = self.builder.build_struct_gep(sb, bp, 1, "sb.data.p").expect("LLVM builder error");
        let len = self.builder.build_struct_gep(sb, bp, 2, "sb.len.p").expect("LLVM builder error");
        let cap = self.builder.build_struct_gep(sb, bp, 3, "sb.cap.p").expect("LLVM builder error");
        (data, len, cap)
    }

    /// Reads a builder's length or capacity, saying it is the header.
    fn sb_load(&self, at: PointerValue<'ctx>, name: &str) -> IntValue<'ctx> {
        let got = self.builder.build_load(self.ctx.i64_type(), at, name).expect("LLVM builder error");
        self.mark_head(got.as_instruction_value());
        got.into_int_value()
    }

    /// Says this instruction leaves every builder header alone.
    fn mark_off_head(&self, inst: Option<InstructionValue<'ctx>>) {
        let Some(inst) = inst else { return };
        let (head, _) = self.sb_scopes();
        let _ = inst.set_metadata(head, self.ctx.get_kind_id("noalias"));
    }

    /// Room for `need` more bytes, and the block and the length as they now
    /// stand. Everything the header gets is written here rather than inside
    /// the runtime, which is asked only for a bigger block: a call that took
    /// the builder itself would cost the caller the length and the capacity
    /// it is holding in registers, and reading those back is a store waiting
    /// on a load in the middle of the tightest loop a page has.
    fn sb_room(
        &mut self,
        bp: PointerValue<'ctx>,
        need: IntValue<'ctx>,
        func: FunctionValue<'ctx>,
    ) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let (data_p, len_p, cap_p) = self.sb_fields(bp);
        let len = self.sb_load(len_p, "sb.len");
        let cap = self.sb_load(cap_p, "sb.cap");
        let had = self.builder.build_load(ptrt, data_p, "sb.data").expect("LLVM builder error");
        self.mark_head(had.as_instruction_value());
        let had = had.into_pointer_value();
        let want = self.builder.build_int_add(len, need, "sb.want").expect("LLVM builder error");
        let fits = self
            .builder
            .build_int_compare(IntPredicate::SLE, want, cap, "sb.fits")
            .expect("LLVM builder error");
        let grow_bb = self.ctx.append_basic_block(func, "sb.grow");
        let on_bb = self.ctx.append_basic_block(func, "sb.room");
        let from_bb = self.builder.get_insert_block().expect("a block");
        self.builder
            .build_conditional_branch(fits, on_bb, grow_bb)
            .expect("LLVM builder error");

        // Twice what it had, or what is wanted if that is more, and never
        // less than the thirty-two a builder starts with.
        self.builder.position_at_end(grow_bb);
        let twice = self
            .builder
            .build_int_mul(cap, i64t.const_int(2, false), "sb.twice")
            .expect("LLVM builder error");
        let small = self
            .builder
            .build_int_compare(IntPredicate::SLT, twice, i64t.const_int(32, false), "sb.small")
            .expect("LLVM builder error");
        let base = self
            .builder
            .build_select(small, i64t.const_int(32, false), twice, "sb.base")
            .expect("LLVM builder error")
            .into_int_value();
        let short = self
            .builder
            .build_int_compare(IntPredicate::SLT, base, want, "sb.short")
            .expect("LLVM builder error");
        let fresh_cap = self
            .builder
            .build_select(short, want, base, "sb.cap2")
            .expect("LLVM builder error")
            .into_int_value();
        let f = self.module.get_function("rill_sb_grow").unwrap();
        let got = self
            .builder
            .build_call(f, &[had.into(), len.into(), cap.into(), fresh_cap.into()], "sb.fresh")
            .expect("LLVM builder error");
        let fresh = match got.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_pointer_value(),
            _ => unreachable!(),
        };
        self.mark_off_head(fresh.as_instruction_value());
        let put = self.builder.build_store(data_p, fresh).expect("LLVM builder error");
        self.mark_head(Some(put));
        let put = self.builder.build_store(cap_p, fresh_cap).expect("LLVM builder error");
        self.mark_head(Some(put));
        self.builder.build_unconditional_branch(on_bb).expect("LLVM builder error");

        self.builder.position_at_end(on_bb);
        let which = self.builder.build_phi(ptrt, "sb.block").expect("LLVM builder error");
        which.add_incoming(&[(&had, from_bb), (&fresh, grow_bb)]);
        (which.as_basic_value().into_pointer_value(), len)
    }

    /// Writes the new length back, which is all an append leaves behind.
    fn sb_wrote(&self, bp: PointerValue<'ctx>, len: IntValue<'ctx>, by: IntValue<'ctx>) {
        let (_, len_p, _) = self.sb_fields(bp);
        let want = self.builder.build_int_add(len, by, "sb.now").expect("LLVM builder error");
        let put = self.builder.build_store(len_p, want).expect("LLVM builder error");
        self.mark_head(Some(put));
    }

    /// The two tables a number is written with: every pair of digits from
    /// "00" to "99", and the powers of ten. Both are laid down once per
    /// module, the first time an integer is appended anywhere in it.
    fn digit_tables(&mut self) -> (PointerValue<'ctx>, PointerValue<'ctx>) {
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let pairs = match self.module.get_global("rill.pairs") {
            Some(g) => g,
            None => {
                // Two hundred bytes and one to spare: a single digit is
                // written as the halfword starting at its own byte, and that
                // reads one past the last pair.
                let mut text = Vec::with_capacity(202);
                for n in 0..100u8 {
                    text.push(b'0' + n / 10);
                    text.push(b'0' + n % 10);
                }
                text.push(b'0');
                text.push(b'0');
                let init = self.ctx.const_string(&text, false);
                let g = self.module.add_global(init.get_type(), None, "rill.pairs");
                g.set_initializer(&init);
                g.set_linkage(inkwell::module::Linkage::Private);
                g.set_constant(true);
                g
            }
        };
        let tens = match self.module.get_global("rill.tens") {
            Some(g) => g,
            None => {
                let mut vals = Vec::with_capacity(20);
                let mut at = 1u64;
                for i in 0..20 {
                    vals.push(i64t.const_int(at, false));
                    if i < 19 {
                        at *= 10;
                    }
                }
                let init = i64t.const_array(&vals);
                let g = self.module.add_global(init.get_type(), None, "rill.tens");
                g.set_initializer(&init);
                g.set_linkage(inkwell::module::Linkage::Private);
                g.set_constant(true);
                g
            }
        };
        let _ = i8t;
        (pairs.as_pointer_value(), tens.as_pointer_value())
    }

    /// `b += int_to_str(n)`, written out here rather than called for. Almost
    /// every number a page carries is one or two digits, and that case is a
    /// compare, a halfword out of a table and a store, with no branch in it
    /// past the first: the minus goes down whether or not there is one, and
    /// the digits land on top of it when there is not. How many digits a
    /// larger number has is read off its top bit rather than found by
    /// dividing, and its digits go in two at a time from the end. The
    /// magnitude is carried unsigned, which is what makes the smallest
    /// integer — the one with no positive counterpart — need no case of its
    /// own, and the whole append comes out with no call in it at all.
    fn gen_strbuf_int(
        &mut self,
        bp: PointerValue<'ctx>,
        v: IntValue<'ctx>,
        func: FunctionValue<'ctx>,
    ) {
        let i64t = self.ctx.i64_type();
        let i8t = self.ctx.i8_type();
        let i16t = self.ctx.i16_type();
        let one = i64t.const_int(1, false);
        let two = i64t.const_int(2, false);
        let hundred = i64t.const_int(100, false);
        let (pairs, tens) = self.digit_tables();

        let neg = self
            .builder
            .build_int_compare(IntPredicate::SLT, v, i64t.const_zero(), "sb.neg")
            .expect("LLVM builder error");
        let flipped = self.builder.build_int_neg(v, "sb.flip").expect("LLVM builder error");
        // Unsigned from here on, so that the smallest integer's magnitude —
        // which no signed value can hold — is simply the bits it already has.
        let mag = self
            .builder
            .build_select(neg, flipped, v, "sb.mag")
            .expect("LLVM builder error")
            .into_int_value();
        let sign = self
            .builder
            .build_select(neg, one, i64t.const_zero(), "sb.sign")
            .expect("LLVM builder error")
            .into_int_value();

        let small_bb = self.ctx.append_basic_block(func, "sb.small");
        let wide_bb = self.ctx.append_basic_block(func, "sb.wide");
        let done_bb = self.ctx.append_basic_block(func, "sb.done");
        let is_small = self
            .builder
            .build_int_compare(IntPredicate::ULT, mag, hundred, "sb.is.small")
            .expect("LLVM builder error");
        self.builder
            .build_conditional_branch(is_small, small_bb, wide_bb)
            .expect("LLVM builder error");

        // One digit or two, written as a halfword either way. When there is
        // only one, the table is read from its second byte and the byte that
        // follows lands past the length, where nothing looks.
        self.builder.position_at_end(small_bb);
        let ten_up = self
            .builder
            .build_int_compare(IntPredicate::UGE, mag, i64t.const_int(10, false), "sb.is.two")
            .expect("LLVM builder error");
        let digits = self
            .builder
            .build_select(ten_up, two, one, "sb.digits")
            .expect("LLVM builder error")
            .into_int_value();
        let need = self.builder.build_int_add(digits, sign, "sb.need").expect("LLVM builder error");
        let spare = self.builder.build_int_add(need, one, "sb.spare").expect("LLVM builder error");
        let (data, len) = self.sb_room(bp, spare, func);
        self.put_minus(data, len, i8t);
        // The halfword starts where the digits do, so it ends two past that.
        let from = self.builder.build_int_add(len, sign, "sb.from").expect("LLVM builder error");
        let at = self.builder.build_int_add(from, two, "sb.at").expect("LLVM builder error");
        let base = self.builder.build_int_mul(mag, two, "sb.pair.i").expect("LLVM builder error");
        let skip = self.builder.build_int_sub(two, digits, "sb.skip").expect("LLVM builder error");
        let idx = self.builder.build_int_add(base, skip, "sb.idx").expect("LLVM builder error");
        self.put_pair(pairs, data, at, idx, i16t, i8t);
        self.sb_wrote(bp, len, need);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        // Anything larger: floor(log10) from the position of the top bit,
        // then one comparison to settle the case the estimate is short by one.
        self.builder.position_at_end(wide_bb);
        let ctlz = inkwell::intrinsics::Intrinsic::find("llvm.ctlz")
            .and_then(|i| i.get_declaration(&self.module, &[i64t.into()]))
            .expect("llvm.ctlz");
        let lz = self
            .builder
            .build_call(
                ctlz,
                &[mag.into(), self.ctx.bool_type().const_int(1, false).into()],
                "sb.lz",
            )
            .expect("LLVM builder error");
        let lz = match lz.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(x) => x.into_int_value(),
            _ => unreachable!(),
        };
        let bits = self
            .builder
            .build_int_sub(i64t.const_int(64, false), lz, "sb.bits")
            .expect("LLVM builder error");
        let scaled = self
            .builder
            .build_int_mul(bits, i64t.const_int(1233, false), "sb.scaled")
            .expect("LLVM builder error");
        let guess = self
            .builder
            .build_right_shift(scaled, i64t.const_int(12, false), false, "sb.guess")
            .expect("LLVM builder error");
        let ten_at = unsafe {
            self.builder
                .build_in_bounds_gep(i64t, tens, &[guess], "sb.ten.p")
                .expect("LLVM builder error")
        };
        let ten = self.builder.build_load(i64t, ten_at, "sb.ten").expect("LLVM builder error");
        self.mark_invariant(ten);
        let short = self
            .builder
            .build_int_compare(IntPredicate::ULT, mag, ten.into_int_value(), "sb.short")
            .expect("LLVM builder error");
        let carry = self
            .builder
            .build_select(short, i64t.const_zero(), one, "sb.carry")
            .expect("LLVM builder error")
            .into_int_value();
        let many = self.builder.build_int_add(guess, carry, "sb.many").expect("LLVM builder error");
        let need = self.builder.build_int_add(many, sign, "sb.need.w").expect("LLVM builder error");
        let (data, len) = self.sb_room(bp, need, func);
        self.put_minus(data, len, i8t);
        let end = self.builder.build_int_add(len, need, "sb.end").expect("LLVM builder error");

        // Two digits at a time, from the end back.
        let loop_bb = self.ctx.append_basic_block(func, "sb.pairs");
        let body_bb = self.ctx.append_basic_block(func, "sb.pair");
        let last_bb = self.ctx.append_basic_block(func, "sb.last");
        let start_bb = self.builder.get_insert_block().expect("a block");
        self.builder.build_unconditional_branch(loop_bb).expect("LLVM builder error");
        self.builder.position_at_end(loop_bb);
        let at = self.builder.build_phi(i64t, "sb.at.w").expect("LLVM builder error");
        let left = self.builder.build_phi(i64t, "sb.left").expect("LLVM builder error");
        at.add_incoming(&[(&end, start_bb)]);
        left.add_incoming(&[(&mag, start_bb)]);
        let atv = at.as_basic_value().into_int_value();
        let leftv = left.as_basic_value().into_int_value();
        let big = self
            .builder
            .build_int_compare(IntPredicate::UGE, leftv, hundred, "sb.big")
            .expect("LLVM builder error");
        self.builder
            .build_conditional_branch(big, body_bb, last_bb)
            .expect("LLVM builder error");
        self.builder.position_at_end(body_bb);
        let q = self
            .builder
            .build_int_unsigned_div(leftv, hundred, "sb.q")
            .expect("LLVM builder error");
        let back = self.builder.build_int_mul(q, hundred, "sb.q100").expect("LLVM builder error");
        let r = self.builder.build_int_sub(leftv, back, "sb.r").expect("LLVM builder error");
        let idx = self.builder.build_int_mul(r, two, "sb.pair.w").expect("LLVM builder error");
        self.put_pair(pairs, data, atv, idx, i16t, i8t);
        let back2 = self.builder.build_int_sub(atv, two, "sb.back2").expect("LLVM builder error");
        self.builder.build_unconditional_branch(loop_bb).expect("LLVM builder error");
        at.add_incoming(&[(&back2, body_bb)]);
        left.add_incoming(&[(&q, body_bb)]);

        // What is left is one digit or two. Here the two are told apart,
        // because everything after them is already written and a byte laid
        // down past the end would land on it.
        self.builder.position_at_end(last_bb);
        let two_bb = self.ctx.append_basic_block(func, "sb.tail.two");
        let one_bb = self.ctx.append_basic_block(func, "sb.tail.one");
        let ten_up = self
            .builder
            .build_int_compare(IntPredicate::UGE, leftv, i64t.const_int(10, false), "sb.tenup")
            .expect("LLVM builder error");
        self.builder.build_conditional_branch(ten_up, two_bb, one_bb).expect("LLVM builder error");
        self.builder.position_at_end(two_bb);
        let idx = self.builder.build_int_mul(leftv, two, "sb.pair.i.w").expect("LLVM builder error");
        self.put_pair(pairs, data, atv, idx, i16t, i8t);
        self.sb_wrote(bp, len, need);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
        self.builder.position_at_end(one_bb);
        let ch = self
            .builder
            .build_int_add(leftv, i64t.const_int(48, false), "sb.ch")
            .expect("LLVM builder error");
        let ch = self.builder.build_int_truncate(ch, i8t, "sb.byte").expect("LLVM builder error");
        let where_ = self.builder.build_int_sub(atv, one, "sb.one.at").expect("LLVM builder error");
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(i8t, data, &[where_], "sb.one.p")
                .expect("LLVM builder error")
        };
        let put = self.builder.build_store(slot, ch).expect("LLVM builder error");
        self.mark_data(Some(put));
        self.sb_wrote(bp, len, need);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
        self.builder.position_at_end(done_bb);
    }

    /// The minus, laid down whether or not the number has one: when it does
    /// not, the first digit is written over it.
    fn put_minus(&self, data: PointerValue<'ctx>, len: IntValue<'ctx>, i8t: IntType<'ctx>) {
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(i8t, data, &[len], "sb.minus.at")
                .expect("LLVM builder error")
        };
        let put = self
            .builder
            .build_store(slot, i8t.const_int(45, false))
            .expect("LLVM builder error");
        self.mark_data(Some(put));
    }

    /// Two bytes out of the table, written as one halfword ending at `at`.
    fn put_pair(
        &self,
        pairs: PointerValue<'ctx>,
        data: PointerValue<'ctx>,
        at: IntValue<'ctx>,
        idx: IntValue<'ctx>,
        i16t: IntType<'ctx>,
        i8t: IntType<'ctx>,
    ) {
        let i64t = self.ctx.i64_type();
        let from = unsafe {
            self.builder
                .build_in_bounds_gep(i8t, pairs, &[idx], "sb.pair.p")
                .expect("LLVM builder error")
        };
        let both = self.builder.build_load(i16t, from, "sb.pair").expect("LLVM builder error");
        self.mark_invariant(both);
        let where_ = self
            .builder
            .build_int_sub(at, i64t.const_int(2, false), "sb.pair.at")
            .expect("LLVM builder error");
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(i8t, data, &[where_], "sb.pair.slot")
                .expect("LLVM builder error")
        };
        let put = self.builder.build_store(slot, both).expect("LLVM builder error");
        self.mark_data(Some(put));
    }

    /// `b += s`: the bytes of a string, however it was arrived at. The length
    /// comes out of its header, the room is taken, and the copy is the whole
    /// of it — the same shape as the literal above, which knows its length
    /// without looking.
    fn gen_strbuf_str(
        &mut self,
        bp: PointerValue<'ctx>,
        sp: PointerValue<'ctx>,
        fixed: Option<usize>,
        func: FunctionValue<'ctx>,
    ) {
        let i64t = self.ctx.i64_type();
        let i8t = self.ctx.i8_type();
        // A literal is copied in whole words: sixteen bytes at a time up to
        // the next boundary past its end. The bytes past the text are inside
        // the object either way, and inside the builder's room, and they sit
        // past its length where nothing reads them.
        let wide = fixed.map(|n| (n + 15) / 16 * 16);
        let need = match fixed {
            Some(n) => i64t.const_int(n as u64, false),
            None => {
                let at = unsafe {
                    self.builder
                        .build_in_bounds_gep(i8t, sp, &[i64t.const_int(8, false)], "str.len.p")
                        .expect("LLVM builder error")
                };
                let got =
                    self.builder.build_load(i64t, at, "str.len").expect("LLVM builder error");
                self.mark_off_head(got.as_instruction_value());
                got.into_int_value()
            }
        };
        let room = match wide {
            Some(n) => i64t.const_int(n as u64, false),
            None => need,
        };
        let (data, len) = self.sb_room(bp, room, func);
        let at = unsafe {
            self.builder.build_in_bounds_gep(i8t, data, &[len], "sb.at").expect("LLVM builder error")
        };
        // The bytes sit sixteen in, past the count and the length.
        let from = unsafe {
            self.builder
                .build_in_bounds_gep(i8t, sp, &[i64t.const_int(16, false)], "str.at")
                .expect("LLVM builder error")
        };
        self.builder.build_memcpy(at, 1, from, 1, room).expect("LLVM builder error");
        self.mark_data(self.last_inst());
        self.sb_wrote(bp, len, need);
    }

    /// The first element's address, which is also what C is handed.
    fn buf_data(&self, p: PointerValue<'ctx>) -> PointerValue<'ctx> {
        p
    }

    /// The count of a buffer: sixteen bytes before its first element.
    fn buf_rc_ptr(&self, p: PointerValue<'ctx>) -> PointerValue<'ctx> {
        unsafe {
            self.builder
                .build_gep(self.ctx.i64_type(), p, &[self.ctx.i64_type().const_int(-2i64 as u64, true)], "rc.ptr")
                .expect("LLVM builder error")
        }
    }

    /// The address of element `i`, after checking it is one. The comparison is
    /// unsigned, so a negative index fails the same test as one off the end.
    /// The slot, with the bounds test unless the checker proved it in front
    /// of the loop (`unchecked`) or the build asked for none (`--unchecked`).
    fn buf_slot_at(
        &mut self,
        p: PointerValue<'ctx>,
        i: IntValue<'ctx>,
        w: &CTy,
        func: FunctionValue<'ctx>,
        unchecked: bool,
    ) -> PointerValue<'ctx> {
        if unchecked || self.unchecked {
            let data = self.buf_data(p);
            return unsafe {
                self.builder
                    .build_gep(self.c_type(w).expect("a width"), data, &[i], "slot")
                    .expect("LLVM builder error")
            };
        }
        self.buf_slot(p, i, w, func)
    }

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
        let ptr = self.emit_alloc(size, name);
        let rc_ptr = self
            .builder
            .build_struct_gep(struct_ty, ptr, 0, "rc.init")
            .expect("LLVM builder error");
        self.builder
            .build_store(rc_ptr, self.ctx.i64_type().const_int(1, false))
            .expect("LLVM builder error");
        ptr
    }

    /// The pool's free lists and the live-allocation counter, as the
    /// runtime exports them, declared on first use.
    fn pool_globals(&mut self) -> (PointerValue<'ctx>, PointerValue<'ctx>) {
        let ptrt = self.ctx.ptr_type(Default::default());
        let pools = match self.module.get_global("rill_pools") {
            Some(g) => g,
            None => self.module.add_global(ptrt.array_type(16), None, "rill_pools"),
        };
        let live = match self.module.get_global("rill_live_allocs") {
            Some(g) => g,
            None => self.module.add_global(self.ctx.i64_type(), None, "rill_live_allocs"),
        };
        (pools.as_pointer_value(), live.as_pointer_value())
    }

    /// `rill_alloc(size)`, with the pool's fast path laid down here: a block
    /// of the right size class is popped off its free list without a call,
    /// and the runtime is asked only when that list is empty (or the object
    /// is too big for the pool). Object sizes are constants, so LLVM folds
    /// the class and drops whichever branch cannot be taken. A program that
    /// is nothing but building and dropping — five cons cells and a closure
    /// per step of an n-body — spent half its time in these two calls.
    ///
    /// Only where one strand's pool is *the* pool: a parallel build has one
    /// per worker and asks the runtime which, and wasm keeps the call.
    fn emit_alloc(&mut self, size: IntValue<'ctx>, name: &str) -> PointerValue<'ctx> {
        let alloc = self.module.get_function("rill_alloc").unwrap();
        let slow_call = |cg: &mut Self| -> PointerValue<'ctx> {
            let call = cg.builder.build_call(alloc, &[size.into()], name).expect("LLVM builder error");
            match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(v) => v.into_pointer_value(),
                _ => unreachable!(),
            }
        };
        if self.wasm {
            return slow_call(self);
        }
        let func = self.builder.get_insert_block().and_then(|b| b.get_parent()).expect("in a function");
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let (pools, live) = self.pool_globals();
        let fast_bb = self.ctx.append_basic_block(func, "alloc.fast");
        let take_bb = self.ctx.append_basic_block(func, "alloc.take");
        let slow_bb = self.ctx.append_basic_block(func, "alloc.slow");
        let done_bb = self.ctx.append_basic_block(func, "alloc.done");
        let small = self
            .builder
            .build_int_compare(IntPredicate::ULE, size, i64t.const_int(256, false), "small")
            .expect("LLVM builder error");
        let small = self.alone_and(small);
        self.builder.build_conditional_branch(small, fast_bb, slow_bb).expect("LLVM builder error");

        self.builder.position_at_end(fast_bb);
        let class = self.pool_class(size);
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(ptrt.array_type(16), pools, &[i64t.const_zero(), class], "free.slot")
                .expect("LLVM builder error")
        };
        let head = self.builder.build_load(ptrt, slot, "head").expect("LLVM builder error").into_pointer_value();
        let empty = self.builder.build_is_null(head, "empty").expect("LLVM builder error");
        self.builder.build_conditional_branch(empty, slow_bb, take_bb).expect("LLVM builder error");

        self.builder.position_at_end(take_bb);
        let next = self.builder.build_load(ptrt, head, "next").expect("LLVM builder error");
        self.builder.build_store(slot, next).expect("LLVM builder error");
        let n = self.builder.build_load(i64t, live, "live").expect("LLVM builder error").into_int_value();
        let n1 = self.builder.build_int_add(n, i64t.const_int(1, false), "live.inc").expect("LLVM builder error");
        self.builder.build_store(live, n1).expect("LLVM builder error");
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(slow_bb);
        let p = slow_call(self);
        let slow_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(done_bb);
        let phi = self.builder.build_phi(ptrt, name).expect("LLVM builder error");
        phi.add_incoming(&[(&head, take_bb), (&p, slow_end)]);
        phi.as_basic_value().into_pointer_value()
    }

    /// `rill_free_sized(p, size)` with the same fast path: the block goes
    /// back on its class's free list in three instructions.
    fn emit_free(&mut self, p: PointerValue<'ctx>, size: IntValue<'ctx>) {
        if self.wasm {
            self.call_rt("rill_free_sized", &[p.into(), size.into()]);
            return;
        }
        let func = self.builder.get_insert_block().and_then(|b| b.get_parent()).expect("in a function");
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let (pools, live) = self.pool_globals();
        let fast_bb = self.ctx.append_basic_block(func, "free.fast");
        let slow_bb = self.ctx.append_basic_block(func, "free.slow");
        let done_bb = self.ctx.append_basic_block(func, "free.done");
        let small = self
            .builder
            .build_int_compare(IntPredicate::ULE, size, i64t.const_int(256, false), "small")
            .expect("LLVM builder error");
        let small = self.alone_and(small);
        self.builder.build_conditional_branch(small, fast_bb, slow_bb).expect("LLVM builder error");

        self.builder.position_at_end(fast_bb);
        let class = self.pool_class(size);
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(ptrt.array_type(16), pools, &[i64t.const_zero(), class], "free.slot")
                .expect("LLVM builder error")
        };
        let head = self.builder.build_load(ptrt, slot, "head").expect("LLVM builder error");
        self.builder.build_store(p, head).expect("LLVM builder error");
        self.builder.build_store(slot, p).expect("LLVM builder error");
        let n = self.builder.build_load(i64t, live, "live").expect("LLVM builder error").into_int_value();
        let n1 = self.builder.build_int_sub(n, i64t.const_int(1, false), "live.dec").expect("LLVM builder error");
        self.builder.build_store(live, n1).expect("LLVM builder error");
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(slow_bb);
        self.call_rt("rill_free_sized", &[p.into(), size.into()]);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(done_bb);
    }

    /// On a parallel build the pool fast path is pool 0's, which is the
    /// pool only while one worker runs (`rill_mt == 0`): a library run on
    /// one host thread outside a parallel section. `cond`, and that.
    fn alone_and(&mut self, cond: IntValue<'ctx>) -> IntValue<'ctx> {
        if !self.parallel {
            return cond;
        }
        let mt = self.mt_flag();
        let flag = self
            .builder
            .build_load(self.ctx.i8_type(), mt, "mt")
            .expect("LLVM builder error")
            .into_int_value();
        let alone = self
            .builder
            .build_int_compare(IntPredicate::EQ, flag, self.ctx.i8_type().const_zero(), "alone")
            .expect("LLVM builder error");
        self.builder.build_and(cond, alone, "small.alone").expect("LLVM builder error")
    }

    /// `(size + 15) / 16 - 1`: the runtime's `pool_class`.
    fn pool_class(&self, size: IntValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let up = self.builder.build_int_add(size, i64t.const_int(15, false), "sz15").expect("LLVM builder error");
        let q = self.builder.build_int_unsigned_div(up, i64t.const_int(16, false), "sz16").expect("LLVM builder error");
        self.builder.build_int_sub(q, i64t.const_int(1, false), "class").expect("LLVM builder error")
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

    /// The signature of a named function's instance: a flat value arrives as
    /// its fields, one parameter each, rather than as one aggregate. The
    /// registers are the same either way — `cc_for` counts fields — but a
    /// loop carrying a `(Float, Float, Float, Float)` accumulator is a
    /// function that tail-calls itself with one, and LLVM turns that into a
    /// loop whose induction variable is an aggregate `phi`, which nothing
    /// downstream can see through: the four independent sums never became
    /// vector lanes. Four scalar `phi`s are what the vectorizer reads.
    fn instance_llvm_type(&self, params: &[Ty], ret: &Ty) -> FunctionType<'ctx> {
        let mut ps: Vec<BasicMetadataTypeEnum> = Vec::new();
        for p in params {
            match self.flat_fields(p) {
                Some(fields) => {
                    for f in &fields {
                        ps.push(self.llvm_type(f).expect("a scalar").into());
                    }
                }
                None => ps.push(self.llvm_type(p).expect("parameters are never Unit").into()),
            }
        }
        match self.llvm_type(ret) {
            None => self.ctx.void_type().fn_type(&ps, false),
            Some(t) => t.fn_type(&ps, false),
        }
    }

    /// Arguments for an instance's signature: every flat value taken apart
    /// into its fields, in place.
    fn explode_args(
        &self,
        argv: Vec<BasicMetadataValueEnum<'ctx>>,
        params: &[Ty],
    ) -> Vec<BasicMetadataValueEnum<'ctx>> {
        let mut out: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(argv.len());
        for (v, t) in argv.into_iter().zip(params) {
            match self.flat_fields(t) {
                Some(fields) => {
                    let agg: BasicValueEnum<'ctx> = v.try_into().expect("a value argument");
                    for i in 0..fields.len() {
                        out.push(self.extract_field(agg, i as u32).into());
                    }
                }
                None => out.push(v),
            }
        }
        out
    }

    /// The two lanes of a `Float2`.
    fn lanes(&self, v: BasicValueEnum<'ctx>) -> (inkwell::values::FloatValue<'ctx>, inkwell::values::FloatValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let vec = v.into_vector_value();
        let a = self
            .builder
            .build_extract_element(vec, i64t.const_zero(), "lane0")
            .expect("LLVM builder error")
            .into_float_value();
        let b = self
            .builder
            .build_extract_element(vec, i64t.const_int(1, false), "lane1")
            .expect("LLVM builder error")
            .into_float_value();
        (a, b)
    }

    fn make_float2(
        &self,
        a: inkwell::values::FloatValue<'ctx>,
        b: inkwell::values::FloatValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let i64t = self.ctx.i64_type();
        let v0 = self
            .builder
            .build_insert_element(self.ctx.f64_type().vec_type(2).get_undef(), a, i64t.const_zero(), "f2")
            .expect("LLVM builder error");
        self.builder
            .build_insert_element(v0, b, i64t.const_int(1, false), "f2")
            .expect("LLVM builder error")
            .into()
    }

    /// The maths functions that are one instruction on every machine Rill
    /// runs on, as the instruction rather than the call. `sqrt` called
    /// through libm is a call the optimizer will not vectorize, because C's
    /// `sqrt` may set `errno`; the intrinsic has no such promise to keep,
    /// and ten million of them become five million `fsqrt` on pairs. The
    /// results are the same bits either way. A C signature that is not the
    /// double-precision one goes to the library as written.
    fn libm_intrinsic(&self, name: &str, sig: &ExternSig) -> Option<FunctionValue<'ctx>> {
        let (iname, arity) = match name {
            "sqrt" => ("llvm.sqrt", 1),
            "fabs" => ("llvm.fabs", 1),
            "floor" => ("llvm.floor", 1),
            "ceil" => ("llvm.ceil", 1),
            "trunc" => ("llvm.trunc", 1),
            "round" => ("llvm.round", 1),
            "rint" => ("llvm.rint", 1),
            "fma" => ("llvm.fma", 3),
            "fmin" => ("llvm.minnum", 2),
            "fmax" => ("llvm.maxnum", 2),
            "copysign" => ("llvm.copysign", 2),
            _ => return None,
        };
        if sig.params.len() != arity
            || sig.ret != CTy::F64
            || sig.params.iter().any(|p| *p != CTy::F64)
        {
            return None;
        }
        let f64t = self.ctx.f64_type();
        inkwell::intrinsics::Intrinsic::find(iname)
            .and_then(|i| i.get_declaration(&self.module, &[f64t.into()]))
    }

    /// The concrete parameter types of an instance, for `explode_args`.
    fn instance_params(&self, fn_idx: usize, subst: &[Ty]) -> Vec<Ty> {
        self.tmod.fns[fn_idx].params.iter().map(|(_, t)| subst_gen(t, subst)).collect()
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
            "rill_str_find_from",
            i64t.fn_type(&[ptrt.into(), ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_str_find_in",
            i64t.fn_type(&[ptrt.into(), ptrt.into(), i64t.into(), i64t.into()], false),
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
        self.module.add_function("rill_str_from_ptr", ptrt.fn_type(&[ptrt.into(), i64t.into()], false), None);
        self.module.add_function("rill_free_ptr", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_null_ptr", ptrt.fn_type(&[], false), None);
        self.module.add_function("rill_is_null", i8t.fn_type(&[ptrt.into()], false), None);
        let buf_new = self.module.add_function(
            "rill_buf_new",
            ptrt.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        // A fresh buffer aliases nothing, which is what lets LLVM carry the
        // length `BufNew` stores through to the checks that read it.
        buf_new.add_attribute(
            AttributeLoc::Return,
            self.ctx.create_enum_attribute(Attribute::get_named_enum_kind_id("noalias"), 0),
        );
        self.module.add_function(
            "rill_buf_release",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        // Cold: the call is what a loop makes when its range is *not* inside
        // the buffer, and reaching it ends the program.
        let check = self.module.add_function(
            "rill_buf_check",
            void.fn_type(&[ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        let cold = Attribute::get_named_enum_kind_id("cold");
        if cold != 0 {
            check.add_attribute(AttributeLoc::Function, self.ctx.create_enum_attribute(cold, 0));
        }
        self.module.add_function("rill_strbuf_new", ptrt.fn_type(&[], false), None);
        self.module.add_function(
            "rill_strbuf_add",
            void.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_strbuf_int",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_strbuf_byte",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_strbuf_reserve", void.fn_type(&[ptrt.into(), i64t.into()], false), None);
        self.module.add_function("rill_str_from_buf", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_buf_copy",
            void.fn_type(&[ptrt.into(), i64t.into(), ptrt.into(), i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_buf_cmp",
            i64t.fn_type(&[ptrt.into(), i64t.into(), ptrt.into(), i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_str_take_buf", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_rc_stale", void.fn_type(&[ptrt.into(), i64t.into()], false), None);
        self.module.add_function(
            "rill_strbuf_add_sub",
            void.fn_type(&[ptrt.into(), ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_strbuf_str", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_strbuf_clear", void.fn_type(&[ptrt.into()], false), None);
        // Cold, so that the room a builder is short of is asked for out of
        // the way: the appends stay a straight line and what is only
        // occasionally reached is laid down elsewhere.
        let grow = self.module.add_function(
            "rill_sb_grow",
            ptrt.fn_type(&[ptrt.into(), i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        let cold = Attribute::get_named_enum_kind_id("cold");
        if cold != 0 {
            grow.add_attribute(AttributeLoc::Function, self.ctx.create_enum_attribute(cold, 0));
        }
        self.module.add_function("rill_strbuf_take", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_strbuf_len", i64t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_strbuf_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_buf_oob",
            void.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_div_zero", void.fn_type(&[], false), None);
        self.module.add_function("rill_yield", void.fn_type(&[], false), None);
        self.module.add_function(
            "rill_call_blocking",
            void.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_assert_failed", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_assert_eq_failed",
            void.fn_type(&[ptrt.into(), ptrt.into(), i64t.into()], false),
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
        self.module.add_function("rill_now_nanos", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_now_millis", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_rss_bytes", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_workers", i64t.fn_type(&[], false), None);
        let uninit = self.module.add_function(
            "rill_buf_new_uninit_f64",
            ptrt.fn_type(&[i64t.into()], false),
            None,
        );
        uninit.add_attribute(
            AttributeLoc::Return,
            self.ctx.create_enum_attribute(Attribute::get_named_enum_kind_id("noalias"), 0),
        );
        self.module.add_function("rill_random_i64", i64t.fn_type(&[], false), None);
        self.module.add_function("rill_sleep_ms", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_tcp_listen", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_tcp_accept", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_tcp_connect",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_sock_read", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_sock_wait",
            self.ctx.bool_type().fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_sock_peer", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_sock_port", i64t.fn_type(&[i64t.into()], false), None);
        self.module
            .add_function("rill_sock_nodelay", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_sock_buffered", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_sock_flush", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_sock_write",
            i64t.fn_type(&[i64t.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_sock_close", void.fn_type(&[i64t.into()], false), None);

        // Files. Descriptors like the sockets above, and declared beside them
        // for the same reason: the one that waits, `rill_file_sync`, needs the
        // scheduler exactly as a socket call does.
        self.module.add_function(
            "rill_file_open",
            i64t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_file_read",
            ptrt.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_file_pread",
            ptrt.fn_type(&[i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_file_pread_into",
            i64t.fn_type(&[i64t.into(), i64t.into(), ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_file_pwrite",
            i64t.fn_type(&[i64t.into(), i64t.into(), ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_file_lock", i8t.fn_type(&[i64t.into(), i8t.into()], false), None);
        self.module.add_function("rill_file_unlock", i8t.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_file_write",
            i64t.fn_type(&[i64t.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function(
            "rill_file_seek",
            i64t.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_file_pos", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_file_size", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function(
            "rill_file_truncate",
            i8t.fn_type(&[i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_file_sync", i8t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_file_close", void.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_file_remove", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_file_rename",
            i8t.fn_type(&[ptrt.into(), ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_file_exists", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_dir_make", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_dir_list", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_dir_sync", i8t.fn_type(&[ptrt.into()], false), None);

        // Hash tables. The table is one piece of code for every key and value
        // type; what it cannot know about them it reads out of the vtable it
        // was made with.
        let words = self.ctx.struct_type(&[i64t.into(), i64t.into()], false);
        self.module.add_function("rill_map_new", ptrt.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_map_set",
            void.fn_type(&[ptrt.into(), i64t.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_map_find",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_map_get_or",
            i64t.fn_type(&[ptrt.into(), i64t.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_map_del",
            i8t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_map_len", i64t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_map_clear", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_map_next",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_map_key_at",
            i64t.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function(
            "rill_map_val_at",
            words.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_map_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_str_hash", i64t.fn_type(&[ptrt.into()], false), None);

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
        // Naked functions: the instructions are the whole of them. No
        // prologue, no epilogue, no operands — LLVM emits the body and puts
        // the name in front of it, which is what an interrupt table or a
        // linker script needs and all it needs. Sorted, so that two builds of
        // the same program lay them out the same way.
        let mut naked: Vec<(&String, &String)> = self.tmod.naked_asms.iter().collect();
        naked.sort_by_key(|(n, _)| n.as_str());
        for (name, template) in naked {
            let f = self.module.add_function(name, void.fn_type(&[], false), None);
            f.add_attribute(
                AttributeLoc::Function,
                self.ctx.create_enum_attribute(Attribute::get_named_enum_kind_id("naked"), 0),
            );
            let entry = self.ctx.append_basic_block(f, "entry");
            let b = self.ctx.create_builder();
            b.position_at_end(entry);
            let body = void.fn_type(&[], false);
            // A naked function has no operands, so `$` cannot be one and is
            // always the literal the assembler wants — an x86 immediate is
            // written `$0x1000` and reads as operand zero otherwise. Doubling
            // it is what LLVM's template syntax calls for, and doing it here
            // means the author writes the instruction the way the manual has
            // it.
            let ptr = self.ctx.create_inline_asm(
                body,
                template.replace('$', "$$"),
                String::new(),
                true,
                false,
                Some(inkwell::InlineAsmDialect::ATT),
                false,
            );
            b.build_indirect_call(body, ptr, &[], "").expect("LLVM builder error");
            // There is no return to emit: the instructions leave however they
            // leave — `iretq`, `eret`, a jump, or not at all.
            b.build_unreachable().expect("LLVM builder error");
        }

        let alloc = self.module.add_function("rill_alloc", ptrt.fn_type(&[i64t.into()], false), None);
        alloc.add_attribute(
            AttributeLoc::Return,
            self.ctx.create_enum_attribute(Attribute::get_named_enum_kind_id("noalias"), 0),
        );
        self.module.add_function(
            "rill_free_sized",
            void.fn_type(&[ptrt.into(), i64t.into()], false),
            None,
        );
        self.module.add_function("rill_retain", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_env_release", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_alloc_report", void.fn_type(&[], false), None);
        self.module.add_function("rill_flush_out", void.fn_type(&[], false), None);
        self.module.add_function(
            "rill_rt_run_direct",
            void.fn_type(&[ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_chan_new", ptrt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_chan_release", void.fn_type(&[ptrt.into()], false), None);
        // Closing, and the two halves the prelude's `recv_opt` is written
        // from. `rill_chan_taken` answers two words like `rill_chan_recv`,
        // because it hands back the same kind of thing.
        self.module.add_function("rill_chan_close", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_chan_closed", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_chan_wait", i8t.fn_type(&[ptrt.into()], false), None);
        self.module.add_function(
            "rill_chan_taken",
            words.fn_type(&[ptrt.into()], false),
            None,
        );
        self.module.add_function("rill_sock_shutdown", i64t.fn_type(&[i64t.into()], false), None);
        self.module.add_function("rill_wait_signal", i64t.fn_type(&[], false), None);
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
        self.module.add_function("rill_rt_host", void.fn_type(&[ptrt.into()], false), None);
        self.module.add_function("rill_host_enter", void.fn_type(&[], false), None);
        self.module.add_function("rill_host_leave", void.fn_type(&[], false), None);
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
        if let Some(fields) = self.flat_fields(ty) {
            // A flat value becomes a word by being boxed, in the layout a
            // counted constructor has, so the table's thunks can hold and
            // let go of it like any other.
            let (adt, targs) = match ty {
                Ty::Adt(a, t) => (*a, t.clone()),
                _ => unreachable!("flat types are ADTs"),
            };
            let st = self.ctor_struct(adt, 0, &targs);
            let p = self.alloc_object(st, "flat.box");
            let tag = self.tmod.adts[adt].ctors[0].tag;
            let tag_ptr = self.builder.build_struct_gep(st, p, 1, "tag").expect("LLVM builder error");
            self.builder.build_store(tag_ptr, i64t.const_int(tag, false)).expect("LLVM builder error");
            for fi in 0..fields.len() {
                let fv = self.extract_field(v, fi as u32);
                let fp = self
                    .builder
                    .build_struct_gep(st, p, (fi + 2) as u32, "field")
                    .expect("LLVM builder error");
                self.builder.build_store(fp, fv).expect("LLVM builder error");
            }
            let w = self.builder.build_ptr_to_int(p, i64t, "w").expect("LLVM builder error");
            return (w, zero);
        }
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
            // Two lanes are two words.
            Ty::Float2 => {
                let bits = b
                    .build_bit_cast(v.into_vector_value(), i64t.vec_type(2), "w2")
                    .unwrap()
                    .into_vector_value();
                let w0 = b
                    .build_extract_element(bits, i64t.const_zero(), "w0")
                    .unwrap()
                    .into_int_value();
                let w1 = b
                    .build_extract_element(bits, i64t.const_int(1, false), "w1")
                    .unwrap()
                    .into_int_value();
                (w0, w1)
            }
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Ptr | Ty::Buf(_)
            | Ty::Map(..) => (
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
        if let Some(fields) = self.flat_fields(ty) {
            // Read back out of the box — only read: whoever handed the word
            // over says whether the box is theirs or ours.
            let (adt, targs) = match ty {
                Ty::Adt(x, t) => (*x, t.clone()),
                _ => unreachable!("flat types are ADTs"),
            };
            let st = self.ctor_struct(adt, 0, &targs);
            let ptrt = self.ctx.ptr_type(Default::default());
            let p = self.builder.build_int_to_ptr(a, ptrt, "fbox").expect("LLVM builder error");
            let mut agg = self.flat_struct(ty).get_undef();
            for (fi, cf) in fields.iter().enumerate() {
                let fp = self
                    .builder
                    .build_struct_gep(st, p, (fi + 2) as u32, "field")
                    .expect("LLVM builder error");
                let fv = self
                    .builder
                    .build_load(self.llvm_type(cf).expect("a scalar"), fp, "fv")
                    .expect("LLVM builder error");
                agg = self
                    .builder
                    .build_insert_value(agg, fv, fi as u32, "f")
                    .expect("LLVM builder error")
                    .into_struct_value();
            }
            return agg.into();
        }
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
            Ty::Float2 => {
                let i64t = self.ctx.i64_type();
                let v0 = b
                    .build_insert_element(i64t.vec_type(2).get_undef(), a, i64t.const_zero(), "l0")
                    .unwrap();
                let v1 = b
                    .build_insert_element(v0, bw, i64t.const_int(1, false), "l1")
                    .unwrap();
                b.build_bit_cast(v1, self.ctx.f64_type().vec_type(2), "f2").unwrap()
            }
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Ptr | Ty::Buf(_)
            | Ty::Map(..) => {
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

    fn is_heap(&self, ty: &Ty) -> bool {
        if self.is_flat(ty) {
            return false;
        }
        matches!(
            ty,
            Ty::Adt(..)
                | Ty::Fn(..)
                | Ty::Chan(..)
                | Ty::Str
                | Ty::StrBuf
                | Ty::Buf(_)
                | Ty::Map(..)
        )
    }

    /// A value kept in registers rather than in a box: a type with one
    /// constructor, at least one field, and nothing but scalars in it — a
    /// pair of numbers, a point, an n-body's body. Its LLVM shape is the
    /// struct of its fields, passed and returned by value; it has no count,
    /// no tag and no allocation, and `p.x` is an `extractvalue`. A function
    /// returning `(q, r)` used to allocate a box the caller took apart and
    /// freed a moment later — 100 million times in one benchmark.
    ///
    /// The one place such a value has to become a word is a table or a
    /// channel, which store words: `pack_words` boxes it there in the layout
    /// a counted constructor has, and `unpack_words` reads it back out.
    fn flat_fields(&self, ty: &Ty) -> Option<Vec<Ty>> {
        let Ty::Adt(adt, targs) = ty else { return None };
        let ctors = &self.tmod.adts[*adt].ctors;
        if ctors.len() != 1 || ctors[0].fields.is_empty() {
            return None;
        }
        let fields: Vec<Ty> = ctors[0].fields.iter().map(|f| subst_gen(f, targs)).collect();
        if fields.iter().all(|f| matches!(f, Ty::Int | Ty::Float | Ty::Float2 | Ty::Bool)) {
            Some(fields)
        } else {
            None
        }
    }

    fn is_flat(&self, ty: &Ty) -> bool {
        self.flat_fields(ty).is_some()
    }

    fn flat_struct(&self, ty: &Ty) -> StructType<'ctx> {
        let fields = self.flat_fields(ty).expect("a flat type");
        let members: Vec<BasicTypeEnum> =
            fields.iter().map(|f| self.llvm_type(f).expect("a scalar")).collect();
        self.ctx.struct_type(&members, false)
    }

    fn extract_field(&self, agg: BasicValueEnum<'ctx>, i: u32) -> BasicValueEnum<'ctx> {
        self.builder
            .build_extract_value(agg.into_struct_value(), i, "f")
            .expect("LLVM builder error")
    }

    /// Lets go of the box a flat value travelled in through a table or a
    /// channel: one fewer holder, and freed when that was the last.
    fn release_flat_box(&mut self, word: IntValue<'ctx>, ty: &Ty) {
        let func = self.builder.get_insert_block().and_then(|b| b.get_parent()).expect("in a function");
        let ptrt = self.ctx.ptr_type(Default::default());
        let i64t = self.ctx.i64_type();
        let p = self.builder.build_int_to_ptr(word, ptrt, "fbox").expect("LLVM builder error");
        let rc_ptr = self
            .builder
            .build_struct_gep(self.header_struct(), p, 0, "rc.ptr")
            .expect("LLVM builder error");
        let dec = self.emit_rc_dec(rc_ptr, func);
        let dead = self
            .builder
            .build_int_compare(IntPredicate::EQ, dec, i64t.const_zero(), "dead")
            .expect("LLVM builder error");
        let free_bb = self.ctx.append_basic_block(func, "fbox.free");
        let done_bb = self.ctx.append_basic_block(func, "fbox.done");
        self.builder.build_conditional_branch(dead, free_bb, done_bb).expect("LLVM builder error");
        self.builder.position_at_end(free_bb);
        let (adt, targs) = match ty {
            Ty::Adt(a, t) => (*a, t.clone()),
            _ => unreachable!("flat types are ADTs"),
        };
        let size = self.ctor_struct(adt, 0, &targs).size_of().expect("sized");
        self.emit_free(p, size);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
        self.builder.position_at_end(done_bb);
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
    /// How wide a pointer is on the target. Four on wasm32, eight elsewhere —
    /// which is why a `poke_ptr` index means different bytes in a wasm build
    /// than in a native one, and why a layout written in pointer slots has to
    /// pick a target.
    fn ptr_bytes(&self) -> u64 {
        if self.wasm {
            4
        } else {
            8
        }
    }

    /// Marks a float instruction as fusable. See the note at its use.
    fn contract(v: inkwell::values::FloatValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        if let Some(i) = v.as_instruction() {
            let _ = i.set_fast_math_flags(inkwell::values::FastMathFlags::AllowContract);
        }
        v
    }

    /// Lets go of one reference to a box that was found shared a moment
    /// ago: `rc` is its count as loaded then, above one. On a single
    /// worker that load is still the truth, and the subtract is stored
    /// back. On a parallel build another worker may have retained or
    /// released in between — a retain stored over would be lost, and the
    /// box freed under its holder — so the count is taken down the way
    /// `emit_rc_dec` does, and a zero coming back frees after all.
    fn emit_shared_dec(&mut self, p: PointerValue<'ctx>, rc: IntValue<'ctx>, ty: &Ty) {
        let i64t = self.ctx.i64_type();
        if !self.parallel {
            let less = self.builder.build_int_sub(rc, i64t.const_int(1, false), "dec").expect("builder");
            self.store_rc(p, rc, less);
            return;
        }
        let func = self
            .builder
            .get_insert_block()
            .and_then(|b| b.get_parent())
            .expect("emitting outside a function");
        let new = self.emit_rc_dec(p, func);
        let zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, new, i64t.const_zero(), "last")
            .expect("LLVM builder error");
        let late_bb = self.ctx.append_basic_block(func, "shared.late");
        let done_bb = self.ctx.append_basic_block(func, "shared.done");
        self.builder.build_conditional_branch(zero, late_bb, done_bb).expect("LLVM builder error");
        self.builder.position_at_end(late_bb);
        // Ours alone now: the count is put back to one and the usual
        // last-reference release takes it down and frees.
        self.builder.build_store(p, i64t.const_int(1, false)).expect("LLVM builder error");
        let drop_fn = self.adt_drop(ty);
        self.builder.build_call(drop_fn, &[p.into()], "").expect("LLVM builder error");
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
        self.builder.position_at_end(done_bb);
    }

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

    /// `RILL_RC_CHECK`: every count update first asks the runtime whether
    /// the block is out at all — one that was freed is reported before the
    /// update lands, in the toucher's own frames.
    fn emit_stale_check(&mut self, p: PointerValue<'ctx>, kind: u64) {
        if !self.rc_check {
            return;
        }
        let k = self.ctx.i64_type().const_int(kind, false);
        self.call_rt("rill_rc_stale", &[p.into(), k.into()]);
    }

    fn emit_rc_inc(&mut self, ptr: BasicValueEnum<'ctx>) {
        self.emit_stale_check(ptr.into_pointer_value(), 0);
        if self.parallel {
            // As `emit_rc_dec`: the flag read in place, an atomic add when
            // workers share, the plain path otherwise. This was a runtime
            // call, which on a library run on one worker cost a tenth of
            // every lookup for a branch it would not take.
            let func = self
                .builder
                .get_insert_block()
                .and_then(|b| b.get_parent())
                .expect("emitting outside a function");
            let i64t = self.ctx.i64_type();
            let p = ptr.into_pointer_value();
            let mt = self.mt_flag();
            let flag = self
                .builder
                .build_load(self.ctx.i8_type(), mt, "mt")
                .expect("LLVM builder error")
                .into_int_value();
            let shared = self
                .builder
                .build_int_compare(IntPredicate::NE, flag, self.ctx.i8_type().const_zero(), "shared")
                .expect("LLVM builder error");
            let atomic_bb = self.ctx.append_basic_block(func, "inc.atomic");
            let plain_bb = self.ctx.append_basic_block(func, "inc.plain");
            let cont_bb = self.ctx.append_basic_block(func, "inc.cont");
            self.builder.build_conditional_branch(shared, atomic_bb, plain_bb).expect("LLVM builder error");
            self.builder.position_at_end(atomic_bb);
            let seen = self.builder.build_load(i64t, p, "rc.seen").expect("LLVM builder error").into_int_value();
            let live = self
                .builder
                .build_int_compare(IntPredicate::ULT, seen, i64t.const_int(Self::IMMORTAL_MIN, false), "live")
                .expect("LLVM builder error");
            let do_bb = self.ctx.append_basic_block(func, "inc.atomic.do");
            self.builder.build_conditional_branch(live, do_bb, cont_bb).expect("LLVM builder error");
            self.builder.position_at_end(do_bb);
            self.builder
                .build_atomicrmw(inkwell::AtomicRMWBinOp::Add, p, i64t.const_int(1, false), inkwell::AtomicOrdering::Monotonic)
                .expect("LLVM builder error");
            self.builder.build_unconditional_branch(cont_bb).expect("LLVM builder error");
            self.builder.position_at_end(plain_bb);
            let rc = self.builder.build_load(i64t, p, "rc").expect("LLVM builder error").into_int_value();
            let inc = self.builder.build_int_add(rc, i64t.const_int(1, false), "inc").expect("LLVM builder error");
            self.store_rc(p, rc, inc);
            self.builder.build_unconditional_branch(cont_bb).expect("LLVM builder error");
            self.builder.position_at_end(cont_bb);
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
        if self.is_flat(ty) {
            return;
        }
        match ty {
            // Boxes, channels and strings are never null; closure
            // environments can be, so those keep the null-safe runtime call.
            Ty::Buf(_) => {
                let rc = self.buf_rc_ptr(v.into_pointer_value());
                self.emit_rc_inc(rc.into())
            }
            Ty::Adt(..) | Ty::Chan(..) | Ty::Str | Ty::StrBuf | Ty::Map(..) => self.emit_rc_inc(v),
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
        // A field of a box this frame holds is held with it: `g.data[k]`
        // reads the buffer straight out of the record, with no count taken
        // for the look. Taking one was a retain and a release per element in
        // every loop over a record's buffer, and the release's branch kept
        // the loop from vectorizing.
        if let TK::FieldGet { base, adt, ctor, index } = &e.kind {
            let bty = self.concrete(&base.ty, subst);
            if !self.is_flat(&bty) {
                let (b, held) = self.gen_borrowed(base, scope, subst, func)?;
                if !held {
                    let targs = match &bty {
                        Ty::Adt(_, targs) => targs.clone(),
                        other => unreachable!("field of {other:?}"),
                    };
                    let struct_ty = self.ctor_struct(*adt, *ctor, &targs);
                    let fty = subst_gen(&self.tmod.adts[*adt].ctors[*ctor].fields[*index], &targs);
                    let fp = self
                        .builder
                        .build_struct_gep(struct_ty, b.into_pointer_value(), (*index + 2) as u32, "f")
                        .expect("LLVM builder error");
                    let v = self
                        .builder
                        .build_load(self.llvm_type(&fty).expect("a field is never Unit"), fp, "fv")
                        .expect("LLVM builder error");
                    return Ok((v, false));
                }
                // The base was made for this read and is owned here: finish
                // it the ordinary way, which retains the field and lets the
                // base go.
                let targs = match &bty {
                    Ty::Adt(_, targs) => targs.clone(),
                    other => unreachable!("field of {other:?}"),
                };
                let struct_ty = self.ctor_struct(*adt, *ctor, &targs);
                let fty = subst_gen(&self.tmod.adts[*adt].ctors[*ctor].fields[*index], &targs);
                let fp = self
                    .builder
                    .build_struct_gep(struct_ty, b.into_pointer_value(), (*index + 2) as u32, "f")
                    .expect("LLVM builder error");
                let v = self
                    .builder
                    .build_load(self.llvm_type(&fty).expect("a field is never Unit"), fp, "fv")
                    .expect("LLVM builder error");
                self.emit_retain(v, &fty);
                self.emit_release(b, &bty);
                return Ok((v, true));
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
        if self.is_flat(ty) {
            return;
        }
        // Counted values whose release is a runtime call get the last step of
        // that call laid down here instead: the count is nearly always about
        // to go from something to something, not to zero, and a load, a
        // compare and a store say so without leaving the function. What it
        // buys is not the call — it is that a `+1` here and a `-1` there,
        // once inlining has brought them together, now cancel.
        // A boxed ADT the same way: its drop function is generated code
        // rather than the runtime, but it was a call for every release all
        // the same, and a tree rewriter releases several times per node —
        // `deriv` spent half its time in `drop$Expr` doing decrements.
        if matches!(ty, Ty::Buf(_) | Ty::Str | Ty::StrBuf | Ty::Adt(..)) {
            if let Some(func) = self.builder.get_insert_block().and_then(|b| b.get_parent()) {
                let i64t = self.ctx.i64_type();
                // Where the count lives: at the object for a string or a
                // builder, sixteen bytes before the elements for a buffer.
                let p = match ty {
                    Ty::Buf(_) => self.buf_rc_ptr(v.into_pointer_value()),
                    _ => v.into_pointer_value(),
                };
                self.emit_stale_check(p, 2);
                let rc = self
                    .builder
                    .build_load(i64t, p, "rc")
                    .expect("LLVM builder error")
                    .into_int_value();
                let dead = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, rc, i64t.const_int(1, false), "dead")
                    .expect("LLVM builder error");
                let free_bb = self.ctx.append_basic_block(func, "rel.free");
                let dec_bb = self.ctx.append_basic_block(func, "rel.dec");
                let done_bb = self.ctx.append_basic_block(func, "rel.done");
                self.builder
                    .build_conditional_branch(dead, free_bb, dec_bb)
                    .expect("LLVM builder error");
                // The last reference: the runtime does the freeing, count and
                // all, exactly as it did before.
                self.builder.position_at_end(free_bb);
                let call = match ty {
                    Ty::Buf(w) => {
                        let size = self.ctx.i64_type().const_int(w.size_bytes(), false);
                        self.call_rt("rill_buf_release", &[v.into(), size.into()])
                    }
                    Ty::Str => self.call_rt("rill_str_release", &[v.into()]),
                    Ty::Adt(..) => {
                        let drop_fn = self.adt_drop(ty);
                        self.builder.build_call(drop_fn, &[v.into()], "").expect("LLVM builder error")
                    }
                    _ => self.call_rt("rill_strbuf_release", &[v.into()]),
                };
                if tail {
                    call.set_tail_call(true);
                }
                self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
                // Not the last, or not counted at all: `store_rc` leaves a
                // compile-time constant's count alone. On a parallel build
                // the count is taken down the way `emit_rc_dec` does, and a
                // zero coming back — another worker let go between the
                // look above and the subtract — frees after all.
                self.builder.position_at_end(dec_bb);
                if self.parallel {
                    let new = self.emit_rc_dec(p, func);
                    let zero = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, new, i64t.const_zero(), "last")
                        .expect("LLVM builder error");
                    let late_bb = self.ctx.append_basic_block(func, "rel.late");
                    self.builder.build_conditional_branch(zero, late_bb, done_bb).expect("LLVM builder error");
                    self.builder.position_at_end(late_bb);
                    // Ours alone now: the count is put back to one and the
                    // usual last-reference release takes it down and frees.
                    self.builder.build_store(p, i64t.const_int(1, false)).expect("LLVM builder error");
                    match ty {
                        Ty::Buf(w) => {
                            let size = self.ctx.i64_type().const_int(w.size_bytes(), false);
                            self.call_rt("rill_buf_release", &[v.into(), size.into()]);
                        }
                        Ty::Str => {
                            self.call_rt("rill_str_release", &[v.into()]);
                        }
                        Ty::Adt(..) => {
                            let drop_fn = self.adt_drop(ty);
                            self.builder.build_call(drop_fn, &[v.into()], "").expect("LLVM builder error");
                        }
                        _ => {
                            self.call_rt("rill_strbuf_release", &[v.into()]);
                        }
                    }
                    self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
                } else {
                    let less =
                        self.builder.build_int_sub(rc, i64t.const_int(1, false), "dec").expect("builder");
                    self.store_rc(p, rc, less);
                    self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
                }
                self.builder.position_at_end(done_bb);
                return;
            }
        }
        let call = match ty {
            Ty::Adt(..) => {
                let drop_fn = self.adt_drop(ty);
                Some(self.builder.build_call(drop_fn, &[v.into()], "").expect("LLVM builder error"))
            }
            Ty::Chan(..) => Some(self.call_rt("rill_chan_release", &[v.into()])),
            Ty::Map(..) => Some(self.call_rt("rill_map_release", &[v.into()])),
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

    /// The runtime's `rill_mt`: whether workers share, read where a count
    /// is touched.
    fn mt_flag(&mut self) -> PointerValue<'ctx> {
        self.module
            .get_global("rill_mt")
            .unwrap_or_else(|| {
                let g = self.module.add_global(self.ctx.i8_type(), None, "rill_mt");
                g.set_linkage(inkwell::module::Linkage::External);
                g
            })
            .as_pointer_value()
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
        self.emit_stale_check(rc_ptr, 1);
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
        let mt = self.mt_flag();
        let flag = self
            .builder
            .build_load(self.ctx.i8_type(), mt, "mt")
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
        f.set_linkage(inkwell::module::Linkage::Internal);
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
                if self.is_heap(&cf) {
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
            self.emit_free(ptr, size);
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
            // `(a, b)`, the way a pair shows.
            Ty::Float2 => {
                let (x, y) = self.lanes(v);
                let sx = self.gen_show_of(x.into(), &Ty::Float, line)?;
                let sy = self.gen_show_of(y.into(), &Ty::Float, line)?;
                let open: BasicValueEnum<'ctx> = self.const_str("(").into();
                let sep: BasicValueEnum<'ctx> = self.const_str(", ").into();
                let close: BasicValueEnum<'ctx> = self.const_str(")").into();
                let a1 = self.call_concat(open, sx);
                self.emit_release(sx, &Ty::Str);
                let a2 = self.call_concat(a1, sep);
                self.emit_release(a1, &Ty::Str);
                let a3 = self.call_concat(a2, sy);
                self.emit_release(a2, &Ty::Str);
                self.emit_release(sy, &Ty::Str);
                let a4 = self.call_concat(a3, close);
                self.emit_release(a3, &Ty::Str);
                Ok(a4)
            }
            Ty::Adt(adt, _) if self.is_flat(ty) => {
                let fields = self.flat_fields(ty).expect("flat");
                let c = &self.tmod.adts[*adt].ctors[0];
                let head = if is_tuple_ctor(&c.name, c.fields.len()) {
                    "(".to_string()
                } else {
                    format!("{}(", c.name)
                };
                let mut acc: BasicValueEnum<'ctx> = self.const_str(&head).into();
                for (fi, cf) in fields.iter().enumerate() {
                    if fi > 0 {
                        let sep: BasicValueEnum<'ctx> = self.const_str(", ").into();
                        let next = self.call_concat(acc, sep);
                        self.emit_release(acc, &Ty::Str);
                        acc = next;
                    }
                    let fv = self.extract_field(v, fi as u32);
                    let fs = self.gen_show_of(fv, cf, line)?;
                    let next = self.call_concat(acc, fs);
                    self.emit_release(acc, &Ty::Str);
                    self.emit_release(fs, &Ty::Str);
                    acc = next;
                }
                let close: BasicValueEnum<'ctx> = self.const_str(")").into();
                let out = self.call_concat(acc, close);
                self.emit_release(acc, &Ty::Str);
                Ok(out)
            }
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
        f.set_linkage(inkwell::module::Linkage::Internal);
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
                if self.is_heap(&cf) {
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
            Ty::Float2 => {
                let (a0, a1) = self.lanes(a);
                let (b0, b1) = self.lanes(b);
                let e0 = self.gen_eq_of(a0.into(), b0.into(), &Ty::Float, line)?;
                let e1 = self.gen_eq_of(a1.into(), b1.into(), &Ty::Float, line)?;
                Ok(self.builder.build_and(e0, e1, "eq2").expect("LLVM builder error"))
            }
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
            Ty::Adt(..) if self.is_flat(ty) => {
                let fields = self.flat_fields(ty).expect("flat");
                let mut acc = self.ctx.bool_type().const_int(1, false);
                for (fi, cf) in fields.iter().enumerate() {
                    let fa = self.extract_field(a, fi as u32);
                    let fb = self.extract_field(b, fi as u32);
                    let e = self.gen_eq_of(fa, fb, cf, line)?;
                    acc = self.builder.build_and(acc, e, "eq").expect("LLVM builder error");
                }
                Ok(acc)
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
        f.set_linkage(inkwell::module::Linkage::Internal);
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

    // ---- hash tables ------------------------------------------------------

    /// The key and value types of a map, refusing a `Unit` value: a table
    /// holds what it is given, and `Unit` is the type with nothing to hold.
    fn map_kv(&self, ty: &Ty, line: usize) -> Result<(Ty, Ty), CodegenError> {
        match ty {
            Ty::Map(k, v) => {
                if **v == Ty::Unit {
                    return Err(CodegenError::new(
                        line,
                        "a map cannot hold `Unit` — there would be nothing to look up"
                            .to_string(),
                    ));
                }
                Ok(((**k).clone(), (**v).clone()))
            }
            other => unreachable!("map operation on {other:?}"),
        }
    }

    /// splitmix64's finaliser. A key's own bits are rarely spread evenly —
    /// small integers and pointers least of all — and the table takes the low
    /// ones to pick a bucket with, so every bit is made to depend on the rest.
    /// An `Int` key's hash: one multiply by the golden ratio and a fold of
    /// the high half into the low, which is what a hand-written C table does
    /// and a third of what `mix64` costs. The table takes its slot from the
    /// low bits and its control byte from the top seven, and both are well
    /// stirred by this for the keys tables see. Must agree with the
    /// runtime's `hash_int`, which probes `Int`-keyed tables itself.
    fn hash_int(&self, x: IntValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let b = &self.builder;
        let m = b.build_int_mul(x, i64t.const_int(0x9e37_79b9_7f4a_7c15, false), "hm").expect("builder");
        let s = b.build_right_shift(m, i64t.const_int(29, false), false, "hs").expect("builder");
        b.build_xor(m, s, "h").expect("builder")
    }

    fn mix64(&self, x: IntValue<'ctx>) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let b = &self.builder;
        let s1 = b.build_right_shift(x, i64t.const_int(30, false), false, "s1").expect("builder");
        let x1 = b.build_xor(x, s1, "x1").expect("builder");
        let m1 =
            b.build_int_mul(x1, i64t.const_int(0xbf58_476d_1ce4_e5b9, false), "m1").expect("builder");
        let s2 = b.build_right_shift(m1, i64t.const_int(27, false), false, "s2").expect("builder");
        let x2 = b.build_xor(m1, s2, "x2").expect("builder");
        let m2 =
            b.build_int_mul(x2, i64t.const_int(0x94d0_49bb_1331_11eb, false), "m2").expect("builder");
        let s3 = b.build_right_shift(m2, i64t.const_int(31, false), false, "s3").expect("builder");
        b.build_xor(m2, s3, "h").expect("builder")
    }

    /// The hash of a borrowed value. Nothing here moves a reference count:
    /// hashing only reads. Two values `==` calls equal must hash alike, which
    /// is why this walks exactly what `gen_eq_of` compares.
    fn gen_hash_of(
        &mut self,
        v: BasicValueEnum<'ctx>,
        ty: &Ty,
        line: usize,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let i64t = self.ctx.i64_type();
        match ty {
            Ty::Int => Ok(self.hash_int(v.into_int_value())),
            Ty::Bool => {
                let w = self
                    .builder
                    .build_int_z_extend(v.into_int_value(), i64t, "bw")
                    .expect("LLVM builder error");
                Ok(self.mix64(w))
            }
            // `0.0 == -0.0` is true and their bits differ, so the two are
            // brought together before hashing. A NaN is equal to nothing,
            // itself included, so a key that is one can never be looked up
            // again whatever this returns.
            Ty::Float => {
                let f = v.into_float_value();
                let zero = self.ctx.f64_type().const_zero();
                let is_zero = self
                    .builder
                    .build_float_compare(FloatPredicate::OEQ, f, zero, "fz")
                    .expect("LLVM builder error");
                let bits = self
                    .builder
                    .build_bit_cast(f, i64t, "fb")
                    .expect("LLVM builder error")
                    .into_int_value();
                let sel = self
                    .builder
                    .build_select(is_zero, i64t.const_zero(), bits, "fh")
                    .expect("LLVM builder error")
                    .into_int_value();
                Ok(self.mix64(sel))
            }
            Ty::Float2 => {
                let (x, y) = self.lanes(v);
                let h0 = self.gen_hash_of(x.into(), &Ty::Float, line)?;
                let h1 = self.gen_hash_of(y.into(), &Ty::Float, line)?;
                let both = self.builder.build_xor(h0, h1, "h2").expect("LLVM builder error");
                let mixed = self.mix64(both);
                Ok(self.builder.build_xor(mixed, h1, "h2m").expect("LLVM builder error"))
            }
            Ty::Str => {
                let f = self.module.get_function("rill_str_hash").unwrap();
                let call =
                    self.builder.build_call(f, &[v.into()], "strh").expect("LLVM builder error");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(x) => x.into_int_value(),
                    _ => unreachable!("rill_str_hash returns a number"),
                })
            }
            Ty::Adt(..) if self.is_flat(ty) => {
                let fields = self.flat_fields(ty).expect("flat");
                let mut acc = self.mix64(i64t.const_zero());
                for (fi, cf) in fields.iter().enumerate() {
                    let fv = self.extract_field(v, fi as u32);
                    let fh = self.gen_hash_of(fv, cf, line)?;
                    let x = self.builder.build_xor(acc, fh, "mixin").expect("LLVM builder error");
                    acc = self.mix64(x);
                }
                Ok(acc)
            }
            Ty::Adt(..) => {
                let f = self.adt_hash(ty, line)?;
                let call = self.builder.build_call(f, &[v.into()], "h").expect("builder");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(x) => x.into_int_value(),
                    _ => unreachable!("a hash is a number"),
                })
            }
            other => Err(CodegenError::new(
                line,
                format!(
                    "cannot hash a value of type {}, so it cannot be a map key",
                    mangle_friendly(other, &self.tmod.adts)
                ),
            )),
        }
    }

    /// The generated structural hash for one concrete ADT type: borrows the
    /// box and mixes its tag with each field's hash, in the order `eq$T`
    /// compares them.
    fn adt_hash(&mut self, ty: &Ty, line: usize) -> Result<FunctionValue<'ctx>, CodegenError> {
        let name = format!("hash${}", mangle(ty, &self.tmod.adts));
        if let Some(f) = self.module.get_function(&name) {
            return Ok(f);
        }
        let (adt, targs) = match ty {
            Ty::Adt(i, targs) => (*i, targs.clone()),
            _ => unreachable!("adt_hash on non-ADT"),
        };
        let ptrt = self.ctx.ptr_type(Default::default());
        let i64t = self.ctx.i64_type();
        // In the module before the body is generated, so a recursive type
        // reaches this same function instead of generating for ever.
        let f = self.module.add_function(&name, i64t.fn_type(&[ptrt.into()], false), None);
        f.set_linkage(inkwell::module::Linkage::Private);

        let saved = self.builder.get_insert_block();
        let ptr = f.get_nth_param(0).unwrap().into_pointer_value();
        let entry = self.ctx.append_basic_block(f, "entry");
        let plain_bb = self.ctx.append_basic_block(f, "hash.tag");
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
        let ctors = self.tmod.adts[adt].ctors.clone();
        let mut cases = Vec::new();
        for (ci, c) in ctors.iter().enumerate() {
            if c.fields.is_empty() {
                continue;
            }
            let bb = self.ctx.append_basic_block(f, &format!("hash.{}", c.name));
            cases.push((i64t.const_int(c.tag, false), bb, ci));
        }
        self.builder
            .build_switch(tag, plain_bb, &cases.iter().map(|(v, b, _)| (*v, *b)).collect::<Vec<_>>())
            .expect("LLVM builder error");

        // A constructor with no fields is its tag and nothing else.
        self.builder.position_at_end(plain_bb);
        let h = self.mix64(tag);
        self.builder.build_return(Some(&h)).expect("LLVM builder error");

        for (_, bb, ci) in cases {
            self.builder.position_at_end(bb);
            let struct_ty = self.ctor_struct(adt, ci, &targs);
            let mut acc = self.mix64(tag);
            for (fi, fty) in ctors[ci].fields.clone().iter().enumerate() {
                let cf = subst_gen(fty, &targs);
                let fp = self
                    .builder
                    .build_struct_gep(struct_ty, ptr, (fi + 2) as u32, "f")
                    .expect("LLVM builder error");
                let fv = self
                    .builder
                    .build_load(self.llvm_type(&cf).unwrap(), fp, "fv")
                    .expect("LLVM builder error");
                let fh = self.gen_hash_of(fv, &cf, line)?;
                let x = self.builder.build_xor(acc, fh, "mixin").expect("LLVM builder error");
                acc = self.mix64(x);
            }
            self.builder.build_return(Some(&acc)).expect("LLVM builder error");
        }

        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }

    /// `i64 (i64, i64)` — hashes a key the table handed back in raw words.
    fn map_hash_thunk(
        &mut self,
        ty: &Ty,
        line: usize,
    ) -> Result<FunctionValue<'ctx>, CodegenError> {
        let name = format!("maphash${}", mangle(ty, &self.tmod.adts));
        if let Some(f) = self.module.get_function(&name) {
            return Ok(f);
        }
        let i64t = self.ctx.i64_type();
        let f = self.module.add_function(&name, i64t.fn_type(&[i64t.into()], false), None);
        f.set_linkage(inkwell::module::Linkage::Private);
        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_int_value();
        let v = self.unpack_words(a, i64t.const_zero(), ty);
        let h = self.gen_hash_of(v, ty, line)?;
        self.builder.build_return(Some(&h)).expect("LLVM builder error");
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }

    /// `i8 (i64, i64, i64, i64)` — the stored key against the wanted one.
    fn map_eq_thunk(&mut self, ty: &Ty, line: usize) -> Result<FunctionValue<'ctx>, CodegenError> {
        let name = format!("mapeq${}", mangle(ty, &self.tmod.adts));
        if let Some(f) = self.module.get_function(&name) {
            return Ok(f);
        }
        let (i64t, i8t) = (self.ctx.i64_type(), self.ctx.i8_type());
        let f =
            self.module.add_function(&name, i8t.fn_type(&[i64t.into(), i64t.into()], false), None);
        f.set_linkage(inkwell::module::Linkage::Private);
        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let zero = i64t.const_zero();
        let p: Vec<IntValue> =
            (0..2).map(|i| f.get_nth_param(i).unwrap().into_int_value()).collect();
        let a = self.unpack_words(p[0], zero, ty);
        let b = self.unpack_words(p[1], zero, ty);
        let eq = self.gen_eq_of(a, b, ty, line)?;
        let out = self.builder.build_int_z_extend(eq, i8t, "eqb").expect("LLVM builder error");
        self.builder.build_return(Some(&out)).expect("LLVM builder error");
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(f)
    }

    /// One step of the reference count on a stored key or value: `void (i64)`
    /// for a key, `void (i64, i64)` for a value, since a value may be a
    /// closure and a key may not. For a type that is not counted at all this
    /// is an empty function, which is what lets the table call it without
    /// asking first.
    fn map_rc_thunk(&mut self, ty: &Ty, retain: bool, words: u32) -> FunctionValue<'ctx> {
        let what = if retain { "maprt" } else { "maprl" };
        let name = format!("{what}${words}${}", mangle(ty, &self.tmod.adts));
        if let Some(f) = self.module.get_function(&name) {
            return f;
        }
        let i64t = self.ctx.i64_type();
        let params = vec![i64t.into(); words as usize];
        let f =
            self.module.add_function(&name, self.ctx.void_type().fn_type(&params, false), None);
        f.set_linkage(inkwell::module::Linkage::Private);
        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        if self.is_flat(ty) {
            let a = f.get_nth_param(0).unwrap().into_int_value();
            if retain {
                let ptrt = self.ctx.ptr_type(Default::default());
                let p = self.builder.build_int_to_ptr(a, ptrt, "fbox").expect("LLVM builder error");
                self.emit_rc_inc(p.into());
            } else {
                self.release_flat_box(a, ty);
            }
        } else if self.is_heap(ty) {
            let a = f.get_nth_param(0).unwrap().into_int_value();
            let b = if words > 1 {
                f.get_nth_param(1).unwrap().into_int_value()
            } else {
                i64t.const_zero()
            };
            let v = self.unpack_words(a, b, ty);
            if retain {
                self.emit_retain(v, ty);
            } else {
                self.emit_release(v, ty);
            }
        }
        self.builder.build_return(None).expect("LLVM builder error");
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        f
    }

    /// The constant a table is made with: hashing and comparing a key, and a
    /// retain and a release apiece for the key and the value. The table is one
    /// piece of code in the runtime, and this is everything about the two
    /// types it holds that had to be compiled.
    fn map_vtable(
        &mut self,
        kty: &Ty,
        vty: &Ty,
        line: usize,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let name =
            format!("mapvt${}${}", mangle(kty, &self.tmod.adts), mangle(vty, &self.tmod.adts));
        if let Some(g) = self.module.get_global(&name) {
            return Ok(g.as_pointer_value());
        }
        // A value takes one word of a slot unless it is a closure, which is
        // the only Rill value needing its code and its environment. Saying
        // which here is what keeps an `Int`-to-`Int` entry down to a key and
        // a value.
        let vwords = if matches!(vty, Ty::Fn(..)) { 2 } else { 1 };
        let entries = [
            self.map_hash_thunk(kty, line)?,
            self.map_eq_thunk(kty, line)?,
            self.map_rc_thunk(kty, true, 1),
            self.map_rc_thunk(kty, false, 1),
            // Always two words, whatever `vwords` says about storage: the
            // runtime calls these through a pointer, and wasm checks that an
            // indirect call's type is the one it was declared with.
            self.map_rc_thunk(vty, true, 2),
            self.map_rc_thunk(vty, false, 2),
        ];
        let (ptrt, i64t) = (self.ctx.ptr_type(Default::default()), self.ctx.i64_type());
        let mut fields: Vec<BasicTypeEnum> = vec![ptrt.into(); 6];
        fields.push(i64t.into());
        fields.push(i64t.into());
        let vt_ty = self.ctx.struct_type(&fields, false);
        let mut values: Vec<BasicValueEnum> =
            entries.iter().map(|f| f.as_global_value().as_pointer_value().into()).collect();
        values.push(i64t.const_int(vwords as u64, false).into());
        // What the runtime may do itself rather than call back for: an `Int`
        // key it hashes and compares in place, a scalar value it never
        // retains or releases. The flags match the runtime's `KIND_*`.
        let kind = (if *kty == Ty::Int { 1 } else { 0 })
            | (if matches!(vty, Ty::Int | Ty::Float | Ty::Bool) { 2 } else { 0 });
        values.push(i64t.const_int(kind, false).into());
        let init = vt_ty.const_named_struct(&values);
        let g = self.module.add_global(vt_ty, None, &name);
        g.set_initializer(&init);
        g.set_linkage(inkwell::module::Linkage::Private);
        g.set_constant(true);
        Ok(g.as_pointer_value())
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

    /// A Rill function's own calling convention.
    ///
    /// `tailcc` is LLVM's guaranteed-tail-call convention, and what it buys is
    /// not speed but the promise this language makes: a call in tail position
    /// is a jump. The C convention cannot keep that promise. A tail call is a
    /// hint there, and the optimizer is free to take it away — when it proves
    /// that every path through a loop returns the same value it folds the
    /// returns together, and a call that no longer ends its function is one
    /// the backend will not jump to. That is not a rare shape: it is what
    /// every loop written for its effects looks like, and it cost a running
    /// server one frame per message until it ran off the end of its stack.
    ///
    /// With `tailcc` the call is marked `musttail`, which is an invariant
    /// rather than a hint: no pass may break it, and the two functions need
    /// not even have the same signature — which is what a loop written as
    /// several functions calling each other needs.
    ///
    /// Everything the runtime calls keeps the C convention: `main`, whose
    /// address is handed to the scheduler, the body of a `spawn`, an
    /// environment's drop function, a map's vtable, and every trampoline C
    /// itself calls.
    const TAILCC: u32 = 18;

    /// The convention a Rill function of this signature gets: `tailcc` when
    /// every argument rides in a register on every native target — six
    /// integer-class on x86-64 SysV, eight float-class on both — and the C
    /// convention when any would touch the stack, because LLVM's lowering of
    /// tailcc stack arguments walked a caller's stack pointer sixteen bytes
    /// per call until a live server's return address came out of the heap.
    /// `extra_ints` is for the closure paths, whose LLVM signature carries an
    /// environment pointer the `Ty` list does not mention. The budgets read
    /// an env override so the boundary can be probed without a rebuild.
    fn cc_for(&self, params: &[Ty], extra_ints: usize) -> Option<u32> {
        let iregs: usize = std::env::var("RILL_CC_INT_REGS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
        let fregs: usize = std::env::var("RILL_CC_FLOAT_REGS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        // A flat value is passed as its fields, and each takes a register.
        let mut floats = 0;
        let mut ints = extra_ints;
        for t in params {
            match self.flat_fields(t) {
                Some(fields) => {
                    for f in fields {
                        if f == Ty::Float || f == Ty::Float2 {
                            floats += 1;
                        } else {
                            ints += 1;
                        }
                    }
                }
                None if *t == Ty::Float || *t == Ty::Float2 => floats += 1,
                None => ints += 1,
            }
        }
        if ints > iregs || floats > fregs {
            None
        } else {
            Some(Self::TAILCC)
        }
    }

    /// Gives a call site the convention of what it calls, which is the only
    /// way the two can be sure to agree.
    fn match_cc(&self, call: inkwell::values::CallSiteValue<'ctx>, callee: FunctionValue<'ctx>) {
        call.set_call_convention(callee.get_call_conventions());
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
        let fn_type = self.instance_llvm_type(&params, &ret);
        let wrapped = self.host_wrapped(fn_idx);
        let name = if wrapped { format!("{}$in", self.key_name(&key)) } else { self.key_name(&key) };
        let v = self.module.add_function(&name, fn_type, None);
        // `main` is called by the scheduler, which is C. Everything else is
        // called by Rill, and Rill wants its tail calls to be jumps. On wasm
        // a tail call needs a feature the module may not be allowed to
        // declare, so there the C convention stays.
        if !self.wasm && self.tmod.fns[fn_idx].name != "main" && !self.tmod.fns[fn_idx].exported {
            if let Some(cc) = self.cc_for(&params, 0) {
                v.set_call_conventions(cc);
            }
        }
        // Nothing outside this module calls a Rill function except `main`
        // and a wasm export, so every other one is `internal`. That is not
        // cosmetic: LLVM inlines an external function warily, since it must
        // keep the out-of-line copy and cannot specialize it on its callers,
        // and a local `fn` that is a hot loop's helper pays for it —
        // fannkuch ran 15% slower and n-body 20% slower with them exported.
        if wrapped || (self.tmod.fns[fn_idx].name != "main" && !self.tmod.fns[fn_idx].exported) {
            v.set_linkage(inkwell::module::Linkage::Internal);
        }
        self.instances.insert(key.clone(), v);
        self.queue.push_back(key);
        v
    }

    /// Whether an export of a library with strands is reached through a
    /// wrapper that tells the runtime the call is in and out again (see
    /// `gen_host_wrappers`).
    fn host_wrapped(&self, fn_idx: usize) -> bool {
        self.shared && self.host_strands && self.tmod.fns[fn_idx].exported
    }

    /// The public name of every export: `rill_host_enter`, the export
    /// itself, `rill_host_leave`. The runtime counts the calls in flight so
    /// that the moment a second host thread appears — when counts must turn
    /// atomic — it can wait for the calls still counting plainly to leave.
    fn gen_host_wrappers(&mut self) {
        let keys: Vec<InstanceKey> =
            self.instances.keys().filter(|k| self.host_wrapped(k.0)).cloned().collect();
        let enter = self.module.get_function("rill_host_enter").unwrap();
        let leave = self.module.get_function("rill_host_leave").unwrap();
        for key in keys {
            let inner = self.instances[&key];
            let name = self.key_name(&key);
            let f = self.module.add_function(&name, inner.get_type(), None);
            let entry = self.ctx.append_basic_block(f, "entry");
            self.builder.position_at_end(entry);
            self.builder.build_call(enter, &[], "").expect("LLVM builder error");
            let args: Vec<BasicMetadataValueEnum<'ctx>> = f.get_param_iter().map(|p| p.into()).collect();
            let call = self.builder.build_call(inner, &args, "").expect("LLVM builder error");
            self.builder.build_call(leave, &[], "").expect("LLVM builder error");
            match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(v) => {
                    self.builder.build_return(Some(&v)).expect("LLVM builder error");
                }
                _ => {
                    self.builder.build_return(None).expect("LLVM builder error");
                }
            }
        }
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
                            "`{}` hands its parameter `{}` to C as a callback with no `Env` to \
                             carry an environment, so it has to be a function known while \
                             compiling — pass a top-level function by name, not a lambda or a \
                             value chosen at run time",
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
                Constraint::Numeric => matches!(ty, Ty::Int | Ty::Float | Ty::Float2),
                Constraint::Addable => matches!(ty, Ty::Int | Ty::Float | Ty::Float2 | Ty::Str),
                Constraint::Ordered => {
                    matches!(ty, Ty::Int | Ty::Float | Ty::Str)
                        || self.user_impl(rill_types::ORD_TRAIT, 0, ty).is_some()
                }
                Constraint::Basic => {
                    matches!(ty, Ty::Int | Ty::Float | Ty::Float2 | Ty::Bool | Ty::Str | Ty::Adt(..))
                }
                Constraint::Impl(t) => self.user_impl(*t, 0, ty).is_some(),
            };
            if !ok {
                let what = match c {
                    Constraint::Numeric => "be Int or Float".to_string(),
                    Constraint::Addable => "support `+`".to_string(),
                    Constraint::Ordered => "be ordered (Int, Float, Str or an `Ord` impl)".to_string(),
                    Constraint::Basic => "be printable".to_string(),
                    Constraint::Impl(t) => format!("implement `{}`", self.tmod.traits[*t].name),
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

    /// One line of `rill explain`, about the function and the source line
    /// being generated — for the program's own functions, once each.
    fn note(&mut self, text: String) {
        if self.explain && self.cur_fn >= self.tmod.prelude_fns && self.cur_fn < self.tmod.fns.len() {
            self.notes.insert((self.cur_fn, self.note_line, text));
        }
    }

    fn gen_instance(&mut self, key: &InstanceKey) -> Result<(), CodegenError> {
        self.check_constraints(key)?;
        let (fn_idx, subst, bound) = key;
        self.cur_fn = *fn_idx;
        if self.explain {
            let (fline, params) = {
                let f = &self.tmod.fns[*fn_idx];
                (f.line, f.params.clone())
            };
            self.note_line = fline;
            let borrowed = self.borrowed_params[*fn_idx].clone();
            for (i, (n, t)) in params.iter().enumerate() {
                let ty = self.concrete(t, subst);
                if !self.is_heap(&ty) {
                    continue;
                }
                let how = if borrowed.get(i) == Some(&true) {
                    "borrowed: never kept or given away here, so the caller's reference is enough"
                } else {
                    "owned: the caller hands over a reference, which this function keeps, passes on or releases"
                };
                self.note(format!("`{n}` is {how}"));
            }
        }
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
        let outer_shells = std::mem::take(&mut self.shells);
        let outer_movable = std::mem::take(&mut self.movable);
        let outer_free = std::mem::take(&mut self.root_free);
        let outer_move_ok = std::mem::replace(&mut self.move_ok, true);
        let mut scope: Scope<'ctx> = HashMap::new();
        let mut next_raw = 0u32;
        for (i, (pname, pty)) in f.params.iter().enumerate() {
            let cty = subst_gen(pty, subst);
            // A flat value came in as its fields; put it back together, which
            // is free once the optimizer has looked at it.
            let raw = match self.flat_fields(&cty) {
                Some(fields) => {
                    let st = self.flat_struct(&cty);
                    let mut agg: BasicValueEnum<'ctx> = st.get_undef().into();
                    for (k, _) in fields.iter().enumerate() {
                        let piece = fn_val.get_nth_param(next_raw).unwrap();
                        piece.set_name(&format!("{pname}.{k}"));
                        next_raw += 1;
                        agg = self
                            .builder
                            .build_insert_value(agg.into_struct_value(), piece, k as u32, pname)
                            .expect("LLVM builder error")
                            .into_struct_value()
                            .into();
                    }
                    agg
                }
                None => {
                    let raw = fn_val.get_nth_param(next_raw).unwrap();
                    next_raw += 1;
                    raw.set_name(pname);
                    raw
                }
            };
            scope.insert(pname.clone(), Some(raw));
            // A value the host lends an export is the host's: this frame
            // takes a reference of its own for the call and lets that one
            // go, and the host's is as it was.
            if f.exported && self.shared && self.is_heap(&cty) {
                self.emit_retain(raw, &cty);
            }
            // A borrowed parameter is the caller's to release, not this
            // frame's: nothing here retained it and nothing here lets it go.
            if self.is_heap(&cty) && !self.borrowed_params[*fn_idx][i] {
                self.cleanup.push((raw, cty));
            }
        }

        let ret_ty = subst_gen(&f.ret, subst);
        let body = &self.tmod.fns[*fn_idx].body;
        let (outer_mt, outer_nts) = (self.saw_musttail, self.saw_nontail_self);
        self.saw_musttail = false;
        self.saw_nontail_self = false;
        self.gen_tail(body, &mut scope, subst, fn_val, &ret_ty)?;
        // LLVM 18's tail-recursion elimination, given a function that calls
        // itself somewhere that is not a tail — `eval(a) + eval(b)` — turns
        // that recursion into a loop with an accumulator and rewrites every
        // `ret` of the function as `ret acc + value`, a `musttail` one
        // included, and a `musttail` whose value is added to before the
        // return is a broken module. The attribute keeps that pass out of
        // exactly such a function; the backend still honours `musttail`
        // under it, and only the hinted tail calls are given up. A loop
        // whose self-calls are all `musttail` is left alone: there the pass
        // is what turns the calls into the loop the optimizer vectorizes.
        if self.saw_musttail && self.saw_nontail_self {
            fn_val.add_attribute(
                AttributeLoc::Function,
                self.ctx.create_string_attribute("disable-tail-calls", "true"),
            );
        }
        self.saw_musttail = outer_mt;
        self.saw_nontail_self = outer_nts;
        self.cleanup = outer_cleanup;
        self.shells = outer_shells;
        self.movable = outer_movable;
        self.root_free = outer_free;
        self.move_ok = outer_move_ok;
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
        if e.line > 0 {
            self.note_line = e.line;
        }
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
                let outer_movable = std::mem::take(&mut self.movable);
                let outer_free = std::mem::take(&mut self.root_free);
                let saved_held_tail = self.held_above.clone();
                let saved_scope = scope.clone();
                for (si, stmt) in init.iter().enumerate() {
                    let rest = &stmts[si + 1..];
                    self.begin_statement(stmt_value(stmt), rest);
                    match stmt {
                        TStmt::Bind { name, value } if self.try_bind_borrowed(name, value, rest, scope, subst, func)? => {}
                        TStmt::Bind { name, value } => {
                            let v = match self.take_builder(value, rest, scope) {
                                Some(v) => Some(v),
                                None => self.gen_expr(value, scope, subst, func)?,
                            };
                            scope.insert(name.clone(), v);
                            self.rebind_known_fn(name, value, subst);
                            if let Some(v) = v {
                                let ty = self.concrete(&value.ty, subst);
                                if self.is_heap(&ty) {
                                    self.cleanup.push((v, ty));
                                }
                            }
                        }
                        TStmt::Expr(v) => {
                            let out = self.gen_expr(v, scope, subst, func)?;
                            if let Some(out) = out {
                                let ty = self.concrete(&v.ty, subst);
                                if self.is_heap(&ty) {
                                    self.emit_release(out, &ty);
                                }
                            }
                        }
                    }
                }
                match last {
                    Some(last) => self.begin_statement(last, &[]),
                    None => {
                        self.movable.clear();
                        self.root_free.clear();
                    }
                }
                let out = match last {
                    Some(last) => match self.take_builder(last, &[], scope) {
                        Some(v) => self.finish_leaf(Some(v), &Ty::Str, ret_ty),
                        None => self.gen_tail(last, scope, subst, func, ret_ty),
                    },
                    None => self.finish_leaf(None, &Ty::Unit, ret_ty),
                };
                self.movable = outer_movable;
                self.root_free = outer_free;
                self.bound_fns = saved_bound;
                self.held_above = saved_held_tail;
                *scope = saved_scope;
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
                let saved_reuse = self.reuse.clone();
                let saved_shells = self.shells.clone();
                self.builder.position_at_end(then_bb);
                let mut then_scope = scope.clone();
                self.gen_tail(then, &mut then_scope, subst, func, ret_ty)?;
                self.cleanup = saved_cleanup;
                self.reuse = saved_reuse;
                self.shells = saved_shells;
                self.builder.position_at_end(else_bb);
                let mut else_scope = scope.clone();
                self.gen_tail(els, &mut else_scope, subst, func, ret_ty)
            }
            TK::Match { scrutinee, arms } => {
                // Arms end on their own paths, but the scrutinee is held for
                // the arm's length by the match itself: nothing moves here.
                let saved = std::mem::replace(&mut self.move_ok, false);
                let out = self.gen_tail_match(scrutinee, arms, scope, subst, func, ret_ty);
                self.move_ok = saved;
                out
            }
            TK::CallFn { fn_idx, subst: inner, args } => {
                if let Some(v) = self.try_fuse(e, scope, subst, func)? {
                    self.note("this pipeline is one loop: no list is built".to_string());
                    let ty = self.concrete(&e.ty, subst);
                    return self.finish_leaf(Some(v), &ty, ret_ty);
                }
                if let Some(v) = self.try_map_or(e, scope, subst, func)? {
                    self.note("`map_or` reads the table without building a `Some`".to_string());
                    let ty = self.concrete(&e.ty, subst);
                    return self.finish_leaf(Some(v), &ty, ret_ty);
                }
                if let Some(v) = self.try_map_list(e, scope, subst, func)? {
                    self.note("`map` walks the list in place, writing each link over the one it read".to_string());
                    let ty = self.concrete(&e.ty, subst);
                    return self.finish_leaf(Some(v), &ty, ret_ty);
                }
                // A borrowed parameter is kept alive by its caller, and a
                // frame in tail position releases everything it owns before
                // it jumps. So only what some *earlier* frame holds — a
                // borrowed parameter of this very function — can go through
                // a borrowed position as a tail call; a value this frame owns
                // or makes goes by an ordinary call and is released after.
                let borrowed = self.borrowed_params[*fn_idx].clone();
                let fits_tail = args
                    .iter()
                    .enumerate()
                    .all(|(i, a)| borrowed.get(i) != Some(&true) || self.borrowed_here(a));
                if !fits_tail {
                    let ty = self.concrete(&e.ty, subst);
                    let out = self.gen_expr(e, scope, subst, func)?;
                    return self.finish_leaf(out, &ty, ret_ty);
                }
                // Before the arguments, so there is as little live across the
                // yield as possible.
                if self.preempt && self.call_goes_round(*fn_idx) {
                    self.emit_preempt_check(func);
                }
                let out_ty = self.concrete(&e.ty, subst);
                let inner: Vec<Ty> = inner.iter().map(|t| self.concrete(t, subst)).collect();
                let bound = self.bind_static_args(*fn_idx, args, subst, e.line)?;
                let callee = self.request_instance(*fn_idx, inner.clone(), bound);
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    if borrowed.get(i) == Some(&true) {
                        if self.explain {
                            if let TK::Local(n) = &a.kind {
                                self.note(format!("`{n}` is passed to the tail call borrowed: the callee never keeps it"));
                            }
                        }
                        let (v, _) = self.gen_borrowed(a, scope, subst, func)?;
                        argv.push(v.into());
                        continue;
                    }
                    // A binding this frame owns and hands to the call is
                    // moved, not copied: the retain a use would take and the
                    // release that follows it cancel exactly, and every loop
                    // written as a tail call passes its whole working set.
                    let v = match self.take_owned(a, scope) {
                        Some(v) => {
                            if let TK::Local(n) = &a.kind {
                                self.note(format!("`{n}` is handed to the tail call: this frame lets go of everything before the jump, so no count is taken"));
                            }
                            v
                        }
                        None => match &a.kind {
                            TK::CallCtor { adt, ctor, targs, args } if !args.is_empty() => self
                                .gen_ctor(*adt, *ctor, targs, args, scope, subst, func, true)?
                                .expect("a constructor is never Unit"),
                            _ => self
                                .gen_expr(a, scope, subst, func)?
                                .expect("arguments are never Unit"),
                        },
                    };
                    argv.push(v.into());
                }
                // Everything this frame still owns dies before the call, which
                // is what makes it a genuine tail call.
                self.flush_cleanup();
                let argv = self.explode_args(argv, &self.instance_params(*fn_idx, &inner));
                let call =
                    self.builder.build_call(callee, &argv, "tcall").expect("LLVM builder error");
                self.match_cc(call, callee);
                call.set_tail_call(true);
                // `musttail` is the promise rather than the hint, and it is
                // only sayable where the shape it requires already holds: the
                // return has to be this call's value and nothing else, which
                // is exactly what `ret_leaf` writes when the two types agree.
                // A `Unit` return releases what came back first, and a call
                // with an instruction after it is not a tail call at all.
                let caller_tailcc = self
                    .builder
                    .get_insert_block()
                    .and_then(|b| b.get_parent())
                    .is_some_and(|f| f.get_call_conventions() == Self::TAILCC);
                let callee_tailcc = callee.get_call_conventions() == Self::TAILCC;
                if callee_tailcc
                    && caller_tailcc
                    && out_ty == *ret_ty
                    && self.llvm_type(ret_ty).is_some()
                {
                    call.set_tail_call_kind(inkwell::values::LLVMTailCallKind::LLVMTailCallKindMustTail);
                    self.saw_musttail = true;
                } else if !self.wasm
                    && !(callee_tailcc && caller_tailcc)
                    && *fn_idx != self.cur_fn
                    && self.call_goes_round(*fn_idx)
                {
                    // Not on wasm: there the C convention is universal by
                    // design — the tail-call proposal may not be available to
                    // declare — so this would flag every mutual loop in every
                    // module while naming a fix that does not apply.
                    // The promise is quietly absent exactly here: a call in
                    // tail position that can come back around, made under the
                    // C convention because one signature or the other did not
                    // fit the registers. LLVM keeps such calls flat when it
                    // happens to turn the loop into one and lets the stack
                    // grow when it does not — the valley's reader leaked two
                    // frames per message for weeks and died at 87,385, so
                    // this is said out loud rather than left to the core dump.
                    //
                    // A function calling *itself* is exempt: a same-signature
                    // self call is the one shape LLVM's sibling-call pass
                    // reliably folds whatever the convention — verified five
                    // million deep — and it is also what every `for` loop
                    // desugars to, so warning on it would bury the real
                    // finding under compiler-made helpers nobody can fix.
                    self.warn_untailed(*fn_idx, e.line);
                }
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                self.ret_leaf(out, &out_ty, ret_ty)
            }
            // A box that is the function's answer leaves the frame as
            // surely as one handed to a tail call, on this path alone: what
            // the frame owns may be moved into it, and a box this arm took
            // apart may be written over to make it. `simp`'s
            // `(Num(x), Mul(Num(y), z)) -> Mul(Num(x * y), z)` then
            // allocates nothing.
            TK::CallCtor { adt, ctor, targs, args } if !args.is_empty() => {
                let ty = self.concrete(&e.ty, subst);
                let out = self.gen_ctor(*adt, *ctor, targs, args, scope, subst, func, true)?;
                self.finish_leaf(out, &ty, ret_ty)
            }
            // A local the frame owns, handed back as the answer: the frame's
            // reference goes with it, so nothing is counted.
            TK::Local(_) if self.is_heap(&self.concrete(&e.ty, subst)) => {
                let ty = self.concrete(&e.ty, subst);
                let out = match self.take_owned(e, scope) {
                    Some(v) => Some(v),
                    None => self.gen_expr(e, scope, subst, func)?,
                };
                self.finish_leaf(out, &ty, ret_ty)
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
    /// `range(a, b) |> filter(p) |> map(f) |> sum` — and `len`, and `fold`
    /// — as the one loop it describes, with no list in between.
    ///
    /// Written as the prelude has it, that pipeline builds every element of
    /// the range as a box, then walks the list once per stage: five million
    /// numbers were 154 MB and 70 ms against a C loop's 5. The rewrite here
    /// is the classic one — the consumer at the end folds each element as
    /// the range produces it, each `map` becomes an assignment and each
    /// `filter` a branch back to the loop — and it fires only when the whole
    /// chain is visible at once: the prelude's own functions (a program that
    /// redefines `map` gets what it wrote), lambdas written in place, and
    /// scalar element and accumulator types, so no reference count is ever
    /// in play. Anything else is generated as the calls it is.
    fn try_fuse(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        enum Consumer<'e> {
            Sum,
            Len,
            Fold(&'e TExpr, &'e TExpr),
        }
        enum Stage<'e> {
            Map(&'e TExpr),
            Filter(&'e TExpr),
        }
        let TK::CallFn { fn_idx, args, .. } = &e.kind else { return Ok(None) };
        let (consumer, chain) = if self.is_prelude(*fn_idx, "sum") && args.len() == 1 {
            (Consumer::Sum, &args[0])
        } else if self.is_prelude(*fn_idx, "len") && args.len() == 1 {
            (Consumer::Len, &args[0])
        } else if self.is_prelude(*fn_idx, "fold") && args.len() == 3 {
            (Consumer::Fold(&args[1], &args[2]), &args[0])
        } else {
            return Ok(None);
        };
        // Peel the chain down to the range it starts from.
        let mut stages: Vec<Stage> = Vec::new();
        let mut cur = chain;
        let (lo, hi) = loop {
            let TK::CallFn { fn_idx: f, args: a, .. } = &cur.kind else { return Ok(None) };
            if self.is_prelude(*f, "map") && a.len() == 2 {
                stages.push(Stage::Map(&a[1]));
                cur = &a[0];
            } else if self.is_prelude(*f, "filter") && a.len() == 2 {
                stages.push(Stage::Filter(&a[1]));
                cur = &a[0];
            } else if self.is_prelude(*f, "range") && a.len() == 2 {
                break (&a[0], &a[1]);
            } else {
                return Ok(None);
            }
        };
        // The stages were collected from the outside in; they run inside out.
        stages.reverse();
        for st in &stages {
            let (f, n) = match st {
                Stage::Map(f) | Stage::Filter(f) => (*f, 1),
            };
            if self.fusible_lambda(f, n, subst).is_none() {
                return Ok(None);
            }
        }
        if let Consumer::Fold(init, f) = &consumer {
            if !Self::is_scalar(&self.concrete(&init.ty, subst))
                || self.fusible_lambda(f, 2, subst).is_none()
            {
                return Ok(None);
            }
        }
        if !Self::is_scalar(&self.concrete(&e.ty, subst)) {
            return Ok(None);
        }

        let i64t = self.ctx.i64_type();
        let lo_v = self.gen_expr(lo, scope, subst, func)?.expect("value").into_int_value();
        let hi_v = self.gen_expr(hi, scope, subst, func)?.expect("value").into_int_value();
        let init_v: BasicValueEnum<'ctx> = match &consumer {
            Consumer::Sum | Consumer::Len => i64t.const_zero().into(),
            Consumer::Fold(init, _) => self.gen_expr(init, scope, subst, func)?.expect("value"),
        };
        let pre_bb = self.builder.get_insert_block().unwrap();
        let head_bb = self.ctx.append_basic_block(func, "fuse.head");
        let body_bb = self.ctx.append_basic_block(func, "fuse.body");
        let latch_bb = self.ctx.append_basic_block(func, "fuse.latch");
        let exit_bb = self.ctx.append_basic_block(func, "fuse.exit");
        self.builder.build_unconditional_branch(head_bb).expect("LLVM builder error");

        self.builder.position_at_end(head_bb);
        let i_phi = self.builder.build_phi(i64t, "fuse.i").expect("LLVM builder error");
        let acc_phi =
            self.builder.build_phi(init_v.get_type(), "fuse.acc").expect("LLVM builder error");
        i_phi.add_incoming(&[(&lo_v, pre_bb)]);
        acc_phi.add_incoming(&[(&init_v, pre_bb)]);
        // `range(a, b)` is a..=b: `range(1, n)` has n elements.
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLE, i_phi.as_basic_value().into_int_value(), hi_v, "more")
            .expect("LLVM builder error");
        self.builder.build_conditional_branch(more, body_bb, exit_bb).expect("LLVM builder error");

        self.builder.position_at_end(body_bb);
        let mut x: BasicValueEnum<'ctx> = i_phi.as_basic_value();
        // Blocks that leave for the latch with the accumulator unchanged.
        let mut skips: Vec<BasicBlock<'ctx>> = Vec::new();
        for st in &stages {
            match st {
                Stage::Map(f) => {
                    x = self.gen_inline_lambda(f, &[x], scope, subst, func)?;
                }
                Stage::Filter(p) => {
                    let keep = self.gen_inline_lambda(p, &[x], scope, subst, func)?.into_int_value();
                    let here = self.builder.get_insert_block().unwrap();
                    let next_bb = self.ctx.append_basic_block(func, "fuse.kept");
                    self.builder
                        .build_conditional_branch(keep, next_bb, latch_bb)
                        .expect("LLVM builder error");
                    skips.push(here);
                    self.builder.position_at_end(next_bb);
                }
            }
        }
        let acc_cur = acc_phi.as_basic_value();
        let acc_new: BasicValueEnum<'ctx> = match &consumer {
            Consumer::Sum => self
                .builder
                .build_int_add(acc_cur.into_int_value(), x.into_int_value(), "sum")
                .expect("LLVM builder error")
                .into(),
            Consumer::Len => self
                .builder
                .build_int_add(acc_cur.into_int_value(), i64t.const_int(1, false), "len")
                .expect("LLVM builder error")
                .into(),
            Consumer::Fold(_, f) => self.gen_inline_lambda(f, &[acc_cur, x], scope, subst, func)?,
        };
        let body_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(latch_bb).expect("LLVM builder error");

        self.builder.position_at_end(latch_bb);
        let acc_next =
            self.builder.build_phi(init_v.get_type(), "fuse.acc.next").expect("LLVM builder error");
        acc_next.add_incoming(&[(&acc_new, body_end)]);
        for bb in &skips {
            acc_next.add_incoming(&[(&acc_cur, *bb)]);
        }
        // A loop is where a strand has to be interruptible, as at a tail call.
        if self.preempt {
            self.emit_preempt_check(func);
        }
        let i_next = self
            .builder
            .build_int_add(i_phi.as_basic_value().into_int_value(), i64t.const_int(1, false), "fuse.i.next")
            .expect("LLVM builder error");
        let latch_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(head_bb).expect("LLVM builder error");
        i_phi.add_incoming(&[(&i_next, latch_end)]);
        acc_phi.add_incoming(&[(&acc_next.as_basic_value(), latch_end)]);

        self.builder.position_at_end(exit_bb);
        Ok(Some(acc_cur))
    }

    /// The prelude's `map_or(m, k, d)` on a table of scalar values, as the
    /// one runtime call it is: a find and a read used to be two calls, each
    /// hashing and comparing through the table's function pointers.
    fn try_map_or(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let TK::CallFn { fn_idx, args, .. } = &e.kind else { return Ok(None) };
        if !self.is_prelude(*fn_idx, "map_or") || args.len() != 3 {
            return Ok(None);
        }
        let mty = self.concrete(&args[0].ty, subst);
        let (kty, vty) = self.map_kv(&mty, e.line)?;
        if !Self::is_scalar(&vty) {
            return Ok(None);
        }
        let (m, mowned) = self.gen_borrowed(&args[0], scope, subst, func)?;
        let (kv, kowned) = self.gen_borrowed(&args[1], scope, subst, func)?;
        let dv = self.gen_expr(&args[2], scope, subst, func)?.expect("value");
        let (k0, _) = self.pack_words(kv, &kty);
        let (d0, _) = self.pack_words(dv, &vty);
        let w = if kty == Ty::Int {
            self.inline_int_probe(m.into_pointer_value(), k0, d0, func)
        } else {
            let call = self.call_rt("rill_map_get_or", &[m.into(), k0.into(), d0.into()]);
            match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                _ => unreachable!("rill_map_get_or returns a word"),
            }
        };
        let out = self.unpack_words(w, self.ctx.i64_type().const_zero(), &vty);
        if kowned {
            self.emit_release(kv, &kty);
        }
        if mowned {
            self.emit_release(m, &mty);
        }
        Ok(Some(out))
    }

    /// The table's probe for an `Int` key, laid down in place: the hash,
    /// the walk along the control bytes, the one key compare, and the value
    /// word — or the default when an empty control byte ends the walk. This
    /// is the runtime's `map_lookup` for the case its `KIND_INT_KEY` flag
    /// covers, so the two must agree on the layout of `Map` (`ctrl` at word
    /// 2, `slots` at 3, `cap` at 4, `stride` at 5), on `ctrl_of` and on the
    /// sentinels. A million lookups were a runtime call each; C inlines its.
    fn inline_int_probe(
        &mut self,
        m: PointerValue<'ctx>,
        k: IntValue<'ctx>,
        d: IntValue<'ctx>,
        func: FunctionValue<'ctx>,
    ) -> IntValue<'ctx> {
        let i64t = self.ctx.i64_type();
        let i8t = self.ctx.i8_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let b = &self.builder;
        let word = |i: u64| -> PointerValue<'ctx> {
            unsafe { b.build_in_bounds_gep(i64t, m, &[i64t.const_int(i, false)], "m.w").expect("builder") }
        };
        let ctrl = b.build_load(ptrt, word(2), "ctrl").expect("builder").into_pointer_value();
        let slots = b.build_load(ptrt, word(3), "slots").expect("builder").into_pointer_value();
        let cap = b.build_load(i64t, word(4), "cap").expect("builder").into_int_value();
        let stride = b.build_load(i64t, word(5), "stride").expect("builder").into_int_value();
        let empty_tbl = b.build_int_compare(IntPredicate::EQ, cap, i64t.const_zero(), "nocap").expect("builder");
        let h = self.hash_int(k);
        let b = &self.builder;
        let mask = b.build_int_sub(cap, i64t.const_int(1, false), "mask").expect("builder");
        let want = b
            .build_int_truncate(
                b.build_right_shift(h, i64t.const_int(57, false), false, "h57").expect("builder"),
                i8t,
                "want",
            )
            .expect("builder");
        let i0 = b.build_and(h, mask, "i0").expect("builder");
        let pre = b.get_insert_block().unwrap();
        let loop_bb = self.ctx.append_basic_block(func, "probe");
        let key_bb = self.ctx.append_basic_block(func, "probe.key");
        let next_bb = self.ctx.append_basic_block(func, "probe.next");
        let hit_bb = self.ctx.append_basic_block(func, "probe.hit");
        let done_bb = self.ctx.append_basic_block(func, "probe.done");
        b.build_conditional_branch(empty_tbl, done_bb, loop_bb).expect("builder");

        b.position_at_end(loop_bb);
        let i = b.build_phi(i64t, "i").expect("builder");
        i.add_incoming(&[(&i0, pre)]);
        let iv = i.as_basic_value().into_int_value();
        let cp = unsafe { b.build_in_bounds_gep(i8t, ctrl, &[iv], "c.p").expect("builder") };
        let c = b.build_load(i8t, cp, "c").expect("builder").into_int_value();
        let is_empty = b.build_int_compare(IntPredicate::EQ, c, i8t.const_int(0xFF, false), "empty").expect("builder");
        let same = b.build_int_compare(IntPredicate::EQ, c, want, "same").expect("builder");
        let miss_now_bb = self.ctx.append_basic_block(func, "probe.miss");
        b.build_conditional_branch(is_empty, miss_now_bb, key_bb).expect("builder");

        b.position_at_end(key_bb);
        let off = b.build_int_mul(iv, stride, "off").expect("builder");
        let kp = unsafe { b.build_in_bounds_gep(i64t, slots, &[off], "k.p").expect("builder") };
        let same_key = b.build_and(
            same,
            b.build_int_compare(IntPredicate::EQ, b.build_load(i64t, kp, "k").expect("builder").into_int_value(), k, "keq").expect("builder"),
            "found",
        ).expect("builder");
        b.build_conditional_branch(same_key, hit_bb, next_bb).expect("builder");

        b.position_at_end(next_bb);
        let i1 = b.build_and(b.build_int_add(iv, i64t.const_int(1, false), "i1").expect("builder"), mask, "i1m").expect("builder");
        i.add_incoming(&[(&i1, next_bb)]);
        b.build_unconditional_branch(loop_bb).expect("builder");

        b.position_at_end(hit_bb);
        let vp = unsafe { b.build_in_bounds_gep(i64t, kp, &[i64t.const_int(1, false)], "v.p").expect("builder") };
        let v = b.build_load(i64t, vp, "v").expect("builder").into_int_value();
        b.build_unconditional_branch(done_bb).expect("builder");

        b.position_at_end(miss_now_bb);
        b.build_unconditional_branch(done_bb).expect("builder");

        b.position_at_end(done_bb);
        let out = b.build_phi(i64t, "got").expect("builder");
        out.add_incoming(&[(&d, pre), (&v, hit_bb), (&d, miss_now_bb)]);
        out.as_basic_value().into_int_value()
    }

    /// `strbuf_str(b)` where `b` is a builder this frame owns and never
    /// names again: the string is taken out of the builder's block instead
    /// of copied — 39 MB in the string-building benchmark, and every
    /// `render` that ends with the string it built.
    fn take_builder(
        &mut self,
        value: &TExpr,
        rest: &[TStmt],
        scope: &Scope<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let TK::Builtin { rt, args } = &value.kind else { return None };
        // `str_from_buf(v)` on a byte buffer the frame owns and never names
        // again is the same trade: the block becomes the string.
        let take = match *rt {
            "rill_strbuf_str" => "rill_strbuf_take",
            "rill_str_from_buf" => "rill_str_take_buf",
            _ => return None,
        };
        let TK::Local(b) = &args[0].kind else { return None };
        let bv = (*scope.get(b)?)?;
        if !self.cleanup.iter().any(|(c, _)| *c == bv) {
            return None;
        }
        let used_later = rest.iter().any(|st| match st {
            TStmt::Bind { value, .. } | TStmt::Expr(value) => names_local(value, b),
        });
        if used_later {
            return None;
        }
        let call = self.call_rt(take, &[bv.into()]);
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Some(v),
            _ => unreachable!("the hand-over returns a string"),
        }
    }

    /// `map(xs, \x -> …)` over a list of numbers or flat records, as the
    /// loop it is: walk `xs`, lay the lambda's body over each element, and
    /// build the result *forward*, each new cell written into the tail of the
    /// one before, so there is no closure, no indirect call per element and
    /// no second pass to turn the list round. Elements of those types carry
    /// no count, which is what keeps this simple: the only heap objects in
    /// play are the cells. The five-body list an n-body rebuilds each step was
    /// spending more on the machinery of `map` than on the physics.
    fn try_map_list(
        &mut self,
        e: &TExpr,
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let TK::CallFn { fn_idx, args, .. } = &e.kind else { return Ok(None) };
        if !self.is_prelude(*fn_idx, "map") || args.len() != 2 {
            return Ok(None);
        }
        let in_ty = self.concrete(&args[0].ty, subst);
        let out_ty = self.concrete(&e.ty, subst);
        let (Ty::Adt(adt, in_targs), Ty::Adt(adt2, out_targs)) = (&in_ty, &out_ty) else {
            return Ok(None);
        };
        if adt != adt2 || in_targs.len() != 1 || out_targs.len() != 1 {
            return Ok(None);
        }
        let def = &self.tmod.adts[*adt];
        let nil_ci = def.ctors.iter().position(|c| c.name == "Nil" && c.fields.is_empty());
        let cons_ci = def.ctors.iter().position(|c| c.name == "Cons" && c.fields.len() == 2);
        let (Some(nil_ci), Some(cons_ci)) = (nil_ci, cons_ci) else { return Ok(None) };
        if def.name != "List" || def.ctors.len() != 2 {
            return Ok(None);
        }
        let plain = |cg: &Self, t: &Ty| Self::is_scalar(t) || cg.is_flat(t);
        if !plain(self, &in_targs[0]) || !plain(self, &out_targs[0]) {
            return Ok(None);
        }
        let TK::Lambda { params, body, .. } = &args[1].kind else { return Ok(None) };
        if params.len() != 1
            || !plain(self, &self.concrete(&params[0].1, subst))
            || !plain(self, &self.concrete(&body.ty, subst))
            || !self.registers_only(body, subst)
        {
            return Ok(None);
        }
        let (adt, in_targs, out_targs) = (*adt, in_targs.clone(), out_targs.clone());
        let nil_tag = self.tmod.adts[adt].ctors[nil_ci].tag;
        let cons_tag = self.tmod.adts[adt].ctors[cons_ci].tag;

        let (xs, held) = self.gen_borrowed(&args[0], scope, subst, func)?;
        let xs = xs.into_pointer_value();
        let i64t = self.ctx.i64_type();
        let ptrt = self.ctx.ptr_type(Default::default());
        let in_st = self.ctor_struct(adt, cons_ci, &in_targs);
        let out_st = self.ctor_struct(adt, cons_ci, &out_targs);
        let nil = self
            .gen_ctor(adt, nil_ci, &out_targs, &[], scope, subst, func, false)?
            .expect("a constructor")
            .into_pointer_value();
        // Where the next cell's address goes: the result's head to begin
        // with, then each cell's tail. A stack slot, so that the loop needs no
        // branch on "is this the first".
        let entry = func.get_first_basic_block().expect("a function has an entry");
        let here = self.builder.get_insert_block().unwrap();
        match entry.get_first_instruction() {
            Some(i) => self.builder.position_before(&i),
            None => self.builder.position_at_end(entry),
        }
        let head_slot = self.builder.build_alloca(ptrt, "map.head").expect("LLVM builder error");
        self.builder.position_at_end(here);
        self.builder.build_store(head_slot, nil).expect("LLVM builder error");

        let loop_bb = self.ctx.append_basic_block(func, "map.loop");
        let body_bb = self.ctx.append_basic_block(func, "map.body");
        let exit_bb = self.ctx.append_basic_block(func, "map.exit");
        let pre = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(loop_bb).expect("LLVM builder error");

        self.builder.position_at_end(loop_bb);
        let cur = self.builder.build_phi(ptrt, "map.cur").expect("LLVM builder error");
        let slot = self.builder.build_phi(ptrt, "map.slot").expect("LLVM builder error");
        cur.add_incoming(&[(&xs, pre)]);
        slot.add_incoming(&[(&head_slot, pre)]);
        let curp = cur.as_basic_value().into_pointer_value();
        let tag_ptr = self
            .builder
            .build_struct_gep(self.header_struct(), curp, 1, "tag.p")
            .expect("LLVM builder error");
        let tag = self.builder.build_load(i64t, tag_ptr, "tag").expect("LLVM builder error").into_int_value();
        let done = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, i64t.const_int(nil_tag, false), "done")
            .expect("LLVM builder error");
        self.builder.build_conditional_branch(done, exit_bb, body_bb).expect("LLVM builder error");

        self.builder.position_at_end(body_bb);
        let xp = self.builder.build_struct_gep(in_st, curp, 2, "x.p").expect("LLVM builder error");
        let x = self
            .builder
            .build_load(self.llvm_type(&in_targs[0]).expect("an element"), xp, "x")
            .expect("LLVM builder error");
        let np = self.builder.build_struct_gep(in_st, curp, 3, "next.p").expect("LLVM builder error");
        let next = self.builder.build_load(ptrt, np, "next").expect("LLVM builder error");
        let y = self.gen_inline_lambda(&args[1], &[x], scope, subst, func)?;
        let cell = self.alloc_object(out_st, "map.cell");
        let ct = self.builder.build_struct_gep(out_st, cell, 1, "cell.tag").expect("LLVM builder error");
        self.builder.build_store(ct, i64t.const_int(cons_tag, false)).expect("LLVM builder error");
        let ch = self.builder.build_struct_gep(out_st, cell, 2, "cell.head").expect("LLVM builder error");
        self.builder.build_store(ch, y).expect("LLVM builder error");
        let ctail = self.builder.build_struct_gep(out_st, cell, 3, "cell.tail").expect("LLVM builder error");
        self.builder.build_store(ctail, nil).expect("LLVM builder error");
        self.builder
            .build_store(slot.as_basic_value().into_pointer_value(), cell)
            .expect("LLVM builder error");
        if self.preempt {
            self.emit_preempt_check(func);
        }
        let body_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(loop_bb).expect("LLVM builder error");
        cur.add_incoming(&[(&next, body_end)]);
        slot.add_incoming(&[(&ctail, body_end)]);

        self.builder.position_at_end(exit_bb);
        let out = self.builder.build_load(ptrt, head_slot, "map.out").expect("LLVM builder error");
        if held {
            self.emit_release(xs.into(), &in_ty);
        }
        Ok(Some(out))
    }

    fn is_prelude(&self, fn_idx: usize, name: &str) -> bool {
        fn_idx < self.tmod.prelude_fns && self.tmod.fns[fn_idx].name == name
    }

    fn is_scalar(ty: &Ty) -> bool {
        matches!(ty, Ty::Int | Ty::Float | Ty::Bool)
    }

    /// A lambda written in place, of `n` scalar parameters and a scalar
    /// result, whose body binds nothing on the heap — one that can be laid
    /// into a loop body with its parameters bound to registers.
    fn fusible_lambda<'e>(
        &self,
        f: &'e TExpr,
        n: usize,
        subst: &[Ty],
    ) -> Option<(&'e [(String, Ty)], &'e TExpr)> {
        let TK::Lambda { params, body, .. } = &f.kind else { return None };
        if params.len() != n
            || !params.iter().all(|(_, t)| Self::is_scalar(&self.concrete(t, subst)))
            || !Self::is_scalar(&self.concrete(&body.ty, subst))
            || !self.registers_only(body, subst)
        {
            return None;
        }
        Some((params, body))
    }

    /// Whether an expression can be generated in the middle of a loop body:
    /// nothing it binds lives on the heap (a block's heap bindings are
    /// released at function exit, which is once, not once per iteration),
    /// and it makes no closure, strand or select of its own.
    fn registers_only(&self, e: &TExpr, subst: &[Ty]) -> bool {
        match &e.kind {
            TK::Lambda { .. } | TK::Spawn { .. } | TK::Select { .. } => return false,
            TK::Block(stmts) => {
                for s in stmts {
                    if let TStmt::Bind { value, .. } = s {
                        if self.is_heap(&self.concrete(&value.ty, subst)) {
                            return false;
                        }
                    }
                }
            }
            TK::Match { scrutinee, .. } => {
                if self.is_heap(&self.concrete(&scrutinee.ty, subst)) {
                    return false;
                }
            }
            _ => {}
        }
        let mut ok = true;
        walk_children(e, &mut |c| {
            if ok {
                ok = self.registers_only(c, subst);
            }
        });
        ok
    }

    /// Generates a lambda's body here, with its parameters bound to `args`.
    fn gen_inline_lambda(
        &mut self,
        f: &TExpr,
        args: &[BasicValueEnum<'ctx>],
        scope: &Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        let TK::Lambda { params, body, .. } = &f.kind else { unreachable!("checked by fusible_lambda") };
        let mut inner = scope.clone();
        for ((name, _), v) in params.iter().zip(args) {
            inner.insert(name.clone(), Some(*v));
        }
        Ok(self.gen_expr(body, &mut inner, subst, func)?.expect("a scalar"))
    }

    /// `x = q.f` where `q` is held above and every later use of `x` in the
    /// block only borrows it: `x` is read out of the box with no count
    /// taken and is held above too. Before this, `bt = q.db.bt` at the top
    /// of a function was a retain and a release around the body — on a
    /// `--parallel` build two runtime calls, and atomic ones inside a
    /// parallel section — for a box the caller was holding all along.
    /// Answers whether the binding was made this way.
    fn try_bind_borrowed(
        &mut self,
        name: &str,
        value: &TExpr,
        rest: &[TStmt],
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
    ) -> Result<bool, CodegenError> {
        if !matches!(value.kind, TK::FieldGet { .. }) {
            return Ok(false);
        }
        let ty = self.concrete(&value.ty, subst);
        if !self.is_heap(&ty) {
            return Ok(false);
        }
        let Some(root) = root_local(value) else { return Ok(false) };
        if !self.name_held_above(root) {
            return Ok(false);
        }
        // the base must be a box, not a flat value, at every step
        let mut base = value;
        while let TK::FieldGet { base: b, .. } = &base.kind {
            if self.is_flat(&self.concrete(&b.ty, subst)) {
                return Ok(false);
            }
            base = b;
        }
        for st in rest {
            if let TStmt::Bind { name: n, .. } = st {
                if n == name {
                    return Ok(false);
                }
            }
            if !borrows_only(stmt_value(st), name, &self.borrowed_params, &self.nonescaping_fn_params) {
                return Ok(false);
            }
        }
        let (v, held) = self.gen_borrowed(value, scope, subst, func)?;
        scope.insert(name.to_string(), Some(v));
        if held {
            self.cleanup.push((v, ty));
        } else {
            self.held_above.insert(name.to_string());
        }
        Ok(true)
    }

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

    /// Whether `e` names a borrowed parameter of the function being
    /// generated — a value an earlier frame holds, so one that outlives
    /// anything this frame does, including a tail call.
    fn borrowed_here(&self, e: &TExpr) -> bool {
        match &e.kind {
            TK::Local(name) => self.name_held_above(name),
            // a field of a held box, read without a count of its own
            TK::FieldGet { base, .. } => {
                !self.is_flat(&self.concrete(&base.ty, &[]))
                    && root_local(e).is_some_and(|n| self.name_held_above(n))
            }
            _ => false,
        }
    }

    fn name_held_above(&self, name: &str) -> bool {
        if self.held_above.contains(name) {
            return true;
        }
        let Some(f) = self.tmod.fns.get(self.cur_fn) else { return false };
        f.params
            .iter()
            .position(|(n, _)| n == name)
            .is_some_and(|i| self.borrowed_params[self.cur_fn][i])
    }

    fn flush_cleanup(&mut self) {
        let items = self.cleanup.clone();
        for (v, ty) in items.into_iter().rev() {
            self.emit_release(v, &ty);
        }
        let shells = self.shells.clone();
        for (cell, ty, ctor) in shells {
            self.free_shell(cell, &ty, ctor);
        }
    }

    /// Gives an emptied box back, where it was not written over: nothing to
    /// count and nothing inside, so straight to the pool — unless the box
    /// was shared after all and the cell is null.
    fn free_shell(&mut self, cell: PointerValue<'ctx>, ty: &Ty, ctor: usize) {
        let func = self.builder.get_insert_block().and_then(|b| b.get_parent()).expect("in a function");
        let Ty::Adt(adt, targs) = ty else { unreachable!("only an ADT box is emptied") };
        let struct_ty = self.ctor_struct(*adt, ctor, targs);
        let size = struct_ty.size_of().expect("sized struct");
        let some = self.builder.build_is_not_null(cell, "shell.some").expect("LLVM builder error");
        let free_bb = self.ctx.append_basic_block(func, "shell.free");
        let done_bb = self.ctx.append_basic_block(func, "shell.done");
        self.builder.build_conditional_branch(some, free_bb, done_bb).expect("LLVM builder error");
        self.builder.position_at_end(free_bb);
        self.emit_free(cell, size);
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
        self.builder.position_at_end(done_bb);
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
    /// round: `for` and `while` are functions calling themselves in tail
    /// position, and so is a strand that computes forever; without this it
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

    /// A `for` loop whose body calls nothing that loops: it runs a bounded
    /// number of times, each of them bounded, and then returns to a function
    /// that is checked, so it needs no check of its own — and a check inside
    /// it is a volatile load and a branch per element, which is what stood
    /// between a loop over ten million floats and the vector instructions it
    /// was written for. A callee counts as bounded when the call graph has no
    /// way back from it to itself: `upper_byte`, `is_lower`, a record
    /// accessor — the small functions LLVM will inline into the body, and
    /// exactly the ones whose check would have cost the vectorization. A
    /// callee that loops keeps the loop checked, and so does a closure or a
    /// method, whose body is not known here.
    ///
    /// A `while` is not a leaf, whatever it calls: nothing bounds it but its
    /// condition, and a `while` that waits on something another strand
    /// sets — on the same worker — would wait for it forever.
    fn leaf_loop(&mut self, fi: usize) -> bool {
        if let Some(&known) = self.leaf_loops.get(&fi) {
            return known;
        }
        let f = &self.tmod.fns[fi];
        let mut answer = f.name.contains(".for.go");
        if answer {
            let mut edges: Vec<usize> = Vec::new();
            let mut found = false;
            let mut methods = false;
            collect_calls(&f.body, &mut edges, &mut found, &mut methods);
            answer = !found && !methods && !calls_closure(&f.body);
            if answer {
                answer = edges.iter().all(|&e| e == fi || !self.loops_itself(e));
            }
        }
        self.leaf_loops.insert(fi, answer);
        answer
    }

    /// Can `fi` be reached again from inside `fi`? Memoised with the same
    /// sets `call_goes_round` builds.
    fn loops_itself(&mut self, fi: usize) -> bool {
        if !self.reaches.contains_key(&fi) {
            let mut seen: HashSet<usize> = HashSet::new();
            let mut queue = vec![fi];
            while let Some(i) = queue.pop() {
                for &e in &self.callgraph[i] {
                    if seen.insert(e) {
                        queue.push(e);
                    }
                }
            }
            self.reaches.insert(fi, seen);
        }
        self.reaches[&fi].contains(&fi)
    }

    /// Says, once per caller/callee pair, that a recursive call in tail
    /// position is not the jump it reads as.
    ///
    /// The shape this catches is the one that took the valley server down: a
    /// function whose arguments do not all fit the registers is quietly given
    /// the C convention (see `cc_for`), under which a tail call is a hint the
    /// optimizer may keep or drop — and whether it drops it turns on things as
    /// far away as a channel send in the body. A loop like that leaks frames
    /// only under load and only on native, which is the worst possible place
    /// to learn about it. The fix is always the same and is named here: carry
    /// the extra arguments as one tuple, which is one pointer.
    fn warn_untailed(&mut self, callee_idx: usize, line: usize) {
        let caller_idx = self.cur_fn;
        if !self.warned_untailed.insert((caller_idx, callee_idx)) {
            return;
        }
        let iregs: usize =
            std::env::var("RILL_CC_INT_REGS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
        let over = |i: usize| -> Option<String> {
            let f = &self.tmod.fns[i];
            let floats = f.params.iter().filter(|(_, t)| matches!(t, Ty::Float)).count();
            let ints = f.params.len() - floats;
            (ints > iregs).then(|| format!("`{}` carries {ints} non-float arguments", f.name))
        };
        let culprit = over(caller_idx)
            .or_else(|| over(callee_idx))
            .unwrap_or_else(|| "a signature in this loop does not fit the registers".to_string());
        eprintln!(
            "warning at line {line}: `{}` calls `{}` in tail position, and the call can come back \
             around — but it is not guaranteed to be a jump: {culprit}, and past {iregs} the \
             arguments spill to the stack, where the tail-call promise cannot be kept. A loop \
             through this call may grow the stack without bound. Pack the extra arguments into \
             one tuple (a tuple is one pointer).",
            self.tmod.fns[caller_idx].name, self.tmod.fns[callee_idx].name,
        );
    }

    fn emit_preempt_check(&mut self, func: FunctionValue<'ctx>) {
        if !self.preempt || self.leaf_loop(self.cur_fn) {
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
                    if self.is_heap(out_ty) {
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
        shim.set_linkage(inkwell::module::Linkage::Internal);
        // A shim is reached through a closure value, so it is called however
        // lambdas are called — one more integer argument than the type says,
        // for the environment pointer.
        if !self.wasm {
            if let Some(cc) = self.cc_for(&params, 1) {
                shim.set_call_conventions(cc);
            }
        }

        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(shim, "entry");
        self.builder.position_at_end(entry);
        let args: Vec<BasicMetadataValueEnum> =
            (1..shim.count_params()).map(|i| shim.get_nth_param(i).unwrap().into()).collect();
        let args = self.explode_args(args, &params);
        let call = self.builder.build_call(target, &args, "").expect("LLVM builder error");
        self.match_cc(call, target);
        // A closure call hands its arguments over owned; a borrowed parameter
        // of the target releases nothing, so the shim lets those go itself.
        let borrowed = self.borrowed_params[key.0].clone();
        for (i, pty) in params.iter().enumerate() {
            if borrowed.get(i) == Some(&true) && self.is_heap(pty) {
                self.emit_release(shim.get_nth_param(i as u32 + 1).unwrap(), pty);
            }
        }
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
    /// The function a helper thread runs for a blocking extern: takes the
    /// slot the arguments were stored in, makes the call, stores the answer
    /// after them. One per declaration.
    fn blocking_wrapper(
        &mut self,
        name: &str,
        sig: &rill_types::ExternSig,
        st: StructType<'ctx>,
    ) -> FunctionValue<'ctx> {
        let wname = format!("{name}$blocking");
        if let Some(f) = self.module.get_function(&wname) {
            return f;
        }
        let ptrt = self.ctx.ptr_type(Default::default());
        let w = self.module.add_function(&wname, self.ctx.void_type().fn_type(&[ptrt.into()], false), None);
        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(w, "entry");
        self.builder.position_at_end(entry);
        let slot = w.get_nth_param(0).unwrap().into_pointer_value();
        let mut args: Vec<BasicMetadataValueEnum> = Vec::new();
        for (i, c) in sig.params.iter().enumerate() {
            let at = self.builder.build_struct_gep(st, slot, i as u32, "a").expect("LLVM builder error");
            let ty = self.c_type(c).expect("a parameter is never Unit");
            args.push(self.builder.build_load(ty, at, "arg").expect("LLVM builder error").into());
        }
        let callee = self.module.get_function(name).expect("declared above");
        let call = self.builder.build_call(callee, &args, "").expect("LLVM builder error");
        if let inkwell::values::ValueKind::Basic(v) = call.try_as_basic_value() {
            let at = self
                .builder
                .build_struct_gep(st, slot, sig.params.len() as u32, "r")
                .expect("LLVM builder error");
            self.builder.build_store(at, v).expect("LLVM builder error");
        }
        self.builder.build_return(None).expect("LLVM builder error");
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        w
    }

    /// The trampoline C calls for a callback whose `Fn` has an `Env` slot:
    /// one per C signature, since it learns the closure at run time from
    /// the box the `Env` slot brings back — code and environment, two
    /// words — and calls it the way every closure is called. What C hands
    /// over is widened to Rill's types on the way in and narrowed on the
    /// way out, as for a named callback.
    fn gen_closure_trampoline(&mut self, cparams: &[CTy], cret: &CTy) -> FunctionValue<'ctx> {
        let key = (cparams.to_vec(), cret.clone());
        if let Some(&v) = self.closure_tramps.get(&key) {
            return v;
        }
        let ps: Vec<BasicMetadataTypeEnum> = cparams
            .iter()
            .map(|c| self.c_type(c).expect("a callback parameter is never Unit").into())
            .collect();
        let fty = match self.c_type(cret) {
            Some(r) => r.fn_type(&ps, false),
            None => self.ctx.void_type().fn_type(&ps, false),
        };
        let name = format!("closure$c{}", self.closure_tramps.len());
        let tramp = self.module.add_function(&name, fty, None);
        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(tramp, "entry");
        self.builder.position_at_end(entry);
        let slot = cparams.iter().position(|c| matches!(c, CTy::Env)).expect("an `Env` slot");
        let bx = tramp.get_nth_param(slot as u32).unwrap().into_pointer_value();
        let cv = self
            .builder
            .build_load(self.closure_type(), bx, "closure")
            .expect("builder")
            .into_struct_value();
        let code = self.builder.build_extract_value(cv, 0, "code").expect("builder").into_pointer_value();
        let env = self.builder.build_extract_value(cv, 1, "env").expect("builder");
        let rparams: Vec<Ty> =
            cparams.iter().filter(|c| !matches!(c, CTy::Env)).map(|c| c.rill_ty()).collect();
        let rret = cret.rill_ty();
        let mut args: Vec<BasicMetadataValueEnum> = vec![env.into()];
        for (i, c) in cparams.iter().enumerate() {
            if matches!(c, CTy::Env) {
                continue;
            }
            args.push(self.from_c(tramp.get_nth_param(i as u32).unwrap(), c).into());
        }
        let fn_ty = self.fn_llvm_type(&rparams, &rret, true);
        let call = self.builder.build_indirect_call(fn_ty, code, &args, "ccall").expect("builder");
        if !self.wasm {
            if let Some(cc) = self.cc_for(&rparams, 1) {
                call.set_call_convention(cc);
            }
        }
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) if self.c_type(cret).is_some() => {
                let narrowed = self.to_c(v, cret);
                self.builder.build_return(Some(&narrowed)).expect("builder");
            }
            _ => {
                self.builder.build_return(None).expect("builder");
            }
        }
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        self.closure_tramps.insert(key, tramp);
        tramp
    }

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
                 and C has nowhere to put it — unless the signature says where: \
                 an `Env` parameter on the `extern`, and an `Env` slot in the \
                 callback's `Fn(...)`, is the user pointer most such APIs take"
                    .to_string(),
            ));
        };
        let key = (fn_idx, inner.clone(), cparams.to_vec(), cret.clone());
        if let Some(&v) = self.callbacks.get(&key) {
            return Ok(v);
        }

        let f = &self.tmod.fns[fn_idx];
        let ret = subst_gen(&f.ret, &inner);
        if self.is_heap(&ret) {
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
        // C calls this one, so this one is C. The Rill function behind it is
        // called the way Rill functions are.
        let tramp = self.module.add_function(&name, fty, None);

        let saved = self.builder.get_insert_block();
        let entry = self.ctx.append_basic_block(tramp, "entry");
        self.builder.position_at_end(entry);
        let args: Vec<BasicMetadataValueEnum> = cparams
            .iter()
            .enumerate()
            .map(|(i, c)| self.from_c(tramp.get_nth_param(i as u32).unwrap(), c).into())
            .collect();
        let args = self.explode_args(args, &self.instance_params(fn_idx, &inner));
        let call = self.builder.build_call(target, &args, "").expect("LLVM builder error");
        self.match_cc(call, target);
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
        const WAITERS: [&str; 23] = [
            "rill_spawn",
            "rill_wait_signal",
            "rill_chan_close",
            "rill_chan_wait",
            "rill_chan_taken",
            "rill_file_sync",
            "rill_dir_sync",
            "rill_sleep_ms",
            "rill_sock_wait",
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
            "rill_sock_flush",
            "rill_sock_close",
            "rill_yield",
            "rill_call_blocking",
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
        //
        // `m` and `c` are the exception, and not by kindness: wasi-libc is one
        // archive with the maths in it, so `sqrt` and `strlen` come from the
        // same `libc.a` the module already links. A program that asks for
        // `-lm` on a Mac is asking for something wasm gives it anyway.
        if let Some(lib) =
            self.tmod.libs.iter().find(|l| !l.starts_with('-') && !matches!(l.as_str(), "m" | "c"))
        {
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

    /// A library hands its host counted values as handles, and the host has
    /// to be able to let one go: for every counted type that crosses an
    /// export's boundary, `rill_drop_<Type>(handle)` releases the host's
    /// reference. The name is the type's, with the module's dot made an
    /// underscore — `rill_drop_g_Grid` for a `Grid` imported as `g`.
    fn gen_drop_exports(&mut self) {
        let mut seen: Vec<Ty> = Vec::new();
        for f in self.tmod.fns.iter().filter(|f| f.exported) {
            for t in f.params.iter().map(|(_, t)| t).chain(std::iter::once(&f.ret)) {
                if self.is_heap(t) && !seen.contains(t) {
                    seen.push(t.clone());
                }
            }
        }
        let ptrt = self.ctx.ptr_type(Default::default());
        for ty in seen {
            let name = format!("rill_drop_{}", mangle(&ty, &self.tmod.adts).replace('.', "_"));
            let f = self.module.add_function(&name, self.ctx.void_type().fn_type(&[ptrt.into()], false), None);
            let entry = self.ctx.append_basic_block(f, "entry");
            self.builder.position_at_end(entry);
            let raw = f.get_nth_param(0).unwrap();
            let v: BasicValueEnum<'ctx> = match self.llvm_type(&ty) {
                Some(t) if t.is_pointer_type() => raw,
                _ => raw,
            };
            self.emit_release(v, &ty);
            self.builder.build_return(None).expect("LLVM builder error");
        }
    }

    /// The real C `main`: runs `rill_main` as the main strand under the
    /// scheduler, then reports allocations and returns 0.
    fn gen_main_wrapper(&mut self) {
        // A module with exports is a reactor: it stays instantiated and the
        // host calls in, so there is no `_start`, no argv, and no moment at
        // which an allocation report would mean anything. `main` becomes the
        // module's `init` — the host calls it once before anything else, and
        // what it sets up stays live for the exports to use.
        if (self.wasm || self.shared) && self.tmod.fns.iter().any(|f| f.exported) {
            let void = self.ctx.void_type();
            let init = self.module.add_function("rill_init", void.fn_type(&[], false), None);
            let entry = self.ctx.append_basic_block(init, "entry");
            self.builder.position_at_end(entry);
            let user_main = self.instances[&(self.tmod.fn_index["main"], vec![], vec![])];
            // A shared library that spawns gets the scheduler in host mode:
            // its workers are started here, and the host's thread — the one
            // that calls the exports — pumps them whenever it waits on a
            // channel. On wasm there is no thread to be a worker.
            let entry = if self.shared && self.host_strands { "rill_rt_host" } else { "rill_rt_run_direct" };
            let rt_run = self.module.get_function(entry).unwrap();
            self.builder
                .build_call(rt_run, &[user_main.as_global_value().as_pointer_value().into()], "")
                .expect("LLVM builder error");
            self.builder.build_return(None).expect("LLVM builder error");
            if self.shared {
                self.gen_drop_exports();
                self.gen_host_wrappers();
            }
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
        self.call_rt("rill_flush_out", &[]);
        self.call_rt("rill_alloc_report", &[]);
        self.builder.build_return(Some(&i32t.const_int(0, false))).expect("LLVM builder error");
    }

    /// Every function with a body, its start and its name, in a table the
    /// runtime reads when the program crashes: a stripped binary then names
    /// its frames as a debug one would. Two pointers a function — a few
    /// tens of kilobytes in a big program — and the names are the ones
    /// `--debug` would give lldb.
    fn gen_sym_table(&mut self) {
        let ptrt = self.ctx.ptr_type(Default::default());
        let i64t = self.ctx.i64_type();
        let entry_ty = self.ctx.struct_type(&[ptrt.into(), ptrt.into()], false);
        // One more function, empty, after every other: where the program's
        // code ends. The runtime's own functions follow it in the binary,
        // and a frame past it is not a Rill frame, whatever the nearest
        // start before it says. Its entry has no name, which is how the
        // runtime knows it.
        let end = self.module.add_function("rill.end", self.ctx.void_type().fn_type(&[], false), None);
        end.set_linkage(inkwell::module::Linkage::Internal);
        let block = self.ctx.append_basic_block(end, "entry");
        self.builder.position_at_end(block);
        self.builder.build_unreachable().expect("LLVM builder error");
        let fns: Vec<FunctionValue<'ctx>> =
            self.module.get_functions().filter(|f| f.count_basic_blocks() > 0).collect();
        // And a frame pointer in every one of them, so that the runtime can
        // walk the stack at a crash by the chain of saved frame pointers,
        // through its own frames and a signal's: one register on x86-64,
        // what the ABI keeps anyway on arm64.
        let mut entries = Vec::with_capacity(fns.len());
        for (i, f) in fns.iter().enumerate() {
            f.add_attribute(AttributeLoc::Function, self.ctx.create_string_attribute("frame-pointer", "all"));
            let name = if *f == end { &b""[..] } else { f.get_name().to_bytes() };
            let text = self.ctx.const_string(name, true);
            let g = self.module.add_global(text.get_type(), None, &format!("rill_sym.{i}"));
            g.set_initializer(&text);
            g.set_constant(true);
            g.set_linkage(inkwell::module::Linkage::Private);
            entries.push(entry_ty.const_named_struct(&[
                f.as_global_value().as_pointer_value().into(),
                g.as_pointer_value().into(),
            ]));
        }
        let table = entry_ty.const_array(&entries);
        let syms = self.module.add_global(table.get_type(), None, "rill_syms");
        syms.set_initializer(&table);
        syms.set_constant(true);
        let n = self.module.add_global(i64t, None, "rill_nsyms");
        n.set_initializer(&i64t.const_int(entries.len() as u64, false));
        n.set_constant(true);
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
        let callee = self.request_instance(fn_idx, targs.clone(), vec![]);
        let args = self.explode_args(args.to_vec(), &self.instance_params(fn_idx, &targs));
        let call = self.builder.build_call(callee, &args, name).expect("LLVM builder error");
        self.match_cc(call, callee);
        call
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
        if e.line > 0 {
            self.note_line = e.line;
        }
        // Inside a branch, a move would happen on one path only.
        if matches!(e.kind, TK::If { .. } | TK::Match { .. } | TK::Select { .. }) {
            let saved = std::mem::replace(&mut self.move_ok, false);
            let out = self.gen_expr_inner(e, scope, subst, func);
            self.move_ok = saved;
            return out;
        }
        self.gen_expr_inner(e, scope, subst, func)
    }

    /// The pool and the set for one top-level statement of a function body.
    fn begin_statement(&mut self, value: &TExpr, rest: &[TStmt]) {
        if self.move_ok {
            self.root_free = free_once(value, rest);
            let mut spine: Vec<&str> = Vec::new();
            spine_args(value, &mut spine);
            self.movable =
                spine.into_iter().filter(|n| self.root_free.contains(*n)).map(String::from).collect();
        } else {
            self.root_free.clear();
            self.movable.clear();
        }
    }

    fn gen_expr_inner(
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
            // `poke_*` and `peek_*`: a store and a load at `p + i * width`,
            // unaligned because the runtime's were. They were calls into the
            // runtime, which for a program writing a vertex buffer is one
            // call per float — and the whole of what a wasm module does is
            // write buffers the host will read.
            TK::Builtin { rt, args }
                if rt.starts_with("rill_poke_") || rt.starts_with("rill_peek_") =>
            {
                let ctx = self.ctx;
                let (width, elem): (u64, BasicTypeEnum<'ctx>) = match &rt[10..] {
                    "i8" | "u8" => (1, ctx.i8_type().into()),
                    "i16" | "u16" => (2, ctx.i16_type().into()),
                    "i32" | "u32" => (4, ctx.i32_type().into()),
                    "i64" => (8, ctx.i64_type().into()),
                    "f32" => (4, ctx.f32_type().into()),
                    "f64" => (8, ctx.f64_type().into()),
                    "ptr" => (
                        self.ptr_bytes(),
                        ctx.ptr_type(Default::default()).into(),
                    ),
                    other => unreachable!("unknown raw-memory builtin width {other}"),
                };
                // The same decay as everywhere else a `Ptr` is wanted, and
                // the same reason to hold the buffer until the access is over.
                let (p, keep) = if let TK::BufPtr { buf } = &args[0].kind {
                    let (b, held) = self.gen_borrowed(buf, scope, subst, func)?;
                    let data = self.buf_data(b.into_pointer_value());
                    (data, if held { Some((b, self.concrete(&buf.ty, subst))) } else { None })
                } else {
                    (
                        self.gen_expr(&args[0], scope, subst, func)?
                            .expect("value")
                            .into_pointer_value(),
                        None,
                    )
                };
                let i = self
                    .gen_expr(&args[1], scope, subst, func)?
                    .expect("value")
                    .into_int_value();
                let off = self
                    .builder
                    .build_int_mul(i, ctx.i64_type().const_int(width, false), "off")
                    .expect("LLVM builder error");
                let slot = unsafe { self.builder.build_gep(ctx.i8_type(), p, &[off], "slot") }
                    .expect("LLVM builder error");
                if rt.starts_with("rill_poke_") {
                    let v = self.gen_expr(&args[2], scope, subst, func)?.expect("value");
                    // The runtime narrowed on the way in; so does this.
                    let narrowed: BasicValueEnum<'ctx> = match elem {
                        BasicTypeEnum::IntType(t) => self
                            .builder
                            .build_int_truncate_or_bit_cast(v.into_int_value(), t, "n")
                            .expect("LLVM builder error")
                            .into(),
                        // Only `f32` narrows; `f64` is already the width a
                        // `Float` is carried in.
                        BasicTypeEnum::FloatType(t) if width == 4 => self
                            .builder
                            .build_float_trunc(v.into_float_value(), t, "n")
                            .expect("LLVM builder error")
                            .into(),
                        _ => v,
                    };
                    let st = self.builder.build_store(slot, narrowed).expect("LLVM builder error");
                    st.set_alignment(1).expect("store takes an alignment");
                    if let Some((b, ty)) = keep {
                        self.emit_release(b, &ty);
                    }
                    return Ok(None);
                }
                let got = self.builder.build_load(elem, slot, "peek").expect("LLVM builder error");
                got.as_instruction_value()
                    .expect("a load is an instruction")
                    .set_alignment(1)
                    .expect("load takes an alignment");
                // And widened on the way back, signed or not as the name says.
                let out: BasicValueEnum<'ctx> = match elem {
                    BasicTypeEnum::IntType(_) if rt.starts_with("rill_peek_u") => self
                        .builder
                        .build_int_z_extend_or_bit_cast(got.into_int_value(), ctx.i64_type(), "w")
                        .expect("LLVM builder error")
                        .into(),
                    BasicTypeEnum::IntType(_) => self
                        .builder
                        .build_int_s_extend_or_bit_cast(got.into_int_value(), ctx.i64_type(), "w")
                        .expect("LLVM builder error")
                        .into(),
                    BasicTypeEnum::FloatType(_) if width == 4 => self
                        .builder
                        .build_float_ext(got.into_float_value(), ctx.f64_type(), "w")
                        .expect("LLVM builder error")
                        .into(),
                    _ => got,
                };
                if let Some((b, ty)) = keep {
                    self.emit_release(b, &ty);
                }
                Ok(Some(out))
            }
            // `#s` and `s[i]`. A string is `{ i64 rc, i64 len, bytes… }`, so
            // both are a load — but as runtime calls they were among the two
            // most expensive things in a program that reads text, because a
            // call cannot be hoisted out of a loop and a load can. The bounds
            // test is unsigned, which folds `i < 0` into the same compare, and
            // it picks the address rather than branching on it: the loaded
            // byte is thrown away when the index is out of range.
            // `b += int_to_str(n)`: the digits go straight into the builder
            // rather than through a string that exists only to be copied.
            TK::Builtin { rt, args }
                if *rt == "rill_strbuf_add"
                    && matches!(&args[1].kind, TK::Builtin { rt: inner, .. } if *inner == "rill_int_to_str") =>
            {
                let TK::Builtin { args: inner, .. } = &args[1].kind else { unreachable!() };
                let (b, held) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let n = self.gen_expr(&inner[0], scope, subst, func)?.expect("value");
                self.gen_strbuf_int(b.into_pointer_value(), n.into_int_value(), func);
                if held {
                    self.emit_release(b, &Ty::StrBuf);
                }
                Ok(None)
            }
            // `b += s`: the room is taken and the bytes are copied here. A
            // literal knows its own length, so that copy is a handful of
            // stores rather than a call into the runtime with a width it has
            // to load and branch on. A page built out of static runs is
            // mostly this.
            TK::Builtin { rt, args } if *rt == "rill_strbuf_add" => {
                let fixed = match &args[1].kind {
                    TK::Str(lit) => Some(lit.len()),
                    _ => None,
                };
                let (b, held) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let (sv, kept) = self.gen_borrowed(&args[1], scope, subst, func)?;
                if fixed != Some(0) {
                    self.gen_strbuf_str(
                        b.into_pointer_value(),
                        sv.into_pointer_value(),
                        fixed,
                        func,
                    );
                }
                if kept {
                    self.emit_release(sv, &Ty::Str);
                }
                if held {
                    self.emit_release(b, &Ty::StrBuf);
                }
                Ok(None)
            }
            // `b += byte`: one store, and the length that follows it.
            TK::Builtin { rt, args } if *rt == "rill_strbuf_byte" => {
                let (b, held) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let c = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_int_value();
                let i64t = self.ctx.i64_type();
                let i8t = self.ctx.i8_type();
                let bp = b.into_pointer_value();
                let one = i64t.const_int(1, false);
                let (data, len) = self.sb_room(bp, one, func);
                let at = unsafe {
                    self.builder
                        .build_in_bounds_gep(i8t, data, &[len], "sb.at")
                        .expect("LLVM builder error")
                };
                let byte = self.builder.build_int_truncate(c, i8t, "byte").expect("LLVM builder error");
                let put = self.builder.build_store(at, byte).expect("LLVM builder error");
                self.mark_data(Some(put));
                self.sb_wrote(bp, len, one);
                if held {
                    self.emit_release(b, &Ty::StrBuf);
                }
                Ok(None)
            }
            // The one test a `for` over a buffer makes: that the whole range
            // it is about to walk is inside. The length never changes after
            // the buffer is made, so the load hoists; the call is left for
            // the case that ends the program, and is laid down out of the way.
            TK::Builtin { rt, args } if *rt == "rill_buf_check" => {
                let (b, held) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let lo = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_int_value();
                let hi = self.gen_expr(&args[2], scope, subst, func)?.expect("value").into_int_value();
                let bp = b.into_pointer_value();
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, lo, hi, "check.empty")
                    .expect("LLVM builder error");
                let look_bb = self.ctx.append_basic_block(func, "check.look");
                let bad_bb = self.ctx.append_basic_block(func, "check.bad");
                let done_bb = self.ctx.append_basic_block(func, "check.ok");
                self.builder
                    .build_conditional_branch(empty, done_bb, look_bb)
                    .expect("LLVM builder error");
                self.builder.position_at_end(look_bb);
                let len = self.buf_len(bp);
                let under = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, lo, self.ctx.i64_type().const_zero(), "check.lo")
                    .expect("LLVM builder error");
                let over = self
                    .builder
                    .build_int_compare(IntPredicate::SGT, hi, len, "check.hi")
                    .expect("LLVM builder error");
                let bad = self.builder.build_or(under, over, "check.bad?").expect("LLVM builder error");
                self.builder
                    .build_conditional_branch(bad, bad_bb, done_bb)
                    .expect("LLVM builder error");
                self.builder.position_at_end(bad_bb);
                let f = self.module.get_function("rill_buf_check").unwrap();
                self.builder
                    .build_call(f, &[b.into(), lo.into(), hi.into()], "")
                    .expect("LLVM builder error");
                self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
                self.builder.position_at_end(done_bb);
                if held {
                    let ty = self.concrete(&args[0].ty, subst);
                    self.emit_release(b, &ty);
                }
                Ok(None)
            }
            // `s[i]` in a `for i in 0..#s`: every index is in range by
            // construction, so the byte and nothing else.
            TK::Builtin { rt, args } if *rt == "rill_str_at" => {
                let (s, owned) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let s = s.into_pointer_value();
                let i = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_int_value();
                let i64t = self.ctx.i64_type();
                let b = &self.builder;
                let off = b.build_int_add(i, i64t.const_int(16, false), "off").expect("LLVM builder error");
                let at = unsafe { b.build_gep(self.ctx.i8_type(), s, &[off], "at") }.expect("LLVM builder error");
                let byte = b.build_load(self.ctx.i8_type(), at, "byte").expect("LLVM builder error").into_int_value();
                let wide = b.build_int_z_extend(byte, i64t, "chr").expect("LLVM builder error");
                if owned {
                    self.emit_release(s.into(), &Ty::Str);
                }
                Ok(Some(wide.into()))
            }
            TK::Builtin { rt, args } if (*rt == "rill_str_len" || *rt == "rill_str_get") => {
                let (s, owned) = self.gen_borrowed(&args[0], scope, subst, func)?;
                let s = s.into_pointer_value();
                let i64t = self.ctx.i64_type();
                let b = &self.builder;
                let lenp = unsafe { b.build_gep(self.ctx.i8_type(), s, &[i64t.const_int(8, false)], "lenp") }
                    .expect("LLVM builder error");
                let len_load = b.build_load(i64t, lenp, "strlen").expect("LLVM builder error");
                // A plain load, not an `invariant.load`, for the reason
                // given at `buf_len`: the mark let `str_len(f())` be read
                // after the string was released, and a string beyond the
                // pool has its header written over by `free`.
                let len = len_load.into_int_value();
                let out: BasicValueEnum<'ctx> = if *rt == "rill_str_len" {
                    len.into()
                } else {
                    let i = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_int_value();
                    let b = &self.builder;
                    let inb = b
                        .build_int_compare(inkwell::IntPredicate::ULT, i, len, "inb")
                        .expect("LLVM builder error");
                    let off = b.build_int_add(i, i64t.const_int(16, false), "off").expect("LLVM builder error");
                    let at = unsafe { b.build_gep(self.ctx.i8_type(), s, &[off], "at") }
                        .expect("LLVM builder error");
                    // `s` itself is always readable, so it stands in for an
                    // address that must not be dereferenced.
                    let safe = b.build_select(inb, at, s, "safe").expect("LLVM builder error");
                    let byte = b
                        .build_load(self.ctx.i8_type(), safe.into_pointer_value(), "byte")
                        .expect("LLVM builder error")
                        .into_int_value();
                    let wide = b.build_int_z_extend(byte, i64t, "chr").expect("LLVM builder error");
                    b.build_select(inb, wide, i64t.const_int(-1i64 as u64, true), "get")
                        .expect("LLVM builder error")
                };
                if owned {
                    self.emit_release(s.into(), &Ty::Str);
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
                    // Truncates toward zero, like C — and past the ends of
                    // `Int`, or not a number, gives the nearest end, or
                    // zero. C leaves those undefined, and the machine gave
                    // 1 for both.
                    let sat = inkwell::intrinsics::Intrinsic::find("llvm.fptosi.sat")
                        .and_then(|i| {
                            i.get_declaration(
                                &self.module,
                                &[self.ctx.i64_type().into(), self.ctx.f64_type().into()],
                            )
                        })
                        .expect("llvm.fptosi.sat");
                    let call = self
                        .builder
                        .build_call(sat, &[v.into()], "toint")
                        .expect("LLVM builder error");
                    match call.try_as_basic_value() {
                        inkwell::values::ValueKind::Basic(out) => out,
                        _ => unreachable!("fptosi.sat answers an integer"),
                    }
                };
                Ok(Some(out))
            }
            TK::Builtin { rt, args } if rt.starts_with("rill.f2") => {
                let i64t = self.ctx.i64_type();
                let f64t = self.ctx.f64_type();
                match *rt {
                    "rill.f2" => {
                        let a = self.gen_expr(&args[0], scope, subst, func)?.expect("value").into_float_value();
                        let b = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_float_value();
                        Ok(Some(self.make_float2(a, b)))
                    }
                    "rill.f2splat" => {
                        let a = self.gen_expr(&args[0], scope, subst, func)?.expect("value").into_float_value();
                        Ok(Some(self.make_float2(a, a)))
                    }
                    "rill.f2lane0" | "rill.f2lane1" => {
                        let v = self.gen_expr(&args[0], scope, subst, func)?.expect("value");
                        let (a, b) = self.lanes(v);
                        Ok(Some(if *rt == "rill.f2lane0" { a.into() } else { b.into() }))
                    }
                    "rill.f2fma" => {
                        let a = self.gen_expr(&args[0], scope, subst, func)?.expect("value");
                        let b = self.gen_expr(&args[1], scope, subst, func)?.expect("value");
                        let c = self.gen_expr(&args[2], scope, subst, func)?.expect("value");
                        let v2 = f64t.vec_type(2);
                        let fma = inkwell::intrinsics::Intrinsic::find("llvm.fma")
                            .and_then(|i| i.get_declaration(&self.module, &[v2.into()]))
                            .expect("llvm.fma on a vector");
                        let call = self
                            .builder
                            .build_call(fma, &[a.into(), b.into(), c.into()], "fma2")
                            .expect("LLVM builder error");
                        Ok(match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => Some(v),
                            _ => unreachable!("fma returns a value"),
                        })
                    }
                    // The smaller and the larger of each lane. `llvm.minnum`
                    // and `llvm.maxnum` are IEEE 754's `minNum`/`maxNum`:
                    // given a NaN and a number they answer the number, which
                    // is what `if a < b then a else b` already does — a NaN
                    // compares false and never wins — so a reduction written
                    // with these agrees with the scalar loop it replaces.
                    // Neither is a loosening: nothing is reassociated and no
                    // rounding changes, which is why min and max may have
                    // lanes where a sum has to be written out by hand.
                    "rill.f2min" | "rill.f2max" => {
                        let a = self.gen_expr(&args[0], scope, subst, func)?.expect("value");
                        let b = self.gen_expr(&args[1], scope, subst, func)?.expect("value");
                        let v2 = f64t.vec_type(2);
                        let name =
                            if *rt == "rill.f2min" { "llvm.minnum" } else { "llvm.maxnum" };
                        let f = inkwell::intrinsics::Intrinsic::find(name)
                            .and_then(|i| i.get_declaration(&self.module, &[v2.into()]))
                            .expect("llvm.minnum/maxnum on a vector");
                        let call = self
                            .builder
                            .build_call(f, &[a.into(), b.into()], "f2minmax")
                            .expect("LLVM builder error");
                        Ok(match call.try_as_basic_value() {
                            inkwell::values::ValueKind::Basic(v) => Some(v),
                            _ => unreachable!("minnum/maxnum returns a value"),
                        })
                    }
                    // `b[i]` and `b[i + 1]` as one register, and back. One
                    // bounds check covers both elements; `--unchecked`
                    // drops it as it drops every other.
                    "rill.f2load" | "rill.f2store" => {
                        let storing = *rt == "rill.f2store";
                        let (bv, owned) = self.gen_borrowed(&args[0], scope, subst, func)?;
                        let buf = bv.into_pointer_value();
                        let idx = self.gen_expr(&args[1], scope, subst, func)?.expect("value").into_int_value();
                        if !self.unchecked {
                            // Both lanes in range: `i` and `i + 1` below the
                            // length, as unsigned, which catches a negative
                            // `i` as well.
                            let len = self.buf_len(buf);
                            let next = self
                                .builder
                                .build_int_add(idx, i64t.const_int(1, false), "i1")
                                .expect("LLVM builder error");
                            let ok0 = self
                                .builder
                                .build_int_compare(IntPredicate::ULT, idx, len, "in0")
                                .expect("LLVM builder error");
                            let ok1 = self
                                .builder
                                .build_int_compare(IntPredicate::ULT, next, len, "in1")
                                .expect("LLVM builder error");
                            let ok = self.builder.build_and(ok0, ok1, "in2").expect("LLVM builder error");
                            let good = self.ctx.append_basic_block(func, "f2.ok");
                            let bad = self.ctx.append_basic_block(func, "f2.oob");
                            self.builder.build_conditional_branch(ok, good, bad).expect("LLVM builder error");
                            self.builder.position_at_end(bad);
                            let oob = self.module.get_function("rill_buf_oob").unwrap();
                            let which = self
                                .builder
                                .build_select(ok0, next, idx, "which")
                                .expect("LLVM builder error");
                            self.builder.build_call(oob, &[which.into(), len.into()], "").expect("LLVM builder error");
                            self.builder.build_unreachable().expect("LLVM builder error");
                            self.builder.position_at_end(good);
                        }
                        let at = unsafe { self.builder.build_gep(f64t, buf, &[idx], "f2.at") }
                            .expect("LLVM builder error");
                        let out = if storing {
                            let v = self.gen_expr(&args[2], scope, subst, func)?.expect("value");
                            let st = self.builder.build_store(at, v).expect("LLVM builder error");
                            st.set_alignment(8).expect("alignment");
                            None
                        } else {
                            let ld = self
                                .builder
                                .build_load(f64t.vec_type(2), at, "f2.ld")
                                .expect("LLVM builder error");
                            if let Some(i) = ld.as_instruction_value() {
                                i.set_alignment(8).expect("alignment");
                            }
                            Some(ld)
                        };
                        if owned {
                            self.emit_release(bv, &Ty::Buf(CTy::F64));
                        }
                        Ok(out)
                    }
                    other => unreachable!("unknown lane intrinsic {other}"),
                }
            }
            TK::Builtin { rt, args } if rt.starts_with("rill.") => {
                let mut vals = Vec::new();
                for a in args {
                    vals.push(
                        self.gen_expr(a, scope, subst, func)?.expect("value").into_int_value(),
                    );
                }
                let i64t = self.ctx.i64_type();
                let b = &self.builder;
                let out = match *rt {
                    "rill.and" => b.build_and(vals[0], vals[1], "and"),
                    "rill.or" => b.build_or(vals[0], vals[1], "or"),
                    "rill.xor" => b.build_xor(vals[0], vals[1], "xor"),
                    "rill.not" => b.build_not(vals[0], "not"),
                    // A shift by 64 or more, or by less than nothing, is
                    // every bit gone: zero. The machine keeps only the low
                    // six bits of the count and LLVM calls the rest poison,
                    // and neither is an answer a program can use — so the
                    // count is masked for the machine and the answer chosen
                    // by the count as written. `shr` is logical: protocol
                    // code works with bit patterns, not signed magnitudes.
                    "rill.shl" | "rill.shr" => {
                        let n = vals[1];
                        let low = b.build_and(n, i64t.const_int(63, false), "sh.n").expect("builder");
                        let raw = if *rt == "rill.shl" {
                            b.build_left_shift(vals[0], low, "shl")
                        } else {
                            b.build_right_shift(vals[0], low, false, "shr")
                        }
                        .expect("builder");
                        let fits = b
                            .build_int_compare(IntPredicate::ULT, n, i64t.const_int(64, false), "sh.fits")
                            .expect("builder");
                        b.build_select(fits, raw, i64t.const_zero(), "sh").map(|v| v.into_int_value())
                    }
                    other => unreachable!("unknown intrinsic {other}"),
                };
                Ok(Some(out.expect("LLVM builder error").into()))
            }
            TK::Builtin { rt, args } => {
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::new();
                let mut owned: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
                for a in args {
                    let ty = self.concrete(&a.ty, subst);
                    // A buffer written where a `Ptr` is wanted goes in as its
                    // first element's address, and the buffer itself has to
                    // outlive the call — so one made for this call is released
                    // after it rather than before. That is the difference
                    // between an address into memory the callee is reading and
                    // an address into a free list.
                    if let TK::BufPtr { buf } = &a.kind {
                        let (b, held) = self.gen_borrowed(buf, scope, subst, func)?;
                        let data = self.buf_data(b.into_pointer_value());
                        argv.push(data.into());
                        if held {
                            owned.push((b, self.concrete(&buf.ty, subst)));
                        }
                        continue;
                    }
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
                    if held && self.is_heap(&ty) {
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
                if *fn_idx == self.cur_fn {
                    self.saw_nontail_self = true;
                }
                if let Some(v) = self.try_fuse(e, scope, subst, func)? {
                    self.note("this pipeline is one loop: no list is built".to_string());
                    return Ok(Some(v));
                }
                if let Some(v) = self.try_map_or(e, scope, subst, func)? {
                    self.note("`map_or` reads the table without building a `Some`".to_string());
                    return Ok(Some(v));
                }
                if let Some(v) = self.try_map_list(e, scope, subst, func)? {
                    self.note("`map` walks the list in place, writing each link over the one it read".to_string());
                    return Ok(Some(v));
                }
                let inner: Vec<Ty> = inner.iter().map(|t| self.concrete(t, subst)).collect();
                let bound = self.bind_static_args(*fn_idx, args, subst, e.line)?;
                let callee = self.request_instance(*fn_idx, inner.clone(), bound);
                let borrowed = self.borrowed_params[*fn_idx].clone();
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                // A value made for a borrowed position has no owner but this
                // frame, which lets it go once the call is over. A local goes
                // in as it stands: the frame holds it for the duration.
                let mut held: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
                let nonescaping = self.nonescaping_fn_params[*fn_idx].clone();
                for (i, a) in args.iter().enumerate() {
                    if borrowed.get(i) == Some(&true) {
                        let (v, owned) = self.gen_borrowed(a, scope, subst, func)?;
                        if owned {
                            held.push((v, self.concrete(&a.ty, subst)));
                        }
                        argv.push(v.into());
                    } else {
                        // The last word this function has for a binding of
                        // its own: the reference goes with the call, and the
                        // callee is the one holder.
                        if let TK::Local(n) = &a.kind {
                            if self.movable.contains(n) {
                                if let Some(v) = self.take_owned(a, scope) {
                                    self.note(format!("`{n}` is handed to the call: its last use, so the reference goes with it and no count is taken"));
                                    argv.push(v.into());
                                    continue;
                                }
                            }
                        }
                        // A lambda written here for a parameter that never
                        // escapes borrows its captures: this frame holds them
                        // for the whole call. Not in tail position — there the
                        // frame lets go before the jump.
                        if nonescaping.get(i) == Some(&true) && matches!(a.kind, TK::Lambda { .. }) {
                            self.borrow_captures = true;
                        }
                        if self.explain {
                            if let TK::Local(n) = &a.kind {
                                if self.is_heap(&self.concrete(&a.ty, subst)) {
                                    self.note(format!("`{n}` is retained for the call: it is spoken again later, or this call is not on the statement's own straight path"));
                                }
                            }
                        }
                        let v = self.gen_expr(a, scope, subst, func)?;
                        self.borrow_captures = false;
                        argv.push(v.expect("arguments are never Unit").into());
                    }
                }
                let argv = self.explode_args(argv, &self.instance_params(*fn_idx, &inner));
                let call =
                    self.builder.build_call(callee, &argv, "call").expect("LLVM builder error");
                self.match_cc(call, callee);
                for (v, ty) in held {
                    self.emit_release(v, &ty);
                }
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
                let p = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => unreachable!("rill_buf_new returns a pointer"),
                };
                // The runtime has just written this same length into the
                // header; writing it again here, where LLVM can see it, is
                // what lets the optimizer prove `i < #v` for a loop whose
                // bound it can relate to `n` — and drop the check, and then
                // vectorize the loop, which the check's extra exit forbade.
                // A negative `n` never gets here: the runtime stops on it.
                let i64t = self.ctx.i64_type();
                let len_ptr = unsafe {
                    self.builder
                        .build_gep(
                            i64t,
                            p.into_pointer_value(),
                            &[i64t.const_int(-1i64 as u64, true)],
                            "len.ptr",
                        )
                        .expect("LLVM builder error")
                };
                self.builder.build_store(len_ptr, n).expect("LLVM builder error");
                Ok(Some(p))
            }
            TK::BufLen { buf } => {
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let len = self.buf_len(b.into_pointer_value());
                if owned {
                    self.emit_release(b, &self.concrete(&buf.ty, subst));
                }
                Ok(Some(len.into()))
            }
            TK::Unique { val } => {
                // Borrowed for the look, so the count read is the count as
                // held. A compile-time constant's count is zero and never
                // one; a flat value has no count and is never asked.
                let vty = self.concrete(&val.ty, subst);
                let (v, owned) = self.gen_borrowed(val, scope, subst, func)?;
                let i64t = self.ctx.i64_type();
                let p = match &vty {
                    Ty::Buf(_) => self.buf_rc_ptr(v.into_pointer_value()),
                    _ => v.into_pointer_value(),
                };
                let rc = self.builder.build_load(i64t, p, "rc").expect("LLVM builder error").into_int_value();
                let one = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, rc, i64t.const_int(1, false), "unique")
                    .expect("LLVM builder error");
                if owned {
                    self.emit_release(v, &vty);
                }
                Ok(Some(one.into()))
            }
            TK::BufPtr { buf } => {
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let data = self.buf_data(b.into_pointer_value());
                if owned {
                    self.emit_release(b, &self.concrete(&buf.ty, subst));
                }
                Ok(Some(data.into()))
            }
            TK::BufGet { buf, idx, unchecked } => {
                let bty = self.concrete(&buf.ty, subst);
                let w = match &bty {
                    Ty::Buf(w) => w.clone(),
                    _ => unreachable!("checked as a buffer"),
                };
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let i = self.gen_expr(idx, scope, subst, func)?.expect("value").into_int_value();
                let slot = self.buf_slot_at(b.into_pointer_value(), i, &w, func, *unchecked);
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
            TK::BufSet { buf, idx, val, unchecked } => {
                let bty = self.concrete(&buf.ty, subst);
                let w = match &bty {
                    Ty::Buf(w) => w.clone(),
                    _ => unreachable!("checked as a buffer"),
                };
                let (b, owned) = self.gen_borrowed(buf, scope, subst, func)?;
                let i = self.gen_expr(idx, scope, subst, func)?.expect("value").into_int_value();
                let v = self.gen_expr(val, scope, subst, func)?.expect("value");
                let slot = self.buf_slot_at(b.into_pointer_value(), i, &w, func, *unchecked);
                let narrowed = self.to_c(v, &w);
                self.builder.build_store(slot, narrowed).expect("LLVM builder error");
                if owned {
                    self.emit_release(b, &bty);
                }
                Ok(None)
            }
            // One field of a box, by the position its name has. The box is
            // borrowed — a name in scope is not given up by being read — and
            // what comes out is retained, since the caller owns what it holds.
            TK::FieldNamed { .. } => unreachable!("`zonk` names every deferred field"),
            TK::UpdateNamed { .. } => unreachable!("`zonk` builds every deferred update"),
            TK::FieldGet { base, adt, ctor, index } => {
                let bty = self.concrete(&base.ty, subst);
                let targs = match &bty {
                    Ty::Adt(_, targs) => targs.clone(),
                    other => unreachable!("field of {other:?}"),
                };
                let (b, owned) = self.gen_borrowed(base, scope, subst, func)?;
                if self.is_flat(&bty) {
                    return Ok(Some(self.extract_field(b, *index as u32)));
                }
                let struct_ty = self.ctor_struct(*adt, *ctor, &targs);
                let fty = subst_gen(&self.tmod.adts[*adt].ctors[*ctor].fields[*index], &targs);
                let fp = self
                    .builder
                    .build_struct_gep(struct_ty, b.into_pointer_value(), (*index + 2) as u32, "f")
                    .expect("LLVM builder error");
                let v = self
                    .builder
                    .build_load(self.llvm_type(&fty).expect("a field is never Unit"), fp, "fv")
                    .expect("LLVM builder error");
                self.emit_retain(v, &fty);
                if owned {
                    self.emit_release(b, &bty);
                }
                Ok(Some(v))
            }
            TK::MapNew => {
                let (kty, vty) = {
                    let mty = self.concrete(&e.ty, subst);
                    self.map_kv(&mty, e.line)?
                };
                let vt = self.map_vtable(&kty, &vty, e.line)?;
                let f = self.module.get_function("rill_map_new").unwrap();
                let call = self.builder.build_call(f, &[vt.into()], "map").expect("builder");
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => unreachable!("rill_map_new returns a pointer"),
                })
            }
            // The key and the value are moved into the table, exactly as a
            // value sent down a channel is: what the frame owned, the table
            // owns now.
            TK::MapSet { map, key, val } => {
                let mty = self.concrete(&map.ty, subst);
                let (kty, vty) = self.map_kv(&mty, e.line)?;
                let (m, owned) = self.gen_borrowed(map, scope, subst, func)?;
                let kv = self.gen_expr(key, scope, subst, func)?.expect("value");
                let vv = self.gen_expr(val, scope, subst, func)?.expect("value");
                let (k0, _) = self.pack_words(kv, &kty);
                let (v0, v1) = self.pack_words(vv, &vty);
                self.call_rt("rill_map_set", &[m.into(), k0.into(), v0.into(), v1.into()]);
                if owned {
                    self.emit_release(m, &mty);
                }
                Ok(None)
            }
            // A lookup only borrows its key, so one that was made for the
            // call is released after it.
            TK::MapFind { map, key } | TK::MapDel { map, key } => {
                let deleting = matches!(&e.kind, TK::MapDel { .. });
                let mty = self.concrete(&map.ty, subst);
                let (kty, _) = self.map_kv(&mty, e.line)?;
                let (m, owned) = self.gen_borrowed(map, scope, subst, func)?;
                let (kv, kowned) = self.gen_borrowed(key, scope, subst, func)?;
                let (k0, _) = self.pack_words(kv, &kty);
                let rt = if deleting { "rill_map_del" } else { "rill_map_find" };
                let call = self.call_rt(rt, &[m.into(), k0.into()]);
                let raw = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
                    _ => unreachable!("both answer with a number"),
                };
                if kowned {
                    self.emit_release(kv, &kty);
                }
                // A flat key was boxed to be looked up with; the box was ours.
                if self.is_flat(&kty) {
                    self.release_flat_box(k0, &kty);
                }
                if owned {
                    self.emit_release(m, &mty);
                }
                if !deleting {
                    return Ok(Some(raw.into()));
                }
                let out = self
                    .builder
                    .build_int_compare(
                        IntPredicate::NE,
                        raw,
                        self.ctx.i8_type().const_zero(),
                        "was",
                    )
                    .expect("LLVM builder error");
                Ok(Some(out.into()))
            }
            TK::MapLen { map } | TK::MapClear { map } => {
                let clearing = matches!(&e.kind, TK::MapClear { .. });
                let mty = self.concrete(&map.ty, subst);
                let (m, owned) = self.gen_borrowed(map, scope, subst, func)?;
                let call = self
                    .call_rt(if clearing { "rill_map_clear" } else { "rill_map_len" }, &[m.into()]);
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                if owned {
                    self.emit_release(m, &mty);
                }
                Ok(out)
            }
            TK::MapNext { map, from: idx }
            | TK::MapKeyAt { map, idx }
            | TK::MapValAt { map, idx } => {
                let mty = self.concrete(&map.ty, subst);
                let (kty, vty) = self.map_kv(&mty, e.line)?;
                let (m, owned) = self.gen_borrowed(map, scope, subst, func)?;
                let i = self.gen_expr(idx, scope, subst, func)?.expect("value");
                let rt = match &e.kind {
                    TK::MapNext { .. } => "rill_map_next",
                    TK::MapKeyAt { .. } => "rill_map_key_at",
                    _ => "rill_map_val_at",
                };
                let call = self.call_rt(rt, &[m.into(), i.into()]);
                let got = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => unreachable!("all three answer with something"),
                };
                // A slot number is a number; a key or a value comes back in
                // the same two words everything else travels in, and comes
                // back owned — the table retained it before letting go.
                let out = if matches!(&e.kind, TK::MapNext { .. }) {
                    got
                } else if let TK::MapKeyAt { .. } = &e.kind {
                    // A key is one word by construction: what may be a key is
                    // what has structural equality, and a closure — the only
                    // value needing two — has none.
                    let w = got.into_int_value();
                    let k = self.unpack_words(w, self.ctx.i64_type().const_zero(), &kty);
                    if self.is_flat(&kty) {
                        self.release_flat_box(w, &kty);
                    }
                    k
                } else {
                    let words = got.into_struct_value();
                    let w0 = self
                        .builder
                        .build_extract_value(words, 0, "w0")
                        .expect("LLVM builder error")
                        .into_int_value();
                    let w1 = self
                        .builder
                        .build_extract_value(words, 1, "w1")
                        .expect("LLVM builder error")
                        .into_int_value();
                    let v = self.unpack_words(w0, w1, &vty);
                    if self.is_flat(&vty) {
                        self.release_flat_box(w0, &vty);
                    }
                    v
                };
                if owned {
                    self.emit_release(m, &mty);
                }
                Ok(Some(out))
            }
            TK::CallExtern { name, args } => {
                // Two things wear this node. A C call has a symbol to reach;
                // an `asm fn` has a run of instructions to place here instead,
                // and which one it is is settled by the table the name is in.
                let asm = self.tmod.asms.get(name).cloned();
                let sig = match &asm {
                    Some(a) => rill_types::ExternSig {
                        params: a.params.clone(),
                        ret: a.ret.clone(),
                        blocking: false,
                    },
                    None => self.tmod.externs[name].clone(),
                };
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                let mut owned: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
                // A callback whose `Fn` has an `Env` slot may be any closure:
                // the closure — code and environment, two words — is put in
                // a box, the box's address goes out by the `Env` parameter
                // and comes back into the slot, and a trampoline made for
                // the C signature unpacks it and calls the closure. The box
                // and the frame's hold on the closure last the call.
                let carried = sig.params.iter().position(|p| {
                    matches!(p, CTy::Fn(cps, _) if cps.iter().any(|c| matches!(c, CTy::Env)))
                });
                let mut env_box: Option<(PointerValue<'ctx>, BasicValueEnum<'ctx>, bool, Ty)> = None;
                if let Some(pi) = carried {
                    let ai = sig.params[..pi].iter().filter(|p| !matches!(p, CTy::Env)).count();
                    let a = &args[ai];
                    let cty = self.concrete(&a.ty, subst);
                    let (cv, held) = self.gen_borrowed(a, scope, subst, func)?;
                    let alloc = self.module.get_function("rill_alloc_bytes").unwrap();
                    let size = self.ctx.i64_type().const_int(16, false);
                    let call = self.builder.build_call(alloc, &[size.into()], "envbox").expect("builder");
                    let bx = match call.try_as_basic_value() {
                        inkwell::values::ValueKind::Basic(v) => v.into_pointer_value(),
                        _ => unreachable!("rill_alloc_bytes answers a pointer"),
                    };
                    self.builder.build_store(bx, cv).expect("builder");
                    env_box = Some((bx, cv, held, cty));
                }
                let mut ai = 0;
                for pt in &sig.params {
                    if let CTy::Env = pt {
                        let (bx, ..) = env_box.as_ref().expect("an `Env` parameter has its callback");
                        argv.push((*bx).into());
                        continue;
                    }
                    let a = &args[ai];
                    ai += 1;
                    // A callback does not evaluate to a value C can hold: the
                    // named function is wrapped in a C-ABI trampoline and it
                    // is the trampoline's address that crosses.
                    if let CTy::Fn(cps, cret) = pt {
                        let trampoline = if cps.iter().any(|c| matches!(c, CTy::Env)) {
                            self.gen_closure_trampoline(cps, cret)
                        } else {
                            self.gen_callback(a, cps, cret, subst)?
                        };
                        argv.push(trampoline.as_global_value().as_pointer_value().into());
                        continue;
                    }
                    // A buffer decays to its address here, and stays alive
                    // until C is done with it: released after the call, not
                    // before. `strlen(buf_u8(8))` is the shape that needs it.
                    if let TK::BufPtr { buf } = &a.kind {
                        let (b, held) = self.gen_borrowed(buf, scope, subst, func)?;
                        let data = self.buf_data(b.into_pointer_value());
                        argv.push(data.into());
                        if held {
                            owned.push((b, self.concrete(&buf.ty, subst)));
                        }
                        continue;
                    }
                    let v = self.gen_expr(a, scope, subst, func)?.expect("value");
                    argv.push(self.to_c(v, pt).into());
                }
                let call = match &asm {
                    Some(a) => {
                        let ps: Vec<BasicMetadataTypeEnum> = sig
                            .params
                            .iter()
                            .map(|c| self.c_type(c).expect("a parameter is never Unit").into())
                            .collect();
                        let fn_ty = match self.c_type(&sig.ret) {
                            Some(t) => t.fn_type(&ps, false),
                            None => self.ctx.void_type().fn_type(&ps, false),
                        };
                        // Side effects are always assumed. A kernel's `cli` is
                        // not there for its result, and an instruction dropped
                        // because nothing read it is the kind of bug this
                        // language should not be able to write.
                        let ptr = self.ctx.create_inline_asm(
                            fn_ty,
                            a.template.clone(),
                            a.constraints.clone(),
                            true,
                            false,
                            Some(inkwell::InlineAsmDialect::ATT),
                            false,
                        );
                        self.builder
                            .build_indirect_call(fn_ty, ptr, &argv, "asm")
                            .expect("LLVM builder error")
                    }
                    None if sig.blocking => {
                        // A call that blocks is made off the worker: the
                        // arguments go into a slot on this frame, a wrapper
                        // made once for the declaration makes the call from
                        // the slot, and the runtime runs the wrapper on a
                        // helper thread while this strand is parked. The
                        // answer comes back through the same slot.
                        let mut fields: Vec<BasicTypeEnum> = sig
                            .params
                            .iter()
                            .map(|c| self.c_type(c).expect("a parameter is never Unit"))
                            .collect();
                        if let Some(r) = self.c_type(&sig.ret) {
                            fields.push(r);
                        }
                        let st = self.ctx.struct_type(&fields, false);
                        let slot = self.entry_alloca(func, st, "blk");
                        for (i, a) in argv.iter().enumerate() {
                            let v: BasicValueEnum = (*a).try_into().expect("a C argument is a value");
                            let at = self
                                .builder
                                .build_struct_gep(st, slot, i as u32, "blk.arg")
                                .expect("LLVM builder error");
                            self.builder.build_store(at, v).expect("LLVM builder error");
                        }
                        let wrapper = self.blocking_wrapper(name, &sig, st);
                        let run = self.module.get_function("rill_call_blocking").unwrap();
                        self.builder
                            .build_call(
                                run,
                                &[wrapper.as_global_value().as_pointer_value().into(), slot.into()],
                                "",
                            )
                            .expect("LLVM builder error");
                        let mut out = None;
                        if let Some(r) = self.c_type(&sig.ret) {
                            let at = self
                                .builder
                                .build_struct_gep(st, slot, sig.params.len() as u32, "blk.ret")
                                .expect("LLVM builder error");
                            out = Some(self.builder.build_load(r, at, "blk.out").expect("LLVM builder error"));
                        }
                        for (v, ty) in owned {
                            self.emit_release(v, &ty);
                        }
                        return Ok(out.map(|v| self.from_c(v, &sig.ret)));
                    }
                    None => {
                        let callee = match self.libm_intrinsic(name, &sig) {
                            Some(f) => f,
                            None => self.module.get_function(name).expect("declared above"),
                        };
                        self.builder.build_call(callee, &argv, "cffi").expect("LLVM builder error")
                    }
                };
                if let Some((bx, cv, held, cty)) = env_box {
                    let free = self.module.get_function("rill_free_ptr").unwrap();
                    self.builder.build_call(free, &[bx.into()], "").expect("builder");
                    if held {
                        self.emit_release(cv, &cty);
                    }
                }
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                for (v, ty) in owned {
                    self.emit_release(v, &ty);
                }
                Ok(out.map(|v| self.from_c(v, &sig.ret)))
            }
            TK::MethodCall { trait_id, method, recv, args } => {
                let recv_ty = self.concrete(recv, subst);
                let fn_idx = self.resolve_impl(*trait_id, *method, recv, subst, e.line)?;
                let borrowed = self.borrowed_params[fn_idx].clone();
                let mut argv: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                let mut held: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    if borrowed.get(i) == Some(&true) {
                        let (v, owned) = self.gen_borrowed(a, scope, subst, func)?;
                        if owned {
                            held.push((v, self.concrete(&a.ty, subst)));
                        }
                        argv.push(v.into());
                        continue;
                    }
                    let v = self.gen_expr(a, scope, subst, func)?;
                    argv.push(v.expect("arguments are never Unit").into());
                }
                let call = self.call_impl(fn_idx, &recv_ty, &argv, "mcall");
                for (v, ty) in held {
                    self.emit_release(v, &ty);
                }
                Ok(match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                })
            }
            TK::CallCtor { adt, ctor, targs, args } => {
                self.gen_ctor(*adt, *ctor, targs, args, scope, subst, func, false)
            }
            TK::CallClosure { callee, args } => {
                let callee_ty = self.concrete(&callee.ty, subst);
                let (params, ret) = match &callee_ty {
                    Ty::Fn(p, r) => (p.clone(), (**r).clone()),
                    other => unreachable!("closure call on non-fn type {other:?}"),
                };
                // A closure named here is held by this frame for the length
                // of the call, so it is not retained for it — that was two
                // runtime calls around every `f(x)` a `map` makes.
                let (cval, held) = self.gen_borrowed(callee, scope, subst, func)?;
                let cv = cval.into_struct_value();
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
                // Every closure target — a lambda, or a shim over a named
                // function — is made with the same rule over the same types,
                // so an indirect call computing it again lands on the same
                // convention.
                if !self.wasm {
                    if let Some(cc) = self.cc_for(&params, 1) {
                        call.set_call_convention(cc);
                    }
                }
                let out = match call.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Some(v),
                    _ => None,
                };
                // A callee made for this call is owned here and let go now;
                // the call itself only borrowed it.
                if held {
                    self.emit_release(cv.into(), &callee_ty);
                }
                Ok(out)
            }
            TK::Println(arg) => {
                let ty = self.concrete(&arg.ty, subst);
                let v = self.gen_expr(arg, scope, subst, func)?;
                let v = v.expect("println argument is never Unit");
                if matches!(ty, Ty::Adt(..) | Ty::Float2) {
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
                if self.is_heap(&ty) {
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
                if self.is_heap(&ty) {
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

                // Each branch binds for itself, as in tail position.
                self.builder.position_at_end(then_bb);
                let mut then_scope = scope.clone();
                let tv = self.gen_expr(then, &mut then_scope, subst, func)?;
                let t_end = self.builder.get_insert_block().unwrap();

                self.builder.position_at_end(else_bb);
                let mut else_scope = scope.clone();
                let ev = self.gen_expr(els, &mut else_scope, subst, func)?;
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
                let saved_held_block = self.held_above.clone();
                // And what it binds is not in scope after it, by name either:
                // a `z` bound in one branch of an `if`, or an `x` bound again
                // there, is the outer one again once the branch is over.
                let saved_scope = scope.clone();
                // What this block binds is released when the block ends, not
                // at the function's exit: a block that is one branch of an
                // `if` exists on one path only, and a release at the exit of
                // something made on one path is not even valid code. The one
                // exception is the block's own value, which leaves with it.
                // What the frame held when the block began, by identity:
                // a block may hand one of those to a call it makes (a move
                // takes it out of the list), so the block's own bindings
                // are whatever is left that was not there before.
                let outer_held: Vec<BasicValueEnum<'ctx>> =
                    self.cleanup.iter().map(|(v, _)| *v).collect();
                let outer_movable = std::mem::take(&mut self.movable);
                for (i, stmt) in stmts.iter().enumerate() {
                    // A block nested in a statement draws on the statement's
                    // pool: a name spoken once in the whole statement, on
                    // this block's own unconditional path, and not again in
                    // the rest of this block.
                    self.movable = if self.move_ok {
                        let rest = &stmts[i + 1..];
                        let mut spine: Vec<&str> = Vec::new();
                        spine_args(stmt_value(stmt), &mut spine);
                        spine
                            .into_iter()
                            .filter(|n| self.root_free.contains(*n))
                            .filter(|n| !rest.iter().any(|st| names_local(stmt_value(st), n)))
                            .map(String::from)
                            .collect()
                    } else {
                        HashSet::new()
                    };
                    match stmt {
                        TStmt::Bind { name, value } if self.try_bind_borrowed(name, value, &stmts[i + 1..], scope, subst, func)? => {
                            last = None;
                        }
                        TStmt::Bind { name, value } => {
                            let v = match self.take_builder(value, &stmts[i + 1..], scope) {
                                Some(v) => Some(v),
                                None => self.gen_expr(value, scope, subst, func)?,
                            };
                            scope.insert(name.clone(), v);
                            self.rebind_known_fn(name, value, subst);
                            // The binding owns this reference; released at
                            // function exit.
                            if let Some(v) = v {
                                let ty = self.concrete(&value.ty, subst);
                                if self.is_heap(&ty) {
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
                                    if self.is_heap(&ty) {
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
                self.held_above = saved_held_block;
                *scope = saved_scope;
                self.movable = outer_movable;
                let (kept, own): (Vec<_>, Vec<_>) = std::mem::take(&mut self.cleanup)
                    .into_iter()
                    .partition(|(v, _)| outer_held.contains(v));
                self.cleanup = kept;
                for (v, ty) in own.into_iter().rev() {
                    if last == Some(v) {
                        continue;
                    }
                    self.emit_release(v, &ty);
                }
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
                if self.is_flat(&elem_ty) {
                    self.release_flat_box(a, &elem_ty);
                }
                self.emit_release(cv, &ch_ty);
                Ok(Some(out))
            }
            TK::ChanOp { ch, op } => {
                let ch_ty = self.concrete(&ch.ty, subst);
                let cv = self.gen_expr(ch, scope, subst, func)?.expect("value");
                let rt = match op {
                    rill_types::ChanOp::Close => "rill_chan_close",
                    rill_types::ChanOp::Closed => "rill_chan_closed",
                    rill_types::ChanOp::Wait => "rill_chan_wait",
                };
                let f = self.module.get_function(rt).unwrap();
                let call =
                    self.builder.build_call(f, &[cv.into()], "chanop").expect("LLVM builder error");
                self.emit_release(cv, &ch_ty);
                Ok(match call.try_as_basic_value() {
                    // `closed` and `chan_wait` answer a byte at the ABI, as
                    // every bool-returning runtime call does.
                    inkwell::values::ValueKind::Basic(v) => Some(
                        self.builder
                            .build_int_truncate(
                                v.into_int_value(),
                                self.ctx.bool_type(),
                                "b",
                            )
                            .expect("LLVM builder error")
                            .into(),
                    ),
                    _ => None,
                })
            }
            TK::ChanTaken { ch } => {
                let ch_ty = self.concrete(&ch.ty, subst);
                let elem_ty = self.concrete(&e.ty, subst);
                let cv = self.gen_expr(ch, scope, subst, func)?.expect("value");
                let f = self.module.get_function("rill_chan_taken").unwrap();
                let call = self.builder.build_call(f, &[cv.into()], "taken").expect("builder");
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
                if self.is_flat(&elem_ty) {
                    self.release_flat_box(a, &elem_ty);
                }
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
        // Padded out to sixteen so that an append may copy whole words past
        // the end of the text without reading past the end of the object.
        // The header still says how long the string is, so nothing else in
        // the language can tell.
        let mut raw = s.as_bytes().to_vec();
        while raw.len() % 16 != 0 || raw.is_empty() {
            raw.push(0);
        }
        let bytes = self.ctx.const_string(&raw, false);
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

    /// `/` and `%` on integers, with the two cases the machine has no one
    /// answer for decided. A divisor of zero ends the program, saying
    /// where: the machine's answer was a wrong number with no sign it was
    /// wrong. A divisor of `-1` is done as a negation, since the smallest
    /// integer over `-1` is the one quotient a machine may trap on, and so
    /// wraps like every other overflow — to itself, with a remainder of
    /// zero. The `-1` is decided by a select rather than a branch: two
    /// compares and two selects on a path that already waits for a divide.
    fn int_divide(&self, op: BinOp, l: IntValue<'ctx>, r: IntValue<'ctx>) -> IntValue<'ctx> {
        let b = &self.builder;
        let i64t = self.ctx.i64_type();
        let func = b.get_insert_block().unwrap().get_parent().unwrap();
        let zero = i64t.const_zero();
        let by_zero = b.build_int_compare(IntPredicate::EQ, r, zero, "div.zero").expect("builder");
        let bad = self.ctx.append_basic_block(func, "div.by0");
        let good = self.ctx.append_basic_block(func, "div.ok");
        b.build_conditional_branch(by_zero, bad, good).expect("builder");
        b.position_at_end(bad);
        let die = self.module.get_function("rill_div_zero").unwrap();
        b.build_call(die, &[], "").expect("builder");
        b.build_unreachable().expect("builder");
        b.position_at_end(good);
        let minus_one = i64t.const_all_ones();
        let by_m1 = b.build_int_compare(IntPredicate::EQ, r, minus_one, "div.m1").expect("builder");
        let one = i64t.const_int(1, false);
        let safe = b.build_select(by_m1, one, r, "div.safe").expect("builder").into_int_value();
        match op {
            BinOp::Div => {
                let q = b.build_int_signed_div(l, safe, "div").expect("builder");
                let neg = b.build_int_neg(l, "div.neg").expect("builder");
                b.build_select(by_m1, neg, q, "div.out").expect("builder").into_int_value()
            }
            _ => {
                let m = b.build_int_signed_rem(l, safe, "rem").expect("builder");
                b.build_select(by_m1, zero, m, "rem.out").expect("builder").into_int_value()
            }
        }
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
            let raw = self.gen_eq_of(l, r, operand_ty, line)?;
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
                    BinOp::Div | BinOp::Mod => self.int_divide(op, l, r).into(),
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
                    // `contract`, and nothing else. It lets the backend fold a
                    // multiply and the add that consumes it into one fused
                    // instruction, which rounds once instead of twice — the
                    // result is nearer the true one, not further, but it is
                    // not the result two separate roundings would give. C, C++
                    // and Go do this by default; Rust and Zig do not. Nothing
                    // else is loosened: no reassociation, no assumptions about
                    // NaN or infinity, no reciprocals.
                    BinOp::Add => Self::contract(b.build_float_add(l, r, "add").unwrap()).into(),
                    BinOp::Sub => Self::contract(b.build_float_sub(l, r, "sub").unwrap()).into(),
                    BinOp::Mul => Self::contract(b.build_float_mul(l, r, "mul").unwrap()).into(),
                    BinOp::Div => b.build_float_div(l, r, "div").unwrap().into(),
                    cmp => {
                        let pred = match cmp {
                            BinOp::Eq => FloatPredicate::OEQ,
                            // A NaN is equal to nothing, itself included, so
                            // `x == x` is false for one and `x != x` is
                            // true — which is how C, Python and IEEE 754
                            // have it, and how a program tests for one.
                            BinOp::Ne => FloatPredicate::UNE,
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
            // Both lanes at once, the same way and with the same one
            // loosening as a single `Float`. Equality is of both lanes.
            Ty::Float2 => {
                let (l, r) = (l.into_vector_value(), r.into_vector_value());
                let contract = |v: inkwell::values::VectorValue<'ctx>| {
                    if let Some(i) = v.as_instruction() {
                        let _ = i.set_fast_math_flags(inkwell::values::FastMathFlags::AllowContract);
                    }
                    v
                };
                match op {
                    BinOp::Add => contract(b.build_float_add(l, r, "add").unwrap()).into(),
                    BinOp::Sub => contract(b.build_float_sub(l, r, "sub").unwrap()).into(),
                    BinOp::Mul => contract(b.build_float_mul(l, r, "mul").unwrap()).into(),
                    BinOp::Div => b.build_float_div(l, r, "div").unwrap().into(),
                    BinOp::Eq | BinOp::Ne => {
                        let eq = self.gen_eq_of(l.into(), r.into(), &Ty::Float2, line)?;
                        if op == BinOp::Eq {
                            eq.into()
                        } else {
                            self.builder.build_not(eq, "ne").unwrap().into()
                        }
                    }
                    _ => unreachable!("checker restricts Float2 operators"),
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
        if self.explain {
            if captures.is_empty() {
                self.note("a closure over nothing".to_string());
            } else {
                let caps: Vec<String> = captures.iter().map(|(n, _)| format!("`{n}`")).collect();
                self.note(format!("a closure is allocated, holding {}", caps.join(", ")));
            }
        }
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
        // The flag is for this closure alone, not for any written inside it.
        let borrowed_caps = std::mem::take(&mut self.borrow_captures);
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
        lifted.set_linkage(inkwell::module::Linkage::Internal);
        // A `spawn` body is started by the scheduler and keeps the C
        // convention; a lambda is called by Rill through a closure value.
        if !self.wasm && !discard_result {
            if let Some(cc) = self.cc_for(&param_tys, 1) {
                lifted.set_call_conventions(cc);
            }
        }

        let saved = self.builder.get_insert_block();
        let outer_cleanup = std::mem::take(&mut self.cleanup);
        let outer_shells = std::mem::take(&mut self.shells);
        let outer_movable = std::mem::take(&mut self.movable);
        let outer_free = std::mem::take(&mut self.root_free);
        let outer_move_ok = std::mem::replace(&mut self.move_ok, true);
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
            if self.is_heap(&param_tys[i]) {
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
        self.shells = outer_shells;
        self.movable = outer_movable;
        self.root_free = outer_free;
        self.move_ok = outer_move_ok;

        // The environment's drop function: decrement; at zero release heap
        // captures and free.
        let env_drop = if captures.is_empty() {
            None
        } else {
            let drop_ty = self.ctx.void_type().fn_type(&[ptrt.into()], false);
            let d = self.module.add_function(&format!("dropenv${id}"), drop_ty, None);
            d.set_linkage(inkwell::module::Linkage::Internal);
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
                if self.is_heap(cty) && !borrowed_caps {
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
            self.emit_free(p, env_size);
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
                if !borrowed_caps {
                    self.emit_retain(v, &cap_tys[i]);
                }
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

    /// A slot in the function's frame, taken in the entry block.
    ///
    /// Where an `alloca` goes is not a matter of taste. LLVM folds the ones in
    /// the entry block into the frame layout and gives them a fixed offset,
    /// which costs nothing and happens once however many times control comes
    /// back round; anywhere else it is a real adjustment of the stack pointer,
    /// executed every time it is reached. Inside a loop that is a leak — and
    /// worse, a function holding a dynamically sized `alloca` cannot have its
    /// frame popped before a call, so LLVM quietly drops the `tail` marker and
    /// a Rill loop written as tail recursion stops being a loop.
    fn entry_alloca(
        &self,
        func: FunctionValue<'ctx>,
        ty: impl BasicType<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        let b = self.ctx.create_builder();
        let entry = func.get_first_basic_block().expect("every function has an entry block");
        match entry.get_first_instruction() {
            Some(first) => b.position_before(&first),
            None => b.position_at_end(entry),
        }
        b.build_alloca(ty, name).expect("LLVM builder error")
    }

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
        // Both of these are a fixed size — the arms are known here — so they
        // belong in the entry block, and a `select` inside a loop then costs
        // the frame nothing and leaves the loop a loop. Written as array
        // allocas where they are first needed, one `select` in a tail
        // recursive function leaked a hundred and sixty bytes of stack every
        // time round, and thirty-two megabytes later the strand ran off the
        // bottom of its stack into the next mapping's guard page.
        let table = self.entry_alloca(func, case_ty.array_type(n as u32), "cases");

        // Evaluate every channel (and send value) once, in source order.
        let mut chans: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
        let mut sends: Vec<Option<(BasicValueEnum<'ctx>, Ty)>> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            // A receive arm that binds is a different case from one that
            // does not, and the runtime has to be able to tell them apart: a
            // closed channel makes both ready, and only one of them then has
            // a value it is expected to produce.
            let (kind, ch_expr, val) = match &arm.op {
                TSelectOp::Recv { chan, .. } => {
                    (if arm.bind.is_some() { 2u64 } else { 0u64 }, Some(chan), None)
                }
                TSelectOp::Send { chan, value } => (1u64, Some(chan), Some(value)),
                // A timeout waits on no channel at all: the case carries the
                // milliseconds where a send would have carried its value, and
                // a null where the others carry a channel.
                TSelectOp::Timeout { .. } => (3u64, None, None),
            };
            let ch = match ch_expr {
                Some(e) => {
                    let ch_ty = self.concrete(&e.ty, subst);
                    let v = self.gen_expr(e, scope, subst, func)?.expect("channel value");
                    chans.push((v, ch_ty));
                    v
                }
                None => ptrt.const_null().into(),
            };

            let (wa, wb, sent) = match val {
                Some(v) => {
                    let vty = self.concrete(&v.ty, subst);
                    let vv = self.gen_expr(v, scope, subst, func)?.expect("value");
                    let (a, b) = self.pack_words(vv, &vty);
                    (a, b, Some((vv, vty)))
                }
                None => match &arm.op {
                    TSelectOp::Timeout { ms } => {
                        let n = self
                            .gen_expr(ms, scope, subst, func)?
                            .expect("value")
                            .into_int_value();
                        (n, i64t.const_zero(), None)
                    }
                    _ => (i64t.const_zero(), i64t.const_zero(), None),
                },
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

        let out = self.entry_alloca(func, i64t.array_type(2), "sel.out");
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
            if !self.is_heap(ty) {
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
            // What the frame already owed before this arm bound anything, by
            // identity. Whatever is left over at the end and was not there
            // before belongs to this arm and goes when the arm does — the
            // same rule a block's own bindings follow, and for the same
            // reason.
            let outer_held: Vec<BasicValueEnum<'ctx>> =
                self.cleanup.iter().map(|(v, _)| *v).collect();
            if let Some((name, ty)) = &arm.bind {
                let ty = self.concrete(ty, subst);
                let v = self.unpack_words(got_a, got_b, &ty);
                if self.is_flat(&ty) {
                    self.release_flat_box(got_a, &ty);
                }
                arm_scope.insert(name.clone(), Some(v));
                self.bound_fns.remove(name);
                // The value comes out of the channel owned — the channel has
                // given up its reference — and an arm that only reads it hands
                // that ownership to nobody. Without this the reference is
                // dropped on the floor: `x = recv(ch) -> str_len(x)` leaked
                // one string for every message it received, for as long as the
                // loop around it ran. An arm that hands the value on instead
                // moves it, which takes the entry back out of this list, so
                // nothing is released twice.
                if self.is_heap(&ty) {
                    self.cleanup.push((v, ty));
                }
            }
            let v = self.gen_expr(&arm.body, &mut arm_scope, subst, func)?;
            let (kept, own): (Vec<_>, Vec<_>) = std::mem::take(&mut self.cleanup)
                .into_iter()
                .partition(|(c, _)| outer_held.contains(c));
            self.cleanup = kept;
            for (c, ty) in own.into_iter().rev() {
                // Except the arm's own answer, which leaves with the `select`.
                if v == Some(c) {
                    continue;
                }
                self.emit_release(c, &ty);
            }
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

    /// Hands back a box to build in, if this arm left one behind that is the
    /// right shape for it. The count decides at run time: at one, this frame
    /// holds the only reference, so the box's own references to its fields are
    /// let go — exactly as its drop would have done — and the memory is kept
    /// rather than handed back to the pool for the very next allocation to ask
    /// for it again. Above one, the box is still someone else's; the count
    /// comes down by one and a fresh box is taken.
    ///
    /// Reversing a list, which is what `map`, `filter`, `split` and `reverse`
    /// all end in, allocates nothing at all this way: every link is built over
    /// the link it was read from.
    /// Empties a box this frame owns, as an arm takes it apart — see the
    /// note in `compile_match`. `occ` is where the box sits under the
    /// match's root, so that the arm's names for its fields can be found;
    /// a field that is itself matched by a constructor pattern is emptied
    /// in turn, all the way down, so that `Mul(Num(y), z)` leaves the frame
    /// holding `z`, a `Num` shell and a `Mul` shell. Answers whether the
    /// box was emptied at all.
    #[allow(clippy::too_many_arguments)]
    fn empty_box(
        &mut self,
        sv: BasicValueEnum<'ctx>,
        ty: &Ty,
        pat: &Pattern,
        occ: Occ,
        binds: &[(String, Occ)],
        arm_scope: &mut Scope<'ctx>,
        func: FunctionValue<'ctx>,
    ) -> bool {
        // Not through an `@`: the name is the box, and the arm can say it.
        let Pattern::Ctor { name: pname, args: pats, .. } = pat else { return false };
        if pats.is_empty() || !self.is_heap(ty) || self.is_flat(ty) {
            return false;
        }
        let Some(&(_, ctor)) = self.tmod.ctors.get(pname) else { return false };
        let Some(at) = self.cleanup.iter().position(|(c, _)| *c == sv) else { return false };
        let Ty::Adt(adt, targs) = ty else { return false };
        let (adt, targs) = (*adt, targs.clone());
        let struct_ty = self.ctor_struct(adt, ctor, &targs);
        let fields: Vec<Ty> =
            self.tmod.adts[adt].ctors[ctor].fields.iter().map(|f| subst_gen(f, &targs)).collect();
        let sp = sv.into_pointer_value();
        let i64t = self.ctx.i64_type();
        self.cleanup.remove(at);
        // Every counted field, whether the arm names it or not.
        let mut loaded: Vec<(u32, BasicValueEnum<'ctx>, Ty)> = Vec::new();
        for (fi, fty) in fields.iter().enumerate() {
            if !self.is_heap(fty) {
                continue;
            }
            let fp = self
                .builder
                .build_struct_gep(struct_ty, sp, (fi + 2) as u32, "field")
                .expect("LLVM builder error");
            let fv = self
                .builder
                .build_load(self.llvm_type(fty).expect("ADT fields are never Unit"), fp, "fv")
                .expect("LLVM builder error");
            loaded.push((fi as u32, fv, fty.clone()));
        }
        self.emit_stale_check(sp, 3);
        let rc = self.builder.build_load(i64t, sp, "rc").expect("LLVM builder error").into_int_value();
        let alone = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, i64t.const_int(1, false), "alone")
            .expect("LLVM builder error");
        let mine_bb = self.ctx.append_basic_block(func, "empty.mine");
        let shared_bb = self.ctx.append_basic_block(func, "empty.shared");
        let on_bb = self.ctx.append_basic_block(func, "empty.on");
        self.builder.build_conditional_branch(alone, mine_bb, shared_bb).expect("LLVM builder error");
        self.builder.position_at_end(mine_bb);
        self.builder.build_unconditional_branch(on_bb).expect("LLVM builder error");
        self.builder.position_at_end(shared_bb);
        for (_, fv, fty) in &loaded {
            self.emit_retain(*fv, fty);
        }
        self.emit_shared_dec(sp, rc, ty);
        let shared_end = self.builder.get_insert_block().expect("block");
        self.builder.build_unconditional_branch(on_bb).expect("LLVM builder error");
        self.builder.position_at_end(on_bb);
        let ptrt = self.ctx.ptr_type(Default::default());
        let cell = self.builder.build_phi(ptrt, "shell").expect("phi");
        cell.add_incoming(&[(&sp, mine_bb), (&ptrt.const_null(), shared_end)]);
        self.shells.push((cell.as_basic_value().into_pointer_value(), ty.clone(), ctor));
        for (fi, fv, fty) in &loaded {
            self.cleanup.push((*fv, fty.clone()));
            let mut here = occ.clone();
            here.push((ctor, *fi));
            // The arm's own name for the field, if it has one, is this
            // owned value.
            for (name, o) in binds {
                if *o == here {
                    arm_scope.insert(name.clone(), Some(*fv));
                }
            }
            self.empty_box(*fv, fty, &pats[*fi as usize], here, binds, arm_scope, func);
        }
        true
    }

    fn take_reuse(
        &mut self,
        want: &Ty,
        ctor: usize,
        struct_ty: StructType<'ctx>,
        name: &str,
    ) -> Option<PointerValue<'ctx>> {
        // An emptied box of the right shape first: nothing in it to let go
        // of, only the question of whether there is a box at all.
        if let Some(at) = self.shells.iter().rposition(|(_, t, c)| t == want && *c == ctor) {
            let (cell, _, _) = self.shells.remove(at);
            let func = self.builder.get_insert_block().and_then(|b| b.get_parent()).expect("in a function");
            let some = self.builder.build_is_not_null(cell, "shell.some").expect("LLVM builder error");
            let keep_bb = self.ctx.append_basic_block(func, "shell.keep");
            let fresh_bb = self.ctx.append_basic_block(func, "shell.fresh");
            let done_bb = self.ctx.append_basic_block(func, "shell.done");
            self.builder.build_conditional_branch(some, keep_bb, fresh_bb).expect("LLVM builder error");
            self.builder.position_at_end(keep_bb);
            // The count is the one the box had, which was one.
            self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
            self.builder.position_at_end(fresh_bb);
            let fresh = self.alloc_object(struct_ty, name);
            let fresh_end = self.builder.get_insert_block().expect("block");
            self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");
            self.builder.position_at_end(done_bb);
            let phi = self.builder.build_phi(self.ctx.ptr_type(Default::default()), name).expect("phi");
            phi.add_incoming(&[(&cell, keep_bb), (&fresh, fresh_end)]);
            return Some(phi.as_basic_value().into_pointer_value());
        }
        // The latest candidate of the right shape that the frame still holds.
        let at = self.reuse.iter().rposition(|(p, t, c)| {
            t == want && *c == ctor && self.cleanup.iter().any(|(h, _)| h.into_pointer_value() == *p)
        })?;
        let (old, old_ty, old_ctor) = self.reuse.remove(at);
        let i = self.cleanup.iter().position(|(c, _)| c.into_pointer_value() == old)?;
        self.cleanup.remove(i);
        let i64t = self.ctx.i64_type();
        let func = self
            .builder
            .get_insert_block()
            .and_then(|b| b.get_parent())
            .expect("emitting outside a function");
        self.emit_stale_check(old, 4);
        let rc = self
            .builder
            .build_load(i64t, old, "rc")
            .expect("LLVM builder error")
            .into_int_value();
        let alone = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, i64t.const_int(1, false), "alone")
            .expect("LLVM builder error");
        let keep_bb = self.ctx.append_basic_block(func, "reuse.keep");
        let fresh_bb = self.ctx.append_basic_block(func, "reuse.fresh");
        let done_bb = self.ctx.append_basic_block(func, "reuse.done");
        self.builder.build_conditional_branch(alone, keep_bb, fresh_bb).expect("LLVM builder error");

        self.builder.position_at_end(keep_bb);
        let (adt, targs) = match &old_ty {
            Ty::Adt(a, ts) => (*a, ts.clone()),
            _ => unreachable!("only an ADT box is reused"),
        };
        let old_struct = self.ctor_struct(adt, old_ctor, &targs);
        let fields: Vec<Ty> = self.tmod.adts[adt].ctors[old_ctor]
            .fields
            .iter()
            .map(|f| subst_gen(f, &targs))
            .collect();
        for (fi, fty) in fields.iter().enumerate() {
            if !self.is_heap(fty) {
                continue;
            }
            let fp = self
                .builder
                .build_struct_gep(old_struct, old, (fi + 2) as u32, "oldf")
                .expect("LLVM builder error");
            let fv = self
                .builder
                .build_load(self.llvm_type(fty).expect("ADT fields are never Unit"), fp, "oldfv")
                .expect("LLVM builder error");
            self.emit_release(fv, fty);
        }
        let keep_end = self.builder.get_insert_block().expect("block");
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(fresh_bb);
        self.emit_shared_dec(old, rc, &old_ty);
        let fresh = self.alloc_object(struct_ty, name);
        let fresh_end = self.builder.get_insert_block().expect("block");
        self.builder.build_unconditional_branch(done_bb).expect("LLVM builder error");

        self.builder.position_at_end(done_bb);
        let phi = self.builder.build_phi(self.ctx.ptr_type(Default::default()), name).expect("phi");
        phi.add_incoming(&[(&old, keep_end), (&fresh, fresh_end)]);
        Some(phi.as_basic_value().into_pointer_value())
    }

    /// Builds a constructor. `moving` says the box is on its way straight out
    /// of the frame as a tail call's argument, which is what makes moving a
    /// field into it safe — see the note inside.
    #[allow(clippy::too_many_arguments)]
    fn gen_ctor(
        &mut self,
        adt: usize,
        ctor: usize,
        targs: &[Ty],
        args: &[TExpr],
        scope: &mut Scope<'ctx>,
        subst: &[Ty],
        func: FunctionValue<'ctx>,
        moving: bool,
    ) -> Result<Option<BasicValueEnum<'ctx>>, CodegenError> {
        let (adt, ctor) = (&adt, &ctor);
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
                // A flat value is its fields and nothing else: no box, no
                // count, so nothing to move into it either.
                let whole = Ty::Adt(*adt, targs.clone());
                if self.is_flat(&whole) {
                    if self.explain {
                        let cname = self.tmod.adts[*adt].ctors[*ctor].name.clone();
                        self.note(format!("`{cname}(..)` is a flat value: in registers, no box and no count"));
                    }
                    let st = self.flat_struct(&whole);
                    let mut agg = st.get_undef();
                    for (i, a) in args.iter().enumerate() {
                        let v = self
                            .gen_expr(a, scope, subst, func)?
                            .expect("ADT fields are never Unit");
                        agg = self
                            .builder
                            .build_insert_value(agg, v, i as u32, "f")
                            .expect("LLVM builder error")
                            .into_struct_value();
                    }
                    return Ok(Some(agg.into()));
                }
                // Evaluate fields before allocating (an argument may allocate).
                //
                // A binding this frame owns and puts inside the box is moved
                // there, exactly as one handed to a tail call is: the retain
                // the use would take and the release the frame would make
                // cancel, and the box holds the reference the frame held.
                // `Cons(x, acc)` in an accumulating loop is this every time
                // round.
                let mut field_vals = Vec::with_capacity(args.len());
                for a in args {
                    // A binding this frame owns and puts straight inside the
                    // box is moved there, exactly as one handed to a tail call
                    // is: the retain the use would take and the release the
                    // frame would make cancel, and the box keeps the reference
                    // the frame held. `Cons(x, acc)` in an accumulating loop
                    // is this on every link.
                    //
                    // Only a field written as a name, and only where the box
                    // itself is a tail call's argument. A move under a branch
                    // would be a move on one path and not the other, and the
                    // paths that did not move would each be left holding a
                    // count that nothing gives back.
                    let moved = if moving { self.take_owned(a, scope) } else { None };
                    match moved {
                        Some(v) => field_vals.push(Some(v)),
                        // A box built inside a box that is leaving is leaving
                        // with it, so the same moves and the same reuse hold
                        // one level down: `Mul(Num(x * y), z)` fills its
                        // `Num` from the one it took apart too.
                        None => match &a.kind {
                            TK::CallCtor { adt, ctor, targs, args } if moving && !args.is_empty() => {
                                field_vals.push(self.gen_ctor(*adt, *ctor, targs, args, scope, subst, func, true)?)
                            }
                            _ => field_vals.push(self.gen_expr(a, scope, subst, func)?),
                        },
                    }
                }
                let struct_ty = self.ctor_struct(*adt, *ctor, &targs);
                let sig = &self.tmod.adts[*adt].ctors[*ctor];
                let name = format!("{}.box", sig.name);
                let tag = sig.tag;
                let reused = if moving {
                    self.take_reuse(&Ty::Adt(*adt, targs.clone()), *ctor, struct_ty, &name)
                } else {
                    None
                };
                if self.explain {
                    let cname = self.tmod.adts[*adt].ctors[*ctor].name.clone();
                    if reused.is_some() {
                        self.note(format!("`{cname}(..)` is built in the box a match took apart: no allocation"));
                    } else {
                        self.note(format!("`{cname}(..)` allocates a box with {} field(s)", args.len()));
                    }
                }
                let ptr = match reused {
                    Some(p) => p,
                    None => self.alloc_object(struct_ty, &name),
                };
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
        // Matching on a local takes no reference of its own. Whatever keeps
        // that local alive — this frame's own hold on it, or the box an
        // enclosing arm borrowed it out of — keeps it alive across the match
        // too, and the arms retain their bindings as they use them. Counting
        // it again meant a retain and a release on every `match` in the
        // language, which for a list walked one link at a time is once per
        // link.
        let mut borrowed = matches!(&scrutinee.kind, TK::Local(_)) && self.is_heap(&scrut_ty);
        // A field of a box some earlier frame holds — `match q.plan` in a
        // function whose `q` is borrowed — is read the same way: the box is
        // immutable and outlives this frame, so the field is alive for the
        // match's length and what the arms bind out of it is held above too.
        let held_field = matches!(&scrutinee.kind, TK::FieldGet { .. })
            && self.is_heap(&scrut_ty)
            && root_local(scrutinee).is_some_and(|n| self.name_held_above(n));
        // `match (e1, e2)` against tuple patterns: the tuple exists only to
        // be taken apart on the next line, so it is not built. Its parts are
        // an LLVM struct the decision tree reads with `extractvalue`, and
        // each part is released where the box would have been — the box was
        // one allocation and one free per `match`, which in a tree rewriter
        // is per node. A pair of numbers was already flat; this is the pair
        // of anything. Only where no arm names the tuple whole (`t -> …`,
        // `t @ (a, b) -> …`), since there is no tuple to name.
        let parts: Option<&[TExpr]> = match &scrutinee.kind {
            TK::CallCtor { adt, args, .. }
                if !borrowed
                    && self.tmod.adts[*adt].name.starts_with("Tuple")
                    && !self.is_flat(&scrut_ty)
                    && arms.iter().all(|a| {
                        matches!(a.pattern, Pattern::Ctor { .. } | Pattern::Wildcard)
                    }) =>
            {
                Some(args.as_slice())
            }
            _ => None,
        };
        let mut part_vals: Vec<(BasicValueEnum<'ctx>, Ty)> = Vec::new();
        let saved_unboxed = self.unboxed_root;
        let scrut = if let Some(parts) = parts {
            let mut members: Vec<BasicTypeEnum> = Vec::new();
            let mut vals = Vec::new();
            for a in parts {
                let v = self.gen_expr(a, scope, subst, func)?.expect("ADT fields are never Unit");
                let ty = self.concrete(&a.ty, subst);
                members.push(v.get_type());
                vals.push(v);
                part_vals.push((v, ty));
            }
            let mut agg = self.ctx.struct_type(&members, false).get_undef();
            for (i, v) in vals.into_iter().enumerate() {
                agg = self
                    .builder
                    .build_insert_value(agg, v, i as u32, "part")
                    .expect("LLVM builder error")
                    .into_struct_value();
            }
            let agg: BasicValueEnum<'ctx> = agg.into();
            self.unboxed_root = Some(agg);
            Some(agg)
        } else if borrowed {
            let TK::Local(name) = &scrutinee.kind else { unreachable!() };
            scope[name]
        } else if held_field {
            let (v, held) = self.gen_borrowed(scrutinee, scope, subst, func)?;
            if !held {
                borrowed = true;
            }
            Some(v)
        } else {
            self.gen_expr(scrutinee, scope, subst, func)?
        };
        if tail.is_some() && !borrowed {
            // Each arm returns, so the scrutinee is released with the rest of
            // the frame just before the return.
            if parts.is_some() {
                for (v, ty) in &part_vals {
                    if self.is_heap(ty) {
                        self.cleanup.push((*v, ty.clone()));
                    }
                }
            } else if let Some(sv) = scrut {
                if self.is_heap(&scrut_ty) {
                    self.cleanup.push((sv, scrut_ty.clone()));
                }
            }
        }

        let rows: Vec<Row> = arms
            .iter()
            .enumerate()
            .map(|(i, a)| Row { pats: vec![a.pattern.clone()], arm: i })
            .collect();
        let guarded: Vec<bool> = arms.iter().map(|a| a.guard.is_some()).collect();
        let tree = self.build_tree_guarded(
            vec![Vec::new()],
            vec![scrut_ty.clone()],
            rows,
            &guarded,
        );

        let arm_blocks: Vec<BasicBlock<'ctx>> = (0..arms.len())
            .map(|i| self.ctx.append_basic_block(func, &format!("arm{i}")))
            .collect();
        self.emit_tree(&tree, scrut, &scrut_ty, &arm_blocks, func, arms, scope, subst)?;

        let saved_cleanup = self.cleanup.clone();
        let saved_shells = self.shells.clone();
        let mut incoming: Vec<(Option<BasicValueEnum<'ctx>>, BasicBlock<'ctx>)> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            self.builder.position_at_end(arm_blocks[i]);
            self.cleanup = saved_cleanup.clone();
            self.shells = saved_shells.clone();
            let mut arm_scope = scope.clone();
            let mut binds = Vec::new();
            let tmod = self.tmod;
            collect_bindings(&arm.pattern, Vec::new(), &tmod.ctors, &mut binds);
            let saved_bound = self.bound_fns.clone();
            let saved_reuse = self.reuse.clone();
            // A box this frame owns, taken apart by an arm that returns and
            // never says its name again, is emptied rather than borrowed
            // from: if nothing else holds it (one load and a compare), its
            // fields become the frame's own with no count touched, and the
            // box is a shell — built over by the arm's answer if that has
            // the same shape, given back at exit if not. If it is shared,
            // the fields are retained and the box let go, as before. Either
            // way the arm runs once, over the same values; only the shell is
            // null on the shared path. `simp` over a tree that nothing else
            // holds then allocates and counts nothing at all.
            //
            // `owned_fields` is the arm's bindings that are whole fields,
            // now the frame's to move; anything bound deeper is borrowed
            // from one of those, which outlives the arm exactly as the box
            // did.
            let mut emptied = false;
            if tail.is_some() {
                if let (Some(sv), TK::Local(sname)) = (scrut, &scrutinee.kind) {
                    let owned = self.cleanup.iter().any(|(c, _)| *c == sv);
                    if owned && !names_local(&arm.body, sname) {
                        emptied = self.empty_box(sv, &scrut_ty, &arm.pattern, Vec::new(), &binds, &mut arm_scope, func);
                    }
                }
                // The parts of a tuple that was never built are the frame's
                // own already, and have no name an arm could say: each one
                // the arm takes apart is emptied the same way.
                if let (Some(_), Pattern::Ctor { args: pats, .. }) = (parts, &arm.pattern) {
                    for (fi, ((v, ty), pat)) in part_vals.iter().zip(pats).enumerate() {
                        self.empty_box(*v, ty, pat, vec![(0, fi as u32)], &binds, &mut arm_scope, func);
                    }
                }
            }
            // The box this arm just took apart can be built over, if this
            // frame is what holds it, the arm ends in a return, and nothing
            // left in the arm can still say its name. Its fields are already
            // in registers by now, so writing over it cannot lose them.
            if tail.is_some() && !emptied {
                if let (Some(sv), TK::Local(sname)) = (scrut, &scrutinee.kind) {
                    // Deliberately not peeled. `whole @ Circle(r)` is exactly
                    // the case the condition above rules out: the arm can
                    // still say the box's name, because that is what the name
                    // is. Reading through the `@` here would let the arm build
                    // over a box it is still holding.
                    let ctor = match &arm.pattern {
                        Pattern::Ctor { name, .. } => self.tmod.ctors.get(name).map(|&(_, c)| c),
                        _ => None,
                    };
                    let owned = self.cleanup.iter().any(|(c, _)| *c == sv);
                    if let Some(ctor) = ctor {
                        if owned && !names_local(&arm.body, sname) {
                            self.reuse.push((sv.into_pointer_value(), scrut_ty.clone(), ctor));
                        }
                    }
                }
            }
            // Bindings out of a box an earlier frame holds are held there
            // too, and may go through a borrowed position of a tail call.
            let saved_held = self.held_above.clone();
            let held_root = match &scrutinee.kind {
                TK::Local(sname) => Some(sname.as_str()),
                TK::FieldGet { .. } if borrowed => root_local(scrutinee),
                _ => None,
            };
            if let Some(sname) = held_root {
                if !emptied && self.name_held_above(sname) {
                    for (name, _) in &binds {
                        self.held_above.insert(name.clone());
                    }
                }
            }
            for (name, occ) in &binds {
                // Already the frame's own, by emptying.
                if arm_scope.get(name).is_some_and(|v| v.is_some()) && !scope.contains_key(name) {
                    self.bound_fns.remove(name);
                    continue;
                }
                // Borrowed out of the box, which outlives the arm; every use
                // of the binding retains it in the usual way. A name for a
                // whole part of an unboxed tuple — `(x, y) -> Mul(x, y)` —
                // is the part itself, the value this frame holds, so that a
                // use that takes it out of the frame finds it there and moves
                // it instead of counting.
                let part = match (parts, occ.as_slice()) {
                    (Some(_), [(_, fi)]) => Some(part_vals[*fi as usize].0),
                    _ => None,
                };
                let v = match part {
                    Some(v) => v,
                    None => self.value_at(scrut.expect("value"), &scrut_ty, occ).0,
                };
                arm_scope.insert(name.clone(), Some(v));
                self.bound_fns.remove(name);
            }
            match tail {
                Some(ret_ty) => {
                    // The arm is the last thing on its path: what it names
                    // once and hands to a call can be moved there — except
                    // the box itself, which its bindings borrow from for the
                    // arm's length, and a name that is both a field and the
                    // holder of bindings deeper down (`m @ Mul(y, z)`).
                    let saved_move_ok = std::mem::replace(&mut self.move_ok, true);
                    self.begin_statement(&arm.body, &[]);
                    if let TK::Local(sname) = &scrutinee.kind {
                        self.movable.remove(sname);
                    }
                    for (name, occ) in &binds {
                        if binds.iter().any(|(_, o)| o.len() > occ.len() && o.starts_with(occ)) {
                            self.movable.remove(name);
                        }
                    }
                    let out = self.gen_tail(&arm.body, &mut arm_scope, subst, func, ret_ty);
                    self.move_ok = saved_move_ok;
                    out?
                }
                None => {
                    let v = self.gen_expr(&arm.body, &mut arm_scope, subst, func)?;
                    incoming.push((v, self.builder.get_insert_block().unwrap()));
                }
            }
            self.bound_fns = saved_bound;
            self.reuse = saved_reuse;
            self.held_above = saved_held;
        }
        self.cleanup = saved_cleanup;
        self.shells = saved_shells;
        self.unboxed_root = saved_unboxed;

        if tail.is_some() {
            return Ok(None);
        }
        let out = self.merge_branches(func, result_ty, incoming)?;
        if parts.is_some() {
            for (v, ty) in &part_vals {
                if self.is_heap(ty) {
                    self.emit_release(*v, ty);
                }
            }
        } else if let Some(sv) = scrut.filter(|_| !borrowed) {
            if self.is_heap(&scrut_ty) {
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
        match peel(p) {
            Pattern::Int(v) => Some(Head::Int(*v)),
            Pattern::Ctor { name, .. } => self.tmod.ctors.get(name).map(|&(_, ci)| Head::Ctor(ci)),
            _ => None,
        }
    }

    /// `guarded[i]` says whether arm `i` has a condition after its pattern.
    fn build_tree_guarded(
        &self,
        occs: Vec<Occ>,
        tys: Vec<Ty>,
        rows: Vec<Row>,
        guarded: &[bool],
    ) -> Tree {
        if rows.is_empty() {
            return Tree::Fail;
        }
        if rows[0].pats.iter().all(is_wild) {
            let arm = rows[0].arm;
            if !guarded.get(arm).copied().unwrap_or(false) {
                return Tree::Leaf(arm, None);
            }
            // Guarded: if the condition is false the value is still to be
            // matched, against everything written below this arm.
            let rest = rows[1..].to_vec();
            let fallback = self.build_tree_guarded(occs, tys, rest, guarded);
            return Tree::Leaf(arm, Some(Box::new(fallback)));
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
            cases.push((tag, self.build_tree_guarded(next_occs, next_tys, next_rows, guarded)));
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
            Some(Box::new(self.build_tree_guarded(
                occs[1..].to_vec(),
                tys[1..].to_vec(),
                rest_rows,
                guarded,
            )))
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
        for (step, &(ci, fi)) in occ.iter().enumerate() {
            let (adt, targs) = match &ty {
                Ty::Adt(adt, targs) => (*adt, targs.clone()),
                other => unreachable!("occurrence walks into {other:?}"),
            };
            let field_ty = subst_gen(&self.tmod.adts[adt].ctors[ci].fields[fi as usize], &targs);
            let unboxed = step == 0 && self.unboxed_root == Some(root);
            if unboxed || self.is_flat(&ty) {
                val = self.extract_field(val, fi);
                ty = field_ty;
                continue;
            }
            let struct_ty = self.ctor_struct(adt, ci, &targs);
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

    #[allow(clippy::too_many_arguments)]
    fn emit_tree(
        &mut self,
        tree: &Tree,
        root: Option<BasicValueEnum<'ctx>>,
        root_ty: &Ty,
        arm_blocks: &[BasicBlock<'ctx>],
        func: FunctionValue<'ctx>,
        arms: &[TArm],
        scope: &Scope<'ctx>,
        subst: &[Ty],
    ) -> Result<(), CodegenError> {
        match tree {
            Tree::Leaf(k, fallback) => {
                let Some(guard) = arms[*k].guard.as_ref() else {
                    self.builder
                        .build_unconditional_branch(arm_blocks[*k])
                        .expect("LLVM builder error");
                    return Ok(());
                };
                // The guard is asked here rather than in the arm's own block,
                // because a false one has somewhere else to go. Its bindings
                // are borrowed out of the box exactly as the arm's are.
                let mut guard_scope = scope.clone();
                let mut binds = Vec::new();
                collect_bindings(&arms[*k].pattern, Vec::new(), &self.tmod.ctors, &mut binds);
                for (name, occ) in &binds {
                    let (v, _) = self.value_at(root.expect("value"), root_ty, occ);
                    guard_scope.insert(name.clone(), Some(v));
                }
                let cond = self
                    .gen_expr(guard, &mut guard_scope, subst, func)?
                    .expect("a guard is a Bool")
                    .into_int_value();
                let else_bb = self.ctx.append_basic_block(func, "guard.else");
                self.builder
                    .build_conditional_branch(cond, arm_blocks[*k], else_bb)
                    .expect("LLVM builder error");
                self.builder.position_at_end(else_bb);
                match fallback {
                    Some(t) => {
                        self.emit_tree(t, root, root_ty, arm_blocks, func, arms, scope, subst)?
                    }
                    None => {
                        self.builder.build_unreachable().expect("LLVM builder error");
                    }
                }
            }
            Tree::Fail => {
                self.builder.build_unreachable().expect("LLVM builder error");
            }
            Tree::Switch { occ, on_int, cases, default } => {
                let (val, vty) = self.value_at(root.expect("value"), root_ty, occ);
                // A flat value has one constructor and no tag to read: the
                // one case is the only way on. So has a tuple that was never
                // boxed.
                let unboxed = occ.is_empty() && root.is_some() && self.unboxed_root == root;
                if !*on_int && (unboxed || self.is_flat(&vty)) {
                    let bb = self.ctx.append_basic_block(func, "m.flat");
                    self.builder.build_unconditional_branch(bb).expect("LLVM builder error");
                    self.builder.position_at_end(bb);
                    let (_, sub) = &cases[0];
                    return self.emit_tree(sub, root, root_ty, arm_blocks, func, arms, scope, subst);
                }
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
                    self.emit_tree(sub, root, root_ty, arm_blocks, func, arms, scope, subst)?;
                }
                self.builder.position_at_end(default_bb);
                match default {
                    Some(d) => {
                        self.emit_tree(d, root, root_ty, arm_blocks, func, arms, scope, subst)?
                    }
                    None => {
                        self.builder.build_unreachable().expect("LLVM builder error");
                    }
                }
            }
        }
        Ok(())
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
        // in the three backends that are linked. Registering every backend
        // instead would make the linker keep all nineteen of LLVM's, which
        // doubles the size of this compiler for architectures nobody asks for.
        match want {
            None => Target::initialize_native(&InitializationConfig::default())
                .map_err(|e| CodegenError::new(0, format!("failed to initialize LLVM target: {e}")))?,
            Some(_) => {
                let cfg = InitializationConfig::default();
                Target::initialize_aarch64(&cfg);
                Target::initialize_x86(&cfg);
                Target::initialize_webassembly(&cfg);
            }
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
        compile_to_ir(&tmod, false, false).unwrap()
    }

    #[test]
    fn export_needs_a_host_and_scalars() {
        let check = |src: &str, target, shared| {
            let m = rill_syntax::parse(src).unwrap();
            let tmod = rill_types::check(&m).unwrap();
            check_exports(&tmod, target, shared)
        };
        // An export is what a host calls, and a native executable is not one:
        // `--target wasm32-wasip1` and `--shared` are the two that are.
        let e =
            check("export fn f(x: Int) -> Int = x\nfn main() = 0\n", None, false).unwrap_err();
        assert!(e.msg.contains("needs a host"), "{e}");
        let e = check("export fn id(x) = x\nfn main() = 0\n", Some("wasm32-wasip1"), false)
            .unwrap_err();
        assert!(e.msg.contains("generic"), "{e}");
        // Only Int, Float, Bool and Ptr cross to wasm...
        let e = check(
            "export fn shout(s: Str) -> Str = s\nfn main() = 0\n",
            Some("wasm32-wasip1"),
            false,
        )
        .unwrap_err();
        assert!(e.msg.contains("Str"), "{e}");
        check(
            "export fn f(x: Int) -> Float = to_float(x)\nfn main() = 0\n",
            Some("wasm32-wasip1"),
            false,
        )
        .unwrap();
        // ...while a shared library takes the counted values too, since the
        // host holds a handle rather than a copy.
        check("export fn shout(s: Str) -> Str = s\nfn main() = 0\n", None, true).unwrap();
    }

    /// Builds for wasm and hands back the module's IR, or what the target
    /// refused it for.
    fn wasm_ir(src: &str) -> Result<String, CodegenError> {
        let m = rill_syntax::parse(src).unwrap();
        let tmod = rill_types::check(&m).unwrap();
        let context = Context::create();
        let cg =
            Codegen::build_module(&context, &tmod, false, None, Some("wasm32-wasip1"), false, false, false)?;
        cg.check_wasm_supported()?;
        Ok(cg.module.print_to_string().to_string())
    }

    /// `size_t` is 64 bits on every target Rill links natively and 32 on
    /// wasm32, and a declaration cannot say that with a number. The same
    /// source has to produce both, or a signature mismatch traps at run time.
    #[test]
    fn word_width_follows_the_target() {
        let src = "extern fn strlen(s: Ptr) -> USize\n\nfn main() =\n  p = to_cstr(\"abc\")\n  n = strlen(p)\n  free_ptr(p)\n  println(n)\n";
        let native = ir(src);
        assert!(native.contains("declare i64 @strlen"), "{native}");
        let wasm = wasm_ir(src).unwrap();
        assert!(wasm.contains("declare i32 @strlen"), "{wasm}");
        // Narrower than Rill's `Int`, so the result comes back widened — and
        // unsigned, so a big length does not arrive negative.
        assert!(wasm.contains("zext i32"), "{wasm}");
    }

    /// A wasm module links no shared object, but `m` is not one there: the
    /// maths is inside the same wasi-libc archive as `strlen`, so asking for
    /// `-lm` is asking for something the module already has.
    #[test]
    fn wasm_takes_libm_and_refuses_a_real_library() {
        let sqrt = "extern \"m\" fn sqrt(x: Float) -> Float\n\nfn main() = println(sqrt(4.0))\n";
        assert!(wasm_ir(sqrt).unwrap().contains("declare double @sqrt"));
        let glfw = "extern \"glfw\" fn glfwInit() -> I32\n\nfn main() = println(glfwInit())\n";
        let e = wasm_ir(glfw).unwrap_err();
        assert!(e.msg.contains("`glfw`"), "{e}");
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
        let err = compile_to_ir(&tmod, false, false).unwrap_err();
        assert!(err.msg.contains("no implementation of trait `D`"), "{}", err.msg);
    }

    #[test]
    fn static_dispatch_ir() {
        let src = "trait D(a)\n  d(a) -> Str\n\nimpl D(Int)\n  fn d(n) = int_to_str(n)\n\nfn main() = println(d(7))\n";
        let m = rill_syntax::parse(src).unwrap();
        let tmod = rill_types::check(&m).unwrap();
        let ir = compile_to_ir(&tmod, false, false).unwrap();
        assert!(ir.contains("D.Int.d"), "{ir}");
    }

    #[test]
    fn named_fn_as_value_ir() {
        let src = "fn double(x) = x * 2\n\nfn apply(f, x) = f(x)\n\nfn main() = println(apply(double, 21))\n";
        let out = ir(src);
        assert!(out.contains("$shim"), "{out}");
    }
}

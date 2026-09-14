//! AST for Rill (M1 subset).

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub imports: Vec<ImportDecl>,
    pub types: Vec<TypeDef>,
    pub traits: Vec<TraitDef>,
    pub impls: Vec<ImplDef>,
    pub externs: Vec<ExternFn>,
    pub asms: Vec<AsmFn>,
    pub functions: Vec<FnDef>,
    /// How many of `functions` came from the prelude. They are first, so the
    /// program's own functions are the tail — which is what lets a diagnostic
    /// tell whether the line it found is one the author can go and look at.
    pub prelude_fns: usize,
}

/// `extern "m" fn pow(x: Float, y: Float) -> Float` — a function provided by a
/// C library. The Rill name is the C symbol; the string names a library to
/// link (`-lm`), and is omitted for anything already in the C runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternFn {
    pub name: String,
    pub lib: Option<String>,
    pub params: Vec<(String, TypeRef)>,
    pub ret: Option<TypeRef>,
    pub line: usize,
}

/// `asm fn outb(port: U16, v: U8) = "outb $1, $0" : "{dx},{al}"` — a run of
/// instructions rather than a symbol to link. The template and the constraint
/// string go to LLVM as written, the way `extern`'s library string goes to the
/// linker as written: what the machine accepts is the machine's business.
///
/// Omitting the constraints asks for the ordinary shape — `=r` for a result
/// and `r` for each parameter, in order — which is what most of them want. A
/// register the instruction names itself (`outb` wants `dx` and `al`) has to
/// be pinned, and that is what writing them out is for.
#[derive(Debug, Clone, PartialEq)]
pub struct AsmFn {
    pub name: String,
    pub params: Vec<(String, TypeRef)>,
    pub ret: Option<TypeRef>,
    pub template: String,
    /// The constraint string as written, or `None` for the generated one.
    pub constraints: Option<String>,
    /// `naked asm fn` — the instructions are the whole function, placed under
    /// the name with no prologue and no epilogue around them. It takes
    /// nothing and answers nothing because there is no calling convention
    /// left to carry either: what reaches it is an interrupt, a reset vector
    /// or a jump somebody else wrote.
    pub naked: bool,
    pub line: usize,
}

/// `trait Describe(a)` with one method signature per indented line.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub param: String,
    pub methods: Vec<MethodSig>,
    /// `trait Pretty(a): Show, Describe` — traits a type must implement
    /// as well to implement this one.
    pub supers: Vec<String>,
    pub line: usize,
}

/// `describe(a) -> Str` — parameter types may reference the trait parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<TypeRef>,
    /// What each parameter is called, where it is called anything: a
    /// default body needs names to be written in terms of.
    pub names: Vec<Option<String>>,
    pub ret: TypeRef,
    /// `describe(x: a) -> Str = "a thing"`: what the method is where an
    /// impl does not say. It is checked once per impl that leaves it out,
    /// at that impl's type, like a method the impl had written.
    pub default: Option<Expr>,
    pub line: usize,
}

/// `impl Describe(Shape)` with one `fn` per method.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplDef {
    pub trait_name: String,
    pub target: TypeRef,
    pub fns: Vec<FnDef>,
    pub line: usize,
}

/// `import "path"` — loads `path.rill` relative to the importing file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub path: String,
    /// `import "x" as name` — the imported file's own definitions are then
    /// reached as `name.thing`, so two files may define the same name.
    pub alias: Option<String>,
    pub line: usize,
}

/// A reference to a type in source: `Int`, `Shape`, `List(Int)`, or a
/// lowercase type parameter `a` inside a generic type definition.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub name: String,
    pub args: Vec<TypeRef>,
    /// The `-> T` of a callback type, `Fn(I32) -> I32`. Only `Fn` may carry
    /// one; absent means the callback hands nothing back.
    pub ret: Option<Box<TypeRef>>,
    pub line: usize,
}

/// `type Shape` / `type List(a)` with one constructor per indented line.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub params: Vec<String>,
    pub ctors: Vec<CtorDef>,
    /// `type Meters = Float`: a name for a type, not a type. Every mention
    /// is read as what it names before the checker sees the program, so an
    /// alias has no constructors and is never among the types.
    pub alias: Option<TypeRef>,
    pub line: usize,
}

/// `Circle(r: Float)` — field names are optional documentation for now.
#[derive(Debug, Clone, PartialEq)]
pub struct CtorDef {
    pub name: String,
    pub fields: Vec<(Option<String>, TypeRef)>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ann: Option<TypeRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    pub name: String,
    /// `export fn` — kept when compiling to wasm as a function the host may
    /// call. Meaningless (and refused) elsewhere.
    pub exported: bool,
    pub params: Vec<Param>,
    pub ret_ann: Option<TypeRef>,
    pub body: Expr,
    pub line: usize,
    /// Set on a function `lift` pulled out of another one: the name of the
    /// function it was written inside. The checker keeps the two together so
    /// a captured value has the type it had where it was captured.
    pub lifted_from: Option<String>,
    /// `where Describe(a)`: what the signature's type variables are required
    /// to implement, said rather than left for the body to imply.
    pub wheres: Vec<Bound>,
}

/// One requirement of a `where` clause: a trait, of a type variable.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub trait_name: String,
    pub var: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `name = expr`
    Bind { name: String, value: Expr, line: usize },
    /// `Pair(a, b) = expr` — a binding that takes a value apart. Only a type
    /// with one constructor can be taken apart this way, which is what makes
    /// it a binding and not a branch; `lift` turns it into the `match` it is.
    PatBind { pattern: Pattern, value: Expr, line: usize },
    /// A function defined inside a body. It sees the names around it and may
    /// call itself and its siblings; `lift` turns it into a top-level one
    /// before anything else looks at the program.
    Fn(Box<FnDef>),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Var(String, usize),
    /// `f(a, b)`. `piped` records that the source wrote `a |> f(b)`, which
    /// desugars to the same call; only the formatter cares.
    /// `stacked` records that an argument began a line — or, for a
    /// pipeline, that the `|>` did — and the formatter writes it back that
    /// way, one argument (or stage) to a line. Only the formatter cares.
    Call { name: String, args: Vec<Expr>, line: usize, piped: bool, stacked: bool },
    Unary { op: UnOp, expr: Box<Expr> },
    /// `newline` records that the operator began a line — a continuation —
    /// and the formatter keeps it there. Only the formatter cares.
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, line: usize, newline: bool },
    /// `newline` records that `then` or `else` began a line — or, for the
    /// `&&` and `||` this desugars, that the operator did — and the
    /// formatter keeps them there. Only the formatter cares.
    If { cond: Box<Expr>, then: Box<Expr>, els: Box<Expr>, newline: bool },
    Match { scrutinee: Box<Expr>, arms: Vec<Arm>, line: usize },
    /// `\x, y -> expr`
    Lambda { params: Vec<String>, body: Box<Expr>, line: usize },
    /// `spawn expr` — evaluates the expression in a new green thread.
    Spawn { body: Box<Expr>, line: usize },
    /// `select` — waits until one of its channel operations can proceed.
    Select { arms: Vec<SelectArm>, default: Option<Box<Expr>>, line: usize },
    /// `#s` — how many bytes a string has, or how many elements a buffer.
    Len { expr: Box<Expr>, line: usize },
    /// `b[i]` — one element of a buffer, or one byte of a string.
    Index { base: Box<Expr>, index: Box<Expr>, line: usize },
    /// `p.x` — one field of a value whose type has a single constructor. A
    /// dot written against a name is part of that name until the checker
    /// takes it apart, since `geo.area` is one too; this is what the parser
    /// builds where the base is not a name, as in `f(x).y`.
    Field { base: Box<Expr>, name: String, line: usize },
    /// `{ p | x = 9, y = 0 }` — the value `p` with those fields replaced and
    /// every other one carried over. The base has to be a type with one
    /// constructor, for the same reason `p.x` does: with two, a field name
    /// does not say which.
    Update { base: Box<Expr>, fields: Vec<(String, Expr, usize)>, line: usize },
    /// `e?` — the value inside an `Ok`, or the whole function's `Err`. `lift`
    /// turns it into the `match` it is, taking the rest of the block as the
    /// `Ok` arm, so nothing after this pass knows it was ever written.
    Try { expr: Box<Expr>, line: usize },
    /// `s[i:j]` — the bytes from `i` up to but not `j`; either end may be
    /// left out for the string's own. `lift` spells it out as `str_sub`.
    Slice { base: Box<Expr>, from: Option<Box<Expr>>, to: Option<Box<Expr>>, line: usize },
    /// `b[i] = v` — writing one element of a buffer.
    IndexSet { base: Box<Expr>, index: Box<Expr>, value: Box<Expr>, line: usize },
    /// `b[i] += v` and its friends: one read, one write, the index evaluated
    /// once. `lift` spells it out.
    IndexOp { op: BinOp, base: Box<Expr>, index: Box<Expr>, value: Box<Expr>, line: usize },
    /// `b += s` on a string builder: the one `op=` whose target is a name,
    /// because what it changes is the storage the name points at, not the
    /// name. `lift` leaves it; the checker turns it into `strbuf_add`.
    Append { target: Box<Expr>, value: Box<Expr>, line: usize },
    /// `for i in a..b` over the half-open range, its body run for each `i`.
    /// With `with acc = init` the body's value is the next `acc` and the loop
    /// is worth the last one — which is how a loop carries something in a
    /// register rather than in a one-element buffer. `lift` turns either into
    /// the local function a loop is.
    For {
        var: String,
        from: Box<Expr>,
        to: Box<Expr>,
        acc: Option<(String, Box<Expr>)>,
        body: Box<Expr>,
        line: usize,
    },
    /// `while cond with state = init`: the loop that `for` is not, over a
    /// condition instead of a range. The state is what the condition and the
    /// body see, the body's value is the next state, and the loop is worth
    /// the state the condition first refused. The state may be several
    /// names, `with (i, n) = (0, 0)`, each annotated like a parameter if it
    /// has to be (`v: Buf(Int)`); they are carried as separate parameters
    /// wherever the body ends in a tuple written in place. Without `with`
    /// the body runs for its effects and the loop is worth 0, as `for` is.
    /// `lift` turns it into the local function a loop is.
    While {
        cond: Box<Expr>,
        acc: Option<(Vec<Param>, Box<Expr>)>,
        body: Box<Expr>,
        line: usize,
    },
    /// Multi-statement block; its value is the value of the last statement.
    Block(Vec<Stmt>),
}

/// One arm of a `select`: a channel operation and what to do when it wins.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectArm {
    pub op: SelectOp,
    pub body: Expr,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectOp {
    /// `x = recv(ch) ->` or `recv(ch) ->`
    Recv { chan: Expr, bind: Option<String> },
    /// `send(ch, v) ->`
    Send { chan: Expr, value: Expr },
    /// `timeout(ms) ->` — the arm that becomes ready when nothing else has,
    /// for long enough. Not a channel, so nothing is bound.
    Timeout { ms: Expr },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pattern: Pattern,
    /// `pattern if cond ->`. The arm is taken when the pattern matches *and*
    /// this is true; a false one falls through to the arms below, which is
    /// why a guarded arm covers nothing as far as exhaustiveness goes.
    pub guard: Option<Expr>,
    pub body: Expr,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `_`
    Wildcard,
    /// `x` — binds the scrutinee (or a constructor field).
    Var(String),
    /// `42` or `-42`
    Int(i64),
    /// `Circle(r)` — sub-patterns are limited to `_` and variables in M1.
    Ctor { name: String, args: Vec<Pattern>, line: usize },
    /// `whole @ Circle(r)` — matches what follows the `@` and names the whole
    /// of it as well. The name binds the value at the position the pattern is
    /// at, which for an arm's own pattern is the scrutinee.
    As(String, Box<Pattern>, usize),
    /// `1 | 2 | 3` — any one of these. Never survives `lift`, which turns one
    /// arm holding it into one arm per alternative; nothing downstream of that
    /// pass knows it was written. It exists in the tree at all so that the
    /// formatter prints back what was typed rather than the expansion.
    Or(Vec<Pattern>, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

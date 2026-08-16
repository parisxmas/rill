//! AST for Rill (M1 subset).

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub imports: Vec<ImportDecl>,
    pub types: Vec<TypeDef>,
    pub traits: Vec<TraitDef>,
    pub impls: Vec<ImplDef>,
    pub externs: Vec<ExternFn>,
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

/// `trait Describe(a)` with one method signature per indented line.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub param: String,
    pub methods: Vec<MethodSig>,
    pub line: usize,
}

/// `describe(a) -> Str` — parameter types may reference the trait parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<TypeRef>,
    pub ret: TypeRef,
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `name = expr`
    Bind { name: String, value: Expr, line: usize },
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
    Call { name: String, args: Vec<Expr>, line: usize, piped: bool },
    Unary { op: UnOp, expr: Box<Expr> },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, line: usize },
    If { cond: Box<Expr>, then: Box<Expr>, els: Box<Expr> },
    Match { scrutinee: Box<Expr>, arms: Vec<Arm>, line: usize },
    /// `\x, y -> expr`
    Lambda { params: Vec<String>, body: Box<Expr>, line: usize },
    /// `spawn expr` — evaluates the expression in a new green thread.
    Spawn { body: Box<Expr>, line: usize },
    /// `select` — waits until one of its channel operations can proceed.
    Select { arms: Vec<SelectArm>, default: Option<Box<Expr>>, line: usize },
    /// `b[i]` — one element of a buffer, or one byte of a string.
    Index { base: Box<Expr>, index: Box<Expr>, line: usize },
    /// `b[i] = v` — writing one element of a buffer.
    IndexSet { base: Box<Expr>, index: Box<Expr>, value: Box<Expr>, line: usize },
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub pattern: Pattern,
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

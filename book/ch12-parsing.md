# Chapter 12. Parsing

Parsing is the problem functional languages were invented for. ML was built to
write proof tactics; Haskell's early users wrote compilers; the reason sum
types, pattern matching and recursion feel so natural here is that someone
designed them for exactly this shape of work.

This chapter builds a complete expression calculator: a type for the syntax
tree, an evaluator, and a recursive-descent parser with error reporting. It is
the longest program in the book and it is about 60 lines.

---

## 12.1 The tree first

Before parsing anything, decide what you are parsing *into*:

```rill
# book/code/ch12/calc.rill
type Expr
  Num(Int)
  Add(Expr, Expr)
  Sub(Expr, Expr)
  Mul(Expr, Expr)

impl Show(Expr)
  fn show(e) = match e
    Num(n) -> int_to_str(n)
    Add(a, b) -> "(" + show(a) + " + " + show(b) + ")"
    Sub(a, b) -> "(" + show(a) + " - " + show(b) + ")"
    Mul(a, b) -> "(" + show(a) + " * " + show(b) + ")"

fn eval(e) = match e
  Num(n) -> n
  Add(a, b) -> eval(a) + eval(b)
  Sub(a, b) -> eval(a) - eval(b)
  Mul(a, b) -> eval(a) * eval(b)

fn main() =
  e = Add(Mul(Num(2), Num(3)), Sub(Num(10), Num(4)))
  println(e)
  println(eval(e))
```

```
$ rill run calc.rill
((2 * 3) + (10 - 4))
12
```

An abstract syntax tree is a recursive sum type, and that is all it is. Four
cases, and `Add`, `Sub` and `Mul` hold two `Expr`s each — the recursion in the
type is what makes arbitrarily deep expressions representable.

`eval` is then four lines that could not be much else. *The value of a number is
the number; the value of an addition is the sum of the values.* Each arm of the
match is one line of the language's semantics, which is why interpreters in
this style read like specifications.

`show` is the same walk producing text instead of a number, and it prints the
tree fully bracketed — useful, because it shows you what the parser decided
about precedence.

> **This is the visitor pattern, without the pattern**
>
> In a language with classes, an AST is a class hierarchy and every operation
> over it is a Visitor: an interface with a method per node type, an
> `accept` method on each node, and double dispatch to make it work. That is
> perhaps eighty lines of ceremony to express what `match` expresses in four.
>
> The trade is Chapter 5's table. Adding a new *operation* here is one new
> function; adding a new *node type* means the compiler lists every match that
> needs an arm. For a language's AST — where the node types are fixed by the
> grammar and the operations keep coming — that is exactly the right way round.

---

## 12.2 The parser

The grammar is the usual three-level one, which encodes precedence in its
shape:

```
expr := term (("+" | "-") term)*
term := atom ("*" atom)*
atom := number | "(" expr ")"
```

`*` binds tighter than `+` because `term` is *inside* `expr`. Parentheses come
back to the top by having `atom` call `expr` again.

The key design decision in a functional parser is what a parse *returns*. It
cannot mutate a position counter, so it must hand one back:

```rill
type Parsed
  Parsed(value: Expr, next: Int)
  Bad(why: Str, at: Int)
```

*A parse either produced a value and consumed up to position `next`, or failed
with a reason at a position.* That is `Result` with the success case carrying
two things — and writing it as its own type instead of reusing `Result` keeps
the field names.

Here is the whole parser:

```rill
# book/code/ch12/parser.rill
fn digit(c) = c >= 48 && c <= 57

fn skip_space(s, i) = if i < str_len(s) && str_get(s, i) == 32 then skip_space(s, i + 1) else i

fn read_int(s, i, acc) =
  if i < str_len(s) && digit(str_get(s, i)) then read_int(s, i + 1, acc * 10 + str_get(s, i) - 48) else Parsed(Num(acc), i)

# atom := number | "(" expr ")"
fn atom(s, i0) =
  i = skip_space(s, i0)
  if i >= str_len(s) then Bad("expected a number", i) else if digit(str_get(s, i)) then read_int(s, i, 0) else if str_get(s, i) == 40 then bracketed(s, i + 1) else Bad("unexpected character", i)

fn bracketed(s, i) = match expr(s, i)
  Bad(w, at) -> Bad(w, at)
  Parsed(e, j) -> closing(s, e, skip_space(s, j))

fn closing(s, e, j) =
  if j < str_len(s) && str_get(s, j) == 41 then Parsed(e, j + 1) else Bad("expected )", j)

# term := atom ("*" atom)*
fn term(s, i) = match atom(s, i)
  Bad(w, at) -> Bad(w, at)
  Parsed(a, j) -> term_more(s, a, j)

fn term_more(s, a, j0) =
  j = skip_space(s, j0)
  if j < str_len(s) && str_get(s, j) == 42 then term_mul(s, a, j + 1) else Parsed(a, j)

fn term_mul(s, a, j) = match atom(s, j)
  Bad(w, at) -> Bad(w, at)
  Parsed(b, k) -> term_more(s, Mul(a, b), k)

# expr := term (("+" | "-") term)*
fn expr(s, i) = match term(s, i)
  Bad(w, at) -> Bad(w, at)
  Parsed(a, j) -> expr_more(s, a, j)

fn expr_more(s, a, j0) =
  j = skip_space(s, j0)
  plus = j < str_len(s) && str_get(s, j) == 43
  minus = j < str_len(s) && str_get(s, j) == 45
  if plus || minus then expr_op(s, a, j + 1, plus) else Parsed(a, j)

fn expr_op(s, a, j, plus) = match term(s, j)
  Bad(w, at) -> Bad(w, at)
  Parsed(b, k) -> expr_more(s, if plus then Add(a, b) else Sub(a, b), k)
```

```
$ rill run parser.rill
2 + 3 * 4  ->  (2 + (3 * 4)) = 14
(2 + 3) * 4  ->  ((2 + 3) * 4) = 20
10 - 4 - 3  ->  ((10 - 4) - 3) = 3
2 * (3 + (4 - 1))  ->  (2 * (3 + (4 - 1))) = 12
2 +  ->  error: expected a number at 3
(1 + 2  ->  error: expected ) at 6
```

The output confirms the two things a parser must get right. `2 + 3 * 4` parsed
as `(2 + (3 * 4))`, so precedence works. `10 - 4 - 3` parsed as
`((10 - 4) - 3)` and evaluated to 3, so subtraction is left-associative — if it
had built `(10 - (4 - 3))` you would see 9.

### Reading it

**Each grammar rule is a function.** `expr`, `term`, `atom` — the code and the
grammar are the same three lines. This is recursive descent, and it is the
reason this style of parser is the one people write by hand.

**The `("+" term)*` repetition is a tail-recursive helper.** `expr_more` is the
loop: it looks for an operator, and if it finds one it parses another term,
builds a bigger tree, and calls itself. `a` — the tree built so far — is the
accumulator. This is Chapter 7's pattern doing real work, and it is also why
the result comes out left-associative for free.

**Errors propagate by pattern matching.** Every call site has a `Bad` arm that
passes the failure up unchanged. That is more typing than an exception, and it
is the visible cost of Chapter 5's decision. In exchange there is no hidden
control flow: every path out of every function is on the page.

**The position is threaded, never mutated.** `i`, `j`, `k` are three different
positions, each one returned by the previous step. A mutable parser has one
`self.pos` that everything shares, which is convenient right up to the moment
you want to backtrack — at which point you need to save and restore it, and
forgetting to is the classic parser bug. Here backtracking is free: the old
position is still in scope because nothing overwrote it.

### The awkward part

`bracketed` had to be split into two functions, and `term_more`/`term_mul` into
two more. The reason is a real limitation of Rill 0.2: **a `match` arm's body
cannot be an indented block on the following lines**. Where you want

```
Parsed(e, j0) ->
  j = skip_space(s, j0)
  ...
```

you must instead pass the intermediate value to a helper. The compiler is
clear about it —

```
parser.rill: syntax error at line 41, column 5: expected an expression, found indent
```

— but it does push a parser into more small functions than the grammar
strictly needs. In OCaml or Haskell this would be a `let ... in` inside the
branch. It is the single roughest edge encountered in writing this book.

---

## 12.3 How other languages do this

```python
# Python, mutable position
class Parser:
    def __init__(self, s):
        self.s, self.i = s, 0

    def expr(self):
        a = self.term()
        while self.peek() in "+-":
            op = self.take()
            b = self.term()
            a = Add(a, b) if op == "+" else Sub(a, b)
        return a
```

Shorter, and most people find it easier to write the first time. Three
differences are worth naming:

**State.** `self.i` is shared mutable state. Every method may move it, and
whether a failed parse leaves it where it started is a matter of discipline.
The Rill version cannot get this wrong, because a function that fails returns
`Bad` and the caller still holds the old position.

**Errors.** Python raises; the type says nothing. Rill returns `Bad`; every
caller is forced by the compiler to decide what to do about it.

**Exhaustiveness.** Add a node type to the AST in Python and `eval` silently
falls through to whatever the last `elif` was. In Rill it does not compile.

The honest summary: for a hundred-line calculator the Python version is faster
to write. For a language front end that will be maintained for years and grow
node types, the property that *adding a case makes the compiler list every
place that must change* is worth much more than the lines it costs.

Real parser combinator libraries — Haskell's Parsec, Rust's nom — take this
chapter's `Parsed` type and make it composable, so that `many`, `optional` and
`choice` become functions and the grammar becomes an expression. That needs a
type for "a parser" as a first-class value, which Rill can express; it is the
natural next step from here and the last exercise asks for its first piece.

---

## 12.4 Exercises

1. Add division and unary minus. Division by zero should produce a `Bad` rather
   than crashing — which means `eval` must return a `Result` too, and that
   change ripples exactly as far as the compiler tells you.
2. Add floating-point numbers, so `2.5 * 4` works. Where does `read_int` become
   two functions?
3. Add variables and a `let`: `let x = 3 in x * x`. `eval` now needs an
   environment — thread a `Dict` from Chapter 11 through it.
4. Report the error with a caret under the offending column, using the position
   in `Bad`.
5. Write `simplify(e)` that rewrites `Add(Num(0), x)` to `x`, `Mul(Num(1), x)`
   to `x`, and `Mul(Num(0), _)` to `Num(0)`, recursively. This is a compiler
   optimization pass, and it is fifteen lines of pattern matching.
6. Make `Parsed` generic — `Parsed(a)` for any result type — so the same
   machinery can parse something that is not an `Expr`.

---

Chapter 13 leaves trees for graphs, where the data no longer has a shape the
type system can enforce and the algorithms get more interesting.

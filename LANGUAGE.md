# The Rill Language Reference (0.2)

Rill is a strict, statically typed functional language that compiles to native
executables. This document describes the whole language; every construct shown
here is exercised by the test suite.

---

## 1. Program structure

A program is one or more `.rill` files. Compilation starts at the file passed to
`rill build`; `import` pulls in others. Declarations may appear in any order —
a function can call one defined later in the file.

```
import "geometry"        # loads geometry.rill next to this file

type Shape               # a data type
  Circle(r: Float)

trait Describe(a)        # an interface
  describe(a) -> Str

impl Describe(Shape)     # an implementation
  fn describe(s) = "a shape"

fn main() =              # the entry point
  println(describe(Circle(1.0)))
```

Every program needs a `fn main()`. It takes no parameters and returns nothing;
the program exits when it returns. A file that is a library rather than a
program says so on its first line — `module date` — and has no `main`: it
is checked, typed, formatted and `rill test`ed as it is, and `rill run`
refuses it, saying to import it from a program. The name has to be the
file's own (`date.rill`), so a file copied under another name is caught at
once. A file with neither a `module` line nor a `main` is asked which it is.

An import brings the other file's definitions in under their own names. Adding
an alias keeps them apart instead, which is how two modules can define the same
name:

```
import "metric" as m
import "imperial" as i

fn main() = println(m.label(m.Meters(1.5)) + " " + i.label(i.Feet(2.0)))
```

`m.label` and `i.label` are different functions, and `m.Unit` and `i.Unit` are
different types. A pattern names an aliased constructor the same way:
`match u m.Meters(v) -> v`.

A function the file keeps to itself is written `private fn`. An importer
does not see it, cannot call it — the error names the file — and may have a
function of the same name, since the two never meet: the loader renames a
private function to a spelling no program can write and rewrites the file's
own references to match. Everything else a file defines is the importer's
to use, which is the default a library written before `private` still has.

A file that is not beside the importer is looked for on the search path:
`RILL_PATH`, a colon-separated list, and then every `lib/` from the importing
file's directory upwards, stopping at the one holding `.git`. So a library in
the project's `lib/` is `import "data/json"` from anywhere in the project, and a
program's own files still win — a file beside the importer is never taken over
by one on the path.

### Layout

Blocks are written by indentation — two spaces per level, spaces only, no tabs.

An expression continues on the next line when that line begins with something
no statement can begin with: a binary operator (`+ * / % == != < <= > >= && ||
|>`), a `|` between patterns, a `.`, or `then` or `else`. Not `-` or `!`,
which begin a negation as readily as they continue a line. The continuation
is indented past the line it continues:

```
total = xs
  |> map(\x -> x * 10)
  |> filter(\x -> x > 10)
  |> sum
```

Inside parentheses indentation carries no meaning, so a call may spread its
arguments over lines freely. `rill fmt` keeps what was written: an operator,
`then` or `else` that began a line stays at the head of one, and arguments
written one to a line stay one to a line, each a level in.

Comments run from `#` to the end of the line. The `#` needs a space (or the
line's end) after it, because written against something — `#s` — it is the
length operator instead.

---

## 2. Types

| Type | Values | Notes |
|---|---|---|
| `Int` | `42`, `-7` | 64-bit signed |
| `Float` | `3.14`, `2.0` | 64-bit; a decimal point is required |
| `Bool` | `true`, `false` | |
| `Str` | `"hi"`, `"a\nb"` | escapes: `\n` `\t` `\\` `\"` |
| `Unit` | `()` | what `println` and `main` return; `()` is its one value, for a branch with nothing to say |
| `Chan(a)` | `channel()` | see [Concurrency](#7-concurrency) |
| `Map(k, v)` | `map_new()` | a hash table; see [Maps](#maps) |
| `StrBuf` | `strbuf()` | bytes on their way to a string |
| `(A, B) -> C` | `\x, y -> ...` | function values; `() -> C` and `\ -> ...` take nothing |
| user types | `Circle(1.0)` | declared with `type` |

Types are inferred; annotations are optional and only ever needed to resolve an
ambiguity or to document intent:

```
fn area(r: Float) -> Float = 3.14159 * r * r
```

There are no implicit conversions. `1 + 2.0` is a type error; `to_float` and
`to_int` cross between them explicitly.

A **type alias** is a name for a type: `type Meters = Float`, or with
parameters, `type Pair(a) = (a, a)`. It is a name and nothing more — every
mention is read as what it names before the checker sees the program, so a
`Meters` and a `Float` are one type, an alias prints as what it names, and
an alias that names itself is refused. A type to be kept apart from what it
holds is a constructor instead: `type Meters` with `Meters(v: Float)` is a
new type, and `m.v` is the number.

`Int` is 64-bit two's complement and its arithmetic is the machine's: `+`,
`-` and `*` wrap round at the ends, as they do in C, Go and a release build
of Rust. The two cases the machine has no one answer for are decided: `/`
and `%` by zero end the program, saying where — a number that is silently
wrong is worse than a stop — and the smallest integer divided by `-1` wraps
to itself, with a remainder of zero. `to_int` truncates toward zero, and past
the ends of `Int` gives the nearest end; a NaN gives zero. `float_bits(x)` is
the 64 bits of a float as the `Int` they are and `bits_float(n)` the float
those bits spell — no conversion, for a file format or a sort key that
carries a float as bytes. A shift by 64 or
more, or by less than nothing, is zero. There are no narrower numbers as
values: a byte read from a string or a buffer is an `Int`, and the narrow
names — `U8`, `I32`, `F32` and the rest — say how storage and C signatures
are laid out; see [Buffers](#buffers) and [Calling C](#11-calling-c).

A float literal may carry an exponent — `1.0e9`, `2.5e-3`, `1e6`, with or
without a dot before the `e` — since a digit run can be followed by nothing
else that means anything. The canonical spelling has no exponent, so `rill
fmt` writes `1.0e9` back as `1000000000.0`.

Float arithmetic may be **contracted**: a multiply and the add that consumes it
can become one fused instruction, rounding once instead of twice. The answer is
nearer the true one, not further, but it is not the answer two separate
roundings give. C, C++ and Go do this by default and Rill follows them; Rust
and Zig do not. Nothing else about float arithmetic is loosened — no
reassociation, no assumptions about NaN or infinity, no reciprocals. On a
Mandelbrot it is worth about six percent.

### Algebraic data types

```
type Shape
  Circle(r: Float)
  Rect(w: Float, h: Float)

type List(a)                    # generic, and recursive
  Nil
  Cons(head: a, tail: List(a))
```

Field names are optional (`Rect(Float, Float)` is equivalent), but a named
field can be read by its name — see [Fields](#fields). Constructor names are
global and must start with an uppercase letter; a type and one of its
constructors may share a name.

### Fields

A value whose type has **one** constructor is read with a dot:

```
type Point
  P(x: Float, y: Float)

fn dist(a: Point, b: Point) =
  dx = a.x - b.x
  dy = a.y - b.y
  sqrt(dx * dx + dy * dy)
```

One constructor is the whole condition, and it is the condition a binding
already had: `P(x, y) = p` reads as a binding because it cannot fail, and
`p.x` for the same reason. A type with several constructors reads the same
way for a field **every** constructor has, of one type — `s.name` on a
`Circle(r: Float, name: Str)` / `Rect(w: Float, h: Float, name: Str)` is
certainly there whichever it is, and becomes the `match` a program would have
written. A field some constructor lacks is not certainly there, so `match` is
how those are read; the error names the constructor that lacks it. Updating
works the same way: `{ s | name = "disc" }` rebuilds whichever constructor
`s` is.

The dot works on anything, not only a name: `mk(1).y`, `p.pos.x`. What it does
not do is change anything. A record is a value; the way to a different one is
to build it:

```
{ p | health = p.health - 35 }
```

Everything `p` had, apart from these — read the `|` as "except". It is a call
to the constructor, the named fields taking the new values and every other one
read off `p`, which is evaluated once however many it fills in. Written out,
`Player(p.name, p.pos, p.health - 35, p.kills)` builds the same value and says
which fields it keeps as well; either way `p` is what it was. Writing `p.x = 3`
is refused, and says this.

A field read does not need the type in front of it known where the dot is
written. It is answered once the whole function is inferred, so a parameter
read by `.x` on one line and handed to something that says its type on the
next needs no annotation, whichever order the lines are in. When nothing in
the function says the type, the fields do: a value read by `.x` and `.y` is of
the one single-constructor type that has an `x` and a `y`. Two types that both
have them is the one case an annotation is still for, and the error names
them; for a field no type has, it names the types that have the others. An
update is answered the same way, by the fields it sets.

A dot means one of two things and the compiler tells them apart by what the
name in front of it is: a module alias (`geo.area`, from `import "geo" as
geo`) or a value with fields. An alias wins where a program has both.

---

## 3. Expressions

Everything is an expression; there are no statements that don't produce a value
(except bindings).

```
1 + 2 * 3            # arithmetic: + - * / %  (% is Int only)
a == b               # equality on any value type, structural for data
a < b                # ordering: Int, Float, Str, or a type with Ord
x && y || !z         # logical operators, short-circuiting
"a" + "b"            # string concatenation
f(x, y)              # call
\x, y -> x + y       # lambda
xs |> map(f) |> sum  # pipeline: x |> f(a) means f(x, a)
if c then a else b   # conditional (an expression, not a statement)
f(x)?                # the value inside an `Ok`, or the function's `Err`
p.x                  # one field of a one-constructor type
```

Bit operations are functions rather than operators: `bit_and`, `bit_or`,
`bit_xor`, `bit_not`, `shl` and `shr` (`shr` is logical, since protocol code
works with bit patterns).

Precedence, loosest to tightest: `|>`, `||`, `&&`, comparisons, `+ -`, `* / %`,
unary `- !`, then the postfix `[]`, `?` and `.`, calls and literals.

### Tuples

`(a, b)` hands back more than one thing without declaring a type for it:

```
fn divmod(a, b) = (a / b, a % b)

fn main() =
  (q, r) = divmod(17, 5)      # a binding that takes the value apart
  println(q)
```

A tuple holds two to eight values, and `(A, B)` is its type. It is an
ordinary data type underneath — `Tuple2` to `Tuple8` in the prelude —
so it compares, shows and pattern-matches like any other, and `(n, s) -> ...`
is a pattern anywhere a pattern goes.

A binding may take apart any type with exactly one constructor:
`Parsed(v, rest) = parse(s)` reads as a binding because it cannot fail. On a
type with more than one, the compiler says the match is not exhaustive, which
is the truth: use `match`.

`==` and `!=` on floats follow IEEE 754, as C and Python do: a NaN is equal
to nothing, itself included, so `x == x` is false for one and `x != x` is
true — which is how a program asks whether a number is a number.

### Lanes: `Float2`

`Float2` is two `Float`s that travel together in one 128-bit register — what
a kernel written with the machine's vector lanes in mind is written in:

```
p = float2(1.5, -2.0)
q = p * splat2(4.0) + float2(0.25, 0.25)   # both lanes at once: (6.25, -7.75)
lane0(q) + lane1(q)
v = f2_load(buf, i)                        # buf[i] and buf[i + 1], as one
f2_store(buf, i, fma2(a, b, v))            # a fused multiply-add on both lanes
fmin2(a, b)                                # the smaller of each lane; fmax2 too
```

`+ - * /` work lane by lane with the same one loosening as a `Float` (a
multiply and the add that consumes it may fuse), `==` is of both lanes,
`fmin2` and `fmax2` are the smaller and the larger of each lane, and
a `Float2` prints as a pair, goes in a map or down a channel, and sits in a
flat record like any number. It is not a general vector type — no `Float4`,
no lane-wise `<` — but it is what the matrix product in
`lib/num/grid.rill` keeps its block of answers in, and it is why that product
runs at the speed of a BLAS. The rest of a numeric loop needs none of this:
a plain `for` over a buffer is vectorized by the compiler on its own.

The one loop that is not is a **reduction over `Float`**, and the reason is
the language keeping its word rather than the loop being poor. Vectorizing a
sum means splitting it into independent partial sums and adding those at the
end — which is reassociation, and reassociation changes the answer, so a
compiler may only do it once told the program will accept a different one.
Rill never tells it that. So the additions stay in the order they were
written, each one waiting on the last, and a sum that should stream at memory
speed runs at the latency of `fadd` — about six times slower, measured over a
range that fits in L1. Clang with strict floating point does exactly the same
thing, for exactly the same reason.

Writing the accumulators out is what recovers it: several of them so the adds
are independent, in `Float2` so each holds two lanes. `lib/num/col.rill` has the
column aggregates already written that way — `sum`, `sum_sq`, `dot`, `mean`,
`variance`, `min_of`, `max_of` — and they run five to eight times faster than
the plain loop, at better than 100 GB/s, matching a C loop built with
`-ffast-math` while leaving `+` on a `Float` meaning what it says everywhere
else. `fmin2` and `fmax2` need none of that trade, since a minimum does not
care what order it sees its candidates in.

### String builders

A string is a value, so building one a piece at a time with `+` copies what
is already there every time. A `StrBuf` is storage for the bytes on their way
to becoming one — a `Buf` for text:

```
b = strbuf()
b += "hello, "        # a string goes in as it is
b += "world"
b += 33               # a number goes in as one byte: `!`
println(strbuf_str(b))
println(#b)           # how many bytes it holds
```

`b += s` is the one `op=` whose target is a name, and it changes no binding:
what it adds to is the storage the name points at, exactly as `v[i] = x`
writes a buffer. `strbuf_add(b, s)` and `strbuf_byte(b, n)` are the same two
operations spelled out. A builder is not a value type — it does not show,
compare or cross a channel; `strbuf_str` takes the string out of it, and the
builder stays usable. `strbuf_clear(b)` empties one without giving back the
room it took, which is what a page rendered over and over into the same
builder wants.

None of the three is a call. An append is written where it stands: the room
is one compare against a capacity the compiler is holding in a register, a
literal goes in as whole words because its length is known, and `b +=
int_to_str(n)` writes the digits themselves rather than a string that exists
only to be copied. The runtime is asked for one thing only, a bigger block,
and it is handed the block rather than the builder — so the length and the
capacity survive the call in registers, and a loop of appends reads the
header once rather than after every write. `benchmarks/template` is what
that adds up to on a page.

### Slices

`s[i:j]` is the bytes of a string from `i` up to but not `j` — the same
half-open range `for` counts over. Either end may be left out for the
string's own:

```
s[1:#s - 1]      # everything but the first and last byte
s[3:]            # from the fourth byte to the end
s[:3]            # the first three
```

An end outside the string is clamped rather than an error, as `str_sub` has
always been, and each part is evaluated once. Slicing is for strings; a `Buf`
is storage and is not copied by notation this quiet.

### Characters

`#s`, `s[i]` and `s[i:j]` are bytes, and stay bytes: a lexer wants bytes, a
wire protocol wants bytes, and the compiler that compiles the prelude is one
of them. What bytes are not is characters. `"günaydın"` is ten bytes and
eight letters, and `s[0:2]` cuts the `ü` in half. The characters live on top,
in the prelude, and say so in their names:

| | |
|---|---|
| `char_count(s)` | how many characters |
| `char_at(s, i)` | the `i`th character, as a codepoint |
| `char_sub(s, i, n)` | `n` characters from the `i`th |
| `char_take(s, n)` / `char_drop(s, n)` | the first `n`, or all but the first `n` |
| `chars(s)` | every character, a `List(Int)` of codepoints |
| `from_char(c)` / `from_chars(cs)` | back to a string, one to four bytes each |
| `char_decode(s, at)` / `char_wide(b)` | the character at a byte offset, and how many bytes the one beginning with byte `b` takes — for walking a long string without counting from the start each time |

All of it walks, since UTF-8 gives no other way to the `n`th character.
Malformed input never hangs it: a stray continuation byte counts as one
character, and a character cut off by the end of the string comes back as
its first byte.

`chr(n)` is **a byte**, not a character: it writes the low byte of `n` and
nothing else, which is right for building a frame or a hash and wrong for
every character above 127. A codepoint becomes text with `from_char`.

### Blocks and bindings

An indented block runs its lines in order and evaluates to the last one:

```
fn area_of_two() =
  a = area(Circle(1.0))     # binding, visible for the rest of the block
  b = area(Rect(2.0, 3.0))
  a + b                     # the block's value
```

Bindings are immutable. There is no assignment operator — but a name may be
bound again: `x = x + 1` reads the `x` there was to bind a new one, and from
then on `x` means the new one, while anything that captured the old one keeps
it. What a block binds is the block's: a name bound in one branch of an `if`
is not there after it, and a rebinding inside a branch shadows only inside it.

### Pattern matching

```
fn classify(l) = match l
  Nil -> "empty"
  Cons(Some(0), _) -> "starts with zero"
  Cons(Some(n), Cons(Some(m), _)) -> "two: " + int_to_str(n + m)
  Cons(_, _) -> "something else"
```

Patterns nest freely and may be: `_` (wildcard), a name (binds the value), an
integer literal, or a constructor with sub-patterns. The compiler checks that a
match covers every case — if not, it names one that is missing — and rejects
arms that earlier arms already cover.

An arm may carry a **guard**: what a pattern cannot say, written after it.

```
fn size(l) = match l
  Cons(x, _) if x > 100 -> "starts big"
  Cons(x, _) if x > 0 -> "starts small"
  Cons(_, _) -> "starts at or below zero"
  Nil -> "empty"
```

The arm is taken when the pattern matches **and** the condition holds; a false
condition falls through to the arms below, which the value is then matched
against as though the guarded arm were not there. So a guarded arm **covers
nothing**: the first two arms above leave `Cons(_, _)` to be answered, and
without the third the match is not exhaustive — the checker says so, naming
`Cons(_, _)`. For the same reason a guarded arm makes nothing below it
unreachable, while an unguarded one above it still does.

A guard sees the pattern's bindings and everything else in scope, and is an
ordinary expression of type `Bool`. It costs what it says: the arms below a
guarded one are tested again when its condition fails, rather than every value
being tested once as an unguarded match arranges.

A `match` written across lines uses an indented block. Inside parentheses,
where indentation carries no meaning, the arms simply follow one another on the
line — nothing else in the grammar can continue an expression with
`pattern ->`, so the two forms are unambiguous:

```
println(match o None -> 0 Some(n) -> n * 10)
```

`select` follows the same rule.

An arm's body is an inline expression, or — like a function's body — an
indented block of statements. The arrow ends its line and the block sits one
level deeper:

```
fn area(s) = match s
  Circle(r) ->
    pi = 3.14159
    pi * r * r
  Rect(w, h) -> w * h
```

`if` branches take the same block form, with `else` standing level with its
`if`, and `else if` chains staying flat:

```
fn describe(n) =
  if n < 0 then
    m = -n
    "negative " + int_to_str(m)
  else if n == 0 then "zero"
  else
    "positive " + int_to_str(n)
```

The value of a block is its last statement's value, exactly as in a function
body; every arm and branch must still agree on one type.

### Carrying an error out: `?`

Errors are `Result` values, which is the honest way round — a function that can
fail says so in its type and a caller cannot forget. What that used to cost was
a `match` per call, two of whose three lines were the same two lines every time.
`?` is those two lines: **the value inside the `Ok`, or else the whole
function's `Err`.**

```
fn parse(text) =
  host = field(text, "host")?
  port = number(field(text, "port")?)?
  Ok(Config(host, port))
```

It is a `match` and nothing else. The rest of the block becomes the `Ok` arm,
so the two lines above mean:

```
match field(text, "host")
  Ok(host) -> ...the rest of the block...
  Err(e) -> Err(e)
```

Which is where the one rule comes from: **a `?` belongs where the block it is
in is what the function returns**, because that is the door its `Err` leaves
by. A `?` in the middle of an expression is fine — `number(field(text, "port")?)?`
names the inner result first, in the order the expression evaluates — and so is
one inside an `if` branch or a `match` arm, as long as that branch is the
function's answer. One written where the block is worth something else says so,
and says what that something else is.

Nothing is added at run time: the `match` is the one that would have been
written, an error travels no further than it did, and a tail call after a `?`
is still a tail call.

Layers have their own kinds of error, and the function's error type says how
the lower ones are carried: a sum type with a constructor per kind.

```
type AppErr
  Io(IoErr)
  Parse(ParseErr)

fn load(path) -> Result(Config, AppErr) =
  text = read(path)?         # Err(e) -> Err(Io(e))
  Ok(parse(text)?)           # Err(e) -> Err(Parse(e))
```

Where the error a `?` meets is not the function's, it goes out inside the one
constructor of the function's error type that takes it — still the `match`
that would have been written, only with the wrapping in it. Nothing converts
that the type did not declare: two constructors that take it, or none, and the
`?` says so and asks for `map_err(r, f)?`, which rewrites the error before the
`?` carries it. The function's error type has to be known for this: written
as its return type, or settled already by the block. `ok_or(m[k], "no such
key")?` gives an absence the error it should be.

`?` works on an `Option` the same way, in a function that answers one: the
value inside the `Some`, or else `None`. It does not cross between the two —
an `Option` in a function that answers a `Result` is `ok_or(o, e)?`, the
error being something only the program can say, and the error says so.

---

## 4. Functions

```
fn add(a, b) = a + b                    # inferred as Int, Int -> Int
fn scale(v: Float, k: Float) = v * k    # annotated
fn apply(f, x) = f(x)                   # higher-order
```

Functions are values: pass them by name (`map(xs, double)`) or as lambdas.
Closures capture the variables they use by value. A lambda's body is an
expression, or an indented block on the lines after the `->`:

```
nearer = \a, b ->
  da = dist(a, origin)
  db = dist(b, origin)
  da < db
```

Inside parentheses layout is off, so a lambda written as an argument stays on
one line; bind the long one first.

### Generics

A function used at more than one type is generic automatically:

```
fn id(x) = x                # forall a. a -> a
fn len(l) = match l         # forall a. List(a) -> Int
  Nil -> 0
  Cons(_, rest) -> 1 + len(rest)
```

Generic functions are **monomorphized**: the compiler emits one specialized
copy per concrete type, so there is no boxing and no dispatch overhead.

Requirements a body imposes travel with the type. `fn biggest(a, b) = if a > b
then a else b` works for any ordered type, and using it at a type that isn't
ordered is a compile error naming the function and the type. A requirement
may also be said, in a `where` clause after the signature:

```
fn shout(x: a) -> Str where Describe(a) = describe(x) + "!"
fn sorted(xs: List(a)) -> List(a) where Ord(a) = ...
```

It names a trait of one of the signature's type variables — a user trait,
or `Ord` or `Show` — and is checked at every use of the function like a
requirement the body implied, naming the function, the type and the trait.
What it adds is that the signature says it: `rill types` and a reader see
the contract without the body, and a use at the wrong type is refused for
the reason the author gave. The body is still inferred; a `where` says what
is required, not what the body may do.

A lowercase name in an annotation is a type variable, and the same name is
the same type throughout the signature: `fn same(x: a, y: a) = x` takes two
of one type, whatever it is.

A function is generic at every use, wherever in the file it is defined. The
one thing inference will not do is give a function two types inside its own
dependency group — `g` used at `Int` and at `Str` by an `f` that `g` calls
back — or let it call itself at another type. Writing the whole signature
down lifts that: `fn g(x: a, n: Int) -> a` declares the scheme, every use
takes it at its own type, and the body is held to the declaration — one that
only works for `Int` is refused as not keeping its promise. What stays out of
reach is recursion whose types never stop growing: a call to itself at a
bigger type each time is stopped when the types have grown past anything a
program writes, since a copy is made per type; and a type that holds itself
at a bigger type, `Nest(a)` holding a `Nest((a, a))`, is refused where it is
written, for the same reason.

### Loops

Counting over a range has a form of its own:

```
for i in 0..n                  # the half-open range: 0 up to but not n
  v[i] = i * i
```

A loop that carries something says so, and is worth what it carried:

```
total = for i in 0..#v with acc = 0
  acc + v[i]                   # the body's value is the next `acc`
```

Writing the accumulation in the body instead — `acc = acc + v[i]`, with the
`with` left off — is refused, and named as what it is. A binding is not a
value, so a block ending in one has nothing to answer with and the name it
bound can never be read: the loop would have run and thrown every iteration
away. The error says so wherever it happens, and says `with` when the
binding's value reads the name it is binding.

Both forms are spelled out as the tail-recursive function below before
anything else looks at them — the end of the range is evaluated once, and what
the loop carries is a parameter, which is to say a register. Carrying it in a
one-element buffer instead, which is what a program without this had to do,
puts it in memory and reaches through a pointer for every touch. `+=`,
`-=`, `*=` and `/=` write a buffer element in terms of what it held:
`v[i] += 1`. The index is evaluated once, so `off[eu[i]] += 1` reads `eu[i]`
one time.

Everything else is tail recursion: a call in tail position compiles to a
jump, so it runs in constant stack, exactly like a loop. That holds for a loop
written as several functions calling each other as much as for one calling
itself, and it holds whatever the loop's value is — Rill's functions use
LLVM's guaranteed-tail-call convention and mark the call `musttail`, so it is
a promise the optimizer cannot take back rather than a hint it usually
honours.

One limit on the promise: it is made through the machine's registers, and a
signature with more than six integer or pointer parameters, or more than
eight floats, takes the C convention instead (a flat record counts by its
fields, a closure's environment as one more). A function calling itself
still runs in constant stack; several such functions calling each *other*
in tail position do not, and the compiler warns, naming the two and the
count. Pack the extras into one tuple — a tuple is one pointer — and the
promise is back.

```
fn sum_to(n) = go(n, 0)
fn go(i, acc) = if i == 0 then acc else go(i - 1, acc + i)   # a loop
```

A loop written this way is usually about the values around it, so it can be
written there: a `fn` inside a body sees the names in scope, and may call
itself and the functions beside it.

```
fn sum_to(n) =
  fn go(i, acc) = if i > n then acc else go(i + 1, acc + i)
  go(1, 0)                     # `n` comes from the enclosing function
```

Only the loop's own variables are parameters; everything else is simply in
scope. The compiler lifts each such function out to the top level and hands
it the names it uses, so nothing is allocated and the tail call is still a
tail call — a local function is a loop, not a closure. That is also why it can
only be called: there is no single value to pass around for one, since each
call site supplies the captured names. A lambda is the thing to pass around.

A local function that captures something is typed together with the one it is
written inside, which is what lets a captured buffer keep its width. The price
is that it has one type rather than a scheme: a capturing helper used at two
different types has to be a top-level function, where it generalizes as usual.
One that captures nothing is tied to nothing, and generalizes where it stands:

```
fn main() =
  fn id(x) = x

  println(id(1))
  println(id("a"))
```

Each parameter is a loop variable and the `if` is the exit condition. This
holds for loops that print, and for server loops that run forever:

```
fn worker(jobs, results) =
  n = recv(jobs)
  send(results, n * 2)
  worker(jobs, results)        # runs forever, stack never grows
```

Most everyday loops are written with the prelude instead — `range`, `map`,
`filter`, `fold` — which are themselves tail recursive:

```
range(1, 10) |> filter(\x -> x % 2 == 0) |> map(\x -> x * x) |> sum
```

See `examples/loops.rill`.

---

## 5. Traits

A trait declares an interface; an `impl` provides it for one type. Both builtin
types and user types can implement traits.

```
trait Describe(a)
  describe(a) -> Str
  priority(a) -> Int

impl Describe(Shape)
  fn describe(s) = "shape " + show(s)
  fn priority(s) = 2

impl Describe(Int)
  fn describe(n) = "int " + int_to_str(n)
  fn priority(n) = 1

fn announce(x) = println(describe(x))   # works for any implementing type
```

A method may come with a **default**, written after its signature with its
parameters named, and an impl that leaves the method out gets it. The
default is checked once per such impl, at that impl's type, so a default
that calls another method of the trait calls that impl's:

```
trait Describe(a)
  describe(a) -> Str
  priority(x: a) -> Int = 1
  banner(x: a) -> Str = describe(x) + " (" + int_to_str(priority(x)) + ")"
```

A trait may **require others**: `trait Pretty(a): Show, Describe` says a
type implements `Pretty` only if it implements those too, and an impl of
`Pretty` for a type that does not is refused, naming what is missing.

A method's name belongs to its trait, and two traits may use the same one:
`describe(x)` is settled by the type of `x`, whichever trait is implemented
for it, and where that cannot tell — both are, or `x` is a parameter whose
type is not known yet — the call says which, `Describe.describe(x)`, which
is always allowed. Constructor names stay global. Dispatch is resolved
statically during monomorphization: no dictionaries, no vtables, and a
compile error if an implementation is missing.

An impl may target a generic type by naming its parameters, in which case it
covers every instantiation:

```
impl Show(List(a))
  fn show(l) = "{" + join(map(l, \x -> show(x)), " ") + "}"
```

The `show(x)` on elements resolves when the element type is known, so
`List(Box(Int))` composes both impls without any `where` clause. An impl that
quietly works for only one instantiation is rejected.

### Objects

Dispatch is static, so a `List(a)` holds one type. Where a list has to hold
a dog and a robot because both can speak, a value is made an **object** of
the trait: `Speak(rex)` is a record of the trait's methods, each a closure
over `rex`, and its type is the trait's name.

```
trait Speak(a)
  speak(a) -> Str
  greet(a, Str) -> Str

fn main() =
  all = Cons(Speak(Dog("rex")), Cons(Speak(Robot(7)), Nil))   # a List(Speak)
  say(all)

fn say(xs) = match xs
  Nil -> 0
  Cons(s, rest) ->
    println(s.speak() + ", " + s.greet("ada"))
    say(rest)
```

Written out, `Speak(rex)` is `Speak.obj(\ -> speak(rex), \o -> greet(rex, o))`:
an ordinary record whose fields hold functions, made by a constructor the
trait declares and a program cannot name. A field that holds a function is
called as `s.speak()`. Each call inside the closures dispatches on the type
of `rex`, statically, as every method call does; what is paid is the
closures — one allocation per method when the object is made — and the
indirect call through each. Nothing else is boxed, and no dictionary travels
with a value.

A trait is the type of objects when every method takes the trait's type
once, and first: `same(a, a)` cannot be, since an object does not know what
the other one is, nor can a method that answers with the type. `Speak(x)`
on such a trait says which method is in the way. A record of functions
written by hand — `Shape(area: () -> Float, name: Str)` — is the same thing
with no trait at all, and a function's type is written the way a lambda
reads: `(Int, Str) -> Bool`, or `() -> Str` for one that takes nothing,
which is `\ -> e`.

### Built-in traits

Two traits are built in and change how the language's own operators behave:

```
impl Show(Money)                 # replaces derived printing
  fn show(m) = ...

impl Ord(Money)                  # enables <, <=, >, >=
  fn compare(a, b) = ...         # negative, zero or positive
```

Without a `Show` impl, every data value already prints structurally
(`Cons(1, Nil)`), and `==` already compares structurally.

---

## 6. Memory

Rill has no garbage collector. The compiler inserts reference-count operations
following ownership rules (Perceus-style), so memory is freed the moment the
last reference dies — deterministically, with no pauses. Nullary constructors
(`Nil`, `None`) are static singletons and never allocate; small objects come
from a pooled allocator.

When a `match` takes a box apart and nothing left can reach it by name, the
next constructor of exactly that shape is built **in that box** rather than in
a fresh one — so `map`, `filter`, `reverse` and `split` walk a list without
allocating anything, writing each link over the link they read it from.

A type with one constructor and nothing but numbers and booleans in it — a
pair, a point, a body with seven coordinates — is not a box at all: it lives
in registers, passed and returned by value, with no count and no allocation.
`(q, r) = divmod(a, b)` costs what returning two numbers costs in C. Such a
value is boxed only when it is put into a map or sent down a channel, which
store words, and read back out on the other side.

A pipeline over a range — `range(1, n) |> filter(p) |> map(f) |> sum`, and
`len` and `fold` the same way — is generated as one loop with no list in it,
when the functions are written in place as lambdas and the elements are
numbers or booleans. The prelude's `map` and `filter` are what is recognized;
a program that defines its own gets what it wrote.

A binding whose last use is a call is handed to the call rather than copied
into it — the reference goes with the argument, no count taken and none given
back — when the call is on the statement's unconditional path and the name is
not spoken again. And a function may ask what that leaves it holding:
`unique(x)` is true when this frame holds the only reference to `x`. The two
together are what lets an operation write its answer into an operand's own
storage when the operand was a temporary: `lib/num/grid.rill` makes `a * 2.0 +
1.0` one grid and not two by asking `unique` of what it was given. A value
that is asked about is passed owned, so a caller still using it keeps a
reference of its own and the answer is `false`; a compile-time constant is
never unique; a value with no count — a flat record, a number — cannot be
asked.

Reference counting cannot let go of a **ring** — a value that, through
what it holds, holds itself — and a program of immutable values can make one
in exactly one way: a value is built from what exists before it, so only
storage can come to hold what holds it. `m[k] = v` where `v` holds `m` — a
closure over `m`, a record with `m` in it — is refused where it is written,
naming `m`; so is a channel sent a value that holds it. A ring made through
two maps, each holding what holds the other, is not seen at compile time; it
is what the report below is for.

Run any program with `RILL_DEBUG_ALLOC=1` to have the runtime print the number
of live allocations at exit. Built with `--parallel` and run with
`RILL_ALLOC_CHECK=1` as well, it says which functions made the blocks that
were never let go — `3000 block(s) made in make` — which is where a ring
was closed.

Which of these rules fired where is not visible in the source, and does not
have to be guessed: **`rill explain file.rill`** is the code generator's own
account, written down as it generates, for the file's own functions.

```
fn total(xs, acc)   # line 6
  `xs` is borrowed: never kept or given away here, so the caller's reference is enough

fn main()   # line 15
  line 16: `Cons(..)` allocates a box with 2 field(s)
  line 17: `xs` is retained for the call: it is spoken again later, or this call is not on the statement's own straight path
  line 19: `P(..)` is a flat value: in registers, no box and no count
  line 22: this pipeline is one loop: no list is built
```

It says which parameters are borrowed and which owned, which constructor
calls reuse the box a match took apart and which allocate, which bindings
are handed to a call and which retained, which pipelines became loops, and
what each closure holds. A number that came out slower than expected is
usually a retain where a hand-over was meant, and the report says which
binding and why.

### `List` or `Buf`

`List(a)` is a cons list: `Cons` in front is free, and everything else is a
walk. Building one in order means accumulating it backwards and turning it
round, which the prelude does for you and which costs a second pass — the
reuse above makes it allocate nothing, but it is still a pass.

`Buf(w)` is storage: a counted, mutable, bounds-checked array of a fixed
width, indexed in constant time and appended to by keeping your own length.

Reach for `Buf` when the program is really building an array — a growing
sequence read by index, or anything a C version would `realloc`. Reach for
`List` when the program is really consing: recursion over a structure, a stack,
a queue of work. The JSON parser in `benchmarks/tokens/` is the honest example
of the difference: it builds arrays as lists, and that one choice is half of
what it still gives away to the C it is measured against.

### Maps

`Map(k, v)` is a hash table, and the other thing in the language that is
storage rather than a value. It is written like a buffer and read like an
`Option`:

```
counts = map_new()
counts["cat"] = 2            # insert, or replace what was there
counts["cat"]                # Some(2)
counts["dog"]                # None — the key may not be there, and that is
                             # an Option here as it is everywhere else
#counts                      # 1 — entries, not slots
```

Both type arguments are inferred; unlike a buffer's width they are ordinary
types, so nothing has to be annotated. Anything with structural equality may
be a key — a number, a string, a tuple, a data type, nested as deep as you
like — and the compiler works out how to hash it from the type, exactly as it
works out how to print it. A function or a channel cannot be a key, and says so
where the key is written.

| | |
|---|---|
| `map_new()` | an empty table |
| `m[k] = v` (or `map_set(m, k, v)`) | insert or replace |
| `m[k]` (or `map_get(m, k)`) | `Some(v)` or `None` |
| `map_or(m, k, d)` | the value or a default, without the `Option` |
| `map_has(m, k)` | whether the key is there |
| `map_del(m, k)` | remove it, saying whether there was anything to remove |
| `#m` (or `map_len(m)`) | how many entries |
| `map_clear(m)` | empty it, keeping the room it has |
| `map_keys(m)` `map_values(m)` `map_items(m)` | everything in it, as a list |
| `map_of(items)` | a table from a list of `(key, value)` pairs |

A map is reference counted like a string, and what it holds is released with
it: nothing frees a table, and nothing leaks when the last reference goes. A
table crosses a channel like any other value, and — being storage, with nothing
locking it — should be one strand's at a time on a `--parallel` build, exactly
as a buffer should.

Three things are worth knowing. **Order is the table's**: `map_keys` and its
two siblings hand back entries in whatever order the hashes put them in, which
is stable within a run and nothing to depend on across one — a program that
needs an order asks for the keys it wants, in the order it wants them.
**`map_or` allocates nothing** where `unwrap_or(m[k], 0)` builds a `Some` to
take apart again, which a loop doing it a million times can feel. And **a
table is storage**, so `println(m)` and `m == n` are refused the way they are
for a buffer; `map_items(m)` is the same table as a value, and prints and
compares like one.

The one place a type has to be said out loud is a read whose base is otherwise
unknown: `fn get(m) = m[1]` settles `m` as a string, because that is what `[]`
has always meant on an unannotated base. Write `fn get(m: Map(Int, Str)) =
m[1]`, or use `map_get(m, 1)`, which needs nothing. A *write* has no such
ambiguity — a buffer would have been annotated and a string is not storage —
so `fn put(m, k, v) = m[k] = v` is a map function without being told.

Reach for a `Map` when the program is really looking things up by name, and
for a `List` or a `Buf` when it is walking or indexing. Lookup is one hash and
a short probe: a million `Int` keys go in in about 26 ms and come back out in
about 22 on an M4, against 61 and 23 for Go's own map, and peak at 73 MB
against Go's 85.

An entry costs seventeen bytes. One is a **control byte** — whether the slot
holds anything, and if it does, seven bits of the key's hash; the other sixteen
are the key and the value, side by side. A probe walks the control bytes, which
for two million slots is two megabytes and stays in cache while the slots
themselves cannot, and touches a slot once, for the one key it believes it has
found. Keeping the whole hash beside the key would answer the same question and
cost eight bytes an entry to do it; the price of not keeping it is that growing
hashes every key again, which is about a tenth of the time it takes to fill a
table. The table is kept under three quarters full, counting deleted slots, and
grown by doubling.

---

## 7. Concurrency

Rill uses Go's model: many lightweight threads over rendezvous channels. Rill
calls one of them a **strand** — a strand of a rope, of which a program is
many, twisted together. (Go calls the same thing a goroutine, Erlang a process,
Java a virtual thread. The concept is the same; the word here is Rill's own.
Note for anyone coming from C++ networking: Boost.Asio's `strand` is a
different idea — a serialised execution context — and the two are unrelated.)

```
fn worker(jobs, results) =
  n = recv(jobs)
  send(results, n * n)
  worker(jobs, results)

fn main() =
  jobs = channel()
  results = channel()
  spawn worker(jobs, results)     # a new strand
  send(jobs, 7)
  println(recv(results))
```

- `spawn expr` runs the expression on a new strand — all of it, so
  `spawn handle(accept(s))` accepts on the *new* strand, and a loop written
  that way never waits: it spawns strand after strand, each parked on the
  accept, and spins. Bind first — `conn = accept(s)` — so the loop is the one
  that waits, and hand the connection to the strand.
- `channel()` creates a `Chan(a)`; `send(ch, v)` and `recv(ch)` rendezvous — a
  sender blocks until a receiver arrives and vice versa.
- `channel(n)` buffers `n` values, so a sender only blocks once the buffer is
  full and a receiver only blocks once it is empty.
- `select` waits on several channels at once and runs whichever arm becomes
  ready first; a `default` arm turns it into a non-blocking poll, and a
  `timeout(ms)` arm puts a deadline on the wait.
- `close(ch)` says nothing more will be sent, and wakes everything parked on
  the channel at once; `closed(ch)` asks whether it has been.
- The program exits when `main` returns; strands still running are abandoned,
  as in Go.
- A program where every strand is blocked reports a deadlock and aborts.

Strands are cheap: 100,000 of them cost about 45 MB and start in
microseconds.

```
fn drain(a, b) =
  got = select
    x = recv(a) -> x            # bind what arrives
    y = recv(b) -> y
    send(done, 1) -> 0          # a send arm is ready when it can proceed
  got

fn poll(ch) =
  v = select
    x = recv(ch) -> x
    default -> -1               # nothing was ready
  v
```

Arms are tried in order, so an earlier one wins a tie. Like `match`, `select`
needs an indented block, so it goes in a body, a statement or a binding rather
than inside parentheses.

### A deadline on a wait

A `timeout(ms)` arm becomes ready when nothing else has, for that long:

```
fn ask(ch, ms) =
  r = select
    v = recv(ch) -> "answered " + int_to_str(v)
    timeout(ms) -> "gave up"
  r
```

Before this a `select` that had to give up needed a strand and a channel to be
its clock, which is the right price for a *rate* and the wrong one for a
deadline: a server putting a limit on every request in flight would pay a
strand and a channel per request, for a clock each of them looks at once. The
arm parks in the poller instead, beside the channels, and costs nothing while
it waits.

The shortest deadline is the one that answers, so a wait may carry both a
warning and a limit. A deadline already past is ready at once, which makes
`timeout(0)` a poll — `default` by another name, and the better spelling where
the number is the point. Writing `default` and `timeout` in the same `select`
is refused, because `default` means "do not wait" and the deadline could then
never be reached. With no channel arm at all it is a sleep with a body, which
falls out of the rule rather than being a special case.

`examples/timeouts.rill` is all five of those.

### Stopping

A strand parked on `recv` is parked until a value arrives. So tearing down
whatever was going to send one leaves the strand exactly where it is, holding
its stack, for as long as the program runs — and a server that has a strand per
connection has as many of those as it has had connections. `close` is what
there was no way to say.

```
fn produce(ch, n) =
  if n <= 0 then close(ch)
  else
    send(ch, n)
    produce(ch, n - 1)

fn drain(ch, acc) = match recv_opt(ch)
  None -> acc
  Some(v) -> drain(ch, acc + v)
```

Closing is a **broadcast**, which is the thing sending a value cannot be: it
reaches everyone parked on the channel at once, however many that is, and a
program tearing down does not have to know the number. Closing twice is not an
error, so a teardown that runs from two directions does not have to arrange
which of them gets there first. What was already buffered survives, so a
producer may close the moment it has nothing more to say and the consumer still
sees everything in flight.

`recv_opt(ch)` is `recv` for a channel that ends: `Some(v)` while there is
something, `None` once it is closed and empty. `recv` itself still answers the
element type, and that is a decision rather than an oversight — most channels
never end, and making every one of them answer an `Option` would put a `match`
in front of every receive in the language to pay for the few that do. A `recv`
on a closed channel ends the program and says to use `recv_opt`, which is the
same answer an index outside a buffer gets and for the same reason: there is
no value to hand back that would not be a lie.

Sending into a closed channel ends the program too. The rule that avoids it is
the one Go arrived at — **whoever sends is whoever closes** — and where a
strand really is parked on a send that nobody will ever take, `select` is how
it is told:

```
fn push(out, stop, n) =
  r = select
    send(out, n) -> push(out, stop, n + 1)
    recv(stop) -> "gave up"
  r
```

A closed channel makes its receive arm ready, so `close(stop)` reaches that
strand whatever the other arm is doing. The arm binds nothing, and that is what
makes it work: there is no value for a closed channel to produce. An arm
written `x = recv(stop) ->` would have to produce one, and ends the program
saying so.

`chan_wait` and `chan_taken` are the two builtins `recv_opt` is written from,
in the way `map_get` is written from `map_find` and `map_val_at`. They are
there for a receive of a shape the prelude did not think of; a program that
wants `recv_opt` should write `recv_opt`.

### Being told to stop

`wait_signal()` parks until the program is asked something, and answers which:
`sig_hup()`, `sig_int()` or `sig_term()`, whose numbers are 1, 2 and 15 on
every Unix. It is the other end of the same idea as `close`, and the two are
written together:

```
fn shutdown(done) =
  wait_signal()
  close(done)
```

One strand waits and tells the others, so the waiter does not have to know how
many others there are. Waiting costs a worker nothing — the strand is parked in
the same poller a socket would have parked it in — and a program whose only
strand is waiting for a signal is not a deadlock, because it is waiting on
something that answers.

A signal arrives on whichever thread the operating system picked, between any
two instructions, possibly while a strand holds a lock. Nothing a program has
written may run there, and nothing does: the handler writes one byte to a pipe
and everything else happens on the strand.

Two things follow from how signals work rather than from anything Rill decided.
The handlers are installed by the **first** `wait_signal`, so a signal sent
before that does what it always did — and after it, a program that catches an
interrupt and ignores it cannot be stopped with one. And `SIGUSR1` and
`SIGUSR2` are not here: they are 30 and 31 on a Mac and 10 and 12 on Linux, and
a number meaning one thing where it was written and another where it runs is
worse than not having it. `SIGHUP` is the traditional "read your configuration
again" and covers what a server would have wanted them for.

Sixteen strands may wait at once, each on a pipe of its own; the seventeenth is
told to have one wait and close a channel the others watch, which is what it
should have been doing.

### Waiting on the clock

`sleep_ms(ms)` parks the strand for that long and hands its worker to whoever
else is runnable — the same manoeuvre a strand waiting on a socket makes, in
the same poller. A thousand strands can be asleep at once and the program is
using no processor at all; a program whose every strand is asleep is not a
deadlock, because it is waiting on something that always answers.

There is no `Ticker` type, because it is three lines and a channel:

```
fn ticker(ch, ms) =
  sleep_ms(ms)
  send(ch, 1)
  ticker(ch, ms)
```

Give it a `channel(1)` and it composes with everything else through `select`,
which is what a loop that has to answer a client *and* keep a rate needs.

Ask for the time with `now_nanos()`, which is monotonic — only differences
mean anything. A fixed-rate loop should sleep the distance to its next
deadline rather than a constant, because the operating system's timers have
slack in them and a constant accumulates it:

```
fn tick(next, period) =
  now = now_nanos()
  sleep_ms(if next > now then (next - now) / 1000000 else 0)
  # ... a tick's work here ...
  tick(next + period, period)
```

### Running on several cores

By default every strand runs on one OS thread, which is what lets reference
counting be non-atomic and therefore fast. Programs that want real parallelism
opt in twice — once at build time, once at run time:

```sh
rill build main.rill -o main --parallel   # refcounts become MT-safe
RILL_THREADS=8 ./main                     # run on eight workers
```

`--parallel` makes reference counting check a flag and use atomic updates when
more than one worker is active; without it the runtime refuses `RILL_THREADS`
and says so. Workers steal strands that have not started yet from each
other; one that has already run stays on its worker, because its stack frames
live at that worker's addresses.

Parallelism helps when strands compute independently. Work that is a chain
of rendezvous — one strand handing a value to the next — has nothing to
overlap and runs fastest on a single worker. A library that wants to split a
big piece of work asks `workers()` how many there are and spawns that many
strands over ranges of it; `lib/num/grid.rill` does this for anything over a few
million elements, and never spawns on a single-worker run.

A program that spawns anything is compiled with a check at every recursive
tail call, so a strand that loops forever hands the worker over now and then
(§7 above). A `for` loop whose body calls nothing is bounded by its range
and is left alone, which is what keeps a loop over ten million floats in
vector instructions; a `while` is bounded by nothing but its condition and
is checked whatever it calls, so a strand waiting in one for something
another strand sets does not wait forever. A long `for` that should share
its worker says so itself, between chunks of the work: `yield()` gives up
the turn if another strand is waiting on the worker, and returns at once if
none is.

### What strands may share

Storage — a `Buf`, a `Map`, a `StrBuf`, or anything holding one — is the
one kind of value two strands can disagree about: a write from one under a
read or a write from another is a race, and on several workers a race is
what the machine makes of it. So a `--parallel` build checks what strands
share, before the program runs, and `rill check --parallel` asks the same
question of a file:

- A value handed to a strand — captured by a `spawn`, or sent down a
  channel — and **not used afterwards** by the side that gave it is the
  strand's alone. Anything goes.
- One that **is** used afterwards is shared. Sharing to read is fine: three
  strands summing thirds of one buffer while the fourth reads it too. Sharing
  and **writing** — the strand writes it, or the strand reads it while the
  side that kept it writes — is refused, naming the value and the line.
- A **function value is opaque**: what it holds and writes is not in its
  type, so a closure shared between strands is refused unless the program
  says what it knows about it.

What the program knows is said with `apart(x)`, which is `x` and a claim:
that this value is shared on purpose and the writes to it are kept apart —
each strand its own range of a buffer, one writer and readers of a word, a
closure that reads what it holds. The compiler cannot check the claim; it can
make sure there is one to find. `spawn work(apart(f), lo, hi, done)` is how
`lib/num/grid.rill` splits an operation across the workers, and how `lib/http`
hands every connection's strand the one handler.

```
fn fill(b: Buf(Int), v) = for i in 0..#b
  b[i] = v

fn main() =
  b = buf_int(8)
  spawn fill(b, 1)      # refused: `b` is written by this strand and still used here
  spawn fill(b, 2)
  println(int_to_str(b[0]))
```

The check follows a value through bindings, fields, matches and calls that
answer with what they were given, and knows which of a function's
parameters it writes, through every call. Where it cannot see — an extern,
a method call, a closure, a runtime call handed storage — it assumes a
write, so what it refuses may be safe, and what it accepts without a claim
is: no strand writes storage another can still reach, unless `apart` says
so somewhere in the program. A one-worker build interleaves strands only
where they wait, so the same program is not checked there.

---

## 8. The prelude

Every program has these in scope; any definition of your own with the same name
replaces the prelude's.

**Types**: `Option(a)` = `None | Some(a)`, `Result(a, e)` = `Ok(a) | Err(e)`,
`List(a)` = `Nil | Cons(head, tail)`.

**Lists**: `map` `filter` `fold` `len` `sum` `range` `reverse` `append` `take`
`drop` `nth` `any` `all` `find` `index_of` `has` `count` `last` `each`
`zip` `zip_with` `unzip` `fst` `snd` `enumerate` `take_while` `drop_while`
`partition` `flatten` `flat_map` `chunks` `distinct` `group_by` `min_by`
`max_by` `sort` `sort_by` `sort_with`

`sort` is a stable merge sort by `<`; `sort_by(l, key)` orders by what `key`
gives, and `sort_with(l, before)` by `before(a, b)`, whether `a` may come
before `b` — `<=`, not `<`, or equal elements would swap. `group_by(l, key)`
gives `(key, members)` pairs in the order the keys were first seen. Each of
these stands on its own, so a program that defines a `find` of its own loses
the prelude's `find` and nothing else.

**Option/Result**: `unwrap_or` `opt_map` `res_map` `ok_or` `map_err`

**Maps**: `map_get` `map_or` `map_has` `map_keys` `map_values` `map_items`
`map_of`

**Channels**: `recv_opt(ch) -> Option(a)`

**Signals**: `sig_hup()` `sig_int()` `sig_term()` — the numbers `wait_signal`
answers with

**Standard error**: `eprintln(x)` `eprint(x)` — what `println` and `print`
do, on standard error, unbuffered

**Numbers**: `max` `min` `abs`

**Strings**: `join` `repeat` `split` `lines` `trim` `contains` `starts_with`

**Strands**: `apart(x)` — `x`, and the claim that strands share it on
purpose; see [What strands may share](#what-strands-may-share)

**Characters**: `char_count` `char_at` `char_sub` `char_take` `char_drop`
`chars` `from_char` `from_chars` `char_decode` `char_wide` — see
[Characters](#characters); `chr` is a byte

**IO**: `args()` (program name included), `read_file(path) -> Result(Str, Str)`,
`open_file(path, mode) -> Result(Int, Str)`, `dir_names(path) -> List(Str)`,
`replace_file(path, text) -> Result(Int, Str)`

---

### The libraries

What the prelude does not have is in `lib/`, on the search path of every
program in the project, imported by path and usually under a name:

| | |
|---|---|
| `import "text/str" as s` | strings past the prelude: searches, transforms, parsers, every encoding, strands over a long text |
| `import "data/json"`, `"yaml"` | the two formats, read and written |
| `import "web/http" as h`, `"ws"` | an HTTP/1.1 server with middleware, and WebSocket over it |
| `import "web/tls" as tls` | TLS over a socket through OpenSSL: `https://` as a client, verified, and served with a certificate |
| `import "web/url" as url` | a URL taken apart and put together, a query as pairs, a reference resolved against a base |
| `import "web/mime" as mime` | the media type a file is served as, from its name |
| `import "web/form" as form` | what a `<form>` sends: urlencoded pairs, or multipart parts with their files |
| `import "net/udp" as udp` | datagrams, sent and received whole, a receive parking the strand |
| `import "net/dns" as dns` | a name to its addresses through the system's resolver, without holding a worker |
| `import "num/grid" as g` | arrays of numbers with a shape, as NumPy has them |
| `import "db/kv"` | a key–value store on disk: a B+ tree, a page cache, a log |
| `import "crypto/sha1"`, `"passwd"` | SHA-1 and base64; storing a password so it cannot be read back |
| `import "crypto/sha2" as sha` | SHA-256 and SHA-512, HMAC over each, a tag compared in constant time |
| `import "data/checksum" as ck` | CRC-32 and Adler-32, whole or in pieces |
| `import "data/csv" as csv` | CSV as RFC 4180 has it, read to rows or records and written back |
| `import "data/deflate" as z` | DEFLATE both ways, and gzip and zlib around it |
| `import "time/clock" as clock` | elapsed time, a timed call, a duration in words |
| `import "time/date" as date` | civil dates and times in UTC: the counts, the calendar, ISO 8601 |
| `import "text/regex" as re` | regular expressions: a backtracking matcher with groups |
| `import "text/path" as path` | file paths as text: join, take apart, normalize |
| `import "text/fmt" as fmt` | a template with slots, padding by width, a float to so many places, thousands, bytes, ordinals, a table |
| `import "random/rnd" as rnd` | PCG32: a generator that is a value, and the draws made from it |
| `import "random/uuid" as uuid` | UUIDs, random (v4) and time-ordered (v7), from the operating system's entropy |
| `import "cli/args" as cli` | what a program takes on its command line, said once, parsed and explained |
| `import "cli/env" as env` | environment variables, the working directory, the machine's name, the process id |
| `import "cli/proc" as proc` | running a command: its output and exit code, a line built with each argument quoted |
| `import "cli/log" as log` | a logger that is a value: the time, the level, the name, on standard error |
| `import "num/decimal" as dec` | decimals with a fixed number of places, for money |
| `import "num/bigint" as big` | integers of any size |
| `import "coll/set" as set` | a set of any key-able value, as storage like `Map`; union, intersection, difference |
| `import "coll/heap" as heap` | a priority queue that is a value: a leftist heap, by `<` or by an order given |
| `import "coll/deque" as dq` | a queue open at both ends that is a value; constant time on average at either |
| `import "db/oxidb"` and friends | clients for OxiDB's engines |

Each module says at its head what it is for and how it is used, keeps its
helpers `private`, and carries its own tests: `rill test lib/text/regex.rill`
runs them.

## 9. Sockets

TCP, with the scheduler doing the waiting. A strand blocked on a socket
parks and its descriptor goes to the operating system's poller, so a worker is
never held up by a connection that has nothing to say.

```
fn handle(conn) =
  msg = sock_read(conn)
  if str_len(msg) == 0 then sock_close(conn) else reply(conn, msg)

fn reply(conn, msg) =
  sock_write(conn, "echo: " + msg)
  handle(conn)

fn serve(server) =
  conn = tcp_accept(server)
  spawn handle(conn)
  serve(server)
```

| | |
|---|---|
| `tcp_listen(port) -> Int` | listening socket, or -1 |
| `tcp_accept(server) -> Int` | next connection, parking until one arrives |
| `tcp_connect(ipv4, port) -> Int` | client socket, or -1 |
| `sock_read(fd) -> Str` | whatever has arrived; `""` means the peer closed |
| `sock_wait(fd, ms) -> Bool` | wait for something to read, or give up after `ms` |
| `sock_write(fd, s) -> Int` | writes all of it, parking as needed |
| `sock_peer(fd) -> Int` | the address at the far end, packed into one number |
| `sock_port(fd) -> Int` | the port this end is on, which is what a program asks after `listen_on(0)` — the kernel picks a free one and only it knows the number |
| `sock_nodelay(fd) -> Int` | Nagle off; 1 if the kernel took it |
| `sock_buffered(fd)` | give the socket a 64 KB write buffer: `sock_write` queues, and the queue goes out when it fills, on `sock_flush`, when the strand waits, and on `sock_close` |
| `sock_flush(fd) -> Int` | write out what is queued; how many bytes went, or -1 |
| `sock_shutdown(fd) -> Int` | end the connection without closing the descriptor — the only way one strand gets another out of a read |
| `sock_close(fd)` | |

The prelude wraps the three that can fail: `listen_on`, `accept_on` and
`connect_to` return `Result`. Addresses are dotted-quad IPv4; a name is
resolved by `net/dns`, whose lookup is a `blocking` extern, so the strand
parks for it and no worker is held. TLS is `web/tls`, over the same
sockets through OpenSSL, with `read` and `write` of the same shape.

`sock_wait` is how a connection gets a deadline. A read has none — a peer that
opens a socket and then says nothing holds a strand and a descriptor for as
long as it likes, and there is no shortage of people who will open several
thousand of those. Closing the socket from another strand is not the answer:
the reader is parked on that descriptor inside the poller, closing it from
elsewhere does not wake it, and the number can be handed to the next
connection while the old registration is still armed.

```
fn greet(conn) =
  if sock_wait(conn, 5000) then handshake(conn, sock_read(conn)) else sock_close(conn)
```

Underneath it the descriptor and a deadline are armed together and whichever
arrives first wakes the strand; a peek at the socket then says which it was.

`sock_shutdown` is the other half of that, for the case a deadline cannot
answer: one strand deciding that another strand's connection is over. Closing
the descriptor is what does not work, for the reason above. Shutting down acts
on the *connection* rather than on the descriptor, so the number stays this
program's until it closes it, and the socket becomes readable-at-end-of-file at
once — which is a readiness event, so the poller wakes the parked strand and
its `sock_read` answers `""`. The reader closes the descriptor itself, on its
own strand, where that has always been safe. A server dropping every connection
on the way down now has something to call.

`sock_nodelay` is for a server that sends small messages on a clock. Nagle's
algorithm holds a small segment until the previous one has been acknowledged,
which is the right trade for a file being copied and the wrong one the moment
the clock is faster than the round trip: every message then waits on the one
before it, and the wait is the difference between the two. It is a call rather
than a default because it is a real trade — a program that writes in small
pieces and does not care when they land is better off leaving it on.

```
fn serve(conn) =
  sock_nodelay(conn)
  greet(conn)
```

`sock_buffered` is the other half of that trade, for a program that writes
many small pieces before it waits for anything: a reply built from a dozen
`sock_write`s is a dozen system calls, and with a buffer it is one. The
buffer never holds anything back while the program is waiting — a read, a
`recv`, a `sleep_ms`, any park writes every pending buffer first — so a
request-and-reply loop behaves exactly as before, and gains nothing; the
gain is on the side that streams. Buffering is per socket and opt-in
(nothing changes for a socket that has not asked), and a `--parallel` build
with several workers writes straight through, as the buffer is one
worker's. `sock_flush` is for the moment a program wants the bytes gone
without waiting for anything itself.

Everything above TCP is ordinary Rill: `lib/web/http.rill` is an HTTP/1.1
server in about a hundred lines — request parsing, routing, responses, and
keep-alive — and it answers `curl`. Open the listening socket before spawning
the accept loop, or the first client can arrive before the socket exists.

That server outruns Go's `net/http` on the same one-route service: 214k req/s
against 198k with keep-alive, in a 52 KB binary and 7× less memory
(`benchmarks/http/run.sh`).

## 10. Files

`read_file` and `write_file` are for a file a program wants all of at once.
This section is for the other kind: a file too big to hold, or one being
appended to for as long as the program runs — a log, a journal, a store whose
readers are at different places in it.

A file is a descriptor, the same `Int` a socket is, and for the same reason:
the two families are then written alike, and a program holding both is not
holding two ideas of what an open thing is.

```
fn append(path, record) =
  fd = file_open(path, "a")
  at = file_size(fd)
  file_write(fd, record)
  file_sync(fd)
  file_close(fd)
  at
```

| | |
|---|---|
| `file_open(path, mode) -> Int` | a descriptor, or -1. `"r"` `"w"` `"a"`, and each with `+` for reading as well: `"w"` truncates or creates, `"a"` appends and creates |
| `file_read(fd, n) -> Str` | up to `n` bytes from where the descriptor is; `""` at the end |
| `file_pread(fd, off, n) -> Str` | up to `n` bytes from `off`, leaving the descriptor where it was |
| `file_pread_into(fd, off, buf, at, n) -> Int` | the same, into `n` bytes of a `Buf(U8)` from `at`, with no string made: how many arrived, or -1 |
| `file_write(fd, s) -> Int` | writes all of it; how many bytes went, or -1 |
| `file_pwrite(fd, off, buf, at, n) -> Int` | `n` bytes of a `Buf(U8)` from `at` to offset `off`, leaving the descriptor where it was; all of it, or -1 |
| `file_lock(fd, exclusive) -> Bool` `file_unlock(fd) -> Bool` | the whole-file advisory lock (`flock`), taken at once or not at all — a lock another process holds is `false`, not a wait |
| `file_seek(fd, off) -> Int` | move to an absolute offset |
| `file_pos(fd) -> Int` `file_size(fd) -> Int` | where it is, and how big the file is |
| `file_truncate(fd, n) -> Bool` | cut it to `n` bytes, or grow it with zeros |
| `file_sync(fd) -> Bool` | wait until what has been written is on the disk |
| `file_close(fd)` | |
| `file_remove(path) -> Bool` `file_rename(from, to) -> Bool` `file_exists(path) -> Bool` | |
| `dir_make(path) -> Bool` `dir_list(path) -> Str` `dir_sync(path) -> Bool` | a directory: make one, its names one to a line, and make those names durable |

The prelude has `open_file`, which is `file_open` as a `Result` the way
`listen_on` is `tcp_listen` as one; `dir_names`, which is `dir_list` as a
list; and `replace_file`, which is the four steps below written once.

**`file_pread_into` and `file_pwrite` are for a page cache.** A store that
keeps its pages in one buffer and reads the same-sized block into a slot of
it wants no string made on the way, and writes a page back where it came
from without moving the descriptor. `lib/db/kv.rill` is written on the two of
them, and on `file_lock`, which is how it keeps a second process out.

**`file_pread` is what lets one file answer several readers.** A consumer
reading a log from where it left off does not move the descriptor, so it does
not disturb another consumer at a different place in the file, or the producer
appending to the end of it. A program that reads with `file_read` is a program
whose readers have to take turns.

**A write that has returned is not on the disk.** It is in the operating
system's cache, and a machine that loses power between the two loses it.
`file_sync` is the wait for the disk to say it has the bytes, and it is the
only file call slow enough — a millisecond on a good drive, ten on a bad one —
to be worth thinking about. On a Mac it is `F_FULLFSYNC` rather than `fsync`,
because `fsync` there hands the bytes to the drive and returns without waiting
for the drive to keep them, which is fast and is not durability.

**A sync does not hold the worker.** This is the one place in the section
where the concurrency model shows through. Every other file call is a syscall
that returns; a sync hands the work to a helper thread and parks the strand on
a pipe, which is the same manoeuvre a strand waiting on a socket makes and
goes through the same poller. The worker is free from the moment the request
is handed over. A strand computing beside a strand syncing takes about as long
as it takes on its own, which is what `a_sync_does_not_hold_the_worker`
measures. Before
the scheduler exists — a program that neither spawns nor waits runs `main` on
the process stack — the sync is simply done where it stands, because there is
no worker to spare and nothing to be an improvement on.

**Syncing a file does not make its name durable.** Creating a file and
renaming one both change the *directory*, and that change is cached like any
other: a program that creates a file, syncs it, and loses power can come back
to a directory that has never heard of it. `dir_sync` is the call that closes
that gap. It is separate so that a program pays for it after a create or a
rename rather than after every write.

Those two together are how a file is replaced without a moment in which it is
neither the old one nor the new one — write a temporary, sync it, rename it
over, sync the directory. A rename on a Unix filesystem is atomic, so a reader
sees one file or the other and never half of either. `replace_file` in the
prelude is those four steps; `examples/files.rill` is the rest of this
section.

Nothing here buffers. A write is a write, so a program appending a hundred
small records makes a hundred syscalls; build the bytes in a `StrBuf` and
write once, which is what the language would have you do anyway.

---

## 11. Calling C

A Rill function may have any name, C's own included: `fn read`, `fn bind`,
`fn connect` are the program's, since no Rill function is a symbol anything
outside the object can see — only `export fn` and `main` are — and the
runtime's calls to C reach C. The one name a program cannot give a function
is one it also declares `extern`, which the checker refuses.

An `extern` declaration names a C symbol and its types. The optional string is
a library to link with; leave it out for anything already in the C runtime.
A string starting with `-` is passed to the linker as written, and
`framework:Name` links a macOS framework:

```
extern "-L/opt/homebrew/lib" fn glfwInit() -> I32   # where to search
extern "glfw" fn glfwPollEvents()                   # -lglfw
extern "framework:OpenGL" fn glClear(mask: U32)     # -framework OpenGL
```

```
extern "m" fn sqrt(x: Float) -> Float
extern fn getpid() -> Int
extern fn strlen(s: Ptr) -> Int
```

Only shapes the C ABI carries may appear: `Int`, `Float`, `Bool`, `Ptr` and
`Unit`. `Ptr` is an opaque C pointer that reference counting leaves alone —
whatever produced it owns it.

`Int` and `Float` mean C's 64-bit forms, and most C libraries are not written
in those. A foreign signature can name the exact width the symbol was compiled
with — `I8`, `U8`, `I16`, `U16`, `I32`, `U32`, `I64`, `U64`, `F32`, `F64` — and
the call narrows arguments on the way in and widens results on the way back.
The Rill side is unaffected: an `I32` parameter still takes an ordinary `Int`,
and an `F32` result still comes back as a `Float`.

```
extern "m" fn sqrtf(x: F32) -> F32      # C `float`, not `double`
extern fn abs(n: I32) -> I32            # C `int`, not a 64-bit one
```

One width has no number to name it: C's `size_t` is as wide as a pointer, which
is 64 bits everywhere Rill links natively and 32 on `wasm32`. `USize` and
`ISize` are that width, filled in by whatever is being built for, so one
declaration is right on both:

```
extern fn strlen(s: Ptr) -> USize
extern fn qsort(base: Ptr, n: USize, size: USize, cmp: Fn(Ptr, Ptr) -> I32) -> Unit
```

They are for signatures, not storage: `Buf` refuses them, because a buffer's
layout is the program's to decide and this width is the target's.

Getting this wrong is not a compile error but a silently wrong answer, and the
float case is the sharp one: a `float` parameter handed a double reads the
wrong half of the register. Width names are only spellable inside an `extern`.

Raw buffers cover the other half of a C API — the array a library wants to
read or fill. They are plain C memory outside the reference counter, so they
are released with `free_ptr`, and the index is in *elements*, not bytes.

| | |
|---|---|
| `alloc_bytes(n) -> Ptr` | `n` zeroed bytes, or null if `n <= 0` |
| `ptr_offset(p, bytes) -> Ptr` | byte-level addressing, when a layout needs it |
| `poke_i8/i16/i32/i64(p, i, v)` | store an `Int`, narrowed to that width |
| `poke_f32/f64(p, i, v)` | store a `Float` |
| `poke_ptr(p, i, q)` / `peek_ptr(p, i) -> Ptr` | a pointer slot: `char**` in, a handle written by an out-parameter back |
| `peek_i8/i16/i32/i64(p, i) -> Int` | read back, sign-extended |
| `peek_u8/u16/u32(p, i) -> Int` | read back, zero-extended |
| `peek_f32/f64(p, i) -> Float` | read back a float |

```
v = alloc_bytes(3 * 4)      # three 32-bit floats
poke_f32(v, 0, 1.5)
poke_f32(v, 1, 2.5)
draw(v, 3)                  # an extern taking (Ptr, I32)
free_ptr(v)
```

Nothing checks the bounds — a buffer is a length you keep track of yourself,
exactly as in C.

### Buffers

Raw pointers are how C is reached, but inside Rill the same memory is better
had as a `Buf(w)`: a counted block of elements of one C width.

```
v = buf_f32(3 * 5)          # fifteen 32-bit floats, zeroed
v[0] = 1.5
println(v[0])
println(buf_len(v))
draw(v, 3)                  # an extern taking (Ptr, I32)
```

`#v` is how many elements it has, `v[i]` and `v[i] = x` are one of them; `buf_get(v, i)` and
`buf_set(v, i, x)` are the same two operations spelled out, and either may be
written. Indexing binds tighter than every operator, so `v[i + 1] * 2` reads
the way it looks, and `v[i] = x` may only appear as a statement — it is the
one assignment the language has, and only to a buffer element.

| | |
|---|---|
| `buf_i8` `buf_u8` `buf_i16` `buf_u16` `buf_i32` `buf_u32` `buf_int` `buf_f32` `buf_float` `buf_ptr` | one constructor per element width; the argument is a length in elements |
| `b[i]` / `b[i] = v` — or `buf_get(b, i)` / `buf_set(b, i, v)` | one element, bounds-checked |
| `#b` (or `buf_len(b)`) | how many elements |
| `buf_copy(dst, to, src, from, n)` | `n` elements of `src` from `from` into `dst` at `to`, overlapping or not, both bounds-checked |
| `buf_cmp(a, at, b, bt, n)` | `n` elements of each as bytes: -1, 0 or 1, both bounds-checked |

`buf_copy` and `buf_cmp` are `memmove` and `memcmp` with the question asked
first: a length that came from somewhere it should not have — a header on a
page that is not a header — stops at the check, with a message, rather than
in the memory past the buffer. They cost the check and nothing else, and the
check is nothing beside the copy; a program that reaches for `ptr_offset` and
an `extern` `memmove` to move bytes between buffers has no reason to.

Where a `Ptr` is wanted — a foreign parameter, a `peek`/`poke`, a Rill
parameter annotated `Ptr` — a buffer written there **is** its first element's
address (which is also what the value is underneath: the count and the
length live in the sixteen bytes before the elements). This is C's own rule for an array, and it is the only place in the
language where a value is not the type it was written as. Nothing about the
buffer changes: it stays counted, stays the caller's, and a buffer built in
the argument itself is released after the call rather than before, so the
address is good for as long as the call is.

```
b = buf_u8(8)
strlen(b)                   # extern fn strlen(s: Ptr) -> USize
from_cstr(b)                # a builtin taking a Ptr
peek_u8(b, 1)               # so are peek and poke
```

Three things follow from the width living in the type. Reading gives back the
Rill type that width stands for, so `Buf(U8)` yields 200 where `Buf(I8)` yields
-56 for the same byte. An index outside the buffer ends the program with the
index and the length, rather than reading whatever was next in memory. A
`for` loop that indexes a buffer by its own variable — `v[i]` for `i` in
`lo..hi` — is tested once, before the loop, for `lo >= 0` and `hi <= #v`,
and not again at any element; an out-of-range loop stops before it starts,
naming the index it would have reached. And a
buffer is **reference counted like a string** — it is released when the last
reference goes, so there is no `free` to forget.

The width is never inferred, since it is not a type variable: a function that
takes a buffer says which one, `fn fill(v: Buf(F32), i) = ...`. This is also
why a buffer should be held by a Rill binding rather than stashed in a raw
`Ptr` slot, which would lose the count.

`examples/buffers.rill` shows all of it, and `examples/gl/scene.rill` fills
one with the vertex data a GPU is handed. For a buffer read as a column —
scanned in ranges and reduced — `lib/num/col.rill` has the aggregates written
with their accumulators in lanes: `import "num/col" as c`, then `c.sum(v, lo,
hi)`, `c.mean`, `c.variance`, `c.min_of`, `c.max_of`, `c.dot`. For arrays
with a shape — matrices, vectors, the things NumPy does — `lib/num/grid.rill`
lays a shape and strides over one such buffer: `import "num/grid" as g`, then `g.matmul`, `g.sum_axis`,
`g.transpose` (a view, no copy) and the rest; `benchmarks/numpy/README.md`
lists what it has and how it measures.

### Callbacks

`Fn(...)` in a foreign signature is a C function pointer, and what may be
passed for one is a top-level function named directly:

```
extern fn signal(sig: I32, handler: Fn(I32)) -> Ptr

fn on_signal(n) = println("signal " + int_to_str(n))

fn main() = signal(30, on_signal)
```

A callback that answers writes its result after an arrow, and the same
narrowing runs in reverse on the way back out:

```
extern fn qsort(base: Ptr, n: U64, size: U64, cmp: Fn(Ptr, Ptr) -> I32) -> Unit

fn by_size(a, b) = peek_i32(b, 0) - peek_i32(a, 0)
```

Without the arrow the callback hands nothing back, which is the shape most
notification callbacks want. The result may be any type C can carry — a
number, a `Bool` or a `Ptr` — but not a `Str` or anything else on the heap,
which C would discard with nothing left to release it, and not another `Fn`.

The compiler wraps the function in a C-ABI trampoline that widens C's
arguments to Rill's, so the callback is written in ordinary types.

A callback may also travel as an ordinary parameter, so a helper can wrap the
registration rather than every call site repeating it:

```
fn sort_by(b: Buf(I32), cmp) =
  qsort(b, buf_len(b), 4, cmp)
  b

fn main() = sort_by(nums, ascending)
```

What crosses is still a bare code pointer, so the callback has to be settled
while compiling: the caller names it, and `sort_by` is emitted once per
comparator it is called with, each copy with its own trampoline built in. A
helper like this cannot itself be used as a value, since it exists in as many
copies as it has callers. A callback must not block on a channel, since it
runs on C's call stack rather than on its own.

A **lambda or closure** has an environment, and a bare code pointer has
nowhere to keep it. Most APIs that take a callback also take a pointer they
hand back to it unread — `qsort_r`'s `arg`, `pthread_create`'s, a library's
"user data" — and that is where it goes. The signature says so with `Env`:
the parameter the pointer goes out by, and the slot of the callback's
`Fn(...)` it comes back into.

```
extern fn rill_sum_by(n: I64, f: Fn(I64, Env) -> I64, arg: Env) -> I64

fn main() =
  k = 10
  println(int_to_str(rill_sum_by(4, \i -> i * k)))    # 60
```

The Rill call leaves the `Env` out — it is the compiler's to fill — and the
callback is written in the types that remain, `(Int) -> Int` here. What
goes out is the address of a box holding the closure, code and environment
both; what C calls is a trampoline made for the C signature, which unpacks
the box and calls the closure the way every closure is called. The box, and
the frame's hold on the closure, last the call: this is for a callback C
uses **during** the call, as a comparator or a visitor is. A callback C keeps
after the call returns — a handler registered once and called later — is
still a named function, since nothing here can know how long C keeps it.
A named function or a value chosen at run time may be passed for an `Env`
callback too. An `extern` has one `Env` parameter and one callback carrying
it, or neither.

`examples/callbacks.rill` has C calling back during `main`, after it has
returned, and into a comparator that reaches C through such a helper.
`examples/ffi_buffers.rill` exercises all of it, and
`examples/gl/` drives OpenGL through nothing else: a window, shaders, vertex
data and a spinning triangle, with no C written anywhere.

A Rill `Str` is a counted heap object rather than a `char*`, so strings cross
the boundary explicitly:

| | |
|---|---|
| `to_cstr(s) -> Ptr` | NUL-terminated copy; free it with `free_ptr` |
| `from_cstr(p) -> Str` | copies a C string back into a Rill one |
| `str_from_ptr(p, n) -> Str` | the same by length rather than by terminator, for bytes |
| `free_ptr(p)` | frees what `to_cstr` (or C `malloc`) returned |
| `null_ptr()` / `is_null(p)` | for APIs that use null |

`from_cstr` stops at the first zero, which is right for a C string and wrong
for everything a library hands back by filling a buffer and returning a count.
`str_from_ptr(p, n)` takes exactly `n` bytes, zeros included, which is what a
`read`, a decompression or an `SSL_read` gives:

```
b = buf_u8(16384)
n = SSL_read(ssl, b, 16384)
if n > 0 then str_from_ptr(b, n) else ""
```

```
fn env(name) =
  p = to_cstr(name)
  v = getenv(p)
  free_ptr(p)
  if is_null(v) then "<unset>" else from_cstr(v)
```

Two things to keep in mind: a wrong signature is a crash, not a type error —
the compiler believes what you declare; and a C call that blocks holds the
worker thread running it, since the scheduler cannot preempt foreign code —
every other strand on that worker waits with it. A call that may wait — a
read, a sleep, a name lookup — is declared **`extern blocking fn`**, and is
then made on a helper thread while the strand is parked, the worker going
on with the others; the answer comes back as if the call had been made
here. What it costs is a hand-over each way, a few microseconds, which is
why it is said per declaration and not assumed. A blocking call cannot take
a callback, since the callback would run on the helper thread, where Rill
code cannot; a program without strands makes the call itself.

```
extern blocking fn usleep(us: U32) -> I32   # the other strands run meanwhile
```

### Being called: `export`, WebAssembly and shared libraries

`extern` is Rill calling out. `export` is the other direction — a function the
embedding host may call in:

```
export fn frame(t: Float) =
  ...
```

It means something where there is a host: a wasm module (`--target
wasm32-wasip1`), or a shared library (`--shared`) that a C or Python program
loads. A native executable has neither and refuses the word. Either way the
module links as a *reactor*: it has no entry point of its own, it stays
loaded, and the host drives it. `main` still runs — as `rill_init`, which the
host calls once before anything else — and each `export fn` is a symbol under
its own name.

On wasm the boundary takes what wasm can pass: parameters must be `Int`,
`Float`, `Bool` or `Ptr`, the return those or `Unit`, and the function must be
concrete — an export is one entry point, not a generic family. Data wider than
a scalar crosses through a `Buf`: build one, hand it to an import — where a
`Ptr` is wanted a buffer is its address — and the host reads it straight out
of the module's linear memory.

A shared library's boundary is wider: a counted value — a data value, a
buffer, a string, a map — crosses as a handle. One the host passes in is lent
for the call and is the host's still when the call returns; one an export
returns is the host's to keep, and for every such type the library also
exports `rill_drop_<Type>(handle)` — `rill_drop_g_Grid` for a `Grid` imported
as `g`, the module's dot made an underscore — for the host to let it go by.
A buffer's handle is the address of its elements, so the host may read them
straight through it while it holds the grid. `bindings/python/gridlib.rill`
is a library of this kind over `lib/num/grid.rill`, and `bindings/python/grid`
the `ctypes` package that makes it `import grid as g` in Python, NumPy's
notation on Rill's loops. Strands need the scheduler a program's `main` runs
and a library's host does not, so a library runs on one worker.

On wasm an `extern fn` that no library satisfies becomes an *import* — a hole
the host fills at instantiation. That is the whole FFI story in a browser:
declare `extern fn gpu_draw(v: Ptr, n: Int)`, supply `env.gpu_draw` from
JavaScript. `examples/webgpu/` renders through WebGPU exactly this way — the
host owns the asynchronous device setup that a wasm module cannot await, Rill
owns the frame.

What a wasm build will not do is link a library by name: a module imports from
its host rather than loading a shared object, so `extern "glfw"` is refused
where it would otherwise fail obscurely at run time. `"m"` and `"c"` are the
exception, and not as a courtesy — wasi-libc is a single archive with the maths
already in it, so a program written against `-lm` on a Mac needs no second
spelling for the browser. A C name that wasi genuinely lacks — `getpid`,
`signal`, anything about processes or users — is a link error naming it, and a
declaration whose width disagrees with the C is one too rather than a stub that
traps mid-run.

### Assembly

Some of what a machine can do has no C name. An `asm fn` is a run of
instructions given the shape of a function: the same widths cross as for a C
call, because the question is the same one — what fits in a register the way
the machine expects to find it.

```
asm fn cli()                  = "cli"
asm fn read_cr3() -> U64      = "mov %cr3, $0"  : "=r"
asm fn outb(port: U16, v: U8) = "outb $1, $0"   : "{dx},{al}"
asm fn barrier()              = "dmb sy"        : "~{memory}"
```

The template and the constraint string go to LLVM as written, the way
`extern`'s library string goes to the linker as written: what the machine
accepts is the machine's business, and a declaration is true of one
architecture rather than of the language.

Leaving the constraints off asks for the ordinary shape — `=r` for a result and
`r` for each parameter, in order — which is what most instructions want.
Writing them out is for a register the instruction names itself: `outb` reads
`dx` and `al` and nothing else will do. `=` and `+` open an output, `~` a
clobber, and anything else is an input; a string that does not describe the
signature is refused with the line it is on, because LLVM's own answer to one
is to abort the compiler.

Every `asm fn` is emitted with side effects assumed. Two calls that look alike
stay two calls, and one whose result nothing reads still runs: a device
register read twice was meant twice, and `cli` is not there for its value.
There is no way to ask for the other behaviour yet.

A function written *in* assembly says `naked`, and then the instructions are
the whole of it: no prologue saving what a caller expected saved, no epilogue
returning anywhere. This is what an interrupt handler, a reset vector and a
context switch are.

```
naked asm fn isr_timer() = "push %rax\n  ...\n  pop %rax\n  iretq"
naked asm fn _kstart()   = "mov $0x1000, %rsp\n  call kmain\n  hlt"
```

It takes nothing, answers nothing and has no constraints, because there is no
calling convention left to carry any of them — what enters it is an interrupt
or a jump somebody else wrote. Nothing in Rill may call one, and the error says
so. What a program can use is the address, and that is an ordinary instruction:

```
asm fn isr_addr() -> Ptr = "leaq isr_timer(%rip), $0" : "=r"
```

Inside a naked function `$` is the literal the assembler wants rather than an
operand reference, so an immediate is written `$0x1000`, the way the manual has
it.

On wasm there is no such thing: a module is verified before it runs and has no
place for a machine's own encoding, so an `asm fn` is refused when building for
one.

## 12. Built-in functions

| | |
|---|---|
| `println(x)` / `print(x)` | print any value type, with or without a newline; standard output is buffered like C's — a line at a time on a terminal, 64 KB otherwise — and written out before the program ends, before `exit`, before a read of standard input and before a strand waits |
| `show(x) -> Str` | render any value as a string |
| `int_to_str` `float_to_str` `bool_to_str` `str_to_int` | conversions |
| `to_float(n)` `to_int(x)` | between `Int` and `Float`; `to_int` truncates toward zero |
| `#s` (or `str_len(s)`) `s[i]` (or `str_get(s, i)`) `s[i:j]` (or `str_sub(s, start, len)`) `str_find(s, needle)` `str_find_from(s, needle, from)` `str_cmp` `chr(byte)` | strings, byte-indexed; a string reads by index but never writes — it is a value, not storage. `str_find_from` starts partway along, so a long line can be walked field by field without slicing off the part already read; a start outside the string is clamped |
| `channel()` `send(ch, v)` `recv(ch)` `close(ch)` `closed(ch)` | channels; `close` wakes everything parked on one, and the prelude's `recv_opt` is written from `chan_wait` and `chan_taken` — see §7 |
| `map_new()` `m[k] = v` (or `map_set`) `map_len` `map_del` `map_clear` | hash tables; `#m` is `map_len`, and the prelude's `map_get`, `map_or`, `map_has`, `map_keys`, `map_values` and `map_items` are written on top of these |
| `map_find(m, k)` `map_next(m, i)` `map_key_at(m, i)` `map_val_at(m, i)` | the slot a key is in, the next slot with anything in it, and what is in one — what the prelude's map functions are made of, and what to write a walk of your own with. A slot number means nothing after the next insertion |
| `arg_count()` `arg(i)` | raw command-line access (`args()` is friendlier) |
| `yield()` | this strand's turn given up, if another is waiting on the worker — for a long `for` loop with no call to be checked at (§7) |
| `assert(c)` `assert_eq(got, want)` | end the program where the condition is false, saying the line — and, for `assert_eq`, both values; what `rill test` is written with (§13) |
| `read_file_raw(path)` `write_file(path, text)` `read_line()` `scan_int()` `io_ok()` | file and stdin IO; standard input is read in blocks, and `scan_int` scans a number out of it without allocating |
| `file_open` `file_read` `file_pread` `file_pread_into` `file_write` `file_pwrite` `file_seek` `file_pos` `file_size` `file_truncate` `file_sync` `file_lock` `file_unlock` `file_close` `file_remove` `file_rename` `file_exists` `dir_make` `dir_list` `dir_sync` | a file as a descriptor, for one too big to hold or still being appended to; see §10 |
| `strbuf()` `b += s` (or `strbuf_add`) `b += byte` (or `strbuf_byte`) `strbuf_str(b)` `strbuf_clear(b)` `#b` | building a string a piece at a time; `strbuf_clear` empties one and keeps its room |
| `to_cstr` `from_cstr` `str_from_ptr` `free_ptr` `null_ptr` `is_null` | the C string and pointer bridge |
| `alloc_bytes(n)` `ptr_offset(p, bytes)` `poke_*` `peek_*` | raw buffers for C |
| `now_nanos()` `sleep_ms(ms)` | a monotonic clock, and a wait that parks the strand |
| `wait_signal()` | parks until the program is told something, and says which; see §7 |
| `workers()` | how many workers this run has — 1 unless `RILL_THREADS` said otherwise — for deciding how many strands a big piece of work is worth |
| `unique(x)` | whether this frame holds the only reference to a counted value; see §6 |
| `float2(a, b)` `splat2(x)` `lane0(v)` `lane1(v)` `f2_load(buf, i)` `f2_store(buf, i, v)` `fma2(a, b, c)` `fmin2(a, b)` `fmax2(a, b)` | two lanes at once; see §3 |
| `buf_float_uninit(n)` | a buffer of floats whose elements are whatever the memory held, for a caller about to write every one — no clearing pass |
| `exit(code)` | terminate |

---

## 13. The toolchain

```sh
rill build main.rill -o main    # compile (LLVM -O3, stripped)
rill build main.rill --debug    # no optimization, DWARF line info for lldb
rill build main.rill --unchecked  # no bounds test on buffers: run it checked first
rill build main.rill --shared   # a .dylib/.so whose `export fn`s a C or Python host calls
rill build main.rill --emit-ir  # print LLVM IR
rill run main.rill              # compile and run
rill check main.rill            # type-check only, saying everything it finds
rill types main.rill            # the signature the checker worked out for each function
rill explain main.rill          # the memory decisions the generator made, by line
rill test tests.rill            # run every `test_...` function, each in its own process
rill fmt src/*.rill             # canonical formatting (--check, --stdout)
rill repl                       # interactive: definitions persist
rill lsp                        # a language server on stdin/stdout
```

### Tests

A test is a function named `test_...` that takes nothing, written beside
the code it tests or in a file that imports it. `rill test file.rill` builds
the file once, with a `main` of its own that runs the test its first
argument names — the file's `main`, if it has one, stands aside — and runs
each test in a process of its own, so one that fails is one test failing
and not the rest:

```
fn test_add() = assert_eq(add(2, 2), 4)

fn test_rev() =
  assert_eq(reverse(Cons(1, Cons(2, Nil))), Cons(2, Cons(1, Nil)))
  assert(len(reverse(Nil)) == 0)
```

```
$ rill test testing.rill
ok    test_add
FAIL  test_rev
      rill: assertion failed at line 5
        expected: Cons(2, Cons(1, Nil))
             got: Cons(1, Cons(2, Nil))
2 tests: 1 passed, 1 failed
```

`assert(c)` ends the test where `c` is false, saying the line; `assert_eq(got,
want)` says both values too, as `show` would. Nothing is caught, because
nothing has to be: a test that failed is a program that ended, and a fault
or a deadlock in one is reported the way any program's is, with the test
named above it and whatever it printed below. `rill test file.rill test_rev`
runs one; `--parallel` builds the tests as a `--parallel` program would be.

### When a program dies

A program that faults — an address that is nobody's through an `extern` or a
`Ptr`, a strand that runs off the end of its stack, an instruction that is
not one — still dies: nothing in it can be resumed from the middle of a write
it did not finish, and the strand that faulted may hold a channel the others
wait on. But it says what happened first, on standard error:

```
rill: bus error at 0x104f27ff0 - a stack overflow: the strand ran off the end of its stack
  in a strand started at lambda$0, worker 0
  in dive +119
  ... the same, 99999 more times
```

Which signal and where, whether the address is a stack's guard page, which
strand and which worker, and the Rill functions on the stack innermost
first — by name, in a release build as in a debug one, from a table of every
function's start and name the compiler leaves in the binary (two pointers a
function). A bounds check that fails, or a division by zero, says the same
about where it was. Runtime
frames are left out; a recursion that ran away is one line and a count. With
`RILL_CRASH_WAIT=1` in the environment the program then waits, saying its
pid, for a debugger to attach; otherwise the fault runs again under the
system's default, so a core is written if one was asked for and the exit
status is the one a faulting process has always had.

A fault that is a *consequence* — a block freed under a holder, read later
by whoever was handed it next — is found with the allocator's checks.
`RILL_ALLOC_CHECK=1` at run time paints every freed block, keeps the frames
of its last two frees, and reports a block handed out or freed twice, or
one whose count was touched after its free, naming the freer.
`RILL_ALLOC_CHECK=2` gives every small block a page of its own and makes
it inaccessible at the free, so the stale holder faults on its own
instruction — slow, a system call per allocation, for when the report
above is not enough. `RILL_RC_CHECK=1` in the environment of `rill build`
compiles a program whose every count update first asks the runtime
whether the block is out at all, and a touch of a freed block is reported
in the toucher's frames with the block's history: every retain, release,
hand-out and free, by worker and by frame. The checks live in the
runtime's allocator, and a `--parallel` build under them sends every
block there; a plain build pops and pushes the pool's blocks inline, and
is told the checks cover only what the runtime allocates.

Where a program's memory *is*, as against where it went wrong:
`extern fn rill_mem_report() -> Int` writes one line to standard error —
blocks out, the pool's chunks and how much of them sits on free lists,
the big blocks kept for reuse, the strand records and the stacks they
have saved — and the free lists by size where one holds a megabyte or
more. A server's `/stat` route is the place for it: the catalogue example
found 195 MB on one free list that way.

`rill types` is the answer to the question an inferring language raises: what
did it decide? It prints every function in the file as the checker sees it,
including the requirements a body imposed — `fn merge(a: List('a), b: List('a))
-> List('a)    # where 'a is ordered` for a function whose source says nothing
about being ordered.

`rill lsp` is the same knowledge, kept open for an editor: diagnostics on the
buffer as it stands, the type of the name under the cursor, go to definition,
an outline, and formatting. `editors/README.md` has the three lines each that
Neovim, Helix and Emacs need to start it.

---

## 14. What Rill deliberately does not have

- **Mutation.** No mutable bindings, no mutable fields. State is threaded
  through arguments or channels. The exceptions are the two kinds of storage:
  a buffer element, `b[i] = v` (and `+=`, `-=`, `*=`, `/=`), and a map entry,
  `m[k] = v`. Neither is a binding that changes — what changes is the storage
  the name has always pointed at.
- **Exceptions.** Errors are `Result` values, and `?` carries one out without
  the carrying being written.
- **Null.** Absence is `Option`.
- **Implicit conversions.** Numeric types never mix silently.
- **Field assignment.** A field is read with `p.x` and never written: a
  record is a value, and a changed one is a new one built from the old.
- **Inheritance or subtyping.** Composition and traits instead.
- **A garbage collector.** Reference counting, decided at compile time.

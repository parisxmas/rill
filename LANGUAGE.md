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
the program exits when it returns.

An import brings the other file's definitions in under their own names. Adding
an alias keeps them apart instead, which is how two modules can define the same
name:

```
import "metric" as m
import "imperial" as i

fn main() = println(m.label(m.Meters(1.5)) + " " + i.label(i.Feet(2.0)))
```

`m.label` and `i.label` are different functions, and `m.Unit` and `i.Unit` are
different types.

### Layout

Blocks are written by indentation — two spaces per level, spaces only, no tabs.
A construct may continue on the next line only where the grammar allows it
(`then` and `else` may start a line). Inside parentheses indentation is ignored,
so anything written there stays on one line.

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
| `Unit` | — | what `println` and `main` return |
| `Chan(a)` | `channel()` | see [Concurrency](#7-concurrency) |
| `StrBuf` | `strbuf()` | bytes on their way to a string |
| `(A, B) -> C` | `\x, y -> ...` | function values |
| user types | `Circle(1.0)` | declared with `type` |

Types are inferred; annotations are optional and only ever needed to resolve an
ambiguity or to document intent:

```
fn area(r: Float) -> Float = 3.14159 * r * r
```

There are no implicit conversions. `1 + 2.0` is a type error; `to_float` and
`to_int` cross between them explicitly.

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

Field names are optional documentation (`Rect(Float, Float)` is equivalent).
Constructor names are global and must start with an uppercase letter; a type and
one of its constructors may share a name.

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
```

Bit operations are functions rather than operators: `bit_and`, `bit_or`,
`bit_xor`, `bit_not`, `shl` and `shr` (`shr` is logical, since protocol code
works with bit patterns).

Precedence, loosest to tightest: `|>`, `||`, `&&`, comparisons, `+ -`, `* / %`,
unary `- !`, calls and literals.

### Tuples

`(a, b)` hands back more than one thing without declaring a type for it:

```
fn divmod(a, b) = (a / b, a % b)

fn main() =
  (q, r) = divmod(17, 5)      # a binding that takes the value apart
  println(q)
```

A tuple holds two to four values, and `(A, B)` is its type. It is an
ordinary data type underneath — `Tuple2`, `Tuple3`, `Tuple4` in the prelude —
so it compares, shows and pattern-matches like any other, and `(n, s) -> ...`
is a pattern anywhere a pattern goes.

A binding may take apart any type with exactly one constructor:
`Parsed(v, rest) = parse(s)` reads as a binding because it cannot fail. On a
type with more than one, the compiler says the match is not exhaustive, which
is the truth: use `match`.

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
builder stays usable.

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

### Blocks and bindings

An indented block runs its lines in order and evaluates to the last one:

```
fn area_of_two() =
  a = area(Circle(1.0))     # binding, visible for the rest of the block
  b = area(Rect(2.0, 3.0))
  a + b                     # the block's value
```

Bindings are immutable. There is no assignment operator.

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

---

## 4. Functions

```
fn add(a, b) = a + b                    # inferred as Int, Int -> Int
fn scale(v: Float, k: Float) = v * k    # annotated
fn apply(f, x) = f(x)                   # higher-order
```

Functions are values: pass them by name (`map(xs, double)`) or as lambdas.
Closures capture the variables they use by value.

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
ordered is a compile error naming the function and the type.

Two limits worth knowing: a function used *before* its definition is pinned to
the types of that use, and there is no polymorphic recursion.

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

Both forms are spelled out as the tail-recursive function below before
anything else looks at them — the end of the range is evaluated once, and what
the loop carries is a parameter, which is to say a register. Carrying it in a
one-element buffer instead, which is what a program without this had to do,
puts it in memory and reaches through a pointer for every touch. `+=`,
`-=`, `*=` and `/=` write a buffer element in terms of what it held:
`v[i] += 1`. The index is evaluated once, so `off[eu[i]] += 1` reads `eu[i]`
one time.

Everything else is tail recursion: a call in tail position compiles to a
jump, so it runs in constant stack, exactly like a loop.

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

A local function is typed together with the one it is written inside, which
is what lets a captured buffer keep its width. The price is that it has one
type rather than a scheme: a helper used at two different types has to be a
top-level function, where it generalizes as usual.

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

Method names are global (like constructors). Dispatch is resolved statically
during monomorphization: no dictionaries, no vtables, and a compile error if an
implementation is missing.

An impl may target a generic type by naming its parameters, in which case it
covers every instantiation:

```
impl Show(List(a))
  fn show(l) = "{" + join(map(l, \x -> show(x)), " ") + "}"
```

The `show(x)` on elements resolves when the element type is known, so
`List(Box(Int))` composes both impls without any `where` clause. An impl that
quietly works for only one instantiation is rejected.

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

Run any program with `RILL_DEBUG_ALLOC=1` to have the runtime print the number
of live allocations at exit.

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
  `spawn handle(accept(s))` accepts on the *new* strand. Bind first if the
  caller should do that part.
- `channel()` creates a `Chan(a)`; `send(ch, v)` and `recv(ch)` rendezvous — a
  sender blocks until a receiver arrives and vice versa.
- `channel(n)` buffers `n` values, so a sender only blocks once the buffer is
  full and a receiver only blocks once it is empty.
- `select` waits on several channels at once and runs whichever arm becomes
  ready first; a `default` arm turns it into a non-blocking poll.
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
overlap and runs fastest on a single worker.

---

## 8. The prelude

Every program has these in scope; any definition of your own with the same name
replaces the prelude's.

**Types**: `Option(a)` = `None | Some(a)`, `Result(a, e)` = `Ok(a) | Err(e)`,
`List(a)` = `Nil | Cons(head, tail)`.

**Lists**: `map` `filter` `fold` `len` `sum` `range` `reverse` `append` `take`
`drop` `nth` `any` `all`

**Option/Result**: `unwrap_or` `opt_map` `res_map`

**Numbers**: `max` `min` `abs`

**Strings**: `join` `repeat` `split` `lines` `trim` `contains` `starts_with`

**IO**: `args()` (program name included), `read_file(path) -> Result(Str, Str)`

---

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
  spawn handle(tcp_accept(server))
  serve(server)
```

| | |
|---|---|
| `tcp_listen(port) -> Int` | listening socket, or -1 |
| `tcp_accept(server) -> Int` | next connection, parking until one arrives |
| `tcp_connect(ipv4, port) -> Int` | client socket, or -1 |
| `sock_read(fd) -> Str` | whatever has arrived; `""` means the peer closed |
| `sock_write(fd, s) -> Int` | writes all of it, parking as needed |
| `sock_close(fd)` | |

The prelude wraps the three that can fail: `listen_on`, `accept_on` and
`connect_to` return `Result`. Addresses are dotted-quad IPv4 — resolving a name
would block, and nothing in the runtime may block a worker.

Everything above TCP is ordinary Rill: `examples/http/http.rill` is an HTTP/1.1
server in about a hundred lines — request parsing, routing, responses, and
keep-alive — and it answers `curl`. Open the listening socket before spawning
the accept loop, or the first client can arrive before the socket exists.

That server outruns Go's `net/http` on the same one-route service: 214k req/s
against 198k with keep-alive, in a 52 KB binary and 7× less memory
(`benchmarks/http/run.sh`).

## 10. Calling C

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
draw(buf_data(v), 3)        # an extern taking (Ptr, I32)
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
| `buf_data(b) -> Ptr` | the first element's address, for C |

Three things follow from the width living in the type. Reading gives back the
Rill type that width stands for, so `Buf(U8)` yields 200 where `Buf(I8)` yields
-56 for the same byte. An index outside the buffer ends the program with the
index and the length, rather than reading whatever was next in memory. And a
buffer is **reference counted like a string** — it is released when the last
reference goes, so there is no `free` to forget.

The width is never inferred, since it is not a type variable: a function that
takes a buffer says which one, `fn fill(v: Buf(F32), i) = ...`. This is also
why a buffer should be held by a Rill binding rather than stashed in a raw
`Ptr` slot, which would lose the count.

`examples/buffers.rill` shows all of it, and `examples/gl/scene.rill` fills
one with the vertex data a GPU is handed.

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
  qsort(buf_data(b), buf_len(b), 4, cmp)
  b

fn main() = sort_by(nums, ascending)
```

What crosses is still a bare code pointer, so the callback has to be settled
while compiling: the caller names it, and `sort_by` is emitted once per
comparator it is called with, each copy with its own trampoline built in.
A **lambda or closure cannot cross** — it has an environment and C has nowhere
to keep it — and neither can a function chosen at run time. A helper like this
cannot itself be used as a value, since it exists in as many copies as it has
callers. A callback must not block on a channel, since it runs on C's call
stack rather than on its own.

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
| `free_ptr(p)` | frees what `to_cstr` (or C `malloc`) returned |
| `null_ptr()` / `is_null(p)` | for APIs that use null |

```
fn env(name) =
  p = to_cstr(name)
  v = getenv(p)
  free_ptr(p)
  if is_null(v) then "<unset>" else from_cstr(v)
```

Two things to keep in mind: a wrong signature is a crash, not a type error —
the compiler believes what you declare; and a C call that blocks freezes the
worker thread running it, since the scheduler cannot preempt foreign code.

### Being called: `export` and WebAssembly

`extern` is Rill calling out. `export` is the other direction — a function the
embedding host may call in:

```
export fn frame(t: Float) =
  ...
```

It only means something on a wasm target (`--target wasm32-wasip1`), because
only there is a host present; a native build refuses the word. A module with
any export links as a *reactor*: it has no `_start`, it stays instantiated,
and the host drives it. `main` still runs — exported as `rill_init`, which the
host calls once before anything else — and each `export fn` is a wasm export
under its own name.

The boundary takes what wasm can pass: parameters must be `Int`, `Float`,
`Bool` or `Ptr`, the return those or `Unit`, and the function must be
concrete — an export is one entry point, not a generic family. Data wider than
a scalar crosses through a `Buf`: build one, hand `buf_data` to an import, and
the host reads it straight out of the module's linear memory.

On wasm an `extern fn` that no library satisfies becomes an *import* — a hole
the host fills at instantiation. That is the whole FFI story in a browser:
declare `extern fn gpu_draw(v: Ptr, n: Int)`, supply `env.gpu_draw` from
JavaScript. `examples/webgpu/` renders through WebGPU exactly this way — the
host owns the asynchronous device setup that a wasm module cannot await, Rill
owns the frame.

## 11. Built-in functions

| | |
|---|---|
| `println(x)` / `print(x)` | print any value type, with or without a newline |
| `show(x) -> Str` | render any value as a string |
| `int_to_str` `float_to_str` `bool_to_str` `str_to_int` | conversions |
| `to_float(n)` `to_int(x)` | between `Int` and `Float`; `to_int` truncates toward zero |
| `#s` (or `str_len(s)`) `s[i]` (or `str_get(s, i)`) `s[i:j]` (or `str_sub(s, start, len)`) `str_find(s, needle)` `str_find_from(s, needle, from)` `str_cmp` `chr(byte)` | strings, byte-indexed; a string reads by index but never writes — it is a value, not storage. `str_find_from` starts partway along, so a long line can be walked field by field without slicing off the part already read; a start outside the string is clamped |
| `channel()` `send(ch, v)` `recv(ch)` | channels |
| `arg_count()` `arg(i)` | raw command-line access (`args()` is friendlier) |
| `read_file_raw(path)` `write_file(path, text)` `read_line()` `scan_int()` `io_ok()` | file and stdin IO; standard input is read in blocks, and `scan_int` scans a number out of it without allocating |
| `strbuf()` `b += s` (or `strbuf_add`) `b += byte` (or `strbuf_byte`) `strbuf_str(b)` `#b` | building a string a piece at a time |
| `to_cstr` `from_cstr` `free_ptr` `null_ptr` `is_null` | the C string and pointer bridge |
| `alloc_bytes(n)` `ptr_offset(p, bytes)` `poke_*` `peek_*` | raw buffers for C |
| `exit(code)` | terminate |

---

## 12. The toolchain

```sh
rill build main.rill -o main    # compile (LLVM -O3, stripped)
rill build main.rill --debug    # no optimization, DWARF line info for lldb
rill build main.rill --emit-ir  # print LLVM IR
rill run main.rill              # compile and run
rill fmt src/*.rill             # canonical formatting (--check, --stdout)
rill repl                       # interactive: definitions persist
```

---

## 13. What Rill deliberately does not have

- **Mutation.** No mutable bindings, no mutable fields. State is threaded
  through arguments or channels. The one exception is a buffer element,
  `b[i] = v` (and `+=`, `-=`, `*=`, `/=`) — a `Buf` is storage, and always
  was.
- **Exceptions.** Errors are `Result` values.
- **Null.** Absence is `Option`.
- **Implicit conversions.** Numeric types never mix silently.
- **Inheritance or subtyping.** Composition and traits instead.
- **A garbage collector.** Reference counting, decided at compile time.

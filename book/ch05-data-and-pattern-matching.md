# Chapter 5. Data and Pattern Matching

This is the chapter that changes how you design programs. Everything so far has
been a variation on things other languages do; sum types and exhaustive pattern
matching are the part where a reader from Java or Python starts writing
different code, not just different syntax.

---

## 5.1 Declaring a type

```rill
type Shape
  Circle(r: Float)
  Rect(w: Float, h: Float)
  Triangle(base: Float, height: Float)
```

Read it as: *a `Shape` is a `Circle` with a radius, **or** a `Rect` with a
width and a height, **or** a `Triangle` with a base and a height.*

The "or" is the important word. A struct is an *and* — a point has an x and a
y. This is the other one, and most languages before 2015 did not have it.

- Constructor names start with a capital and are **global** — `Circle` is a
  function you call, not `Shape.Circle`.
- Field names are optional documentation. `Rect(Float, Float)` means the same
  thing.
- A constructor with no fields (`Red`, `Nil`, `None`) is a value rather than a
  function, and never allocates.
- A type and one of its constructors may share a name, which is handy for
  single-case wrappers: `type Money  Money(Int)`.

---

## 5.2 Taking it apart

A value built with `Circle(1.0)` has to be examined before you can use it, and
that is what `match` is for:

```rill
# book/code/ch05/shapes.rill
type Shape
  Circle(r: Float)
  Rect(w: Float, h: Float)
  Triangle(base: Float, height: Float)

fn area(s) = match s
  Circle(r) -> 3.14159 * r * r
  Rect(w, h) -> w * h
  Triangle(b, h) -> b * h / 2.0

fn describe(s) = match s
  Circle(_) -> "round"
  _ -> "straight-edged"

fn main() =
  shapes = Cons(Circle(1.0), Cons(Rect(2.0, 3.0), Cons(Triangle(4.0, 5.0), Nil)))
  println(shapes)
  println(map(shapes, area))
  println(map(shapes, describe))
```

```
$ rill run shapes.rill
Cons(Circle(1), Cons(Rect(2, 3), Cons(Triangle(4, 5), Nil)))
Cons(3.14159, Cons(6, Cons(10, Nil)))
Cons(round, Cons(straight-edged, Cons(straight-edged, Nil)))
```

`match` does two things at once, and this is what makes it more than a switch:
it **tests** which constructor a value was built with, and it **binds** the
fields to names in the same breath. `Circle(r) -> ...` means "if this is a
circle, call its radius `r` in the branch that follows".

`_` is a wildcard: it matches anything and binds nothing. `describe` uses it
twice — once inside a constructor to say "a circle, don't care what radius",
once on its own as a catch-all.

Notice also that every data value already prints:
`Cons(Circle(1), Cons(Rect(2, 3), Nil))`. You do not write a `toString`. (The
floats show as `1` and `2` because Rill prints whole floats without a decimal
point. Chapter 8 replaces this derived printing with your own.)

---

## 5.3 The compiler checks you covered every case

Leave out an arm:

```rill
# book/code/ch05/missing.rill
type Colour
  Red
  Green
  Blue

fn name(c) = match c
  Red -> "red"
  Green -> "green"

fn main() = println(name(Red))
```

```
$ rill run missing.rill
missing.rill: type error at line 6: match is not exhaustive; no arm matches `Blue`
```

The program never runs. The compiler enumerated the cases, found one with no
arm, and **named it**.

This is worth pausing on, because it changes what refactoring feels like. Add a
fourth colour to the type and recompile: every `match` that now has a hole is
listed, with a line number and the missing case. Not a runtime `default:` that
logs a warning in production six weeks later — a compile error, immediately,
everywhere.

Programmers who work this way come to rely on it. The move "add a case to the
type, then fix what the compiler complains about" is a genuinely different way
to change a large program, and it is only available if the checker is exact.

The other half of the check is redundancy: an arm that earlier arms already
cover is rejected too, so a `_` in the middle of a match is an error rather
than silent dead code.

---

## 5.4 Option: absence with a name

Rill has no null. A value that might not be there has a type that says so, and
the prelude defines it:

```
type Option(a)
  None
  Some(a)
```

```rill
# book/code/ch05/option.rill
fn find_even(l) = match l
  Nil -> None
  Cons(x, rest) -> if x % 2 == 0 then Some(x) else find_even(rest)

fn describe(o) = match o
  None -> "none found"
  Some(n) -> "found " + int_to_str(n)

fn main() =
  println(describe(find_even(Cons(1, Cons(3, Cons(8, Cons(5, Nil)))))))
  println(describe(find_even(Cons(1, Cons(3, Nil)))))
  println(unwrap_or(find_even(Cons(1, Nil)), 0 - 1))
  println(opt_map(Some(21), \x -> x * 2))
```

```
$ rill run option.rill
found 8
none found
-1
Some(42)
```

`find_even` returns `Option(Int)`, and its signature therefore *tells the
caller* that there may be no answer. There is no way to use the integer without
first saying what happens when there isn't one, because the integer only exists
inside the `Some` branch.

Compare the three ways other languages handle this:

| Approach | The failure mode |
|---|---|
| Return `null` / `None` | forgetting to check compiles fine and crashes later |
| Throw an exception | the signature doesn't say; control jumps somewhere |
| Return `Option` | forgetting to check does not compile |

Two prelude helpers cover the common shapes: `unwrap_or(o, d)` supplies a
default, and `opt_map(o, f)` applies a function inside the `Some` and leaves
`None` alone.

> **The last line prints `Some(42)`, not `42`**
>
> `opt_map` gives back an `Option`, because the value might still be absent.
> Getting the plain integer out means saying what `None` becomes — that is the
> whole discipline in one line.

---

## 5.5 Result: failure as a value

Errors work the same way. Rill has no exceptions; a function that can fail says
so in its type:

```
type Result(a, e)
  Ok(a)
  Err(e)
```

```rill
# book/code/ch05/result.rill
type Money
  Money(Int)

fn parse_amount(s) =
  n = str_to_int(s)
  if str_len(s) == 0 then Err("empty") else if n < 0 then Err("negative: " + s) else Ok(Money(n))

fn show_result(r) = match r
  Ok(Money(c)) -> "ok: " + int_to_str(c)
  Err(msg) -> "error: " + msg

fn main() =
  println(show_result(parse_amount("1250")))
  println(show_result(parse_amount("")))
  println(show_result(parse_amount("-5")))
```

```
$ rill run result.rill
ok: 1250
error: empty
error: negative: -5
```

Two things to take from this listing.

**`Ok(Money(c))` is a nested pattern.** It unwraps the `Result` and the `Money`
in one step and binds the integer inside. Patterns nest as deeply as the data.

**Nothing unwinds the stack.** The error travels back as an ordinary return
value along the ordinary path. There is no invisible second exit from every
function, which means the reading rule from Chapter 1 still holds: what you see
is what happens.

The cost is that errors must be handled or explicitly passed along at each
step, and in a deep call chain that is more typing than an exception. Languages
in this family answer with syntax — Rust's `?`, Haskell's do-notation. Rill 0.2
does not have that sugar yet; you write the `match`, or use `res_map` when you
only want to transform the success case.

---

## 5.6 Patterns nest

```rill
# book/code/ch05/nested.rill
fn classify(l) = match l
  Nil -> "empty"
  Cons(Some(0), _) -> "starts with zero"
  Cons(Some(n), Cons(Some(m), _)) -> "two: " + int_to_str(n + m)
  Cons(Some(n), _) -> "one: " + int_to_str(n)
  Cons(None, _) -> "starts with nothing"

fn main() =
  println(classify(Nil))
  println(classify(Cons(Some(0), Nil)))
  println(classify(Cons(Some(3), Cons(Some(4), Nil))))
  println(classify(Cons(Some(9), Nil)))
  println(classify(Cons(None, Nil)))
```

```
$ rill run nested.rill
empty
starts with zero
two: 7
one: 9
starts with nothing
```

A pattern may be:

| Pattern | Matches |
|---|---|
| `_` | anything, binds nothing |
| `name` | anything, binds it to `name` |
| `0`, `42` | that integer exactly |
| `Ctor(p1, p2)` | that constructor, with sub-patterns for its fields |

Arms are tried **in order**, so put the specific ones first. `Cons(Some(0), _)`
has to come before `Cons(Some(n), _)`, because the second would swallow it —
and if you get that backwards, the redundancy check says so rather than letting
the arm sit there unreachable.

This listing is also a small argument for the whole approach. Five cases, each
one a shape drawn on the page, and no combination of `if`, `is`, `!= null` and
field access to read past. The structure of the data is visible in the
structure of the code.

---

## 5.7 Several patterns, and naming the whole

Two shortcuts that come up constantly once you are writing real matches.

Arms that go the same way can say so once, with `|` between the alternatives:

```rill
fn eksen(y) = match y
  Kuzey | Guney -> "dikey"
  Dogu | Bati -> "yatay"
```

Every alternative has to bind the same names, because the body is one body: a
name it uses must mean something whichever alternative was taken. In practice
that means `|` is for constants and for constructors that take no arguments,
which is what it is wanted for. The compiler still checks that you covered every
case — three directions and an arm naming two of them still leaves one
uncovered — and it still tells you when an alternative is unreachable.

And `@` names the whole of what a pattern matched, so an arm can take a value
apart to ask about it and hand back the value it was given:

```rill
fn buyut(s) = match s
  hepsi @ Daire(r) if r > 10.0 -> hepsi
  Daire(r) -> Daire(r * 2.0)
  hepsi @ Kare(_) -> hepsi
```

Without the `@`, the first arm would have to write `Daire(r)` back out — an
equal value, built again, where the one you already had would do. Unlike `|`,
`@` nests: `Cons(bir @ 1, _)` is a field that must be `1` and is also called
`bir`.

Both of them in one program, `book/code/ch05/alternatives.rill`:

```
$ rill run alternatives.rill
vertical horizontal
4
20
starts with 1, empty
```

The second line is a circle of radius 2 doubled; the third is one of radius 20
handed straight back, because the guard took the `@` arm.

## 5.8 Changing one field

A record with six fields, one of which you want different, is tedious to write
out and easy to get wrong — and positional constructors make the mistake silent
when two fields have the same type. So there is a notation for it:

```rill
a = Nokta(1, 2, "a")
b = { a | x = 9 }              # (9, 2, "a")
c = { a | x = 9, ad = "yeni" } # (9, 2, "yeni")
```

Read the `|` as "except". Nothing is mutated: `a` is still `(1, 2, "a")`
afterwards, and `b` is a new value. It becomes an ordinary call to the
constructor, with the named fields taking the new expressions and every other
one read off `a`.

The base is read once, however many fields are carried over — `{ f(x) | y = 0 }`
calls `f` once — so it is safe to update the result of a call.

The rule about constructors is the one `.x` has, for the same reason: a field
name only says which field when the type has a single constructor for it to
belong to.

`book/code/ch05/update.rill` shows both of those — that the original is
untouched, and that the base is read once:

```
$ rill run update.rill
(9, 2, a)
(9, 2, new)
(1, 2, a)
built once
(5, 0, o)
```

"built once" is printed from inside the function whose result is being updated.
Two of that record's three fields are carried over, and it still appears once.

There is one difference between them worth knowing. `p.x` is answered at the end
of checking rather than where it is written, so it is enough that the type of
`p` becomes known *somewhere* in the function:

```rill
fn f(p) =
  a = p.x        # nothing has said what p is yet
  a + uzak(p)    # this does, and that is soon enough
```

An update is deferred the same way: `{ p | x = 1 }` is built once the type is
known, wherever in the function that is said. And when nothing says it, the
fields do — a value read by `.x` and `.y` is of the one type that has an `x` and
a `y`, so `fn uzak(p) = p.x + p.y` needs no annotation while `Nokta` is the only
type with those two. Only when two types both have them is an annotation still
needed, and the error names the two.

---

## 5.9 Designing with sum types

The technique worth learning from this chapter is **making illegal states
unrepresentable**. It is a design move, not a language feature.

Here is a connection, modelled the way most codebases model it:

```
type Connection
  Connection(host: Str, port: Int, socket: Int, error: Str, connected: Bool)
```

Five fields, and the invariants live in your head: if `connected` then `socket`
is valid and `error` is empty; if not, `socket` is -1 and `error` might be set.
Nothing enforces any of it, every function must check, and one wrong branch
gives you a read from socket -1.

Now the same thing as a sum:

```
type Connection
  Idle(host: Str, port: Int)
  Live(host: Str, port: Int, socket: Int)
  Failed(host: Str, error: Str)
```

There is no state with a socket and an error. There is no "connected but
socket is -1". A function that needs a live socket takes the `Live` case and
cannot be handed anything else. The invariants moved out of your head and into
the type, where the compiler enforces them for free.

The question to ask of any record you write is: **how many combinations of
these fields are meaningful, and how many are possible?** When the second
number is much larger than the first, there is a sum type waiting.

---

## 5.10 Exercises

1. Model a traffic light as a type and write `next(light)`. Then add a
   flashing-amber state and see what the compiler asks you to fix.
2. Write `safe_div(a, b)` returning `Option(Int)`, and use it to write
   `average(list)` returning `Option(Int)` that is `None` for an empty list.
3. Take the `Connection` example in §5.7 and write `describe(conn)` for the
   five-field version and the three-case version. Which one can be wrong?
4. Write `depth(shape_list)`-style nested matching practice: a function over
   `List(Option(Shape))` that returns the total area, treating `None` as zero.

---

Chapter 6 looks at the list type that has been quietly used in every listing so
far, and at the prelude built on top of it.

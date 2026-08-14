# Chapter 15. Memory Without a Collector

Every functional language before this one shipped a garbage collector, because
the style makes so much garbage: every `map` builds a list, every update copies
a path, every intermediate value is allocated and dropped. Something has to
clean up, and for fifty years that something has been a collector.

Rill does not have one. This chapter explains what it has instead, what that
buys, and the one thing it cannot do.

---

## 15.1 The compiler inserts the frees

A tracing collector runs periodically, walks from the roots to find what is
still reachable, and frees the rest. It is a *run-time* activity that needs to
know about every pointer in the program.

Rill does the work at *compile time*. The compiler follows ownership rules —
this is the Perceus approach, from the Koka language — and works out where each
value's last use is. Then it inserts the release right there.

Concretely, for a function like

```
fn area_of(s) =
  a = area(s)
  a * 2.0
```

the compiler knows `s` is not used after `area(s)`, so it emits the release
immediately after that call rather than at the end of the function. The rules
are mechanical: every expression yields an owned reference, arguments are
consumed by the callee, a binding retains, and anything still owned at the end
of a function is released.

Nothing runs periodically. There is no heap walk, no pause, no read barrier,
no write barrier, and no collector thread. A value is freed at the instruction
after it dies.

### Checking it

The runtime can count for you:

```rill
# book/code/ch15/alloc.rill
type Point
  Point(x: Int, y: Int)

fn make(n, acc) = if n == 0 then acc else make(n - 1, Cons(Point(n, n * 2), acc))

fn main() =
  pts = make(100000, Nil)
  println(len(pts))
  println(nth(pts, 0))
```

```
$ RILL_DEBUG_ALLOC=1 rill run alloc.rill
100000
Some(Point(1, 2))
live allocations: 0
```

Two hundred thousand objects — a hundred thousand list cells and a hundred
thousand points — allocated, used, and every one of them freed by the time the
program exits. Nobody wrote a `free`.

`RILL_DEBUG_ALLOC=1` is the tool to reach for when you suspect a leak. Zero is
the number you want, and it is the number every example in this book produces.

---

## 15.2 What this buys

**Determinism.** There is no pause, because there is nothing that runs. The
worst case for any single operation is the cost of freeing a chain of objects
that all died at once, which is bounded by what you allocated. For anything
with a latency budget — a game frame, an audio callback, a request tail — this
is the difference between a guarantee and a hope.

**Small heaps.** A tracing collector needs headroom: it must let garbage
accumulate before a collection is worth doing, so peak memory is typically two
to five times the live set. Reference counting frees on the spot, so peak
memory is close to the live set. This is why the binary-trees benchmark in
Chapter 2 shows Rill using *less* memory than either Go or OCaml while also
running faster — it is not a better collector, it is no collector.

**No runtime to ship.** The 33 KB hello-world in Chapter 2 is 33 KB partly
because there is no collector in it.

**Cheap nullary constructors.** `Nil`, `None`, `Leaf` and every other
constructor with no fields is a static singleton — one immortal object in the
binary, shared by everything:

```
$ RILL_DEBUG_ALLOC=1 rill run nullary.rill
2
live allocations: 0
```

They allocate nothing at all, which matters more than it sounds: a program full
of `Option` and lists creates `None` and `Nil` constantly, and in this language
those are free.

---

## 15.3 What it costs

**Cycles leak.** This is the honest, permanent cost. Reference counting frees an
object when its count reaches zero, and two objects pointing at each other keep
each other's count at one forever. Rill has no cycle collector.

In practice this bites far less than it sounds, and the reason is Chapter 1's:
**you cannot easily build a cycle out of immutable values.** To make `a` point
at `b` and `b` point at `a`, one of them must be modified after the other was
created — and nothing can be modified. Every structure in Chapters 6 through 14
is acyclic by construction.

Where you *can* build one is through a `Buf` or through C. If you store an
index into a buffer that refers back, that is fine — an index is not a
reference. If you stash a counted value inside a raw `Ptr` slot, the count is
lost and you are on your own. Both are avoidable, and `RILL_DEBUG_ALLOC=1` will
tell you if you did not avoid them.

**Counting has a cost.** Every value passed, bound and returned adjusts a
count. The compiler elides many of them — a value that is created, used once
and dropped may never be counted at all — but not all. This is the ordinary
trade against a tracing collector, which makes allocation nearly free and pays
in pauses instead.

That cost is also why Rill's concurrency defaults to a single OS thread
(Chapter 16). One thread means the counts can be plain increments rather than
atomic ones, which is worth a great deal on an operation this frequent.

---

## 15.4 `Buf`: the deliberate hole

Everything above assumes immutable values. But a texture, a pixel row, a
dynamic-programming table and an array a C library wants to fill are all
mutable blocks of one machine type, and pretending otherwise produces bad
programs.

So Rill has one mutable structure, fenced off and named:

```rill
# book/code/ch15/buffers.rill
fn fill(v: Buf(F32), i) =
  buf_set(v, i, to_float(i) * 1.5)
  if i + 1 >= buf_len(v) then 0 else fill(v, i + 1)

fn main() =
  v = buf_f32(5)
  fill(v, 0)
  println(buf_get(v, 3))
  println(buf_len(v))
  w = buf_u8(2)
  buf_set(w, 0, 200)
  println(buf_get(w, 0))
  s = buf_i8(2)
  buf_set(s, 0, 200)
  println(buf_get(s, 0))
  println(buf_get(v, 99))
```

```
$ rill run buffers.rill
4.5
5
200
-56
rill: buffer index 99 out of range, length 5
```

A `Buf(w)` is a counted block of elements of one C width. There is a
constructor per width — `buf_i8` `buf_u8` `buf_i16` `buf_u16` `buf_i32`
`buf_u32` `buf_int` `buf_f32` `buf_float` `buf_ptr` — and the argument is a
length in *elements*, not bytes.

Four properties, and each one is a decision:

**The width lives in the type.** `Buf(U8)` and `Buf(I8)` are different types,
and reading the same byte through them gives 200 and -56 — the fourth and
fifth lines of the output. This is not a cast; it is what the type means.

**Indices are bounds-checked.** Reading past the end ends the program with the
index and the length rather than returning whatever was next in memory. The
last line of the output is that check, and Chapter 14 showed it catching a real
off-by-one in a table recurrence.

**It is reference counted like a string.** It is released when the last
reference goes. There is no `free`, and no way to forget one. This is the
difference between `Buf` and the raw `alloc_bytes` pointer used for C interop,
where you do own the free.

**It is mutable.** `buf_set` changes the block in place. That is the hole in
the language's immutability, and it is deliberate — every use of it in this
book is a case where the alternative was asymptotically worse.

The width is never inferred, because it is not a type variable. A function
taking a buffer says which one: `fn fill(v: Buf(F32), i) = ...`.

### When to reach for it

The rule from Chapter 13, restated:

> **Use values until the shape of the problem is an array. Then use an array.**

The cases in this book were: a flood fill over a grid (Chapter 13), a
dynamic-programming table (Chapter 14), and generating a texture. All three are
dense, integer-indexed, and hot. Everything else in fourteen chapters used
ordinary immutable values, and that ratio is about right for real programs too.

---

## 15.5 How it compares

| | Rill | Go, Java, OCaml, Haskell | Rust | C |
|---|---|---|---|---|
| Who frees | the compiler, via counts | a collector, at run time | the compiler, via ownership | you |
| Pauses | none | yes | none | none |
| Peak heap | ≈ live set | 2–5× live set | ≈ live set | ≈ live set |
| Cycles | leak | collected | leak (`Rc`) | your problem |
| Annotation burden | none | none | lifetimes | none |
| Use after free | impossible | impossible | impossible | possible |

The interesting column is Rust's. Both languages free deterministically with no
collector, and the difference is what they ask of you. Rust makes ownership
part of the type system, so you write lifetimes and get zero counting overhead.
Rill keeps ownership entirely inside the compiler, so you write nothing and pay
for the counts that could not be elided.

That is a real trade rather than a win: Rust is faster on the operations Rill
counts, and Rill is a language you can teach in eight chapters.

---

## 15.6 Exercises

1. Run every program in `book/code/` under `RILL_DEBUG_ALLOC=1`. Which ones
   report a non-zero count, and why?
2. Build a cycle on purpose using two `Buf(Ptr)` values and confirm it leaks.
   Then convince yourself you could not have done it with ordinary values.
3. Write `sum_buf(v: Buf(Int))` and compare its speed against summing a list of
   the same length. Predict the ratio first.
4. Write a growable array — a `Buf` plus a used-count, doubling when full.
   What does "doubling" mean when a `Buf` cannot be resized?
5. Read the `Buf` section of `LANGUAGE.md` and work out why the width cannot be
   inferred, given how the rest of the type system works.

---

Chapter 16 covers the other half of the runtime: how a hundred thousand
concurrent things fit in fifty megabytes.

# Chapter 2. Why Rill

Chapter 1 argued for a style. This chapter argues for a particular language,
which is a harder thing to do honestly, because the style already has good
implementations and most readers should probably use one of them.

So let us start with the case against.

---

## 2.1 The case against a new language

If you want a functional language today, you have excellent choices, and every
one of them is more sensible than a language whose version number starts with
0.2.

**Haskell** is the purest expression of the ideas and has thirty years of
research behind it. **OCaml** is fast, strict, mature, and runs serious
industrial systems. **F#** and **Scala** give you the style with a vast
ecosystem underneath. **Rust** brings sum types, exhaustive matching and
`Option`/`Result` to systems programming with no runtime at all. **Elixir** and
**Erlang** own fault-tolerant distributed systems. **Clojure** gives you Lisp
on the JVM with the best story about immutable data structures anywhere.

Against that, any new language starts with no libraries, no community, no
tooling beyond what its author wrote, and no track record. That is not a small
disadvantage. It is usually decisive, and it should be.

So a new language has to be answering a question the existing ones do not.

---

## 2.2 The question Rill answers

Look at the functional languages by *what they produce*:

| Language | Output | Runtime underneath |
|---|---|---|
| Haskell | native binary | GC, green threads, several MB |
| OCaml | native binary | GC, ~1 MB minimum |
| F#, Scala, Clojure | bytecode | .NET or JVM |
| Elixir, Erlang | bytecode | BEAM VM |
| Rust | native binary | none — but not a functional language |

Every functional language on that list ships a garbage collector. Most ship a
substantial runtime; several ship an entire virtual machine. If you want
functional programming *and* you care about binary size, startup time,
predictable latency, or memory footprint, the list is short and the answer has
historically been "use Rust and give up the functional parts."

Rill's question is:

> **Can you have ML's types and Go's concurrency in a binary the size of a C
> program, with no garbage collector and no pauses?**

That is the whole design brief. Everything specific about the language follows
from it.

---

## 2.3 The four decisions

### Strict, not lazy

Rill evaluates arguments when it is told to, like ML, OCaml, Scheme and every
mainstream language — not on demand, like Haskell.

Laziness buys real things: infinite structures, and control constructs written
as ordinary functions. It costs two, and they are the ones that matter here.
Every unevaluated expression becomes a heap-allocated *thunk*, so allocation
appears where you did not write any; and the time at which work happens stops
being visible in the source, which is why Haskell's characteristic beginner bug
is a space leak in an accumulator that looks perfectly innocent.

For a language whose brief is predictable memory, laziness is the wrong trade.
When a Rill program computes a value, it computes it there.

### Reference counting decided at compile time, not a garbage collector

This is the decision that makes the rest possible, and it is worth being
precise about what it means.

A tracing garbage collector runs periodically, walks the live objects, and
frees the rest. It buys you cycle collection and very cheap allocation; it
costs you a runtime, unpredictable pauses, and a heap several times larger than
the live set.

Rill instead does what Perceus-style systems do: the **compiler** works out
where each value's last use is and inserts the retain and release operations
itself, following ownership rules. Nothing runs periodically. A value is freed
at the instruction after it dies.

Three consequences you will see in this book:

- **Deterministic.** No pauses, ever. The worst case is the cost of a free.
- **Small.** Peak memory is close to the live set rather than a multiple of it.
  The binary-trees benchmark in §2.4 shows Rill using less memory than either
  GC'd language while running faster.
- **Cyclic data leaks.** This is the honest cost. Reference counting cannot
  collect a cycle, and Rill has no cycle collector. In practice immutable data
  is overwhelmingly acyclic — you cannot easily build a cycle out of values
  that never change — but if you construct one through buffers or FFI, it will
  not be freed. Run with `RILL_DEBUG_ALLOC=1` and the runtime tells you how
  many allocations were still live at exit.

### Go's concurrency, not Haskell's or Erlang's

Rill takes `spawn`, typed channels, and `select` more or less directly from Go:
many lightweight threads, communicating by passing values, over a scheduler
that parks a strand when it blocks on a channel or a socket.

The reason is that this model composes with the memory decision. Because
strands run on one OS thread by default, reference counting can be
*non-atomic* — an ordinary increment rather than a locked one — and that is
worth a great deal on every value operation in the language. Programs that want
real parallelism opt in at build time (`--parallel`) and at run time
(`RILL_THREADS=8`), at which point the counts become atomic and you pay for
what you asked for.

100,000 strands cost about 53 MB and start in microseconds. Chapter 16 shows
how that is done — the trick is that a parked strand's stack is copied out
to a heap block sized to what it actually used, rather than reserving a stack
per thread.

### Monomorphized generics and static trait dispatch

A generic function in Rill is compiled once per concrete type it is used at.
Nothing is boxed, no type information is carried at run time, and a trait method
call is resolved to a direct call during compilation. A missing implementation
is a compile error, not a runtime failure.

The cost is compile time and code size for heavily generic code; the benefit is
that `map` over a list of integers compiles to the same machine code you would
have written by hand.

---

## 2.4 What that produces

These are the repository's own benchmarks (`python3 benchmarks/run.py`, Apple
M4, outputs verified identical across all three languages):

| benchmark | rill | go | ocaml | rill RSS | go RSS | ocaml RSS |
|---|---:|---:|---:|---:|---:|---:|
| fib(35) | **17.2 ms** | 22.0 ms | 23.7 ms | **1.3 M** | 3.6 M | 2.1 M |
| binary trees (alloc churn) | **33.0 ms** | 55.9 ms | 60.0 ms | **13.3 M** | 16.6 M | 18.3 M |
| 100k-thread channel ring | **13.5 ms** | 96.2 ms | 61.7 ms | **53.4 M** | 269.4 M | 103.0 M |
| 100M-iteration loop | **127.6 ms** | 271.6 ms | 295.3 ms | **1.3 M** | 3.5 M | 2.1 M |

And the size figure, which you can reproduce in ten seconds:

```
$ rill build hello.rill -o hello && ls -l hello
33 KB
```

Against roughly 2.1 MB for the equivalent Go program and about 1 MB for OCaml.

Three cautions about reading any table like this one. It is four
microbenchmarks, not a workload. It is one machine and one operating system.
And the honest summary of the first row is not "Rill is faster than Go at
arithmetic" — it is "both of these are LLVM-quality code generation and the
difference is small." The rows that carry real information are the third, where
the concurrency implementation differs by 7×, and the second, where a compiler
inserting frees beats two mature garbage collectors on both time *and* memory.

---

## 2.5 What Rill deliberately does not have

A language is defined as much by its omissions, and these are chosen rather
than pending:

- **Mutation.** No assignment, no mutable fields. State is threaded through
  arguments or passed over channels. The exception is `Buf`, a counted block of
  C-width elements, which exists so that C libraries and pixel buffers are
  workable; it is a deliberate hole with a fence around it (Chapter 15).
- **Exceptions.** Errors are `Result` values. Nothing unwinds the stack from
  under you.
- **Null.** Absence is `Option`.
- **Implicit numeric conversion.** `1 + 2.0` is a type error, and the compiler
  says so.
- **Inheritance and subtyping.** Traits and composition instead.
- **Laziness.** Discussed above.
- **A garbage collector.** Discussed above.
- **Macros, effect systems, higher-kinded types.** Not on principle, but
  because the language is 0.2 and each of them is a large design in itself.

---

## 2.6 Who this book is for

You should read on if you want to understand what a functional language looks
like when every feature has to justify itself against a native-code, no-runtime
budget — or if you want a working tour of the ideas in Chapter 1 with a
compiler you can run each example through.

You should not adopt Rill for production work. It is a young language with one
implementation, a small standard library, and no ecosystem. This book will tell
you plainly when a thing is missing rather than working around it quietly.

What Rill does have, and what makes it a good vehicle for a book like this one,
is that the whole language fits in your head. There are no dark corners, no
legacy syntax, and no feature that exists because a committee needed it. Every
construct in Chapters 3 through 8 is one you will use in Chapters 9 through 14,
and there are no others.

---

## 2.7 Setting up

Everything in this book is run with the compiler in this repository:

```sh
cargo build --release            # builds ./target/release/rill
```

Two commands cover the whole book:

```sh
rill run   program.rill          # compile to a temporary binary and run it
rill build program.rill -o prog  # compile to ./prog
```

Two more are worth knowing early:

```sh
rill fmt   program.rill          # canonical formatting — there is exactly one
rill repl                        # try an expression without writing a file
```

Every program in this book lives under `book/code/`, is compiled by the same
compiler as part of preparing the text, and prints the output shown. Where a
listing shows an error message, that message was produced by running the broken
program, not written by hand.

Chapter 3 starts with the smallest program that does anything.

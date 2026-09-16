# Functional Programming with Rill

*A book about writing programs out of values instead of commands, using a
language small enough to hold in your head.*

---

## About this book

This book has two subjects and they are not the same size.

The first is **functional programming** — what it is, where it came from, what
it buys you and what it costs. That subject is fifty years old, belongs to no
particular language, and is worth learning whether or not you ever write a line
of Rill. Chapter 1 covers it at length before any Rill appears.

The second is **Rill**, a small strict functional language that compiles to
native code with no garbage collector and no runtime. It is the vehicle rather
than the destination. It is a good vehicle for a book because the whole
language is nine constructs — there are no dark corners to warn you about, and
by Chapter 8 you have seen all of it.

Every program in this book is in `book/code/`, was compiled and run by the same
compiler that is in this repository, and prints the output shown. Where a
listing shows a compiler error, that error came from running the broken
program.

### Who it is for

Programmers who know at least one language well. The comparisons are drawn
mostly against Python, Java, Go and Rust, and nothing assumes you have used a
functional language before.

### Who it is not for

Anyone looking for a production language. Rill is version 0.2, has one
implementation, a small standard library and no ecosystem. This book says so
whenever a limitation is reached rather than quietly designing around it.

---

## Contents

### Part I — Thinking Functionally

**[Chapter 1. What Functional Programming Is](ch01-what-functional-programming-is.md)**
Two ways to say the same thing · the trouble with boxes · purity and
referential transparency · functions as values · recursion and tail calls ·
types that describe the problem · a short history · what it costs

**[Chapter 2. Why Rill](ch02-why-rill.md)**
The case against a new language · strict not lazy · reference counting decided
at compile time · Go's concurrency · monomorphization · benchmarks · what Rill
deliberately lacks

### Part II — The Language

**[Chapter 3. First Programs](ch03-first-programs.md)**
`fn main` · expressions · no implicit conversions · bindings and blocks ·
layout · the REPL

**[Chapter 4. Functions Are Values](ch04-functions-are-values.md)**
Passing functions · lambdas and closures · the `|>` pipeline · generics without
asking · monomorphization · map, filter and fold

**[Chapter 5. Data and Pattern Matching](ch05-data-and-pattern-matching.md)**
Sum types · `match` · exhaustiveness checking · `Option` instead of null ·
`Result` instead of exceptions · nested patterns · making illegal states
unrepresentable

**[Chapter 6. Lists and the Prelude](ch06-lists-and-the-prelude.md)**
The list is an ordinary type · reading the prelude · what it gives you ·
strings · your own recursive types

**[Chapter 7. Loops Are Recursion](ch07-loops-are-recursion.md)**
The transformation · why tail calls do not overflow · accumulators · an honest
measurement and a surprise about LLVM · loops that never end

**[Chapter 8. Traits](ch08-traits.md)**
`Show` and `Ord` · traits of your own · static dispatch and what it forbids ·
implementations for generic types · the whole language in one page

### Part III — Algorithms

**[Chapter 9. Small Algorithms, Compared](ch09-small-algorithms.md)**
FizzBuzz · Euclid · two prime sieves and why one is much slower · Collatz ·
digits and palindromes

**[Chapter 10. Sorting](ch10-sorting.md)**
Insertion sort · the quicksort that is not quicksort · merge sort · sorting by
a comparator · an honest comparison table

**[Chapter 11. Trees](ch11-trees.md)**
Search trees · where immutability pays: structural sharing · a map, since the
prelude has none · trees are everywhere

**[Chapter 12. Parsing](ch12-parsing.md)**
An AST as a sum type · an evaluator in four lines · recursive descent with
threaded position · errors as values · the visitor pattern without the pattern

**[Chapter 13. Graphs and Grids](ch13-graphs-and-grids.md)**
A graph as a list of edges · BFS with lists and why it is slow · flood fill
with buffers · choosing between them

**[Chapter 14. Dynamic Programming](ch14-dynamic-programming.md)**
The one that is better: Fibonacci without a table · the one that is not: edit
distance in a buffer · accumulators, dictionaries and arrays · how to choose

### Part IV — Underneath

**[Chapter 15. Memory Without a Collector](ch15-memory-without-a-collector.md)**
The compiler inserts the frees · what that buys · cycles, the one real cost ·
`Buf`, the deliberate hole · compared with Go, Rust and C

**[Chapter 16. Strands and Channels](ch16-strands-and-channels.md)**
Three primitives · pipelines · `select` · a hundred thousand strands in 56 MB
and how · blocking on the world · deadlock · several cores on purpose

**[Chapter 17. Machines](ch17-machines.md)**
A process as SDL drew it · `sender`, `reply`, `output`, `post` · timers, `save`,
`priority`, `when` · the `system` checked against the code · a run drawn as
a sequence chart · the model's own clock

---

## Running the code

```sh
cargo build --release                     # builds ./target/release/rill

rill run   book/code/ch03/hello.rill      # compile and run
rill build book/code/ch03/hello.rill -o hello
rill fmt   book/code/ch03/hello.rill      # canonical formatting
rill repl                                 # try one expression
```

Every listing can be run directly:

```sh
$ ./target/release/rill run book/code/ch09/primes.rill
```

---

## Further reading

- **[LANGUAGE.md](../LANGUAGE.md)** — the complete language reference. This
  book teaches; that document specifies.
- **Chris Okasaki, *Purely Functional Data Structures*** — the sequel to
  Chapters 10 and 11. Balanced trees, real queues, and heaps, all immutable.
- **`examples/`** in this repository — a JSON parser, a YAML reader, an HTTP
  server, a WebSocket server, and an OpenGL maze game, all in Rill.

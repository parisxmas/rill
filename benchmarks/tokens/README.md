# The same two algorithms in eight languages

What it costs to *write* an algorithm, and what it costs to *run* it. Rill, C,
C++, Rust, Zig, C#, Go and OCaml; forty-three programs; two tasks that are
compared on both counts and three that are timed only.

```sh
cargo build --release
python3 benchmarks/tokens/gen_inputs.py   # once — 43 MB, made from an LCG
python3 benchmarks/tokens/run.py
```

A missing toolchain is skipped with a note. `pip install tiktoken` adds the
token columns.

## The programs

| | what it is | compared on |
|---|---|---|
| `json` | reads a JSON document from stdin, parses it, writes it back in canonical form | tokens and time |
| `json_short` | the same task in Rill written the plain way — `map` and `join` instead of a builder threaded by hand | tokens and time |
| `dijkstra` | CSR adjacency, hand-written array-backed binary min-heap, source-to-target distance | tokens and time |
| `loop` | 100M indexed reads and adds, and nothing else | time |
| `algo` | the `dijkstra` graph built in process instead of read, so the input reading is out of the way | time |
| `mandel` | 2000×2000 Mandelbrot, 255 iterations, summed — floating point in a loop with a data-dependent exit | time |
| `json_free` | the C `json` with a recursive `free` added — the honest baseline, since the others give their memory back | time |

Every program follows the same data layout and the same algorithm. What is
measured is not the choice of algorithm but the cost of expressing it and of
running it.

**The heap is hand-written in all eight.** Rust, Zig and C# *do* have a
priority queue in their standard library, and using it takes the Dijkstra task
from 803 to 494 tokens in Rust, 776 to 489 in C#, and 1033 to 807 in Zig. But
then what is measured is a library rather than a language, and C, OCaml and
Rill have nothing to answer with. So they all write the same program.

**Two rows are there so that a number does not mean something it does not.**

`dijkstra` in C++ reads its input with `std::cin >>`, which is what a C++
program does, and takes 3.7 seconds — seventeen times C. The same program
reading with `scanf` takes 218 ms, next to C's 217. So `C++ · scanf` is built
and timed too. (The pairing is worth sitting with: C++ writes that task in 692
tokens, the cheapest of the eight and below Rill's 740, and runs it slowest of
the eight. Both are the same operator.)

`mandel` is built twice in C, for the reason below.

**Floating-point contraction.** C, C++ and Go turn `a * b + c` into a single
fused multiply-add by default; Rill, Rust, Zig and C# do not, because it
changes the rounding and they promise something different. On a program that
is nothing but `a * b + c` that is the whole difference between the two groups
— C goes from 206 ms to 252 with `-ffp-contract=off`, landing exactly where
Rust and Zig already are. It is a question of what a language promises rather
than of how fast it is, so `run.py` builds C both ways and prints both.

Rill sits with the first group at 206 ms while emitting no fused multiply-add
at all. Its tail-recursive `escape` compiles to an eight-operation loop with no
register shuffling, where Rust's `while` needs an `fmov` and a second branch
for the same eight operations.

## Reading the output

`run.py` compares every language's output against every other's before it
reports a single time — the header of each block says so. Times are the best
of nine runs with output to a pipe, process startup included; the shortest
test is under 30 ms, so that startup is worth a few percent.

C# is built with NativeAOT, so what is timed is a program rather than a
runtime warming up. Rust is `rustc -O`, Zig `-O ReleaseFast`, C `clang -O2`,
OCaml `ocamlopt`, Go `go build`.

## What the inputs are

`gen_inputs.py` writes both from the linear congruential sequence the `algo`
programs generate their graph with, so nothing depends on a language's own
random number generator and the files come out byte-identical anywhere.

- `big.graph` — 31 MB: the same 200k-node, 2M-edge graph `algo` builds in
  process, written out as text so `dijkstra` has to read it.
- `big.json` — 11 MB on one line: nested arrays and objects, all six kinds of
  value, and strings that need escaping.

## A note on what this does and does not show

Both tasks are hand-written memory and data-layout work, which is where C,
Rust and Zig are strongest; another pair of tasks would draw another table.
And the `json` numbers are worth reading twice, because the C being compared
against never frees the tree it builds — `json_free` is what the same work
actually costs.

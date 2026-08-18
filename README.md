# Rill

A functional language that compiles to fast, small native executables. No VM, no
garbage collector.

```
type Shape
  Circle(r: Float)
  Rect(w: Float, h: Float)

fn area(s) = match s
  Circle(r) -> 3.14159 * r * r
  Rect(w, h) -> w * h

fn main() =
  shapes = Cons(Circle(1.0), Cons(Rect(2.0, 3.0), Nil))
  println(sum(map(shapes, area)))

  ch = channel()
  spawn send(ch, "from a strand")
  println(recv(ch))
```

- **Type inference everywhere.** Annotations are optional; generics, traits and
  requirements like "must be ordered" are all inferred and checked.
- **Algebraic data types and pattern matching**, with nested patterns and an
  exhaustiveness checker that names the case you forgot.
- **No garbage collector.** Compile-time reference counting (Perceus-style)
  frees memory the moment it dies — deterministic, no pauses.
- **Go's concurrency model.** `spawn` and typed channels over a green-thread
  scheduler — one lightweight thread is a **strand**; 100,000 strands cost ~53 MB and start in microseconds. Build
  with `--parallel` and set `RILL_THREADS` to spread them across cores.
- **Native code via LLVM -O3.** Generics are monomorphized, so nothing is boxed
  and trait dispatch is resolved at compile time. Hello world is 33 KB.
- **Enough IO to write servers.** TCP sockets that fit the concurrency model: A strand waiting on a
  socket parks and the scheduler polls the OS, so one strand per connection
  costs nothing while it waits.
- **Calls C directly.** `extern "m" fn sqrt(x: Float) -> Float` and it links,
  with an opaque `Ptr` type and explicit string bridging.
- **One canonical syntax.** `rill fmt` gives every program exactly one spelling,
  which is what makes the language predictable to read, diff and generate.

[**Language reference**](LANGUAGE.md) · [changelog](CHANGELOG.md) ·
[benchmarks](benchmarks/BENCHMARKS.md)

## Benchmarks

Apple M4, median of 25 runs, outputs verified identical across all three
languages (`python3 benchmarks/run.py`):

| benchmark | rill | go | ocaml | rill RSS | go RSS | ocaml RSS |
|---|---:|---:|---:|---:|---:|---:|
| fib(35) | **16.6 ms** | 22.8 ms | 23.9 ms | **1.2 M** | 3.5 M | 2.1 M |
| binary trees (alloc churn) | **34.3 ms** | 57.9 ms | 61.2 ms | **13.2 M** | 16.9 M | 18.3 M |
| 100k-thread channel ring | **13.6 ms** | 99.3 ms | 67.1 ms | **53.5 M** | 269.4 M | 103.0 M |
| 100M-iteration loop | **133.1 ms** | 288.2 ms | 310.3 ms | **1.2 M** | 3.6 M | 2.1 M |

Rill wins every benchmark on time, peak memory, and binary size (32–34 KB versus
2.1 MB for Go and ~1 MB for OCaml). Computation comes from LLVM; the allocation
result comes from reference counting with a pooled allocator and non-allocating
nullary constructors; the concurrency result comes from a copy-on-block stack
model where a parked strand costs about 300 bytes. Independent strands
scale across cores when built with `--parallel`: eight `fib(32)` computations
run 3.8× faster on eight workers.

An HTTP server too — the same `Hello, World!` route on Rill's own HTTP module
versus Go's `net/http`, under `wrk -t4 -c50 -d10s`:

| server | req/sec | p99 latency | RSS | binary |
|---|---:|---:|---:|---:|
| rill, keep-alive | **209,831** | **278 µs** | **2.3 M** | **51 K** |
| go, keep-alive | 198,359 | 880 µs | 21.3 M | 7.1 M |
| rill, connection per request | **41,035** | 98.5 ms | **2.0 M** | |
| go, connection per request | 13,310 | **2.05 ms** | 20.1 M | |

Rill is ahead on throughput in both modes and uses about 9× less memory, and
with keep-alive its p99 is 3.2× tighter because nothing pauses for collection.
The one bad-looking number, the 98 ms tail, is a rate effect rather than a
server one: Rill only reaches the rate where tails grow because it is three
times faster, and instrumenting the scheduler during those spikes finds the
server idle — what runs out is the client's supply of sockets. Connection
churn makes that column too unsteady to rank the two servers by: repeating the
same run gave 34.2 ms, then 1.84, then 4.60. The keep-alive tail, which churns
nothing, does repeat. The
[benchmark notes](benchmarks/BENCHMARKS.md#about-that-tail) show the
measurements.

## Using it

Requires Rust and LLVM 18 (`brew install llvm@18`; the path is wired up in
`.cargo/config.toml`).

```sh
cargo build --release
cargo test                    # 110 tests: unit, plus end-to-end compile-and-run

./target/release/rill build examples/hello.rill -o hello && ./hello
./target/release/rill run examples/showcase.rill
./target/release/rill repl                     # definitions persist, expressions run natively
./target/release/rill fmt examples/*.rill      # --check for CI, --stdout to preview
./target/release/rill build main.rill --debug     # no optimization, DWARF line info for lldb
./target/release/rill build main.rill --parallel  # then RILL_THREADS=8 ./main
```

### In an editor

`editors/vscode/` is a Visual Studio Code extension: syntax highlighting for
every construct in the language, two-space layout-aware indentation, and
snippets. Symlink it in and reload the window:

```sh
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/rill
```

Cursor, Windsurf and VSCodium read the same folder under their own
`extensions` directory. See `editors/vscode/README.md` for packaging it as a
`.vsix` and for how the grammar is tested.

## Examples

`examples/` is a tour, and every file is covered by the test suite:

| | |
|---|---|
| `showcase.rill` | the big one: an expression interpreter, a BST map, merge sort, a worker pool |
| `patterns.rill` | nested patterns and the decision-tree compiler |
| `ordering.rill` | custom `Show`, `Ord`, and one generic sort used at three types |
| `traits.rill` | user-defined traits with static dispatch |
| `pingpong.rill`, `ring.rill` | strands and channels |
| `select.rill`, `buffered.rill` | multiplexing with `select`, buffered channels |
| `io.rill` | files, arguments, text processing |
| `ffi.rill` | calling libm and libc through `extern` |
| `echo_server.rill` | a TCP server and client in one program |
| `http/` | HTTP/1.1 and WebSocket servers written in Rill, plus SHA-1 and base64 |
| `json/` | a JSON parser and printer, also written in Rill |
| `yaml/` | YAML for config files, sharing the JSON value type |
| `stdlib.rill` | what the prelude gives you |
| `loops.rill` | how repetition is written: tail recursion and folds |
| `blocks.rill` | block bodies in match arms, select arms and `if` branches |
| `multi/` | a program split across files, plus aliased imports that keep colliding names apart |
| `webgpu/` | a spinning triangle: Rill compiled to wasm computing frames for WebGPU |
| `webgpu/world/` | a walkable low-poly valley — terrain, physics and matrices in Rill; shadows, MSAA and compute particles on the GPU — and `server.rill`, which is the same valley compiled for a Linux box so that several browsers can walk about in it at once |

`selfhost/` goes further: Rill compiled by Rill. The lexer, parser and
formatter come first, proven byte-for-byte against the Rust implementation
over every `.rill` file in this repository (`./selfhost/check.sh`); then the
type checker, held to the same errors in the same words
(`./selfhost/check_types.sh`); then `rillc` itself — a command that checks,
emits LLVM IR and links, and compiles its own source to a byte-identical
fixpoint (`./selfhost/check_rillc.sh`).

## How it is built

A Rust workspace; source flows through it in one direction.

| crate | what it does |
|---|---|
| `rill_syntax` | lexer with indentation layout, parser, the prelude, the module loader, `rill fmt` |
| `rill_types` | Hindley–Milner inference with schemes and constraints, traits, pattern exhaustiveness; emits a typed AST |
| `rill_codegen` | monomorphization, reference-count insertion, decision-tree pattern compilation, LLVM |
| `rill_runtime` | a `no_std` staticlib: allocator, string and channel primitives, the strand scheduler |
| `rill_cli` | `build`, `run`, `fmt`, `repl` |

## Where it stands

Rill 0.4.0 is a complete, working language: you can write real programs in it, and
the compiler is honest about what it cannot do. Known limits, all deliberate:

- Multi-core execution is opt-in (`rill build --parallel`, then
  `RILL_THREADS=N`), because making reference counts atomic costs
  single-threaded speed and most programs do not need it. Workers steal only
  strands that have not started yet, since one that has already run is tied
  to its worker's stack.
- A function used before its definition is pinned to the types of that use, and
  there is no polymorphic recursion.
- Imports are either unqualified or aliased whole (`import "x" as m`); there is
  no per-item visibility control.

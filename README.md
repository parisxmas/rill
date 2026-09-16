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
- **Algebraic data types and pattern matching**, with nested patterns, guards
  (`Cons(x, _) if x > 0 ->`) and an exhaustiveness checker that names the case
  you forgot — a guarded arm covers nothing, so it cannot hide one. A type with one
  constructor reads by field name — `p.pos.x` — and a type with several is
  read by `match`, which is the only thing that can answer for it. Errors are `Result`
  values, and `?` carries one out of a chain without the carrying being
  written — it compiles to the `match` you would have typed.
- **Hash tables.** `Map(k, v)` with anything that has structural equality for a
  key — a number, a string, a tuple, a data type — hashed by code the compiler
  writes from the type. `m[k] = v` puts something in and `m[k]` answers an
  `Option`, because a key may not be there. A million `Int` keys go in in 26 ms
  and come back out in 22, against 61 and 23 for Go's own map, in 73 MB against
  Go's 85.
- **No garbage collector.** Compile-time reference counting (Perceus-style)
  frees memory the moment it dies — deterministic, no pauses.
- **Go's concurrency model.** `spawn` and typed channels over a green-thread
  scheduler — one lightweight thread is a **strand**; 100,000 strands cost ~53 MB and start in microseconds. Build
  with `--parallel` and set `RILL_THREADS` to spread them across cores. `select`
  takes a `timeout(ms)` arm, so a deadline costs no strand. `close(ch)` is how
  a program stops: one broadcast wakes everything parked on the channel,
  `wait_signal()` is how it hears that it should, and `sock_shutdown` does the
  same for a strand parked on a socket read.
- **Native code via LLVM -O3.** Generics are monomorphized, so nothing is boxed
  and trait dispatch is resolved at compile time. Hello world is 33 KB.
- **Enough IO to write servers.** TCP sockets that fit the concurrency model: A strand waiting on a
  socket parks and the scheduler polls the OS, so one strand per connection
  costs nothing while it waits.
- **Files that outlive a read.** A file is a descriptor like a socket, with
  reads at an offset so several readers share one open file, and a `file_sync`
  that hands the wait to a helper thread and parks the strand — so the worker
  keeps running everything else while the disk takes its millisecond.
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

And the page that server would be answering with. `tools/tmplgen.rill`
compiles an HTML template into Rill before the program is compiled, so there
is nothing to interpret at run time. On the two pages the
[askama-rs suite](https://github.com/askama-rs/template-benchmark) renders,
byte for byte the same output (`python3 benchmarks/template/run.py`):

| engine | 100×100 table | a small page |
|---|---:|---:|
| **rill** | **11.46 µs** | **29 ns** |
| sailfish | 11.55 µs | 32 ns |
| markup | 26.28 µs | 58 ns |
| askama | 28.72 µs | 45 ns |
| maud | 33.29 µs | 68 ns |

Sailfish is the fastest of the Rust engines and, like this one, a compile-time
generator. Almost none of the difference is the generator: a page is a run of
appends, and `benchmarks/template/README.md` is about what an append costs.

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

Requires Rust and LLVM 21 (`brew install llvm@21`; the path is wired up in
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
./target/release/rill run untrusted.rill --sandbox  # refuses files, sockets, foreign code, raw memory
```

### In an editor

The compiler is also a language server: `rill lsp` answers an editor with
diagnostics on the buffer as it stands, the type of the name under the cursor,
go to definition, an outline and formatting — which in a language that infers
everything is the only place a signature gets said out loud. `editors/README.md`
has the few lines Neovim, Helix or Emacs needs to start it, and `rill types
main.rill` prints the same signatures for a whole file.

`editors/vscode/` is a Visual Studio Code extension: syntax highlighting for
every construct in the language, two-space layout-aware indentation, and
snippets. Symlink it in and reload the window:

```sh
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/rill
```

Cursor, Windsurf and VSCodium read the same folder under their own
`extensions` directory. See `editors/vscode/README.md` for packaging it as a
`.vsix` and for how the grammar is tested.

`rill doc lib -o docs/lib` writes a Markdown page per library module from
the comments the modules already have, with the signatures the checker
infers; there is no doc-comment syntax, because the comment above a
declaration is its documentation. A line in it indented four spaces is an
example, and `rill doctest lib` runs every one and holds it to what it
says after `# =>` — every function of the standard library carries one.

The model editor — a page that shows a file's `system` as the block
diagram and each `machine` as SDL drew a process, edits them as forms
written back through the formatter, and replays a scenario's run on
them — is a repository of its own, `rillsdl`, and runs at
<https://rillsdl.baltavista.com>. Its server is a Rill program on the
standard library that runs this compiler for the page.

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
| `http/` | a server answering its own requests, and a WebSocket one |
| `json/`, `yaml/` | reading and writing both, from `lib/` |
| `oxidb/` | the five OxiDB clients driven, four of them against stand-in servers on strands answering the shapes a real engine answers with |
| `oxidb/catalogue.rill` | a hundred thousand documents seeded into OxiDB and served over HTTP: a browsable page with filters and a pager at `/`, and JSON at `/count`, `/doc/42`, `/find?role=eng&limit=3`, `/top?by=score&limit=5`. Three libraries meet and none knows about the others; the page is four templates — one including the others — compiled to Rill before the program is, and a signals library takes the results over in the browser without the page needing to be built twice |
| `maps.rill` | hash tables: data types as keys, `Option` out of every read |
| `results.rill` | `?`: a chain of things that can fail, and the first failure out of it |
| `records.rill` | field names read with a dot, and the building that changing one is |
| `stdlib.rill` | what the prelude gives you |
| `loops.rill` | how repetition is written: tail recursion, folds, `for` and `while` |
| `blocks.rill` | block bodies in match arms, select arms and `if` branches |
| `multi/` | a program split across files, plus aliased imports that keep colliding names apart |
| `template/` | a page built out of four templates: one includes the others and hands them what they need, and `tmplgen` turns all four into Rill before the program is compiled |
| `webgpu/` | a spinning triangle: Rill compiled to wasm computing frames for WebGPU |
| `kv_store.rill` | a key–value store on disk from `lib/db/kv.rill`: a B+ tree of pages, a page cache and a write-ahead log; ahead of SQLite on reads, level on the durable commit |
| `grid_tour.rill` | arrays with a shape from `lib/num/grid.rill`: views, masks, reductions, a matrix product — NumPy's shapes, and faster than NumPy where there is arithmetic to do |
| `lanes.rill`, `holders.rill` | `Float2`, two floats in one register; `unique`, the question that lets an operation reuse a temporary's storage |
| `exchange.rill`, `abp.rill` | machines, as SDL drew a process: a telephone exchange, and the alternating-bit protocol with its retransmission timer, lossy medium, block diagram, and tests on virtual time |

The largest thing written in Rill is not in this table. Rill Valley — a
walkable low-poly valley with terrain, physics and matrices in Rill, shadows,
MSAA and compute particles on the GPU, and a `server.rill` that is the same
valley compiled for a Linux box so several browsers can walk about in it at
once — outgrew the examples directory and lives in its own repository. Point
its `build.sh` at a `rill` binary and it builds against whatever compiler you
give it.

`lib/` is what the examples above are written on, and is written in Rill
itself, a folder per kind (`text/`, `data/`, `web/`, `db/`, `num/`, `crypto/`,
`time/`, `random/`, `cli/`): `data/json.rill` and `data/yaml.rill`,
`web/http.rill` for HTTP/1.1, `web/ws.rill` for WebSocket, `crypto/sha1.rill`
for the handshake, `web/tmpl.rill` for HTML templates (with
`tools/tmplgen.rill`, which compiles a template into Rill rather than
interpreting one — its header is the syntax), `num/grid.rill` for arrays with a
shape (and `bindings/python`, which makes it `import grid as g` in Python —
NumPy's notation on Rill's loops, through `rill build --shared`), and a client
for each of the five engines OxiDB mounts. Four are on its native wire and
share `db/oxiwire.rill`, the server's binary format — about a third larger on
the wire than its JSON one and about 1.4 times faster to talk: `db/oxidb.rill`
for documents, `db/oxisql.rill` for SQL, `db/oxitsdb.rill` for time series and
`db/oxirec.rill` for co-occurrence recommendations. The fifth,
`db/oxiresp.rill`, speaks RESP to the key-value engine on a port of its own —
the protocol Redis speaks. It is not a standard library and nothing is promised
about it between releases — the prelude is the part that ships with the
compiler. It is a directory, so `import "data/json"` finds it from anywhere
under this repository, and `RILL_PATH` points at it from outside.

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

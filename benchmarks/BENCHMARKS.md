# Rill vs Go vs C# vs OCaml

The four languages here are the ones that ship lightweight concurrency, which
is what `ring` is about; the other three benchmarks come along because the
same four programs answer them.

Apple M4, macOS. Rill via LLVM 21 `-O3` (host CPU features) with the release
runtime; Go 1.23.4 (`go build`); C# 11 preview (NativeAOT, so what is timed is
a program rather than a runtime warming up); OCaml 5.5.0 (`ocamlopt`, Homebrew
build, non-flambda). Time = median of 25 runs (warmup 3); peak RSS = median of 3 via
`/usr/bin/time -l`; outputs cross-checked identical across all three
languages. Reproduce with `python3 benchmarks/run.py`.

Twenty-five runs rather than five because these programs take tens of
milliseconds and process startup is a large part of that: a five-run median
moved by as much as 40% between repeats, which is enough to invent a
regression that is not there, or hide one that is.

## Time

Rill 0.13.0, 2026-09-16, the release the downloads are: fib 15.0 ms
(go 20.9, c# 21.3, ocaml 22.0), trees 16.3 (55.1, 46.5, 57.3), ring 12.7
(94.4, 150.1, 64.8), lcg 121.0 (264.6, 175.8, 284.2); RSS the same as
below except `ring`, 64.7 MB, up from 53.5, and binaries 66–83 KB, up
from 32–34 — both since the crash reporter and its per-function table
landed, neither yet looked into. The table below is the 2026-09-05 run,
kept for the history.

Re-measured 2026-09-05 evening, all four languages in the same run, after the
day's compiler work (see *Against C* below). The machine was busier than on
earlier passes — every language's absolute number is higher than the tables
this page carried before — so the ratios are the column to read.

| benchmark | what it measures | rill (ms) | go (ms) | c# (ms) | ocaml (ms) | fastest |
|---|---|---:|---:|---:|---:|---|
| fib | fib(35), pure recursion, no allocation | 19.3 | 26.1 | 27.5 | 28.9 | **rill, 1.35× over 2nd** |
| trees | 10× build+drop perfect binary tree, depth 18 | 21.0 | 67.1 | 59.1 | 68.5 | **rill, 2.81× over 2nd** |
| ring | 100k lightweight threads chained by channels | 15.7 | 111.5 | 200.8 | 82.2 | **rill, 5.23× over 2nd** |
| lcg | 100M-iteration LCG loop (tail recursion vs `for`) | 148.1 | 325.4 | 215.6 | 345.9 | **rill, 1.46× over 2nd** |

`trees` moved from 1.80× to 2.81× over the second place in one day: the tree
walk no longer touches a reference count per node (borrowed data
parameters) and every allocation and free is three inline instructions.

## Peak memory (RSS)

| benchmark | rill | go | c# | ocaml | smallest |
|---|---:|---:|---:|---:|---|
| fib | 1.2 MB | 3.5 MB | 6.7 MB | 2.1 MB | **rill** |
| trees | 13.2 MB | 14.2 MB | 38.2 MB | 18.3 MB | **rill** |
| ring | 53.5 MB | 269.3 MB | 138.6 MB | 103.0 MB | **rill** |
| lcg | 1.2 MB | 3.6 MB | 6.7 MB | 2.1 MB | **rill** |

The same evening's run of `tokens/` (eight languages) puts Rill ahead of Go
on every one of its five timed programs as well: json 148 ms against 309,
dijkstra 125 against 1049, loop 36 against 40, algo 56 against 93, mandel 238
against 260.

## Parallel scaling

`parallel.rill` gives eight independent `fib(32)` computations to strands.
Built with `rill build --parallel` and run with `RILL_THREADS=N`:

| workers | time | speedup |
|---:|---:|---:|
| 1 | 29.1 ms | 1.0× |
| 2 | 15.7 ms | 1.9× |
| 4 | 9.3 ms | 3.1× |
| 8 | 7.6 ms | 3.8× |

The machine has four performance cores and six efficiency cores, so the curve
flattening past four workers is the hardware, not the scheduler.

Note that the `ring` benchmark gets *slower* with more workers (13.5 ms at one,
57 ms at two, 83 ms at four): it is a chain of rendezvous, so there is nothing
to run in parallel and every hop pays cross-thread wakeups. That is exactly why
parallelism is opt-in per build and per run rather than the default.

## Binary size

| | rill | go | ocaml |
|---|---:|---:|---:|
| all four benchmarks | 32–34 KB | 2.1 MB | ~1.0 MB |

**Rill sweeps all three tables against both languages.**

## HTTP server

`benchmarks/http/`: the same one-route service — `GET /` → `Hello, World!` —
written on Rill's HTTP module (`lib/web/http.rill`, itself written in
Rill on top of the socket builtins) and on Go's `net/http`. Load from `wrk
-t4 -c50 -d10s` on the same machine. Reproduce with `benchmarks/http/run.sh`.
Go's connection-per-request run reported 48 connect errors out of 133k
requests, which is the client running out of sockets rather than the server
refusing them.

Both servers keep connections alive and let the client decide, so the two
modes below differ only in the `Connection:` header wrk sends — neither server
is configured into a different code path than the other.

| server | req/sec | avg latency | p99 latency | RSS |
|---|---:|---:|---:|---:|
| rill, keep-alive | **209,831** | 227 µs | **278 µs** | **2.3 MB** |
| go, keep-alive | 198,359 | 221 µs | 880 µs | 21.3 MB |
| rill, connection per request | **41,035** | 3.36 ms | 98.5 ms | **2.0 MB** |
| go, connection per request | 13,310 | 1.32 ms | **2.05 ms** | 20.1 MB |

| | rill | go |
|---|---:|---:|
| server binary | **51 KB** | 7.1 MB (138×) |

**Keep-alive** is the mode a real service runs in, and the two are within 6%
of each other on throughput and identical on average latency — but Rill's p99
is 3.2× tighter, because there is no garbage collector to run, and it does it
in 9.3× less memory.

**Connection per request** is dominated by accept plus per-connection setup,
and Rill serves 3.1× as many.

### About that tail

The p99 in the last row looks like a Rill defect and is not one: it is a
function of the connection *rate*, and Rill only reaches it because it is
three times faster. At 50 connections Go cannot go past ~13k conn/s in this
mode, so it never enters the regime where the tail grows, and Rill — running
at three times the rate — does.

Driving both servers at rates that match, three runs each, each server started
fresh and the kernel drained between runs:

| connections | rill req/sec | rill p99 | go req/sec | go p99 |
|---:|---:|---:|---:|---:|
| 1 | **19,000** | 24.5 ms *(17.7–29.8)* | 16,667 | **13.4 ms** *(7.2–14.7)* |
| 2 | **28,265** | **28.1 ms** *(28.0–30.1)* | 25,190 | 38.1 ms *(38.1–41.2)* |
| 50 | **41,035** | 98.5 ms | 13,310 | **2.05 ms** |

Throughput is reproducible and Rill leads at every rate. The tail is not, and
the ranges are why it is quoted here rather than a single figure: repeating the
one-connection case without changing anything gave a Rill p99 of 34.2 ms, then
1.84 ms, then 4.60 ms. What that column is measuring at these rates is mostly
the state of the client's socket table, not the server.

Read at face value these three runs put Go's one-connection tail ahead of
Rill's and Rill's two-connection tail ahead of Go's, which is a reversal of
what this table said when it was first measured with one run per cell. The
honest reading is that connection-per-request p99 does not separate the two
servers on this machine; the keep-alive p99, which churns no connections and
does repeat, does — and there Rill is 3.2× tighter.

The spikes are not the server waiting on itself. Instrumenting the scheduler
during a 40k conn/s run: no strand ever ran longer than 20 ms, and at every
latency spike the server was completely idle — one strand parked on the
listening socket, nothing queued, nothing running. Cutting the scheduler's
poll timeout from 20 ms to 1 ms removed all internal waiting and did not move
the p99 at all. What runs out at that rate is the client side: 16,384
ephemeral ports against a 30-second TIME_WAIT means every connection reuses a
4-tuple that is still in it.

Two more caveats. Go sends a `Date` header and Rill does not, which is a real
HTTP requirement Rill's module skips (Go caches the formatted date per second,
so the cost is small but not zero). And a connection-per-request run leaves
hundreds of thousands of sockets in TIME_WAIT, which throttles both servers
and any run that follows — `run.sh` waits for the kernel to drain between
cases, and longer runs measure lower for both sides, so the ratio is the
meaningful part, not the absolute number.

### Several workers

Servers get the same opt-in parallelism as everything else
(`rill build --parallel`, then `RILL_THREADS=N`):

| workers | keep-alive req/sec | connection per request |
|---:|---:|---:|
| 1 | 194,286 | 41,076 |
| 2 | 215,064 | 38,863 |
| 4 | 199,268 | 38,549 |

There is little to gain here — this workload is bound by syscalls and by the
load generator, not by CPU — and the point of the table is that spreading it
across workers no longer costs anything. Until 0.2.11 it cost everything: the
allocator kept one global free list with no synchronization, so an
allocation-heavy program on several workers corrupted it and crashed, and the
netpoller delivered a wakeup to whichever worker happened to be asleep rather
than to the one that owned the strand, which put a 20 ms stall on nearly
every request (2,219 req/s on four workers, against 42,179 on one).

## Notes

**Computation (fib, lcg):** LLVM's O3 pipeline beats both the Go compiler and
`ocamlopt` on scalar code and tight loops.

**Allocation churn (trees):** Rill's Perceus-style reference counting with a
size-class pool allocator and immortal nullary constructor singletons (`Leaf`
never allocates, matching OCaml's unboxed constant constructors and Go's
`nil`) outruns both Go's concurrent GC and OCaml's generational GC — in time
*and* peak memory, since dead trees are reclaimed the instant their count hits
zero rather than waiting for a collection. Go originally won this benchmark
2.25× before those optimizations landed.

**Concurrency (ring):** Rill uses a copy-on-block stack model: all spawned
strands run on one shared, guarded 8 MB execution stack at a fixed address;
when a strand parks on a channel, only the bytes it actually uses
(typically a few hundred) are copied to a heap buffer, and copied back to the
same address on resume — so interior pointers stay valid without compiler
stack maps. A parked strand costs ~300 bytes instead of a stack page,
spawning does zero syscalls and zero page faults, and every strand gets
full overflow protection. Combined with release-before-tail-call codegen
(tail recursion with heap parameters runs in constant stack), this made the
100k-thread ring both the fastest (7.3× vs Go, 4.9× vs OCaml) and the
smallest (5.0× less memory than Go, 1.9× less than OCaml). Go uses growable
8 KB-initial stacks; OCaml 5 has no built-in lightweight threads — its
benchmark hand-rolls a ~50-line effects-based scheduler (the canonical
pattern), which notably beats Go's time here while Rill beats both.

The August 2026 pass over reference counting moved these too, without any of
them being what it was aimed at: `trees` — which is nothing but building and
dropping — went from 34.3 ms to 26.8, `fib` from 16.6 to 15.6, `ring` from
13.6 to 12.4, `lcg` from 133.1 to 125.2. The changes were an inlined release
fast path, a `match` that stops taking a reference of its own, a field moved
into the box it is being put in rather than counted twice, and Perceus reuse.

## History

Ring time: 321 → 13.6 ms (24×) via release toolchain, arena spawns, then the
copy-on-block stack model. Ring memory: 399 → 53 MB (7.5×) from the same
model. Trees: 125 → 34 ms (3.7×) via immortal nullary constructor singletons
and the sized pool allocator (`rill_free_sized` — object sizes are statically
known from monomorphized types, so frees are a two-instruction list push).

The shutdown sweep that releases what abandoned strands were holding briefly
cost ring half its time again: it walked every strand record ever made, which
is a cache miss apiece and a hundred thousand of them here, to find the none
that were still running. It now asks a counter first.

## Files

Each benchmark is a `.rill`/`.go`/`.cs`/`.ml` quadruple kept as close to
idiomatic-equivalent as possible; `run.py` builds all four, times them,
measures peak RSS, and fails if outputs differ.

C#'s `ring` is `System.Threading.Channels` and `async Task`, which is what a
C# program uses and needs no thread per hop — the honest counterpart to a
goroutine or a strand. It is the slowest of the four here and the second
hungriest: 150 ms and 139 MB for a hundred thousand awaiting tasks, against
Rill's 12 ms and 54 MB.

The four languages on this page are the ones with lightweight concurrency in
the box. Rust has no executor in its standard library, and C, C++ and Zig have
only OS threads, so a hundred thousand of anything means either an external
runtime or a scheduler written by hand — at which point the benchmark measures
the scheduler rather than the language. That is why they are in
`tokens/` and not here.

## What a program costs to write

`tokens/` is a second suite with a different question: not just how fast the
same algorithm runs, but how much source it takes to say it. The same JSON
parser and the same Dijkstra are written in eight languages — Rill, C, C++,
Rust, Zig, C#, Go and OCaml — and counted with `tiktoken` as well as timed.

| | Dijkstra | JSON |
|---|---:|---:|
| C++ | **692** | 1503 |
| Rill | 740 | 1353 |
| Go | 744 | **1338** |
| C# | 776 | 1440 |
| Rust | 803 | 1698 |
| C | 836 | 1988 |
| OCaml | 895 | 1343 |
| Zig | 1033 | 1846 |

`o200k_base` tokens. Measuring this is what most of the language's surface
came from: `b[i]`, local recursive functions, `for` and `+=`, tuples, `#s`,
`s[i:j]`, `for … with`, `scan_int` and a string builder were each added
because a count said where the words were going. The Dijkstra program went
from 1579 tokens to 740 along the way.

The suite times three more programs it does not count: a tight indexed loop, a
Dijkstra with the graph built in process, and a Mandelbrot. That last one is
there because nothing else in either suite is floating point in a loop with a
data-dependent exit, and it produced the most surprising number in the set —
Rill 206 ms against C's 206, while emitting no fused multiply-add at all where
C emits three. Turning C's contraction off puts it at 252, exactly where Rust
and Zig already sit.

C++ writes the Dijkstra in the fewest tokens of the eight and runs it the
slowest of the eight, and both are `std::cin >>`: 3.7 seconds against C's 217
ms, and 218 ms once the same program reads with `scanf`.

See `tokens/README.md`; reproduce with `python3 benchmarks/tokens/gen_inputs.py`
then `python3 benchmarks/tokens/run.py`.

## Strands against goroutines

`benchmarks/strands/` is six more programs where only the scheduler and the
channels are being measured — spawning 200k strands, a million ping-pongs,
a hundred producers into one channel, a worker pool, `select` over four
producers, fifty stages in a line. Rill is ahead of Go on every one, by 1.2×
(`fanin`) to 4.3× (`select`) over whichever of Go's two configurations is
faster, and holds 200k parked strands in 81 MB where Go holds them in 517.
Go on all cores is slower than Go on one in five of the six: a hand-off
between processors is a cross-core wakeup. Numbers and the reasons in
`benchmarks/strands/README.md`.

## Disk and network against Go

`benchmarks/io/` is eight programs that only move bytes — a 32 MB file
there and back, five thousand small files, `wc` over 31 MB of standard input,
two million lines to standard output, an echo round trip 100k times, 500
connections at once, 64 MB down one socket, a million small writes through a
buffered one. Rill is ahead on the four disk and stream programs (2.1× on
output, 1.3× on `wc`), 1.7× ahead on the buffered stream (`sock_buffered`
against `bufio.Writer`), and within 1–4% of Go on one processor on the three
round-trip socket programs, which are the kernel's; the
first run of the suite had found standard output unbuffered and `read_line`
copying twice, both fixed. 500 live connections are 1.9 MB against Go's 20.
Numbers and the reasons in `benchmarks/io/README.md`.

## grid against NumPy

`lib/num/grid.rill` is the array library — a shape over one `Buf(Float)`, views
for free, masks a byte wide, a `Float2` matrix kernel, a bitwise sort, an LU
factorisation — and `benchmarks/numpy/` puts it beside NumPy 2.4 with
OpenBLAS on twelve programs, one core against one thread and all cores
against all threads. On one core Rill is ahead on eight of the twelve and
level on the other four: two hundred thousand small operations 11×, sorting
2.3×, reductions and moving numbers about 1.9×, a mask and a `where` 1.8×,
order statistics and an 800 × 800 matrix product 1.1×. Where both sides do
nothing but stream memory — an elementwise expression, sums along an axis, a
dot product — call the same maths library, or solve a linear system, they
are level. On all cores NumPy keeps two, its threaded matrix product and its
threaded factorisation. On the whole process — Python's start and NumPy's
import are 60 ms — Rill is 2–12× ahead on all twelve. Numbers and the
compiler work they took in `benchmarks/numpy/README.md`.

## Against C

`benchmarks/c/` holds the same question asked of C: nineteen programs, each a
`.rill`/`.c` pair to the same algorithm and layout. On the morning of
2026-09-05 Rill won three, tied four and lost twelve; by the evening, after
twenty compiler changes — internal linkage, borrowed parameters, pipeline
fusion, flat records, hoisted bounds checks, `map` as a loop, an inlined
allocator and map probe, buffers addressed at their first element — it wins
eight (`trees` 0.40×, `dijkstra` 0.44×, `json`, `nbody_rec`, `hashmap`,
`mandel`, `fannkuch`, `qsort`) and is within run-to-run noise on the other
eleven. Nothing is behind. Every step is measured in
`benchmarks/c/README.md`. Reproduce with `python3 benchmarks/c/run.py`.

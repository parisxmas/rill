# Rill vs Go vs OCaml

Apple M4, macOS. Rill via LLVM 18 `-O3` (host CPU features) with the release
runtime; Go 1.23.4 (`go build`); OCaml 5.5.0 (`ocamlopt`, Homebrew build,
non-flambda). Time = median of 25 runs (warmup 3); peak RSS = median of 3 via
`/usr/bin/time -l`; outputs cross-checked identical across all three
languages. Reproduce with `python3 benchmarks/run.py`.

Twenty-five runs rather than five because these programs take tens of
milliseconds and process startup is a large part of that: a five-run median
moved by as much as 40% between repeats, which is enough to invent a
regression that is not there, or hide one that is.

## Time

| benchmark | what it measures | rill (ms) | go (ms) | ocaml (ms) | fastest |
|---|---|---:|---:|---:|---|
| fib | fib(35), pure recursion, no allocation | 16.6 | 22.8 | 23.9 | **rill, 1.38× over 2nd** |
| trees | 10× build+drop perfect binary tree, depth 18 | 34.3 | 57.9 | 61.2 | **rill, 1.69× over 2nd** |
| ring | 100k lightweight threads chained by channels | 13.6 | 99.3 | 67.1 | **rill, 4.93× over 2nd** |
| lcg | 100M-iteration LCG loop (tail recursion vs `for`) | 133.1 | 288.2 | 310.3 | **rill, 2.16× over 2nd** |

## Peak memory (RSS)

| benchmark | rill | go | ocaml | smallest |
|---|---:|---:|---:|---|
| fib | 1.2 MB | 3.5 MB | 2.1 MB | **rill** |
| trees | 13.2 MB | 16.9 MB | 18.3 MB | **rill** |
| ring | 53.5 MB | 269.4 MB | 103.0 MB | **rill** |
| lcg | 1.2 MB | 3.6 MB | 2.1 MB | **rill** |

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
written on Rill's HTTP module (`examples/http/http.rill`, itself written in
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

Each benchmark is a `.rill`/`.go`/`.ml` triple kept as close to
idiomatic-equivalent as possible; `run.py` builds all three, times them,
measures peak RSS, and fails if outputs differ.

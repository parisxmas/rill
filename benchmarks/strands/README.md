# Strands against goroutines

Six programs, each written once with Rill's strands and once with Go's
goroutines, doing nothing but spawning, handing off and choosing — the
scheduler and the channels are what is measured, not arithmetic. Every
workload is communication-bound on purpose, which is also why Go is timed
twice: on one processor (`GOMAXPROCS=1`, the shape Rill runs in by default)
and as it ships, on all of them. Apple M4, Go 1.23.4, Rill via LLVM 18 `-O3`;
time is the median of 25 runs after 3 warmups, outputs cross-checked, peak RSS
the median of 3. Reproduce with `python3 benchmarks/strands/run.py`.

| benchmark | what it does | rill (ms) | go, 1 proc | go, all procs | rill over the better Go |
|---|---|---:|---:|---:|---|
| spawn | 200k strands, each sends one number and ends | 36.8 | 230.3 | 147.6 | **4.0×** |
| pingpong | two strands, a million round trips on rendezvous channels | 46.9 | 141.4 | 215.8 | **3.0×** |
| fanin | 100 producers × 10k messages into one buffered channel | 15.7 | 19.3 | 184.4 | **1.2×** |
| workers | 8 workers over a job queue, a million jobs | 14.2 | 35.4 | 100.9 | **2.5×** |
| select | one consumer choosing among four producers, a million messages | 36.4 | 156.8 | 329.6 | **4.3×** |
| stages | 50 stages in a line, 20k numbers through — a million hand-offs | 27.2 | 77.7 | 62.7 | **2.3×** |

| peak RSS | rill | go, 1 proc | go, all procs |
|---|---:|---:|---:|
| spawn | 81 MB | 517 MB | 517 MB |
| the other five | 1.3–1.4 MB | 3.2–3.6 MB | 3.6–4.4 MB |

## What the numbers say

**A hand-off is the unit.** `pingpong` is two million of them and nothing
else: 23 ns each in Rill against 71 in Go on one processor. Rill's strands
share one execution stack and a parked strand is the few hundred bytes it
was actually using, copied out; switching is a memcpy and a jump, with no
kernel, no stack pool and no work-stealing to consult. Go's parked goroutine
keeps its own stack and its wakeup goes through the run queue.

**More processors make a rendezvous slower**, not faster: Go on all cores is
worse than Go on one in five of the six, by up to 9× (`fanin`), because every
hand-off between goroutines on different processors is a cross-core wakeup.
That is the same effect `ring` in the main suite shows, and the reason Rill's
parallelism is opt-in per build (`--parallel`) and per run (`RILL_THREADS`)
rather than the default. `spawn` is the exception — the 200k goroutines have
nothing to hand off, so spreading their creation helps Go — and Rill is still
4× ahead of that.

**`spawn`'s memory** is the copy-on-block model again: 200k parked senders are
81 MB in Rill, 517 MB in Go — a goroutine's stack starts at 8 KB whether it
uses it or not, and the 200k senders here are all parked on a full channel at
once.

**`fanin` is the closest**, and it is the one where the channel does the most
work relative to the scheduler: a hundred senders queueing on one buffer of a
hundred. Rill's channel is a ring buffer with a wait list, Go's is the same
idea with a lock, and on one processor they are within 25%.

## Files

Each `.rill`/`.go` pair keeps the same channel shapes: rendezvous where the
other is rendezvous, the same buffer sizes, the same counts. The Rill
programs' loops are tail-recursive functions, which is what a loop is in
Rill; the Go programs' are `for`.

# Chapter 16. Strands and Channels

Rill's concurrency is Go's: many lightweight threads passing values over typed
channels, rather than shared memory and locks.

One of those lightweight threads is a **strand** — a strand of a rope, of which
a program is many, twisted together. Go calls the same thing a goroutine,
Erlang a process, Java a virtual thread, Rust a task. The concept is common; the
word here is Rill's own.

> **A note for C++ networking people:** Boost.Asio uses `strand` for something
> else — a serialised execution context that guarantees handlers do not run
> concurrently. The two ideas are unrelated.

---

## 16.1 Three primitives

```rill
# book/code/ch16/strands.rill
fn worker(jobs, results) =
  n = recv(jobs)
  send(results, n * n)
  worker(jobs, results)

fn feed(jobs, i, n) =
  send(jobs, i)
  if i >= n then 0 else feed(jobs, i + 1, n)

fn collect(results, left, acc) =
  v = recv(results)
  if left <= 1 then acc + v else collect(results, left - 1, acc + v)

fn main() =
  jobs = channel()
  results = channel()
  spawn worker(jobs, results)
  spawn worker(jobs, results)
  spawn worker(jobs, results)
  spawn feed(jobs, 1, 10)
  println(collect(results, 10, 0))
```

```
$ rill run strands.rill
385
```

385 is 1² + 2² + … + 10², computed by three workers pulling from one queue.

That is the whole API:

| | |
|---|---|
| `spawn expr` | run `expr` on a new strand |
| `channel()` | a `Chan(a)`; `channel(n)` buffers n values |
| `send(ch, v)` / `recv(ch)` | rendezvous — a sender blocks until a receiver arrives, and vice versa |
| `select` | wait on several channels; first ready wins |

Three things about the listing are worth drawing out.

**`worker` is an infinite tail recursion.** It receives, sends, and calls
itself. That is Chapter 7's "loop that never ends", running in constant stack
forever. There is no `while true` because there does not need to be.

**Three workers share one channel with no lock.** A rendezvous channel *is* the
synchronisation: whichever worker reaches `recv` first gets the value, and the
others wait. There is no mutex in this program, and there is nothing to
forget to unlock.

**Nothing is shared but the channels.** Each strand has its own stack and its
own values. This is why Chapter 1's point about immutability and concurrency
matters — a data race needs a write, and there is nothing to write.

> **`spawn` takes the whole expression**
>
> `spawn handle(accept(s))` runs *both* calls on the new strand, including the
> accept. If the caller was supposed to do the accepting, bind it first:
> `c = accept(s)` then `spawn handle(c)`. This is the most common mistake with
> `spawn`, and it turns an accept loop into a strand-creation loop.

---

## 16.2 Pipelines

Strands compose into stages, each doing one job and passing the result on:

```rill
# book/code/ch16/pipeline.rill
fn source(out, i, n) =
  send(out, i)
  if i >= n then send(out, 0 - 1) else source(out, i + 1, n)

fn square(inp, out) =
  v = recv(inp)
  if v < 0 then send(out, 0 - 1) else pass_on(inp, out, v)

fn pass_on(inp, out, v) =
  send(out, v * v)
  square(inp, out)

fn drain(inp, acc) =
  v = recv(inp)
  if v < 0 then acc else drain(inp, acc + v)

fn main() =
  a = channel()
  b = channel()
  spawn source(a, 1, 10)
  spawn square(a, b)
  println(drain(b, 0))
```

```
$ rill run pipeline.rill
385
```

Same answer, different structure: a generator, a transformer and a consumer,
each an independent strand, connected by two channels.

This is `map` over a stream instead of over a list — and the difference is that
the stages run concurrently and nothing is ever fully materialised. A negative
number is the end-of-stream marker; a language with sum types over channels
would use `Option` and be tidier, and there is no reason Rill could not.

---

## 16.3 `select`

`select` waits on several channels at once and takes whichever is ready first:

```rill
# book/code/ch16/selecting.rill
fn poll(ch) =
  v = select
    x = recv(ch) -> x
    default -> 0 - 1
  v

fn both(a, b, left, acc) =
  got = select
    x = recv(a) -> x
    y = recv(b) -> y
  if left <= 1 then acc + got else both(a, b, left - 1, acc + got)
```

```
$ rill run selecting.rill
309
-1
```

309 is (1+2+3) + (100+101+102) — six values taken from two channels in whatever
order they arrived. The `-1` is the second call: `poll` on a channel with
nothing in it takes the `default` arm rather than blocking.

- Arms are tried **in order**, so an earlier arm wins a tie.
- A **`default` arm** turns `select` into a non-blocking poll.
- A **send arm** (`send(ch, v) -> e`) is ready when the send could proceed.
- Like `match`, `select` needs an indented block, so it goes in a body, a
  statement or a binding.

---

## 16.4 A hundred thousand strands

```rill
# book/code/ch16/many.rill
fn relay(inp, out) = send(out, recv(inp) + 1)

fn chain(first, n) = if n == 0 then first else chain(link(first), n - 1)

fn link(inp) =
  out = channel()
  spawn relay(inp, out)
  out

fn main() =
  head = channel()
  tail = chain(head, 100000)
  spawn send(head, 0)
  println(recv(tail))
```

```
$ /usr/bin/time -l ./many
100000
        0.31 real
            56049664  maximum resident set size
```

A hundred thousand strands, each holding a channel and waiting to pass a number
along, and the whole thing costs **56 MB and a third of a second** — including
creating all of them, passing a value down the entire chain, and tearing it
down.

That is about 560 bytes per strand. An OS thread would be a megabyte of stack
each, so this program would need 100 GB.

### How

A strand needs a stack, and a stack is the expensive part. The trick is not to
give each one a stack of its own.

All running strands share **one** guarded 8 MB execution stack at a fixed
address. When a strand parks — blocks on a channel or a socket — the scheduler
copies out only the bytes it actually used, into a heap block sized to fit, and
keeps that on the strand's record. When it resumes, the bytes are copied back
to the *same address*, so every interior pointer in those frames is still
valid.

A parked strand therefore costs roughly what it was actually using — a few
hundred bytes — rather than a reserved stack. There are no per-strand guard
pages to install, no `mmap` per spawn, and no page faults for stack that was
never touched.

The cost is the copy on every park and resume, which is why this design suits
strands that block often and briefly. The Chapter 2 benchmark table is the
measurement: the 100k-strand ring is 13.5 ms against Go's 96 ms and OCaml's
62 ms, and 53 MB against 269 MB and 103 MB.

---

## 16.5 Blocking on the world

A strand that waits on a socket does not hold up anything. Its descriptor goes
to the operating system's poller and the worker moves on to another strand;
when the socket is ready the scheduler wakes it.

```
fn serve(server) =
  c = accept_on(server)
  spawn handle(c)
  serve(server)
```

One strand per connection, ten thousand connections, and the cost while they
wait is the parked stacks. This is why `examples/http/http.rill` — an HTTP/1.1
server in about a hundred lines of Rill — outruns Go's `net/http` on the same
one-route service.

The limitation is C. A foreign call that blocks freezes the OS thread running
it, because the scheduler cannot preempt code it did not compile. Chapter 17
covers the boundary.

---

## 16.6 Deadlock, and the exit rule

Two behaviours to know, both inherited from Go and both visible immediately.

**A program where every strand is blocked is reported rather than hung:**

```
$ rill run dead.rill
rill: deadlock - all strands are blocked
```

That is the whole program `ch = channel()` then `recv(ch)`. Nothing will ever
send, the scheduler notices that no strand can make progress, and it says so
instead of hanging forever.

**The program exits when `main` returns.** Strands still running are abandoned,
exactly as in Go. If the answer matters, `main` must wait for it — which is
what the `collect` and `drain` functions in this chapter are doing. Under
`RILL_DEBUG_ALLOC=1` an abandoned strand shows up as live allocations at exit;
that is not a leak in the reference counter, it is a program that stopped while
holding things.

---

## 16.7 Several cores, on purpose

By default every strand runs on **one OS thread**. That is not a limitation
that was left in; it is what allows reference counting to be non-atomic
(Chapter 15), and that is worth a great deal on an operation as frequent as
counting.

Programs that want real parallelism opt in twice:

```sh
rill build main.rill -o main --parallel   # counts become MT-safe
RILL_THREADS=8 ./main                     # eight workers
```

`--parallel` makes each count check a flag and use an atomic update when more
than one worker is live. Without it the runtime refuses `RILL_THREADS` and says
so — you cannot get the parallelism by accident, and you cannot get data races
by forgetting the flag.

Workers steal strands that have not started yet from one another. A strand that
has already run stays on its worker, because its frames live at that worker's
stack address.

**Parallelism helps when strands compute independently.** A chain of
rendezvous — one strand handing a value to the next, like §16.4 — has nothing
to overlap, and runs fastest on a single worker. Measure before you reach for
the flag.

---

## 16.8 Compared

| | Rill strands | Go goroutines | Erlang processes | OS threads |
|---|---|---|---|---|
| Cost each | ~560 B parked | ~2–4 KB | ~2 KB | ~1 MB stack |
| Communication | typed channels | typed channels | mailboxes | shared memory |
| Shared mutable state | none | yes, with locks | none | yes, with locks |
| Parallel by default | no (opt in) | yes | yes | yes |
| Failure isolation | none | none | supervision trees | none |

Rill is closest to Go, by design. It differs in defaulting to one thread — a
consequence of the memory model — and it lacks Erlang's supervision, which is
the feature that makes that language special and is a much larger design than a
scheduler.

---

## 16.9 Exercises

1. Turn §16.2's pipeline into three stages and make the end marker an `Option`
   over the channel instead of a negative number.
2. Write a worker pool that shuts down cleanly: a poison value on the jobs
   channel, and each worker acknowledging before it stops.
3. Use `select` with a `default` to write `try_send` that gives up rather than
   blocking when a buffered channel is full.
4. Build the ring from §16.4 with 1,000 strands and pass a token round it ten
   times. Where does the time go — creating strands, or passing the token?
5. Take a program that spawns strands doing independent arithmetic, build it
   with and without `--parallel`, and measure with `RILL_THREADS` from 1 to 8.
   Then do the same with §16.4's chain and explain the difference.

---

That is the runtime. `LANGUAGE.md` covers what remains — calling C, sockets,
and the toolchain — and `examples/` has the programs that use them.

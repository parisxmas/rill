# Disk and network against Go

Seven programs, each written once in Rill and once in Go, that do nothing but
move bytes: a 32 MB file written and read back, five thousand small files,
`wc` over standard input, two million lines to standard output, an echo round
trip a hundred thousand times, five hundred connections at once, and 64 MB
down one socket. Server and client live in one process on the loopback, so
what is timed is the runtime's sockets and scheduler against Go's, not a
network. Go is timed on one processor (`GOMAXPROCS=1`, the shape Rill runs in
by default) and as it ships. Apple M4, Go 1.23.4; time is the median of 15
runs after 2 warmups (7 after 1 for the socket programs, with a breath between
runs so the client does not run out of ports); outputs cross-checked; peak RSS
the median of 3. Reproduce with `python3 benchmarks/io/run.py` after
`benchmarks/tokens/gen_inputs.py` has made `wc`'s 31 MB input.

| benchmark | what it does | rill (ms) | go, 1 proc | go, all procs | |
|---|---|---:|---:|---:|---|
| bigfile | build 32 MB of lines, `write_file`, `read_file`, count the lines | 54.6 | 57.0 | 57.2 | rill 1.04× |
| smallfiles | 5000 files written, then read back | 232.2 | 246.8 | 244.1 | rill 1.05× |
| wc | lines and bytes of a 31 MB standard input, line by line | 20.6 | 26.5 | 26.9 | **rill 1.29×** |
| emit | two million `println`s | 17.3 | 36.5 | 36.0 | **rill 2.07×** |
| echo | 100k round trips of 64 bytes, client and server strands | 1366 | 1312 | 1320 | go 1.04× |
| conns | 500 clients at once, 50 round trips each | 172.5 | 190.7 | 171.6 | even |
| bulk | 64 MB down one socket in 64 KB writes | 33.7 | 33.0 | 30.5 | go 1.02× (1.08× on all cores) |

| peak RSS | rill | go, 1 proc | go, all procs |
|---|---:|---:|---:|
| bigfile | 140 MB | 113 MB | 103 MB |
| conns | 1.9 MB | 19.6 MB | 21.9 MB |
| the other five | 1.3–1.8 MB | 3.9–8.0 MB | 4.2–8.8 MB |

## What the morning looked like, and what changed

The first run of this suite lost four of the seven, one of them by 15×:

| | before | after | go (best) |
|---|---:|---:|---:|
| emit | 556.0 | 17.3 | 36.0 |
| bigfile | 73.0 | 54.6 | 56.7 |
| wc | 35.8 | 20.6 | 26.1 |
| echo | 1349.5 | 1366 | 1287 |

- **Standard output was unbuffered**: every `println` was two `write` calls,
  four million of them here. It is now buffered the way C's is — a line at a
  time on a terminal, 64 KB at a time otherwise — and flushed before anything
  that waits: the program ending (through `atexit`, so a C callback that
  prints after `main` is not lost either), `exit`, a read of standard input,
  a strand parking on a socket or a timer, and an abort, whose message would
  otherwise have come before the output it followed.
- **`read_line` copied every line twice**, through a scratch block it grew
  and freed per call. The line is nearly always whole in the block read last
  time, so it is now one `memchr` and one copy into the string. Standard
  input is read 256 KB at a time.
- **Short copies went a byte at a time.** A string of up to sixteen bytes is
  now two overlapping word moves. This is not an IO change and it moved the
  C suite too: json 144→124, strbuild 36→31, dijkstra 105→91.
- **`s[i]` in `for i in 0..#s`** is in range by construction, so that loop
  reads bytes without the per-byte test — `bigfile`'s line count — while
  every other string index keeps its `-1` past the end.
- **`sock_read` allocated and freed a 64 KB scratch per call.** One scratch
  per worker now; the string it returns is the only allocation.

## Reading the three that are even

After this suite was first run the poller was changed to keep a socket's
registration across waits instead of arming it for each one (a `kevent` call
fewer per wait). It did not move `echo`, `conns` or `bulk` measurably, which
is itself the finding: at 13 µs a round trip the time is the kernel's
wakeups, not the number of calls asking for them.

`echo`, `conns` and `bulk` are the kernel's: a round trip on the loopback is
two `write`s, two `read`s and two wakeups whichever runtime asks, and the
copying in `bulk` is `memcpy` on both sides. Rill sits within 1–4% of Go on
one processor on all three, and Go on all cores buys itself 8% on `bulk`
alone, where sender and receiver genuinely overlap. What Rill has that Go
does not is the memory column: 500 live connections are 1.9 MB against
20 MB, because a parked strand is the bytes it was using and not an 8 KB
stack.

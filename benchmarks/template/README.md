# A page, rendered

Two templates, compiled to Rill by `tools/tmplgen.rill` before the program is
compiled, measured against the Rust template engines on the same two pages.

```sh
cargo build --release
python3 benchmarks/template/run.py
```

## What is measured

The workloads are the ones the [askama-rs template
benchmark](https://github.com/askama-rs/template-benchmark) uses, and the
templates here are laid out tag for tag with that suite's so that the bytes
come out the same. `big-table.expected` and `teams.expected` are what the Rust
engines print; `run.py` compares the rendered page against them before it
reports a single time, and so does the end-to-end test.

| | what it is |
|---|---|
| `big-table` | a hundred rows of a hundred numbers — 292 KB of table, and thirty thousand appends |
| `teams` | four teams in a list, with escaping and an `if` — 382 bytes, and the cost of *starting* a page rather than filling one |

One buffer is rendered into over and over and emptied between renders, which
is what the Rust benchmark does with its `String`, and the clock is read once
around the whole round rather than once around each page.

## What it comes to

Best of ten rounds on an M-series laptop, nanoseconds a page:

| engine | `big-table` | `teams` |
|---|---:|---:|
| **Rill** | **11 455** | **29** |
| sailfish 0.10 | 11 546 | 32 |
| markup 0.16 | 26 281 | 58 |
| askama 0.15 | 28 720 | 45 |
| maud 0.27 | 33 287 | 68 |
| minijinja 2.24 | 687 000 | 1 232 |

Sailfish is the one to beat: of the engines in that suite it is the fastest on
both pages, and like this one it is a compile-time generator rather than an
interpreter. The `minijinja` row is what interpreting the template instead
costs, and is measured by the suite's own `cargo bench` rather than by
`run.py`.

The two are close enough on `big-table` that the ordering moves between runs;
`teams` is the page where the difference is real, and it is the shape a web
server actually renders.

## Measuring the Rust engines next to it

`run.py` on its own reports Rill. To put the others beside it, clone the suite,
drop the harness in this directory into it, and point at what it builds:

```sh
git clone https://github.com/askama-rs/template-benchmark
cp benchmarks/template/time.rs template-benchmark/src/bin/time.rs
cd template-benchmark && cargo build --release \
    --no-default-features --features sailfish,maud,askama,markup --bin time
python3 benchmarks/template/run.py --rust template-benchmark/target/release/time
```

`time.rs` times what `bench.rill` times, in the same shape, so the two numbers
mean the same thing.

## Where the time goes

A cell of the big table is three appends: a literal, a number, a literal. What
that costs is not the template engine but the language underneath it, and
almost all of the work behind these numbers went there rather than into the
generator:

- an append is written where it stands rather than called for, and a literal
  goes in as whole words because its length is known at compile time;
- the runtime is asked for one thing only, a bigger block, and it is handed
  the block rather than the builder — so the length and the capacity stay in
  registers across a run of appends instead of being read back after each one;
- the digits of a number are written into the builder directly, one or two
  digits at a time out of a table, with the count read off the top bit rather
  than found by dividing;
- the growth path and the out-of-range path are marked cold, so the loop body
  is the appends and nothing else.

The generated Rill is checked in next to each template — `big_table.rill` and
`teams_tmpl.rill` — so that what the generator does can be read without
running it.

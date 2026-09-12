# lib/str.rill against C

One program in each language, the same text (44 MB of a seven-word
vocabulary from one linear congruential sequence, newlines every so often)
and the same operations on it, each timed on its own. The C is what a
careful C programmer writes for the job: `memmem` for the searches, a
`realloc`-doubling buffer for `replace`, two rows for the edit distance.
Apple M4, `cc -O3` (Apple clang 17), Rill via LLVM 18 `-O3`, 2026-09-12.
Reproduce with `benchmarks/strings/run.sh`.

| operation | what it does | rill ms | c ms | rill / c |
|---|---|---:|---:|---:|
| count | non-overlapping occurrences of "shells", 1.43 M of them | 31 | 71 | **0.44** |
| last_index_of | the last "ashes aaa", found from the front | 17 | 53 | **0.32** |
| replace | every "sells" → "buys", 42.9 MB out | 47 | 84 | **0.56** |
| upper | ASCII upper-case of the whole text | 2 | 3 | **0.67** |
| count_byte | newlines | 4 | 5 | **0.80** |
| edit_distance | Levenshtein over two 4000-byte slices | 18 | 19 | 0.95 |
| hash | FNV-1a over the text | 40 | 40 | 1.00 |
| words | 1.33 M words cut out into their own strings | 32 | 33 | 0.97 |

With `RILL_THREADS=8` on a `--parallel` build, `par_count` does the count in
6 ms (5× the single strand, on four performance and six efficiency cores)
and `par_lines` maps 2.86 M lines in 69 ms against 91 for the plain scan;
the plain scan's cost is mostly the allocation of the lines, which
parallelizes, and the stitching of the result list, which does not.

## Where the wins come from

- **The searches** are one runtime function, `find_bytes`, that crosses the
  text sixteen bytes at a time and looks at a position only when the
  needle's first byte *and* its last byte are both in place. On this text
  the first byte of every needle is `s`, which is everywhere; `memchr` to
  each `s` and `memcmp` from there — Rill's earlier search, and what libc's
  `memmem` amounts to here — spent its time in the compares. Two rare
  things together are much rarer than one.
- **`replace`** copies each piece straight from the text into the builder
  (`strbuf_add_sub`, no string in between) and reserves the input's size up
  front (`strbuf_reserve`), so nothing is copied twice.
- **`upper`** fills a byte buffer in a loop LLVM vectorizes and hands the
  buffer over as the string (`str_from_buf` on a buffer nothing names again
  is the block itself: a byte buffer's header is a string's header). Three
  things had to give way first, all in the compiler: the string length
  load is marked invariant so it hoists out of a loop that also stores; the
  range test on a byte is written as one unsigned compare so the vector
  lanes stay eight bits wide; and a `for` loop whose body calls only
  functions that cannot loop no longer carries the strand-preemption check
  on its back edge, which was a volatile load per element.
- **`hash`** and **`edit_distance`** are the same dependency chain in both
  languages — a multiply per byte, a min per cell — and land within noise.

## What the numbers said before

The first run of this table, on the morning of 2026-09-12: count 84 / 71,
upper 77 / 3, replace 107 / 85, last_index_of 61 / 53, `par_lines` on eight
workers 395 ms against 90 on one. `upper` was a builder taking one byte at
a time; `par_lines` was every worker adding to one global allocation
counter, which has since become one counter per worker.

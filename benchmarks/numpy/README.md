# grid against NumPy

`lib/num/grid.rill` is Rill's array library — a shape laid over one flat
`Buf(Float)`, with strides and an offset so that `transpose`, `rows`, `col`
and `slice1` are views; masks a byte wide; a matrix kernel written in
`Float2`; an LU factorisation for solving — and this suite puts it beside
NumPy 2.4 (OpenBLAS 0.3.31) on twelve programs.

Every program does its work twice and times the second. The first read of
freshly written memory costs about twice what later reads do, and a
process's first few milliseconds run on a colder clock; neither is what a
library is being asked about, and NumPy's setup had already paid both before
its clock started. Then it prints its answers and the time of the array work
by its own clock, so two things are compared: the work, and the whole
process. Four columns: Rill on one worker; Rill built `--parallel` and run
with a worker per core (`RILL_THREADS=10`); NumPy pinned to one thread
(`OMP_NUM_THREADS=1`, which is what pins this OpenBLAS); NumPy as installed,
OpenBLAS on every core. The four are run round-robin, one of each per round,
so a drifting clock is shared out evenly. Rill is built `--unchecked`, as the
C suite is: NumPy's kernels check no index either, and both libraries check
shapes once at the door. Apple M4; median of 5 after 1 warm-up; answers
agree to six digits. Reproduce with `python3 benchmarks/numpy/run.py`.

## The array work alone (ms)

Measured 2026-09-25, after the sort, the sums and the median were moved
onto the runtime and `fast fn` (see the end of this page).

| benchmark | what it does | rill, 1 core | rill, 10 cores | numpy, 1 thread | numpy, all threads | |
|---|---|---:|---:|---:|---:|---|
| small | `v = v*0.999 + 0.5` on 16 elements, 200k times | 4.4 | 5.4 | 104.3 | 106.9 | **rill 24×** |
| sort | 5 M sorted, and 1 M put in order by position | 78.4 | 83.0 | 218.5 | 219.0 | **rill 2.8×** |
| reduce | mean, std, max, argmax of 20 M | 12.3 | 8.1 | 28.7 | 28.6 | **rill 2.3×**, 3.5× on all cores |
| where | `where(a > k, a, b)`, a sum and a count, 10 M | 6.2 | 4.6 | 12.1 | 12.2 | **rill 2.0×**, 2.7× on all cores |
| shape | joining, tiling, reversing, rolling, gathering, 10 M | 35.7 | 34.6 | 66.9 | 64.0 | **rill 1.9×** |
| stats | median, two percentiles, and NaN-skipping sums of 5 M | 30.7 | 33.0 | 46.9 | 45.4 | **rill 1.5×** |
| matmul | 800 × 800 · 800 × 800 | 17.6 | 4.3 | 19.1 | 6.0 | rill 1.09×, 1.4× on all cores |
| solve | a 400 × 400 system, its determinant and its inverse | 5.6 | 6.3 | 6.4 | 5.5 | rill 1.1× on one core; level on all |
| axis | 2000 × 2000 transposed, summed along both axes | 1.0 | 1.0 | 1.0 | 1.0 | level |
| dot | dot of two 20 M vectors | 4.3 | 3.1 | 4.4 | 3.0 | level |
| elementwise | `sqrt(a)*(2a+1) + a`, 10 M | 12.2 | 11.5 | 12.6 | 12.7 | level |
| trig | `sin`, `exp` and `log` over 10 M each | 61.9 | 16.8 | 62.4 | 62.3 | level; **3.7× on all cores** |

## The whole process (ms)

| benchmark | rill, 1 core | rill, 10 cores | numpy, 1 thread | numpy, all threads |
|---|---:|---:|---:|---:|
| elementwise | 42 | 43 | 97 | 96 |
| reduce | 43 | 35 | 162 | 161 |
| matmul | 39 | 13 | 93 | 66 |
| axis | 7 | 8 | 65 | 64 |
| small | 11 | 13 | 261 | 271 |
| dot | 42 | 40 | 142 | 140 |
| where | 36 | 33 | 118 | 120 |
| trig | 154 | 63 | 217 | 208 |
| sort | 175 | 189 | 514 | 512 |
| solve | 14 | 15 | 66 | 64 |
| stats | 75 | 83 | 181 | 177 |
| shape | 111 | 111 | 239 | 240 |

Python's start and NumPy's import are about 60 ms; a Rill program is a 40 KB
binary that starts in a millisecond. On the whole process Rill is ahead on
all twelve.

## What the numbers say

**Where there is arithmetic to do, Rill is ahead.** `reduce`'s sums are
`fast fn`s, which the compiler splits into running sums in the vector lanes,
and read their twenty million numbers at the memory's pace; NumPy's `std` makes a temporary the size of the input on
the way to its answer. `where` writes and reads a mask one byte wide, as
NumPy does, and its `where` is an in-place kernel on the temporary it was
handed. `matmul` is a 4 × 10 register-blocked kernel written in `Float2`
over a packed panel of ten columns — what a BLAS does — and matches
OpenBLAS's single-threaded kernel on one core and beats its threaded one on
ten. `small` is the cost of an operation that is small and many: a grid
operation in Rill is a function call and an allocation from a pool, and in
NumPy a trip through the interpreter, argument parsing, dtype dispatch and
`malloc`.

**`sort` is ahead because it does not compare.** A comparison sort spends
its time on branches it cannot predict; sorting by the bits does not
branch at all. A float's order is its bit pattern's order once the sign is
folded in, so above two hundred thousand elements `sort` puts the numbers
in place a byte at a time, eight counting passes, one histogram pass for all
eight. The price is two buffers the size of the input, which is why below
that it is the runtime's pattern-defeating quicksort (`buf_sort_range`),
measured faster up to there. `argsort` sorts each number's bits beside its
position in the runtime — a byte at a time for a big grid, stable by
construction — in half the time of the merge sort over positions it was.

**`stats` selects, it does not sort.** A median is the runtime's
introselect in a copy, and the value above it the least of what the
selection left after it — a scan, where it was a second selection.

**Where both sides stream memory, they are level, and that is the wall.** An
`elementwise` pass over eighty megabytes, an `axis` sum, a `dot` of two
vectors: a C loop, NumPy's kernel and Rill's `for` all move the same bytes at
the same speed — about 75 GB/s on one core of this machine, about 110 on all
of them — and there is nothing left to be faster at. `trig` is the same
story with the maths library in place of the memory: both sides call the
same `sin` once per element, so the two are level on one core, and Rill
pulls ahead on ten only because NumPy does not thread its ufuncs.

**`solve` is level on one core, and getting there took four things.** The
factorisation is blocked sixteen columns at a time and its trailing update
goes through the same `Float2` kernel as `matmul`, which runs at 51 GFLOP/s
here — the kernel is no longer what is slow. The block of columns being
factored is copied out transposed first, because a column of a row-major
matrix is one number per cache line and four hundred lines to read four
hundred numbers; transposed it is a run of neighbours, and the pivot
search, the scaling and the rank-one update all read it at the speed of
memory. `inv` does not solve against the identity but exploits its shape:
the forward pass turns it into a lower triangle, so a block of rows only
touches the columns it can have reached, which is a third of the work
rather than all of it, and the answer's columns are put back in order at
the end. And what remains of the factorisation — a quarter of it — is
divided between the workers in pieces sized by the arithmetic in them
rather than by how many workers there are, because starting ten strands
costs thirty-five microseconds and four costs five.

Where it still loses is on all cores: OpenBLAS threads its factorisation
throughout and reaches 5.3 ms where this reaches 6.2. Splitting an even
share to each worker is the wrong move on a machine whose cores are not
alike — four fast and six slow here — and the even share waits for the
slow ones.

**`small` on ten cores is slower than on one**, and that is the honest cost
of `--parallel`: every reference count becomes an atomic operation, and two
hundred thousand tiny operations are two hundred thousand of them. Nothing
here is split across strands below a few million elements — a strand's start
and the wait for it are tens of microseconds, and an operation that takes
less than a millisecond is not worth it.

## What was built to get here

The first run of this suite was 5–300× behind, and nearly every step of the
way was a compiler improvement rather than a library trick — the list is in
`CHANGELOG.md`. The ones that mattered most:

- a record's field indexed in a loop (`g.data[k]`) was retained and released
  on every element — now borrowed from the record the frame holds;
- flat values now cross a call as their fields, one parameter each: a loop
  carrying a tuple of sums is a function tail-calling itself with one, and
  LLVM's aggregate `phi` for it hid the sums from the vectorizer;
- tuples hold up to eight values, so a reduction carried eight partial sums —
  since replaced by `fast fn`, which lets the compiler do the same;
- `unique(x)` and last-use moves, which is how a temporary's storage is
  reused (`elementwise` 29 → 12 ms);
- `Float2`, so a kernel can say its vectors instead of hoping the vectorizer
  pairs its scalars (`matmul` 64 → 17 ms, `solve` 43 → 6.3 ms);
- `sqrt` from libm is the instruction, not the call — C's `sqrt` may set
  `errno`, which is why a loop of them never vectorized;
- a `for` loop whose body calls nothing is not preemption-checked: once the
  library spawned strands, every loop had a volatile load and a branch per
  element;
- a cache of the last few big blocks freed, and `buf_float_uninit` for a
  result about to be written in full, so eighty-megabyte results are neither
  page-faulted in afresh nor cleared before being overwritten.

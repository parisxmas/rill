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

| benchmark | what it does | rill, 1 core | rill, 10 cores | numpy, 1 thread | numpy, all threads | |
|---|---|---:|---:|---:|---:|---|
| small | `v = v*0.999 + 0.5` on 16 elements, 200k times | 9.3 | 15.0 | 102.7 | 102.4 | **rill 11×** |
| sort | 5 M sorted, and 1 M put in order by position | 89.4 | 98.2 | 209.9 | 207.5 | **rill 2.3×** |
| reduce | mean, std, max, argmax of 20 M | 14.1 | 7.7 | 27.2 | 27.3 | **rill 1.9×**, 3.5× on all cores |
| shape | joining, tiling, reversing, rolling, gathering, 10 M | 40.2 | 44.3 | 75.8 | 64.1 | **rill 1.9×** |
| where | `where(a > k, a, b)`, a sum and a count, 10 M | 6.5 | 4.7 | 11.8 | 12.0 | **rill 1.8×**, 2.6× on all cores |
| stats | median, two percentiles, and NaN-skipping sums of 5 M | 40.4 | 43.1 | 44.4 | 44.6 | rill 1.1× |
| matmul | 800 × 800 · 800 × 800 | 17.3 | 4.5 | 18.5 | 5.7 | rill 1.07×, 1.3× on all cores |
| axis | 2000 × 2000 transposed, summed along both axes | 0.9 | 1.0 | 1.0 | 1.0 | level |
| dot | dot of two 20 M vectors | 4.3 | 3.0 | 4.4 | 2.9 | level |
| elementwise | `sqrt(a)*(2a+1) + a`, 10 M | 12.0 | 11.4 | 12.1 | 12.1 | level |
| trig | `sin`, `exp` and `log` over 10 M each | 61.7 | 18.2 | 62.2 | 62.1 | level; **3.4× on all cores** |
| solve | a 400 × 400 system, its determinant and its inverse | 6.3 | 6.2 | 6.4 | 5.3 | level on one core; numpy 1.2× on all |

## The whole process (ms)

| benchmark | rill, 1 core | rill, 10 cores | numpy, 1 thread | numpy, all threads |
|---|---:|---:|---:|---:|
| elementwise | 42 | 49 | 91 | 89 |
| reduce | 48 | 36 | 152 | 152 |
| matmul | 38 | 13 | 89 | 64 |
| axis | 7 | 8 | 63 | 62 |
| small | 21 | 32 | 256 | 255 |
| dot | 60 | 58 | 144 | 137 |
| where | 44 | 42 | 114 | 116 |
| trig | 155 | 65 | 209 | 207 |
| sort | 203 | 224 | 494 | 490 |
| solve | 15 | 16 | 64 | 62 |
| stats | 101 | 110 | 177 | 174 |
| shape | 120 | 120 | 259 | 244 |

Python's start and NumPy's import are about 60 ms; a Rill program is a 40 KB
binary that starts in a millisecond. On the whole process Rill is ahead on
all twelve.

## What the numbers say

**Where there is arithmetic to do, Rill is ahead.** `reduce` runs eight
partial sums in four vector lanes and reads its twenty million numbers at
the memory's pace; NumPy's `std` makes a temporary the size of the input on
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
folded in, so above a few thousand elements `sort` puts the numbers in
place a byte at a time, eight counting passes, one histogram pass for all
eight. The price is two buffers the size of the input, which is why below
that it is still a quicksort. `argsort` is a stable merge sort over the
positions and is 2.4× ahead of NumPy's stable argsort on its own.

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
- tuples hold up to eight values, so a reduction carries eight partial sums;
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

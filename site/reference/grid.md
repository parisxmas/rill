`num/grid` is Rill's array library: arrays of numbers with a shape, the
way NumPy has them, written in Rill on one buffer — and, on one core,
ahead of NumPy on eight of twelve benchmarks and level on the other
four. This page is the whole of it: what a grid is, how one is made,
read, reshaped and combined, the masks a comparison gives, the
reductions, sorting and searching, the linear algebra, what makes it
fast, and how it is reached from Python. Every example here was run when
the page was built, by `rill doctest`.

```
import "num/grid" as g

a = g.arange(6) |> g.reshape2(2, 3)
a                                   # => [[0, 1, 2], [3, 4, 5]]
b = g.transpose(a)                  # a view: the same buffer, read the other way
g.matmul(a, b)                      # => [[5, 14], [14, 50]]
g.sum(a)                            # => 15
g.mean_axis(a, 1)                   # => [1, 4]
big = g.gtk(a, 2.0)                 # a Mask: one byte an element
g.where(big, a, g.zeros_like(a))    # => [[0, 0, 0], [3, 4, 5]]
```

### What a grid is

A `Grid` is one flat `Buf(Float)` and the shape laid over it: `shape`
says how many along each axis, `strides` how far to step in the buffer
for one along each axis, and `off` where element zero sits. One or two
axes; the element type is `Float` and nothing else. That is a smaller
thing than a NumPy array — no dtypes, no broadcasting between different
shapes, no third dimension — and the smallness is where the speed comes
from: every operation is a loop the compiler can lay out and vectorize,
with no dispatch on type or rank inside it.

A grid is storage, as a `Buf` is: `put1` and `put2` write in place, and
a view shares the buffer it was taken from.

```
import "num/grid" as g
v = g.zeros1(3)
g.put1(v, 1, 9.0)
v                    # => [0, 9, 0]
g.ndim(v)            # => 1
g.size(v)            # => 3
g.shape2(2, 3)[1]    # => 3
```

### Making one

`zeros`, `ones`, `full` and their `1`/`2` forms take a shape or its
dimensions; `arange`, `linspace`, `logspace` and `geomspace` make a
sequence; `of` takes a list; `from_fn1` and `from_fn2` call a function
for each element; `identity` is the unit matrix; the `_like` family
takes the shape of a grid you have.

```
import "num/grid" as g
g.zeros2(2, 2)                                 # => [[0, 0], [0, 0]]
g.full2(1, 3, 7.5)                             # => [[7.5, 7.5, 7.5]]
g.arange(5)                                    # => [0, 1, 2, 3, 4]
g.linspace(0.0, 1.0, 5)                        # => [0, 0.25, 0.5, 0.75, 1]
g.geomspace(1.0, 8.0, 4)                       # => [1, 2, 4, 8]
g.of(Cons(1.5, Cons(2.5, Nil)))                # => [1.5, 2.5]
g.from_fn2(2, 2, \i, j -> to_float(i * 2 + j)) # => [[0, 1], [2, 3]]
g.identity(2)                                  # => [[1, 0], [0, 1]]
g.ones_like(g.arange(3))                       # => [1, 1, 1]
```

### Reading, views and copies

`at1` and `at2` read one element. `transpose`, `rows`, `row`, `col` and
`slice1` are views — no copying, the same storage — and `copy` or
`dense` lays a view out contiguously, which is the shape every fast path
recognises. `reshape`, `flatten`, `ravel` and `squeeze` change the shape
over the same elements.

```
import "num/grid" as g
a = g.arange(6) |> g.reshape2(2, 3)
g.at2(a, 1, 2)                 # => 5
g.row(a, 1)                    # => [3, 4, 5]
g.col(a, 2)                    # => [2, 5]
t = g.transpose(a)
g.contiguous(t)                # => false
g.copy(t)                      # => [[0, 3], [1, 4], [2, 5]]
g.contiguous(g.copy(t))        # => true
g.flatten(a)                   # => [0, 1, 2, 3, 4, 5]
g.ravel(t)                     # => [0, 3, 1, 4, 2, 5]
g.squeeze(g.ones2(1, 3))       # => [1, 1, 1]
g.rows(a, 1, 2)                # => [[3, 4, 5]]
g.slice1(g.arange(5), 2, 4)    # => [2, 3]
```

### Joining and rearranging

`concat` along an axis, `vstack`, `hstack` and `stack`; `tile` and
`repeat`; `diag` both ways, `tril` and `triu`; `fliplr`, `flipud`,
`flip` and `roll`; `take` by positions; `meshgrid` for every pair.

```
import "num/grid" as g
g.concat(g.ones2(1, 2), g.zeros2(1, 2), 0)   # => [[1, 1], [0, 0]]
g.hstack(g.ones2(2, 1), g.zeros2(2, 1))      # => [[1, 0], [1, 0]]
g.stack(g.arange(2), g.ones1(2))             # => [[0, 1], [1, 1]]
g.tile(g.arange(2), 2)                       # => [0, 1, 0, 1]
g.repeat(g.arange(2), 2)                     # => [0, 0, 1, 1]
g.diag(g.arange(2))                          # => [[0, 0], [0, 1]]
g.triu(g.ones2(2, 2), 0)                     # => [[1, 1], [0, 1]]
g.roll(g.arange(4), 1)                       # => [3, 0, 1, 2]
g.flip(g.arange(4) |> g.reshape2(2, 2))      # => [[3, 2], [1, 0]]
g.take(g.of(Cons(10.0, Cons(20.0, Cons(30.0, Nil)))), g.of(Cons(2.0, Cons(0.0, Nil))))   # => [30, 10]
(xs, ys) = g.meshgrid(g.arange(2), g.arange(2))
xs                                           # => [[0, 1], [0, 1]]
ys                                           # => [[0, 0], [1, 1]]
```

### Arithmetic and the maths

`add`, `sub`, `mul`, `div`, `maximum`, `minimum` take two grids of one
shape; `scale`, `shift` and `power` take a grid and a number; then the
elementwise functions, each named with a `g` in front so they do not
shadow the prelude's — `gsqrt`, `gexp`, `glog`, the trigonometry, the
roundings, `gclip`, `gsign` and the rest — and `gatan2`, `ghypot`,
`gfmod`, `gpowg` over pairs. `map` and `zip` take a lambda for anything
else; they cost a call an element, which the named ones do not.

```
import "num/grid" as g
g.add(g.arange(3), g.ones1(3))                     # => [1, 2, 3]
g.mul(g.arange(3), g.arange(3))                    # => [0, 1, 4]
g.scale(g.arange(3), 2.0)                          # => [0, 2, 4]
g.shift(g.arange(3), 10.0)                         # => [10, 11, 12]
g.power(g.arange(4), 2.0)                          # => [0, 1, 4, 9]
g.gsqrt(g.of(Cons(4.0, Cons(9.0, Nil))))           # => [2, 3]
g.gexp2(g.arange(4))                               # => [1, 2, 4, 8]
g.ground(g.of(Cons(2.5, Cons(3.5, Nil))))          # => [2, 4]
g.gclip(g.arange(5), 1.0, 3.0)                     # => [1, 1, 2, 3, 3]
g.ghypot(g.of(Cons(3.0, Nil)), g.of(Cons(4.0, Nil)))   # => [5]
g.map(g.arange(3), \x -> x * x + 1.0)              # => [1, 2, 5]
g.zip(g.arange(3), g.ones1(3), \x, y -> x - y)     # => [-1, 0, 1]
```

An operation writes its result into an operand's own storage when that
operand is a temporary nobody else can see, so `a |> g.scale(2.0) |>
g.shift(1.0)` makes one grid and not two — the same trick NumPy cannot
do, since Python holds every temporary until the statement ends.

### Masks and `where`

A comparison gives a `Mask` — one byte an element, as NumPy's boolean
arrays are — with `gt`, `lt`, `ge`, `le`, `eq`, `ne` between two grids
and the `k` forms against one number. `where` picks from two grids by a
mask; `mcount`, `any` and `all` read one; `mask_not`, `mask_and` and
`mask_or` combine them; `to_grid` turns one into ones and zeros;
`isnan`, `isinf`, `isfinite` and `isclose` make them.

```
import "num/grid" as g
a = g.arange(5)
m = g.gtk(a, 2.0)
m                                        # => [false, false, false, true, true]
g.mcount(m)                              # => 2
g.where(m, a, g.zeros1(5))               # => [0, 0, 0, 3, 4]
g.mask_and(m, g.ltk(a, 4.0))             # => [false, false, false, true, false]
g.to_grid(g.mask_not(m))                 # => [1, 1, 1, 0, 0]
g.isnan(g.of(Cons(1.0, Cons(g.nan(), Nil))))   # => [false, true]
g.allclose(g.arange(3), g.arange(3), 0.0)      # => true
```

### Reductions and order statistics

`sum`, `mean`, `prod`, `dot`, `norm`, `gmax`, `gmin`, `argmax`,
`argmin`, `var`, `std`, `ptp`, `average`; `cumsum`, `cumprod`, `diff`;
`sum_axis` and `mean_axis` along an axis of a 2-D grid; the `nan`
family that leaves NaNs out; `median`, `percentile` and `quantile`,
which interpolate as NumPy does.

```
import "num/grid" as g
a = g.arange(6) |> g.reshape2(2, 3)
g.sum(a)                          # => 15
g.mean(a)                         # => 2.5
g.sum_axis(a, 0)                  # => [3, 5, 7]
g.sum_axis(a, 1)                  # => [3, 12]
g.gmax(a)                         # => 5
g.argmax(g.of(Cons(2.0, Cons(9.0, Cons(9.0, Nil)))))   # => 1
g.var(g.of(Cons(2.0, Cons(4.0, Cons(4.0, Cons(6.0, Nil))))))   # => 2
g.cumsum(g.arange(4))             # => [0, 1, 3, 6]
g.diff(g.of(Cons(1.0, Cons(4.0, Cons(9.0, Nil)))))     # => [3, 5]
g.median(g.of(Cons(4.0, Cons(1.0, Cons(2.0, Cons(3.0, Nil))))))   # => 2.5
g.percentile(g.arange(5), 25.0)   # => 1
g.nansum(g.of(Cons(1.0, Cons(g.nan(), Cons(2.0, Nil)))))   # => 3
```

The reductions run eight partial sums in four vector lanes. Floating
point addition is not associative, so a compiler may not do that on its
own — Rill keeps `+` in the order it was written, as C does — and doing
it by hand is what lets a sum stream at memory speed rather than at the
latency of one add waiting on the last. The answers agree with a plain
loop's to the last few bits, as NumPy's pairwise sums do.

### Sorting and searching

`sort` is a quicksort for small grids and a radix sort — by the bits,
eight passes, nothing to mispredict — above a few thousand elements;
`argsort` is a stable merge sort over the positions; `distinct`,
`searchsorted`, `nonzero` and `count_nonzero`.

```
import "num/grid" as g
g.sort(g.of(Cons(3.0, Cons(1.0, Cons(2.0, Nil)))))       # => [1, 2, 3]
g.argsort(g.of(Cons(3.0, Cons(1.0, Cons(2.0, Nil)))))    # => [1, 2, 0]
g.distinct(g.of(Cons(3.0, Cons(1.0, Cons(3.0, Nil)))))   # => [1, 3]
g.searchsorted(g.of(Cons(1.0, Cons(3.0, Cons(5.0, Nil)))), 4.0)   # => 2
g.nonzero(g.of(Cons(0.0, Cons(2.0, Cons(0.0, Cons(5.0, Nil))))))  # => [1, 3]
```

### Linear algebra

`matmul` is a register-blocked kernel written in `Float2`, the way a
BLAS does it: ten columns of the right operand packed into a panel that
stays in cache, four rows of the left walked down it with the 4 × 10
block of answers in twenty vector registers. `matvec`, `outer`,
`inner`, `vdot`, `kron`, `cross`, `trace`. `lu` is a blocked
factorisation with partial pivoting, and on it `solve` (a vector or a
matrix of right-hand sides), `inv`, `det`, `matrix_power`, `lstsq`
through the normal equations, `cond` and the three norms. The `try_`
forms answer a grid of no elements for a singular matrix instead of
stopping the program, which is what a host that must answer for itself
calls.

```
import "num/grid" as g
a = g.arange(6) |> g.reshape2(2, 3)
g.matmul(a, g.transpose(a))                     # => [[5, 14], [14, 50]]
g.matvec(a, g.ones1(3))                         # => [3, 12]
m = g.of(Cons(2.0, Cons(1.0, Cons(1.0, Cons(3.0, Nil))))) |> g.reshape2(2, 2)
g.solve(m, g.of(Cons(3.0, Cons(5.0, Nil))))     # => [0.8, 1.4]
g.det(m)                                        # => 5
g.inv(g.of(Cons(4.0, Cons(7.0, Cons(2.0, Cons(6.0, Nil))))) |> g.reshape2(2, 2))   # => [[0.6, -0.7], [-0.2, 0.4]]
g.matrix_power(g.of(Cons(1.0, Cons(1.0, Cons(1.0, Cons(0.0, Nil))))) |> g.reshape2(2, 2), 10)   # => [[89, 55], [55, 34]]
g.size(g.try_inv(g.ones2(2, 2)))                # => 0
g.norm_fro(g.of(Cons(3.0, Cons(0.0, Cons(0.0, Cons(4.0, Nil))))) |> g.reshape2(2, 2))   # => 5
g.cross(g.of(Cons(1.0, Cons(0.0, Cons(0.0, Nil)))), g.of(Cons(0.0, Cons(1.0, Cons(0.0, Nil)))))   # => [0, 0, 1]
```

### Several cores

On a `--parallel` build with `RILL_THREADS` set, anything over a
quarter of a million elements is split across the workers in strands —
a reduction combines the workers' partial answers in worker order, so
the answer is the same every run — and a matrix product hands column
blocks out. A single-worker run never spawns, and work worth less than
a few hundred microseconds is not split at all, since waking a thread
costs about that.

### What it measures against

`benchmarks/numpy/` puts the library beside NumPy 2.4 with OpenBLAS on
twelve programs, one core against one thread and all cores against all
threads, on an Apple M4. On one core Rill is ahead on eight of the
twelve and level on four: two hundred thousand small operations 11×,
sorting 2.3×, reductions and rearranging 1.9×, a mask and a `where`
1.8×, order statistics and an 800 × 800 matrix product 1.1×. Where both
sides do nothing but stream memory — an elementwise expression, sums
along an axis, a dot product — or call the same maths library, or solve
a linear system, they are level: that is the memory bus, and C streams
it identically. On all cores NumPy keeps two, its threaded matrix
product and its threaded factorisation. On the whole process — Python's
start and NumPy's import are 60 ms; a Rill program is a 50 KB binary
that starts in a millisecond — Rill is 2–12× ahead on all twelve.

| benchmark | what it does | rill, 1 core | numpy, 1 thread | |
|---|---|---:|---:|---|
| small | `v = v*0.999 + 0.5` on 16 elements, 200k times | 9.3 ms | 102.7 ms | rill 11× |
| sort | 5 M sorted, 1 M ordered by position | 89.4 | 209.9 | rill 2.3× |
| reduce | mean, std, max, argmax of 20 M | 14.1 | 27.2 | rill 1.9× |
| shape | joining, tiling, reversing, rolling, gathering, 10 M | 40.2 | 75.8 | rill 1.9× |
| where | `where(a > k, a, b)`, a sum and a count, 10 M | 6.5 | 11.8 | rill 1.8× |
| stats | median, two percentiles, NaN-skipping sums of 5 M | 40.4 | 44.4 | rill 1.1× |
| matmul | 800 × 800 · 800 × 800 | 17.3 | 18.5 | rill 1.07× |
| axis | 2000 × 2000 transposed, summed along both axes | 0.9 | 1.0 | level |
| dot | two 20 M vectors | 4.3 | 4.4 | level |
| elementwise | `sqrt(a)*(2a+1) + a`, 10 M | 12.0 | 12.1 | level |
| trig | `sin`, `exp`, `log` over 10 M each | 61.7 | 62.2 | level |
| solve | a 400 × 400 system, its determinant and inverse | 6.3 | 6.4 | level |

The numbers, the four columns and what each step of the compiler work
bought are in `benchmarks/numpy/README.md`.

### From Python

`bindings/python` builds the library into a shared object with
`rill build --shared` and gives its handles NumPy's notation, so a
Python program computes nothing in Python:

```
# not run
import grid as g

a = g.arange(6).reshape(2, 3)
b = a * 2.0 + 1.0                 # one grid made, not two
print(a @ b.T)                    # grid([[8, 26], [26, 98]])
print(b.mean(axis=0), a[a > 2])   # grid([5, 7, 9]) grid([3, 4, 5])
```

Operators, indexing, `.T`, `.sum(axis=...)`, masks as `a[a > 2]`,
`g.linalg.solve` and the rest, with shape checks that raise
`ValueError` before the library is called; `np.asarray(grid)` and
`g.array(ndarray)` cross over. `bindings/python/README.md` has the
table of what there is.

### What it does not have

Dtypes other than `Float`; more than two dimensions; broadcasting
between different shapes — an operation takes two grids of one shape,
or a grid and a number; and the decompositions beyond LU, so a badly
conditioned least-squares problem wants a QR this library does not
have yet. Each is a deliberate absence: the library is small enough to
read, and what it has runs at the speed of the memory it touches.

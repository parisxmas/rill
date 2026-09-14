# grid for Python

Rill's array library, `lib/num/grid.rill`, used from Python the way NumPy is:

```python
import grid as g

a = g.arange(6).reshape(2, 3)
b = a * 2.0 + 1.0                 # one grid made, not two
print(a @ b.T)                    # grid([[8, 26], [26, 98]])
print(b.mean(axis=0), a[a > 2])   # grid([5, 7, 9]) grid([3, 4, 5])
```

Nothing is computed in Python. `gridlib.rill` is a set of `export fn`s over
the library, built by `rill build --shared` into `grid/_gridlib.dylib`
(`.so` on Linux), and the package is a thin `ctypes` layer that gives the
handles operators, indexing, `.T`, `.sum(axis=...)` and shape checks that
raise `ValueError` before the library is called.

```sh
cargo build --release                  # the compiler, once
python3 bindings/python/build.py       # the library
python3 bindings/python/test_grid.py   # against NumPy if it is installed
```

## What there is

| | |
|---|---|
| making | `zeros`, `ones`, `full`, `arange`, `linspace`, `logspace`, `geomspace`, `identity`/`eye`, `array(list \| nested list \| numpy array)`, `zeros_like`, `ones_like`, `full_like`, `empty_like`, `meshgrid` |
| shape | `.shape`, `.ndim`, `.size`, `.reshape`, `.T`, `.flatten`/`ravel`, `.copy`, `squeeze`; `a[i]`, `a[i, j]`, `a[lo:hi]`, `a[i, :]`, `a[:, j]`, `a[mask]`; `a[i] = v` |
| joining | `concatenate`, `vstack`, `hstack`, `stack`, `tile`, `repeat`, `diag`, `diagonal`, `tril`, `triu`, `fliplr`, `flipud`, `flip`, `roll`, `take` |
| arithmetic | `+ - * /` with a grid or a number, `**`, unary `-`, `@`, `abs`, `power`, `fmod`/`mod`, `maximum`, `minimum`, `clip` |
| maths | `sqrt`, `exp`, `log`, `log2`, `log10`, `log1p`, `exp2`, `expm1`, `cbrt`, `square`, `reciprocal`, `sign`, `sin`, `cos`, `tan`, `arcsin`, `arccos`, `arctan`, `arctan2`, `sinh`, `cosh`, `tanh`, `hypot`, `copysign`, `floor`, `ceil`, `trunc`, `round`/`rint`, `degrees`, `radians`, `pi`, `e` |
| comparisons | `> < >= <= == !=` with a grid or a number, giving a `Mask`; `~ & \|` on masks; `.sum()`, `.any()`, `.all()`, `.astype(float)`; `where(mask, a, b)`, `isnan`, `isinf`, `isfinite`, `isclose`, `allclose` |
| reductions | `sum`, `mean`, `prod`, `max`, `min`, `argmax`, `argmin`, `var`, `std`, `median`, `percentile`, `quantile`, `ptp`, `average`, `norm`, `dot`, `cumsum`, `cumprod`, `diff`, `trace`, `count_nonzero`; `sum(axis=)`, `mean(axis=)`; `nansum`, `nanmean`, `nanmax`, `nanmin`, `nanargmax`, `nanargmin` |
| order | `sort`, `argsort` (stable), `unique`, `searchsorted`, `nonzero` |
| linear algebra | `matmul`/`@`, matrix by vector, `outer`, `inner`, `vdot`, `kron`, `cross`, and under `g.linalg` as well: `solve`, `inv`, `det`, `norm(ord=)`, `cond`, `matrix_power`, `lstsq` |
| interop | `np.asarray(grid)`, `g.array(ndarray)`, `.tolist()` |

Grids are one or two dimensional and hold floats; masks hold a byte per
element. `print(a)` shows the grid. A singular matrix raises
`g.LinAlgError`, as NumPy's `linalg` raises its own.

What is not here: dtypes other than float (no integer or complex arrays),
more than two dimensions, broadcasting between different shapes, indexing by
a list of positions, `einsum`, the decompositions (`eig`, `svd`, `qr`,
`cholesky`), random numbers, and reading or writing files. `grid` is the
part of NumPy that is arithmetic over dense arrays, not the whole of it.

## Temporaries

`a * 2.0 + 1.0` makes one grid: the `+ 1.0` finds its operand held by
nothing but the expression — the interpreter's reference count says so,
which is how NumPy elides its temporaries too — and adds in place through
the library's `_into` entry points. A grid with a name is never written
through.

## Handles

A `Grid` holds a handle the Rill runtime owns and drops it when the Python
object goes; `grid_data` hands out the elements' address for `tolist` and
NumPy, valid as long as the grid is. The library runs on one worker: a
`--parallel` build with `RILL_THREADS` would spread big operations over
strands, but strands need the scheduler that a program's `main` runs and a
library's host does not, so the library stays single-threaded.

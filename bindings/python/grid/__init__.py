"""grid — Rill's array library, from Python, the way NumPy is used.

    import grid as g
    a = g.arange(6).reshape(2, 3)
    b = a * 2.0 + 1.0
    print((a @ b.T).sum(), b.mean(axis=0), a[a > 2])

A `Grid` is a handle to a grid the Rill runtime owns; every operation is a
call into `_gridlib` (built by `build.py` from `gridlib.rill`). Nothing is
computed in Python: the loops, the vector kernels and the strands are
Rill's. What Python adds is the notation — operators, indexing, `.T`,
`.sum(axis=...)` — and the shape checks that turn a mistake into a
`ValueError` before the library sees it.

Temporaries are elided as NumPy elides them: `a * 2.0 + 1.0` makes one
grid, not two, because the `+ 1.0` finds its operand held by nothing but
the expression and adds in place.
"""
import atexit
import builtins
import ctypes
import os
import sys

__all__ = [
    "Grid", "Mask", "zeros", "ones", "full", "arange", "linspace", "identity", "eye",
    "array", "where", "sqrt", "exp", "log", "abs", "maximum", "minimum", "dot",
    "matmul", "outer", "sum", "mean", "prod", "max", "min", "argmax", "argmin",
    "var", "std", "norm", "cumsum", "trace", "transpose", "allclose",
]


def _load():
    here = os.path.dirname(os.path.abspath(__file__))
    for name in ("_gridlib.dylib", "_gridlib.so"):
        path = os.path.join(here, name)
        if os.path.exists(path):
            return ctypes.CDLL(path)
    raise ImportError(
        "grid: the library is not built — run `python3 build.py` in bindings/python "
        "(it needs `rill` on PATH or RILL set, and RILL_PATH pointing at funclang/lib)"
    )


_L = _load()
_L.rill_init()
_L.rill_flush_out.restype = None
atexit.register(_L.rill_flush_out)

_P = ctypes.c_void_p
_F = ctypes.c_double
_I = ctypes.c_int64
_B = ctypes.c_bool


def _sig(name, res, *args):
    f = getattr(_L, name)
    f.restype = res
    f.argtypes = list(args)
    return f


_drop_grid = _sig("rill_drop_g_Grid", None, _P)
_drop_mask = _sig("rill_drop_g_Mask", None, _P)
_drop_fbuf = _sig("rill_drop_Buf_Float", None, _P)
_drop_bbuf = _sig("rill_drop_Buf_U8", None, _P)
_free = _sig("rill_free_ptr", None, _P)

_zeros1 = _sig("grid_zeros1", _P, _I)
_zeros2 = _sig("grid_zeros2", _P, _I, _I)
_full1 = _sig("grid_full1", _P, _I, _F)
_full2 = _sig("grid_full2", _P, _I, _I, _F)
_arange = _sig("grid_arange", _P, _I)
_linspace = _sig("grid_linspace", _P, _F, _F, _I)
_identity = _sig("grid_identity", _P, _I)
_ndim = _sig("grid_ndim", _I, _P)
_dim = _sig("grid_dim", _I, _P, _I)
_size = _sig("grid_size", _I, _P)
_contiguous = _sig("grid_contiguous", _B, _P)
_data = _sig("grid_data", ctypes.POINTER(_F), _P)
_reshape1 = _sig("grid_reshape1", _P, _P, _I)
_reshape2 = _sig("grid_reshape2", _P, _P, _I, _I)
_transpose = _sig("grid_transpose", _P, _P)
_rows = _sig("grid_rows", _P, _P, _I, _I)
_row = _sig("grid_row", _P, _P, _I)
_col = _sig("grid_col", _P, _P, _I)
_copy = _sig("grid_copy", _P, _P)
_at1 = _sig("grid_at1", _F, _P, _I)
_at2 = _sig("grid_at2", _F, _P, _I, _I)
_put1 = _sig("grid_put1", _I, _P, _I, _F)
_put2 = _sig("grid_put2", _I, _P, _I, _I, _F)
_bin = {op: _sig("grid_" + op, _P, _P, _P) for op in ("add", "sub", "mul", "div", "maximum", "minimum")}
_bin_into = {op: _sig("grid_" + op + "_into", _I, _P, _P) for op in ("add", "sub", "mul", "div")}
_scale = _sig("grid_scale", _P, _P, _F)
_shift = _sig("grid_shift", _P, _P, _F)
_power = _sig("grid_power", _P, _P, _F)
_scale_into = _sig("grid_scale_into", _I, _P, _F)
_shift_into = _sig("grid_shift_into", _I, _P, _F)
_sqrt_into = _sig("grid_sqrt_into", _I, _P)
_UNARY = (
    "neg abs sqrt exp log cumsum sin cos tan asin acos atan sinh cosh tanh "
    "exp2 expm1 log2 log10 log1p cbrt floor ceil trunc round square reciprocal "
    "sign degrees radians"
).split()
_un = {op: _sig("grid_" + op, _P, _P) for op in _UNARY}
_bin2 = {op: _sig("grid_" + op, _P, _P, _P) for op in ("atan2", "hypot", "fmod", "copysign", "powg")}
_clip = _sig("grid_clip", _P, _P, _F, _F)
_maxk = _sig("grid_maxk", _P, _P, _F)
_mink = _sig("grid_mink", _P, _P, _F)
_fmodk = _sig("grid_fmodk", _P, _P, _F)
_cmp = {op: _sig("grid_" + op, _P, _P, _P) for op in ("gt", "ge", "eq")}
_cmpk = {op: _sig("grid_" + op, _P, _P, _F) for op in ("gtk", "ltk", "eqk")}
_mask_not = _sig("mask_not", _P, _P)
_mask_and = _sig("mask_and", _P, _P, _P)
_mask_or = _sig("mask_or", _P, _P, _P)
_mask_count = _sig("mask_count", _I, _P)
_mask_ndim = _sig("mask_ndim", _I, _P)
_mask_dim = _sig("mask_dim", _I, _P, _I)
_mask_bits = _sig("mask_bits", ctypes.POINTER(ctypes.c_uint8), _P)
_mask_to_grid = _sig("mask_to_grid", _P, _P)
_where = _sig("grid_where", _P, _P, _P, _P)
_red = {op: _sig("grid_" + op, _F, _P) for op in ("sum", "mean", "prod", "max", "min", "var", "std", "norm", "trace")}
_argmax = _sig("grid_argmax", _I, _P)
_argmin = _sig("grid_argmin", _I, _P)
_dot = _sig("grid_dot", _F, _P, _P)
_sum_axis = _sig("grid_sum_axis", _P, _P, _I)
_mean_axis = _sig("grid_mean_axis", _P, _P, _I)
_matmul = _sig("grid_matmul", _P, _P, _P)
_matvec = _sig("grid_matvec", _P, _P, _P)
_outer = _sig("grid_outer", _P, _P, _P)
_show = _sig("grid_show", _P, _P)
_like = {op: _sig("grid_" + op + "_like", _P, _P) for op in ("zeros", "ones", "empty")}
_full_like = _sig("grid_full_like", _P, _P, _F)
_squeeze = _sig("grid_squeeze", _P, _P)
_logspace = _sig("grid_logspace", _P, _F, _F, _I, _F)
_concat = _sig("grid_concat", _P, _P, _P, _I)
_vstack = _sig("grid_vstack", _P, _P, _P)
_hstack = _sig("grid_hstack", _P, _P, _P)
_tile = _sig("grid_tile", _P, _P, _I)
_repeat = _sig("grid_repeat", _P, _P, _I)
_diag = _sig("grid_diag", _P, _P)
_tril = _sig("grid_tril", _P, _P, _I)
_triu = _sig("grid_triu", _P, _P, _I)
_mesh_x = _sig("grid_meshgrid_x", _P, _P, _P)
_mesh_y = _sig("grid_meshgrid_y", _P, _P, _P)
_shapefn = {op: _sig("grid_" + op, _P, _P) for op in ("fliplr", "flipud", "flip", "sort", "argsort", "distinct", "nonzero", "diff", "cumprod")}
_roll = _sig("grid_roll", _P, _P, _I)
_take = _sig("grid_take", _P, _P, _P)
_searchsorted = _sig("grid_searchsorted", _I, _P, _F)
_count_nonzero = _sig("grid_count_nonzero", _I, _P)
_median = _sig("grid_median", _F, _P)
_percentile = _sig("grid_percentile", _F, _P, _F)
_quantile = _sig("grid_quantile", _F, _P, _F)
_ptp = _sig("grid_ptp", _F, _P)
_isfn = {op: _sig("grid_is" + op, _P, _P) for op in ("nan", "inf", "finite")}
_isclose = _sig("grid_isclose", _P, _P, _P, _F)
_nan_red = {op: _sig("grid_nan" + op, _F, _P) for op in ("sum", "mean", "max", "min")}
_nanargmax = _sig("grid_nanargmax", _I, _P)
_nanargmin = _sig("grid_nanargmin", _I, _P)
_solve = _sig("grid_solve", _P, _P, _P)
_inv = _sig("grid_inv", _P, _P)
_det = _sig("grid_det", _F, _P)
_matrix_power = _sig("grid_matrix_power", _P, _P, _I)
_lstsq = _sig("grid_lstsq", _P, _P, _P)
_norm1 = _sig("grid_norm1", _F, _P)
_norm_inf = _sig("grid_norm_inf", _F, _P)
_norm_fro = _sig("grid_norm_fro", _F, _P)
_cond = _sig("grid_cond", _F, _P)
_kron = _sig("grid_kron", _P, _P, _P)
_cross = _sig("grid_cross", _P, _P, _P)
_mask_show = _sig("mask_show", _P, _P)


def _text(p):
    s = ctypes.string_at(p).decode()
    _free(p)
    return s


class _Probe:
    """Measures, on this interpreter, what the reference count of an operand
    held by nothing but the expression looks like from inside a method and
    from inside a function — interpreters differ in how they count the
    stack — so that a temporary can be told from a named object. If they
    cannot be told apart, nothing is ever written through."""

    def __pos__(self):
        return sys.getrefcount(self)


def _probe_fn(x):
    return sys.getrefcount(x)


def _calibrate():
    temp_method = +_Probe()
    named = _Probe()
    named_method = +named
    temp_fn = _probe_fn(_Probe())
    named_fn = _probe_fn(named)
    method = temp_method if temp_method < named_method else -1
    fn = temp_fn if temp_fn < named_fn else -1
    return method, fn


_TEMP_IN_METHOD, _TEMP_IN_FN = _calibrate()


def _is_temp(obj):
    """Whether `obj`, the `self` of the operator method that calls this, is
    held by nothing but the expression being evaluated. NumPy elides its
    temporaries by the same count."""
    return sys.getrefcount(obj) <= _TEMP_IN_METHOD


def _is_temp_arg(obj):
    """The same for the argument of a module function such as `sqrt`."""
    return sys.getrefcount(obj) <= _TEMP_IN_FN


class Grid:
    """A grid of floats with a shape. Made by the module's functions."""

    __slots__ = ("_h",)

    def __init__(self, handle):
        if not handle:
            raise ValueError("grid: null handle")
        self._h = handle

    def __del__(self):
        h = getattr(self, "_h", None)
        if h:
            self._h = None
            _drop_grid(h)

    # -- shape ------------------------------------------------------------------

    @property
    def ndim(self):
        return _ndim(self._h)

    @property
    def shape(self):
        return tuple(_dim(self._h, d) for d in range(_ndim(self._h)))

    @property
    def size(self):
        return _size(self._h)

    def __len__(self):
        return _dim(self._h, 0)

    @property
    def T(self):
        return Grid(_transpose(self._h))

    def transpose(self):
        return self.T

    def reshape(self, *shape):
        if len(shape) == 1 and isinstance(shape[0], (tuple, list)):
            shape = tuple(shape[0])
        n = self.size
        if len(shape) == 1:
            (m,) = shape
            m = n if m == -1 else m
            if m != n:
                raise ValueError(f"cannot reshape {n} elements into ({m},)")
            return Grid(_reshape1(self._h, m))
        if len(shape) == 2:
            r, c = shape
            if r == -1:
                r = n // c
            if c == -1:
                c = n // r
            if r * c != n:
                raise ValueError(f"cannot reshape {n} elements into ({r}, {c})")
            return Grid(_reshape2(self._h, r, c))
        raise ValueError("grid: one or two dimensions")

    def flatten(self):
        return self.reshape(self.size)

    def copy(self):
        return Grid(_copy(self._h))

    # -- elements -----------------------------------------------------------------

    def _index(self, key):
        if isinstance(key, tuple):
            if len(key) != 2 or self.ndim != 2:
                raise IndexError("grid: (row, column) indexes a 2-D grid")
            i, j = key
            r, c = self.shape
            if isinstance(i, slice) or isinstance(j, slice):
                return None
            i = i + r if i < 0 else i
            j = j + c if j < 0 else j
            if not (0 <= i < r and 0 <= j < c):
                raise IndexError(f"grid: index ({i}, {j}) out of range for shape {(r, c)}")
            return (i, j)
        if isinstance(key, slice):
            return None
        n = len(self)
        i = key + n if key < 0 else key
        if not (0 <= i < n):
            raise IndexError(f"grid: index {key} out of range for length {n}")
        return (i,)

    def __getitem__(self, key):
        if isinstance(key, Mask):
            return _masked(self, key)
        at = self._index(key)
        if at is None:
            if isinstance(key, tuple):
                i, j = key
                if isinstance(i, int) and isinstance(j, slice):
                    return Grid(_row(self._h, i if i >= 0 else i + self.shape[0]))[j]
                if isinstance(i, slice) and isinstance(j, int):
                    return Grid(_col(self._h, j if j >= 0 else j + self.shape[1]))[i]
                raise IndexError("grid: a slice in both positions is not supported")
            lo, hi, step = key.indices(len(self))
            if step != 1:
                raise IndexError("grid: slices step by one")
            return Grid(_rows(self._h, lo, builtins.max(lo, hi)))
        if len(at) == 2:
            return _at2(self._h, at[0], at[1])
        if self.ndim == 2:
            return Grid(_row(self._h, at[0]))
        return _at1(self._h, at[0])

    def __setitem__(self, key, v):
        at = self._index(key)
        if at is None or (len(at) == 1 and self.ndim == 2):
            raise IndexError("grid: assign one element, by index")
        if len(at) == 2:
            _put2(self._h, at[0], at[1], float(v))
        else:
            _put1(self._h, at[0], float(v))

    def __iter__(self):
        if self.ndim == 2:
            return (self[i] for i in range(len(self)))
        return (self[i] for i in range(len(self)))

    def tolist(self):
        d = _data(self._h)
        n = self.size
        flat = [d[k] for k in range(n)]
        _drop_fbuf(ctypes.cast(d, _P))
        if self.ndim == 2:
            r, c = self.shape
            return [flat[i * c:(i + 1) * c] for i in range(r)]
        return flat

    def __array__(self, dtype=None, copy=None):
        import numpy as np
        d = _data(self._h)
        out = np.ctypeslib.as_array(d, shape=(self.size,)).copy()
        _drop_fbuf(ctypes.cast(d, _P))
        out = out.reshape(self.shape)
        return out.astype(dtype) if dtype is not None else out

    # -- arithmetic ---------------------------------------------------------------

    def _same(self, other, what):
        if self.shape != other.shape:
            raise ValueError(f"grid: {what} needs the same shape, not {self.shape} and {other.shape}")

    def _binary(self, temp, other, op, k_scale=None, k_shift=None):
        if isinstance(other, Grid):
            self._same(other, op)
            if op in _bin_into and temp and _contiguous(self._h):
                _bin_into[op](self._h, other._h)
                return self
            return Grid(_bin[op](self._h, other._h))
        v = float(other)
        if k_scale is not None:
            return self._scalar(temp, _scale, _scale_into, k_scale(v))
        if k_shift is not None:
            return self._scalar(temp, _shift, _shift_into, k_shift(v))
        return Grid(_bin[op](self._h, full(self.shape, v)._h))

    def _scalar(self, temp, fresh, into, v):
        if temp and _contiguous(self._h):
            into(self._h, v)
            return self
        return Grid(fresh(self._h, v))

    # Each operator measures its own operand's holders on the way in: the
    # count is only meaningful at the method's entry.
    def __add__(self, other):
        return self._binary(_is_temp(self), other, "add", k_shift=lambda v: v)

    def __radd__(self, other):
        return self._binary(_is_temp(self), other, "add", k_shift=lambda v: v)

    def __sub__(self, other):
        return self._binary(_is_temp(self), other, "sub", k_shift=lambda v: -v)

    def __rsub__(self, other):
        return (-self) + float(other)

    def __mul__(self, other):
        return self._binary(_is_temp(self), other, "mul", k_scale=lambda v: v)

    def __rmul__(self, other):
        return self._binary(_is_temp(self), other, "mul", k_scale=lambda v: v)

    def __truediv__(self, other):
        return self._binary(_is_temp(self), other, "div", k_scale=lambda v: 1.0 / v)

    def __rtruediv__(self, other):
        return full(self.shape, float(other)) / self

    def __pow__(self, other):
        return Grid(_power(self._h, float(other)))

    def __neg__(self):
        return self._scalar(_is_temp(self), _scale, _scale_into, -1.0)

    def __abs__(self):
        return Grid(_un["abs"](self._h))

    def __matmul__(self, other):
        return matmul(self, other)

    # -- comparisons: a Mask ----------------------------------------------------------

    def _compare(self, other, op, kop, flip=None):
        if isinstance(other, Grid):
            self._same(other, "comparison")
            if flip:
                return Mask(_cmp[flip](other._h, self._h))
            return Mask(_cmp[op](self._h, other._h))
        return Mask(_cmpk[kop](self._h, float(other)))

    def __gt__(self, other):
        return self._compare(other, "gt", "gtk")

    def __lt__(self, other):
        return self._compare(other, "gt", "ltk", flip="gt")

    def __ge__(self, other):
        return self._compare(other, "ge", "gek") if isinstance(other, Grid) else ~(self < other)

    def __le__(self, other):
        return self._compare(other, "ge", "lek", flip="ge") if isinstance(other, Grid) else ~(self > other)

    def __eq__(self, other):
        return self._compare(other, "eq", "eqk")

    def __ne__(self, other):
        return ~(self == other)

    __hash__ = None

    # -- reductions ---------------------------------------------------------------

    def sum(self, axis=None):
        return _red["sum"](self._h) if axis is None else Grid(_sum_axis(self._h, self._axis(axis)))

    def mean(self, axis=None):
        return _red["mean"](self._h) if axis is None else Grid(_mean_axis(self._h, self._axis(axis)))

    def _axis(self, axis):
        if self.ndim != 2 or axis not in (0, 1):
            raise ValueError("grid: axis is 0 or 1 of a 2-D grid")
        return axis

    def prod(self):
        return _red["prod"](self._h)

    def max(self):
        return _red["max"](self._h)

    def min(self):
        return _red["min"](self._h)

    def argmax(self):
        return _argmax(self._h)

    def argmin(self):
        return _argmin(self._h)

    def var(self):
        return _red["var"](self._h)

    def std(self):
        return _red["std"](self._h)

    def norm(self):
        return _red["norm"](self._h)

    def trace(self):
        return _red["trace"](self._h)

    def dot(self, other):
        return dot(self, other)

    def cumsum(self):
        return Grid(_un["cumsum"](self._h))

    def clip(self, lo, hi):
        return clip(self, lo, hi)

    def sort(self):
        return sort(self)

    def argsort(self):
        return argsort(self)

    def take(self, idx):
        return take(self, idx)

    def ptp(self):
        return ptp(self)

    def squeeze(self):
        return squeeze(self)

    def diagonal(self):
        return diagonal(self)

    def ravel(self):
        return self.flatten()

    def repeat(self, k):
        return repeat(self, k)

    def round(self):
        return Grid(_un["round"](self._h))

    def __repr__(self):
        return "grid(" + _text(_show(self._h)) + ")"

    __str__ = __repr__


def _masked(a, m):
    """`a[m]`: the elements where the mask holds, as a 1-D grid."""
    if a.shape != m.shape:
        raise ValueError(f"grid: mask shape {m.shape} does not match {a.shape}")
    d = _data(a._h)
    bits = _mask_bits(m._h)
    kept = [d[k] for k in range(a.size) if bits[k]]
    _drop_fbuf(ctypes.cast(d, _P))
    _drop_bbuf(ctypes.cast(bits, _P))
    return array(kept)


class Mask:
    """Where a comparison held: one byte per element, as NumPy's bool arrays."""

    __slots__ = ("_h",)

    def __init__(self, handle):
        if not handle:
            raise ValueError("grid: null handle")
        self._h = handle

    def __del__(self):
        h = getattr(self, "_h", None)
        if h:
            self._h = None
            _drop_mask(h)

    @property
    def shape(self):
        return tuple(_mask_dim(self._h, d) for d in range(_mask_ndim(self._h)))

    @property
    def size(self):
        n = 1
        for d in self.shape:
            n *= d
        return n

    def sum(self):
        return _mask_count(self._h)

    count = sum

    def any(self):
        return _mask_count(self._h) > 0

    def all(self):
        return _mask_count(self._h) == self.size

    def __invert__(self):
        return Mask(_mask_not(self._h))

    def __and__(self, other):
        return Mask(_mask_and(self._h, other._h))

    def __or__(self, other):
        return Mask(_mask_or(self._h, other._h))

    def astype(self, kind=float):
        return Grid(_mask_to_grid(self._h))

    def tolist(self):
        bits = _mask_bits(self._h)
        flat = [bool(bits[k]) for k in range(self.size)]
        _drop_bbuf(ctypes.cast(bits, _P))
        shape = self.shape
        if len(shape) == 2:
            r, c = shape
            return [flat[i * c:(i + 1) * c] for i in range(r)]
        return flat

    def __repr__(self):
        return "mask(" + _text(_mask_show(self._h)) + ")"

    __str__ = __repr__


# -- making grids ------------------------------------------------------------------

def _shape(shape):
    if isinstance(shape, int):
        return (shape,)
    shape = tuple(shape)
    if len(shape) not in (1, 2):
        raise ValueError("grid: a shape is one or two numbers")
    return shape


def zeros(shape):
    s = _shape(shape)
    return Grid(_zeros1(s[0]) if len(s) == 1 else _zeros2(s[0], s[1]))


def full(shape, v):
    s = _shape(shape)
    return Grid(_full1(s[0], float(v)) if len(s) == 1 else _full2(s[0], s[1], float(v)))


def ones(shape):
    return full(shape, 1.0)


def arange(n):
    return Grid(_arange(int(n)))


def linspace(a, b, n):
    return Grid(_linspace(float(a), float(b), int(n)))


def identity(n):
    return Grid(_identity(int(n)))


eye = identity


def array(values):
    """A grid from a list of numbers, a list of equal-length lists, or
    anything with the buffer protocol (a NumPy array)."""
    if isinstance(values, Grid):
        return values.copy()
    try:
        mv = memoryview(values)
    except TypeError:
        mv = None
    if mv is not None and mv.format == "d":
        shape = tuple(mv.shape)
        g = zeros(shape)
        d = _data(g._h)
        flat = mv.tobytes()
        ctypes.memmove(d, flat, len(flat))
        _drop_fbuf(ctypes.cast(d, _P))
        return g
    values = list(values)
    if values and isinstance(values[0], (list, tuple)):
        r = len(values)
        c = len(values[0])
        if any(len(row) != c for row in values):
            raise ValueError("grid: rows of one length")
        g = zeros((r, c))
        flat = [float(x) for row in values for x in row]
    else:
        g = zeros(len(values))
        flat = [float(x) for x in values]
    d = _data(g._h)
    buf = (_F * len(flat))(*flat)
    ctypes.memmove(d, buf, 8 * len(flat))
    _drop_fbuf(ctypes.cast(d, _P))
    return g


# -- functions over grids ------------------------------------------------------------

def where(m, a, b):
    if not isinstance(a, Grid):
        a = full(m.shape, a)
    if not isinstance(b, Grid):
        b = full(m.shape, b)
    if a.shape != b.shape or a.shape != m.shape:
        raise ValueError(f"grid: where needs one shape, not {m.shape}, {a.shape} and {b.shape}")
    return Grid(_where(m._h, a._h, b._h))


def _unary(op, into=None):
    def f(a):
        if into is not None and _is_temp_arg(a) and _contiguous(a._h):
            into(a._h)
            return a
        return Grid(_un[op](a._h))
    f.__name__ = op
    return f


sqrt = _unary("sqrt", _sqrt_into)
abs = _unary("abs")

# Every other one-argument function, under the name NumPy gives it. `round`
# and `rint` are the same function: to the nearest whole number, a half to
# the even one, as NumPy rounds.
for _name in _UNARY:
    if _name not in ("neg", "cumsum", "sqrt", "abs"):
        globals()[_name] = _unary(_name)
rint = globals()["round"]
arcsin, arccos, arctan = asin, acos, atan  # noqa: F821  (made just above)
__all__ += [n for n in _UNARY if n not in ("neg", "cumsum")]
__all__ += ["rint", "arcsin", "arccos", "arctan", "arctan2", "hypot", "fmod",
            "copysign", "clip", "pi", "e"]

pi = 3.141592653589793
e = 2.718281828459045


def _binary2(op, name):
    def f(a, b):
        if isinstance(b, Grid):
            a._same(b, name)
            return Grid(_bin2[op](a._h, b._h))
        return Grid(_bin2[op](a._h, full(a.shape, float(b))._h))
    f.__name__ = name
    return f


arctan2 = _binary2("atan2", "arctan2")
hypot = _binary2("hypot", "hypot")
copysign = _binary2("copysign", "copysign")


def fmod(a, b):
    if isinstance(b, Grid):
        a._same(b, "fmod")
        return Grid(_bin2["fmod"](a._h, b._h))
    return Grid(_fmodk(a._h, float(b)))


def mod(a, b):
    return fmod(a, b)


def power(a, b):
    if isinstance(b, Grid):
        a._same(b, "power")
        return Grid(_bin2["powg"](a._h, b._h))
    return a ** float(b)


def clip(a, lo, hi):
    return Grid(_clip(a._h, float(lo), float(hi)))


def maximum(a, b):
    if isinstance(b, Grid):
        a._same(b, "maximum")
        return Grid(_bin["maximum"](a._h, b._h))
    return Grid(_maxk(a._h, float(b)))


def minimum(a, b):
    if isinstance(b, Grid):
        a._same(b, "minimum")
        return Grid(_bin["minimum"](a._h, b._h))
    return Grid(_mink(a._h, float(b)))


def dot(a, b):
    if a.size != b.size:
        raise ValueError(f"grid: dot of {a.size} against {b.size} elements")
    return _dot(a._h, b._h)


def matmul(a, b):
    if a.ndim == 2 and b.ndim == 1:
        if a.shape[1] != b.shape[0]:
            raise ValueError(f"grid: matvec {a.shape} by {b.shape}")
        return Grid(_matvec(a._h, b._h))
    if a.ndim == 1 and b.ndim == 1:
        return dot(a, b)
    if a.ndim != 2 or b.ndim != 2 or a.shape[1] != b.shape[0]:
        raise ValueError(f"grid: matmul {a.shape} by {b.shape}")
    return Grid(_matmul(a._h, b._h))


def outer(a, b):
    return Grid(_outer(a._h, b._h))


def sum(a, axis=None):
    return a.sum(axis)


def mean(a, axis=None):
    return a.mean(axis)


def prod(a):
    return a.prod()


def max(a):
    return a.max()


def min(a):
    return a.min()


def argmax(a):
    return a.argmax()


def argmin(a):
    return a.argmin()


def var(a):
    return a.var()


def std(a):
    return a.std()


def norm(a):
    return a.norm()


def cumsum(a):
    return a.cumsum()


def trace(a):
    return a.trace()


def transpose(a):
    return a.T


def allclose(a, b, atol=1e-8):
    if a.shape != b.shape:
        return False
    return abs(a - b).max() <= atol


# -- shape, joining, order ---------------------------------------------------------

nan = float("nan")
inf = float("inf")


def zeros_like(a):
    return Grid(_like["zeros"](a._h))


def ones_like(a):
    return Grid(_like["ones"](a._h))


def empty_like(a):
    return Grid(_like["empty"](a._h))


def full_like(a, v):
    return Grid(_full_like(a._h, float(v)))


def squeeze(a):
    return Grid(_squeeze(a._h))


def ravel(a):
    return a.flatten()


def logspace(a, b, n, base=10.0):
    return Grid(_logspace(float(a), float(b), int(n), float(base)))


def geomspace(a, b, n):
    import math
    return logspace(math.log10(a), math.log10(b), n)


def concatenate(grids, axis=0):
    grids = list(grids)
    if not grids:
        raise ValueError("grid: concatenate needs at least one grid")
    out = grids[0]
    for nxt in grids[1:]:
        out = Grid(_concat(out._h, nxt._h, int(axis)))
    return out


def _stacked(grids, f, what):
    grids = list(grids)
    if not grids:
        raise ValueError(f"grid: {what} needs at least one grid")
    out = grids[0]
    for nxt in grids[1:]:
        out = Grid(f(out._h, nxt._h))
    return out


def vstack(grids):
    return _stacked(grids, _vstack, "vstack")


def hstack(grids):
    return _stacked(grids, _hstack, "hstack")


def stack(grids):
    return vstack(grids)


def tile(a, k):
    return Grid(_tile(a._h, int(k)))


def repeat(a, k):
    return Grid(_repeat(a._h, int(k)))


def diag(a):
    return Grid(_diag(a._h))


def diagonal(a):
    return Grid(_diag(a._h))


def tril(a, k=0):
    return Grid(_tril(a._h, int(k)))


def triu(a, k=0):
    return Grid(_triu(a._h, int(k)))


def meshgrid(x, y):
    return Grid(_mesh_x(x._h, y._h)), Grid(_mesh_y(x._h, y._h))


def fliplr(a):
    return Grid(_shapefn["fliplr"](a._h))


def flipud(a):
    return Grid(_shapefn["flipud"](a._h))


def flip(a):
    return Grid(_shapefn["flip"](a._h))


def roll(a, k):
    return Grid(_roll(a._h, int(k)))


def take(a, idx):
    if not isinstance(idx, Grid):
        idx = array([float(i) for i in idx])
    return Grid(_take(a._h, idx._h))


def sort(a):
    return Grid(_shapefn["sort"](a._h))


def argsort(a):
    """Where the elements would go if they were sorted, ties in the order
    they were in. The positions come back as a grid of numbers, this being a
    library of numbers; `take` accepts one as it stands."""
    return Grid(_shapefn["argsort"](a._h))


def unique(a):
    return Grid(_shapefn["distinct"](a._h))


def searchsorted(a, v):
    return _searchsorted(a._h, float(v))


def nonzero(a):
    return Grid(_shapefn["nonzero"](a._h))


def count_nonzero(a):
    return _count_nonzero(a._h)


def median(a):
    return _median(a._h)


def percentile(a, q):
    return _percentile(a._h, float(q))


def quantile(a, q):
    return _quantile(a._h, float(q))


def ptp(a):
    return _ptp(a._h)


def average(a):
    return a.mean()


def diff(a):
    return Grid(_shapefn["diff"](a._h))


def cumprod(a):
    return Grid(_shapefn["cumprod"](a._h))


def isnan(a):
    return Mask(_isfn["nan"](a._h))


def isinf(a):
    return Mask(_isfn["inf"](a._h))


def isfinite(a):
    return Mask(_isfn["finite"](a._h))


def isclose(a, b, atol=1e-8):
    a._same(b, "isclose")
    return Mask(_isclose(a._h, b._h, float(atol)))


def nansum(a):
    return _nan_red["sum"](a._h)


def nanmean(a):
    return _nan_red["mean"](a._h)


def nanmax(a):
    return _nan_red["max"](a._h)


def nanmin(a):
    return _nan_red["min"](a._h)


def nanargmax(a):
    return _nanargmax(a._h)


def nanargmin(a):
    return _nanargmin(a._h)


# -- solving -----------------------------------------------------------------------

def solve(a, b):
    """`x` such that `a @ x == b`, for a vector `b` or a matrix of them."""
    if a.ndim != 2 or a.shape[0] != a.shape[1]:
        raise ValueError(f"grid: solve needs a square matrix, not {a.shape}")
    if b.shape[0] != a.shape[0]:
        raise ValueError(f"grid: solve of {a.shape} against {b.shape}")
    return _or_singular(Grid(_solve(a._h, b._h)), "solve")


def _or_singular(x, what):
    """The library hands back a grid of no elements when the matrix turned
    out to be singular."""
    if x.size == 0:
        raise LinAlgError(f"grid: {what} of a singular matrix")
    return x


class LinAlgError(ValueError):
    """What a singular matrix raises, as `numpy.linalg.LinAlgError` does."""


def inv(a):
    if a.ndim != 2 or a.shape[0] != a.shape[1]:
        raise ValueError(f"grid: inv needs a square matrix, not {a.shape}")
    return _or_singular(Grid(_inv(a._h)), "inv")


def det(a):
    if a.ndim != 2 or a.shape[0] != a.shape[1]:
        raise ValueError(f"grid: det needs a square matrix, not {a.shape}")
    return _det(a._h)


def matrix_power(a, k):
    return Grid(_matrix_power(a._h, int(k)))


def lstsq(a, b):
    """The least-squares solution, through the normal equations."""
    return _or_singular(Grid(_lstsq(a._h, b._h)), "lstsq")


def norm(a, ord=None):
    """The 2-norm of a vector and the Frobenius norm of a matrix by default;
    `ord` of 1 or `inf` gives the sum and the largest, and for a matrix the
    largest column and row sums."""
    if a.ndim == 1:
        if ord is None or ord == 2:
            return a.norm()
        if ord == 1:
            return abs(a).sum()
        if ord == inf:
            return abs(a).max()
        raise ValueError(f"grid: no vector norm for ord={ord!r}")
    if ord is None or ord == "fro":
        return _norm_fro(a._h)
    if ord == 1:
        return _norm1(a._h)
    if ord == inf:
        return _norm_inf(a._h)
    raise ValueError(f"grid: no matrix norm for ord={ord!r}")


def cond(a):
    return _cond(a._h)


def kron(a, b):
    return Grid(_kron(a._h, b._h))


def cross(a, b):
    if a.size != 3 or b.size != 3:
        raise ValueError("grid: cross is of two three-element vectors")
    return Grid(_cross(a._h, b._h))


def inner(a, b):
    return dot(a, b)


def vdot(a, b):
    return dot(a, b)


class _Linalg:
    """`g.linalg.solve(a, b)`, where NumPy puts these."""

    solve = staticmethod(solve)
    inv = staticmethod(inv)
    det = staticmethod(det)
    norm = staticmethod(norm)
    cond = staticmethod(cond)
    matrix_power = staticmethod(matrix_power)
    LinAlgError = LinAlgError
    lstsq = staticmethod(lstsq)
    matmul = staticmethod(matmul)
    outer = staticmethod(outer)
    cross = staticmethod(cross)

    @staticmethod
    def matrix_transpose(a):
        return a.T


linalg = _Linalg()


__all__ += [
    "solve", "inv", "det", "matrix_power", "lstsq", "norm", "cond", "kron",
    "cross", "inner", "vdot", "linalg", "LinAlgError",
]

__all__ += [
    "nan", "inf", "zeros_like", "ones_like", "empty_like", "full_like", "squeeze",
    "ravel", "logspace", "geomspace", "concatenate", "vstack", "hstack", "stack",
    "tile", "repeat", "diag", "diagonal", "tril", "triu", "meshgrid", "fliplr",
    "flipud", "flip", "roll", "take", "sort", "argsort", "unique", "searchsorted",
    "nonzero", "count_nonzero", "median", "percentile", "quantile", "ptp",
    "average", "diff", "cumprod", "isnan", "isinf", "isfinite", "isclose",
    "nansum", "nanmean", "nanmax", "nanmin", "nanargmax", "nanargmin",
]

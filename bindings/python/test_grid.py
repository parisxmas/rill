#!/usr/bin/env python3
"""The Python `grid` package against NumPy, line for line — or against known
answers when NumPy is not installed.

    python3 bindings/python/build.py && python3 bindings/python/test_grid.py
"""
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import grid as g  # noqa: E402

try:
    import numpy as np
except ImportError:
    np = None


def close(a, b, eps=1e-7):
    if isinstance(a, str) or isinstance(b, str):
        return a == b
    if isinstance(a, (list, tuple)) != isinstance(b, (list, tuple)):
        return False
    if isinstance(a, (list, tuple)):
        return len(a) == len(b) and all(close(x, y, eps) for x, y in zip(a, b))
    if isinstance(a, bool) or isinstance(b, bool):
        return a == b
    return abs(a - b) <= eps * max(1.0, abs(a), abs(b))


def check(label, got, want):
    if isinstance(got, (g.Grid, g.Mask)):
        got = got.tolist()
    if isinstance(want, (g.Grid, g.Mask)):
        want = want.tolist()
    if np is not None and isinstance(want, np.ndarray):
        want = want.tolist()
    if np is not None and isinstance(got, np.ndarray):
        got = got.tolist()
    if np is not None and isinstance(want, (np.floating, np.integer, np.bool_)):
        want = want.item()
    try:
        ok = close(got, want)
    except TypeError:
        ok = False
    if not ok:
        print(f"FAIL {label}: got {got!r}, want {want!r}")
        sys.exit(1)
    print(f"ok   {label}")


def main():
    a = g.arange(12)
    m = a.reshape(3, 4)
    check("shape", m.shape, (3, 4))
    check("arange", a, list(range(12)))
    b = m * 2.0 + 1.0
    check("scale+shift", b, [[1, 3, 5, 7], [9, 11, 13, 15], [17, 19, 21, 23]])
    check("operand untouched", m, [[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10, 11]])
    named = m * 2.0
    named_plus = named + 1.0
    check("named operand untouched", named, [[0, 2, 4, 6], [8, 10, 12, 14], [16, 18, 20, 22]])
    check("named plus", named_plus, b)
    c = g.sqrt(m) * b + m
    check("sqrt*b+m sum", c.sum(), 450.0952697018908)
    t = m.T
    check("T.sum(0)", t.sum(axis=0), [6, 22, 38])
    check("T.sum(1)", t.sum(axis=1), [12, 15, 18, 21])
    check("mean(0)", m.mean(axis=0), [4, 5, 6, 7])
    check("matmul", m @ t, [[14, 38, 62], [38, 126, 214], [62, 214, 366]])
    check("dot", a.dot(a), 506.0)
    check("max/min", (b.max(), b.min()), (23.0, 1.0))
    check("argmax/argmin", (a.argmax(), a.argmin()), (11, 0))
    w = g.where(a > 5, a, a * 10)
    check("where", w, [0, 10, 20, 30, 40, 50, 6, 7, 8, 9, 10, 11])
    check("var/std", (a.var(), b.std()), (11.916666666666666, 6.904105059069326))
    check("cumsum", a.cumsum(), [0, 1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 66])
    check("matvec", m @ g.ones(4), [6, 22, 38])
    check("outer", g.outer(g.arange(2), g.arange(3)), [[0, 0, 0], [0, 1, 2]])
    check("trace", (m @ t).trace(), 506.0)
    check("prod", (a + 1).prod(), 479001600.0)
    check("norm", a.norm(), math.sqrt(506.0))
    check("linspace", g.linspace(0, 1, 5), [0, 0.25, 0.5, 0.75, 1])
    check("index", (m[1, 2], a[-1], m[2][1]), (6.0, 11.0, 9.0))
    check("slice", a[3:6], [3, 4, 5])
    check("row slice", m[1, :], [4, 5, 6, 7])
    check("col slice", m[:, 1], [1, 5, 9])
    mk = a > 5
    check("mask", mk, [False] * 6 + [True] * 6)
    check("mask sum/any/all", (mk.sum(), mk.any(), mk.all()), (6, True, False))
    check("mask ops", (mk & (a < 9)).tolist(), [False] * 6 + [True] * 3 + [False] * 3)
    check("mask or", ((a == 1) | (a >= 11)).tolist(), [False, True] + [False] * 9 + [True])
    check("a[mask]", a[a > 8], [9, 10, 11])
    check("array of lists", g.array([[1, 2], [3, 4]]) @ g.array([[1], [1]]), [[3], [7]])
    x = g.array([1.5, 2.5, 3.5])
    x[1] = 10.0
    check("setitem", x, [1.5, 10.0, 3.5])
    check("scalar left", (2.0 - a).tolist()[:3], [2, 1, 0])
    check("scalar div", (1.0 / (a + 1)).tolist()[:2], [1.0, 0.5])
    check("neg/abs", abs(-a).sum(), 66.0)
    check("pow", (a ** 2).sum(), 506.0)
    check("maximum", g.maximum(a, 5.0 + 0 * a).tolist()[:7], [5, 5, 5, 5, 5, 5, 6])
    check("allclose", g.allclose(b, m + m + g.ones((3, 4))), True)
    check("repr", repr(g.arange(3)), "grid([0, 1, 2])")
    # -- the maths, the shapes and the solving, against NumPy where it is here
    if np is not None:
        na = np.linspace(-2, 2, 9)
        for name in ("sin cos tan arctan sinh cosh tanh exp2 cbrt floor ceil "
                     "trunc round rint sign square degrees radians abs exp").split():
            check(name, getattr(g, name)(g.array(na)), getattr(np, name)(na))
        nb = np.linspace(-0.9, 0.9, 5)
        for name in ("arcsin", "arccos", "expm1", "log1p"):
            check(name, getattr(g, name)(g.array(nb)), getattr(np, name)(nb))
        nc = np.linspace(0.5, 4, 5)
        for name in ("log2", "log10", "log", "sqrt", "reciprocal"):
            check(name, getattr(g, name)(g.array(nc)), getattr(np, name)(nc))
        ny, nx = np.array([1.0, -1.0, 0.5]), np.array([1.0, 2.0, -0.5])
        check("arctan2", g.arctan2(g.array(ny), g.array(nx)), np.arctan2(ny, nx))
        check("hypot", g.hypot(g.array(ny), g.array(nx)), np.hypot(ny, nx))
        check("copysign", g.copysign(g.array(ny), g.array(nx)), np.copysign(ny, nx))
        check("fmod", g.fmod(g.array(na), 2.0), np.fmod(na, 2.0))
        check("power grid", g.power(g.array(nc), g.array(np.full(5, 2.0))), nc ** 2)
        check("clip", g.clip(g.array(na), -1, 1), np.clip(na, -1, 1))
        check("maximum scalar", g.maximum(g.array(na), 0.0), np.maximum(na, 0.0))

        A = np.array([3.0, -1, 4, 1, -5, 9, 2, 6, 5, 3, 5])
        B, M = np.arange(6.0), np.arange(12.0).reshape(3, 4)
        ga, gb, gm = g.array(A), g.array(B), g.array(M)
        check("zeros_like", g.zeros_like(gm), np.zeros_like(M))
        check("full_like", g.full_like(gb, 7), np.full_like(B, 7))
        check("logspace", g.logspace(0, 3, 4), np.logspace(0, 3, 4))
        check("geomspace", g.geomspace(1, 1000, 4), np.geomspace(1, 1000, 4))
        check("concatenate", g.concatenate([gb, ga]), np.concatenate([B, A]))
        check("concatenate axis 1", g.concatenate([gm, gm], 1), np.concatenate([M, M], 1))
        check("vstack", g.vstack([gb, gb * 2, gb * 3]), np.vstack([B, B * 2, B * 3]))
        check("hstack", g.hstack([gb, gb * 2]), np.hstack([B, B * 2]))
        check("tile", g.tile(gb, 2), np.tile(B, 2))
        check("repeat", g.repeat(gb, 3), np.repeat(B, 3))
        check("diag vector", g.diag(gb), np.diag(B))
        check("diag matrix", g.diag(gm), np.diag(M))
        check("tril", g.tril(gm), np.tril(M))
        check("triu 1", g.triu(gm, 1), np.triu(M, 1))
        mx, my = g.meshgrid(g.arange(3), g.arange(2))
        nmx, nmy = np.meshgrid(np.arange(3.0), np.arange(2.0))
        check("meshgrid x", mx, nmx)
        check("meshgrid y", my, nmy)
        check("fliplr", g.fliplr(gm), np.fliplr(M))
        check("flipud", g.flipud(gm), np.flipud(M))
        check("roll", g.roll(ga, 3), np.roll(A, 3))
        check("take", g.take(ga, [0, 4, 2, 10]), np.take(A, [0, 4, 2, 10]))
        check("sort", g.sort(ga), np.sort(A))
        check("sort rows", g.sort(gm), np.sort(M))
        check("argsort", g.argsort(ga), np.argsort(A, kind="stable").astype(float))
        check("unique", g.unique(ga), np.unique(A))
        check("searchsorted", g.searchsorted(g.sort(ga), 4.5), np.searchsorted(np.sort(A), 4.5))
        z = np.array([0.0, 3, 0, -1, 2])
        check("nonzero", g.nonzero(g.array(z)), np.nonzero(z)[0].astype(float))
        check("count_nonzero", g.count_nonzero(g.array(z)), np.count_nonzero(z))
        check("median", g.median(ga), np.median(A))
        check("percentile", g.percentile(ga, 25), np.percentile(A, 25))
        check("quantile", g.quantile(ga, 0.9), np.quantile(A, 0.9))
        check("ptp", g.ptp(ga), np.ptp(A))
        check("diff", g.diff(ga), np.diff(A))
        check("cumprod", g.cumprod(g.array([1.0, 2, 3, 4])), np.cumprod([1.0, 2, 3, 4]))
        q = np.array([1.0, np.nan, -3.0, np.inf, 2.0])
        check("isnan", g.isnan(g.array(q)), np.isnan(q).tolist())
        check("isinf", g.isinf(g.array(q)), np.isinf(q).tolist())
        check("isfinite", g.isfinite(g.array(q)), np.isfinite(q).tolist())
        check("isclose", g.isclose(ga, ga + 1e-10), np.isclose(A, A + 1e-10).tolist())
        r = np.array([1.0, np.nan, -3.0, 2.0])
        check("nansum", g.nansum(g.array(r)), np.nansum(r))
        check("nanmean", g.nanmean(g.array(r)), np.nanmean(r))
        check("nanmax", g.nanmax(g.array(r)), np.nanmax(r))
        check("nanargmin", g.nanargmin(g.array(r)), np.nanargmin(r))

        S = np.array([[4.0, -2, 1], [3, 6, -4], [2, 1, 8]])
        V, SM = np.array([12.0, -25, 32]), np.array([[12.0, 1], [-25, 0], [32, 2]])
        gs, gv, gsm = g.array(S), g.array(V), g.array(SM)
        check("solve", g.solve(gs, gv), np.linalg.solve(S, V))
        check("solve matrix", g.solve(gs, gsm), np.linalg.solve(S, SM))
        check("inv", g.inv(gs), np.linalg.inv(S))
        check("det", g.det(gs), np.linalg.det(S))
        check("matrix_power", g.matrix_power(gs, 3), np.linalg.matrix_power(S, 3))
        check("matrix_power -1", g.matrix_power(gs, -1), np.linalg.matrix_power(S, -1))
        check("norm fro", g.norm(gs), np.linalg.norm(S, "fro"))
        check("norm 1", g.norm(gs, 1), np.linalg.norm(S, 1))
        check("norm inf", g.norm(gs, g.inf), np.linalg.norm(S, np.inf))
        check("cond", g.cond(gs), np.linalg.cond(S, 1))
        check("kron", g.kron(g.array([[1.0, 2], [3, 4]]), g.array([[0.0, 5], [6, 7]])),
              np.kron(np.array([[1.0, 2], [3, 4]]), np.array([[0.0, 5], [6, 7]])))
        check("cross", g.cross(g.array([1.0, 2, 3]), g.array([4.0, 5, 6])), np.cross([1.0, 2, 3], [4.0, 5, 6]))
        LM = np.array([[float(i * 2 + j % 3) + 1 for j in range(2)] for i in range(5)])
        LY = np.array([1.0, 3, 2, 5, 4])
        check("lstsq", g.lstsq(g.array(LM), g.array(LY)), np.linalg.lstsq(LM, LY, rcond=None)[0])
        rng = np.random.default_rng(7)
        R, RV = rng.random((40, 40)) + np.eye(40) * 20, rng.random(40)
        check("solve 40x40", g.solve(g.array(R), g.array(RV)), np.linalg.solve(R, RV))
        check("inv 40x40", g.inv(g.array(R)), np.linalg.inv(R))
        big = np.array([(i * 7919) % 10007 - 5000.0 for i in range(20011)])
        check("sort 20k", g.sort(g.array(big)), np.sort(big))
        check("argsort 20k stable", g.argsort(g.array(big)), np.argsort(big, kind="stable").astype(float))
        check("unique 20k", g.unique(g.array(big)).size, len(np.unique(big)))
        for what, call in (("solve", lambda: g.solve(g.zeros((3, 3)), g.zeros(3))),
                           ("inv", lambda: g.inv(g.zeros((3, 3))))):
            try:
                call()
                print(f"FAIL {what} of a singular matrix did not raise")
                sys.exit(1)
            except g.LinAlgError:
                pass
        print("ok   singular matrices raise")

        arr = np.arange(20.0).reshape(4, 5)
        ga = g.array(arr)
        check("from numpy", ga, arr)
        check("to numpy", np.asarray(ga @ ga.T), arr @ arr.T)
        check("numpy where", g.where(ga > 7, ga, -ga), np.where(arr > 7, arr, -arr))
        r = np.random.default_rng(1).random((60, 70))
        s = np.random.default_rng(2).random((70, 30))
        check("random matmul", g.array(r) @ g.array(s), r @ s)
        check("random reductions", (g.array(r).sum(), g.array(r).std(), g.array(r).argmax()), (r.sum(), r.std(), r.argmax()))
    print("all good")


if __name__ == "__main__":
    main()

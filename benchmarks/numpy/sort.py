import numpy as np, time
n, m = 5_000_000, 1_000_000
a = (np.arange(n, dtype=np.int64) * 7919 % 1000003).astype(np.float64) - 500000.0
b = (np.arange(m, dtype=np.int64) * 104729 % 999983).astype(np.float64) - 500000.0
ix = np.arange(n, dtype=np.float64)
warm = np.sort(a).sum() + np.argsort(b, kind="stable").sum()
t0 = time.perf_counter()
s = np.sort(a)
o = np.argsort(b, kind="stable").astype(np.float64)
t1 = time.perf_counter()
print("%g" % s.dot(ix)); print("%g" % o.dot(np.arange(m, dtype=np.float64))); print("%g" % warm)
print("ms=%.1f" % ((t1 - t0) * 1000))

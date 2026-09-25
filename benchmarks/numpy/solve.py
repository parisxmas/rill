import numpy as np, time
n = 400
i, j = np.indices((n, n))
a = ((i * 37 + j * 11) % 23).astype(np.float64) * 0.0001 + np.diag(1.0 + (np.arange(n) % 5) * 0.05)
b = (np.arange(n) % 7).astype(np.float64) - 3.0
def work(a, b):
    return np.linalg.solve(a, b).sum(), np.linalg.det(a), np.linalg.inv(a).sum()
ws, wd, wi = work(a, b)
t0 = time.perf_counter()
s, d, iv = work(a, b)
t1 = time.perf_counter()
print(repr(float(s))); print(repr(float(d))); print(repr(float(iv))); print(repr(float((ws + wd + wi))))
print("ms=%.1f" % ((t1 - t0) * 1000))

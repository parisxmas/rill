import numpy as np, time
n = 800
i, j = np.indices((n, n))
a = ((i * j) % 7).astype(np.float64)
b = ((i + j) % 5).astype(np.float64)
warm = a @ b
t0 = time.perf_counter()
c = a @ b
t1 = time.perf_counter()
print(repr(float(np.trace(c)))); print(repr(float(c.sum()))); print(repr(float(np.trace(warm))))
print("ms=%.1f" % ((t1 - t0) * 1000))

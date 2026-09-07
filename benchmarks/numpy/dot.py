import numpy as np, time
n = 20_000_000
a = (np.arange(n) % 100).astype(np.float64)
b = (np.arange(n) % 37).astype(np.float64)
warm = np.dot(a, b)
t0 = time.perf_counter()
d = np.dot(a, b)
t1 = time.perf_counter()
print("%g" % d); print("%g" % warm)
print("ms=%.1f" % ((t1 - t0) * 1000))

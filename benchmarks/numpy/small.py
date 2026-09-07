import numpy as np, time
v0 = np.arange(16, dtype=np.float64)
def step(v, k):
    for _ in range(k):
        v = v * 0.999 + 0.5
    return v
warm = step(v0, 200000)
t0 = time.perf_counter()
w = step(v0, 200000)
t1 = time.perf_counter()
print("%g" % w.sum()); print("%g" % warm.sum())
print("ms=%.1f" % ((t1 - t0) * 1000))

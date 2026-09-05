#!/usr/bin/env python3
"""Strands against goroutines.

Six programs, each a `.rill`/`.go` pair doing the same thing with lightweight
threads and channels: spawning them, handing off between two of them, many
into one, a worker pool, `select`, and a long line of stages. Every workload
is communication-bound on purpose — the point is the scheduler and the
channels, not arithmetic — so Go is timed twice: on one processor
(`GOMAXPROCS=1`, the shape Rill runs in by default) and as it ships, on all
of them. Time = median of RUNS after WARMUP; outputs cross-checked; peak RSS
= median of 3.

    python3 benchmarks/strands/run.py
"""
import os
import statistics
import subprocess
import sys
import time

DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DIR))
RILL = os.path.join(ROOT, "target", "release", "rill")
BIN = os.path.join(DIR, "bin")
BENCHES = ["spawn", "pingpong", "fanin", "workers", "select", "stages"]
WARMUP, RUNS = 3, 25


def sh(args, **kw):
    r = subprocess.run(args, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"command failed: {' '.join(args)}\n{r.stderr}")


def timed(exe, env):
    times, out = [], None
    for i in range(WARMUP + RUNS):
        t0 = time.perf_counter()
        r = subprocess.run([exe], capture_output=True, text=True, env=env)
        dt = time.perf_counter() - t0
        if r.returncode != 0:
            sys.exit(f"{exe} failed: {r.stderr}")
        out = r.stdout.strip()
        if i >= WARMUP:
            times.append(dt)
    return statistics.median(times), out


def peak_rss(exe, env):
    samples = []
    for _ in range(3):
        r = subprocess.run(["/usr/bin/time", "-l", exe], capture_output=True, text=True, env=env)
        for line in r.stderr.splitlines():
            if "maximum resident set size" in line:
                samples.append(int(line.split()[0]))
    return statistics.median(samples)


def mem(b):
    return f"{b / (1 << 20):.1f}M" if b >= 1 << 20 else f"{b / 1024:.0f}K"


def main():
    os.makedirs(BIN, exist_ok=True)
    if not os.path.exists(RILL):
        sys.exit("build the compiler first: cargo build --release")
    base = dict(os.environ)
    go1 = dict(base, GOMAXPROCS="1")
    cols = [("rill", None, base), ("go, 1 proc", None, go1), ("go, all procs", None, base)]
    print(f"{'benchmark':<10}" + "".join(f"{c[0] + " (ms)":>20}" for c in cols) + "   fastest")
    print("-" * (10 + 20 * len(cols) + 12))
    rss = {}
    for b in BENCHES:
        rexe, gexe = os.path.join(BIN, b + "_rill"), os.path.join(BIN, b + "_go")
        sh([RILL, "build", os.path.join(DIR, b + ".rill"), "-o", rexe])
        sh(["go", "build", "-o", gexe, os.path.join(DIR, b + ".go")])
        results, outs = {}, set()
        for name, _, env in cols:
            exe = rexe if name == "rill" else gexe
            t, out = timed(exe, env)
            results[name] = t
            outs.add(out)
            rss[(b, name)] = peak_rss(exe, env)
        if len(outs) != 1:
            sys.exit(f"{b}: OUTPUT MISMATCH {outs!r}")
        best = min(results, key=results.get)
        second = sorted(results.values())[1]
        print(f"{b:<10}" + "".join(f"{results[n] * 1000:>20.1f}" for n, _, _ in cols)
              + f"   {best} ({second / results[best]:.2f}x)")
    print(f"\n{'peak RSS':<10}" + "".join(f"{c[0]:>20}" for c in cols))
    print("-" * (10 + 20 * len(cols)))
    for b in BENCHES:
        print(f"{b:<10}" + "".join(f"{mem(rss[(b, n)]):>20}" for n, _, _ in cols))
    print(f"\noutputs verified identical; time = median of {RUNS} runs after {WARMUP} warmup, RSS = median of 3")


if __name__ == "__main__":
    main()

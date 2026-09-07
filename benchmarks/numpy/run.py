#!/usr/bin/env python3
"""The grid library against NumPy.

Twelve programs, each a `.rill`/`.py` pair doing the same array work: an
elementwise expression, reductions, a matrix product, sums along the axes of
a transposed view, a tiny vector stepped many times, a dot product, a mask
and a `where`, the transcendental functions, sorting, solving a linear
system, order statistics with NaNs about, and moving numbers around. Every program does its work twice and times the second — the
first read of freshly written memory costs about twice what later reads do,
and a process's first few milliseconds run on a colder clock, and neither
is what a library is being asked about — then prints its answers and `ms=`,
the time of the array work alone by its own clock. So two things are
reported: the whole process (Python's start and NumPy's import included)
and the work. Four columns: Rill on one worker, Rill built `--parallel`
and run with a worker per core (`RILL_THREADS`), NumPy pinned to one
thread (`OPENBLAS_NUM_THREADS=1`), and NumPy as installed (OpenBLAS on
every core). Answers are compared to six significant digits. Median of
RUNS after WARMUP.

    python3 benchmarks/numpy/run.py
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
BENCHES = ["elementwise", "reduce", "matmul", "axis", "small", "dot", "where",
           "trig", "sort", "solve", "stats", "shape"]
WARMUP, RUNS = 1, 5


def sh(args, **kw):
    r = subprocess.run(args, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"command failed: {' '.join(args)}\n{r.stderr}")


def run_once(cmd, env):
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, env=env)
    wall = time.perf_counter() - t0
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed: {r.stderr}")
    lines = r.stdout.strip().split("\n")
    ms = float(lines[-1].split("=")[1])
    return wall * 1000, ms, lines[:-1]


def same(a, b):
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        try:
            fx, fy = float(x), float(y)
        except ValueError:
            if x != y:
                return False
            continue
        if fx != fy and abs(fx - fy) > 1e-6 * max(abs(fx), abs(fy), 1e-300):
            return False
    return True


def main():
    os.makedirs(BIN, exist_ok=True)
    if not os.path.exists(RILL):
        sys.exit("build the compiler first: cargo build --release")
    base = dict(os.environ)
    one = dict(base, OPENBLAS_NUM_THREADS="1", OMP_NUM_THREADS="1")
    cores = os.cpu_count() or 1
    many = dict(base, RILL_THREADS=str(cores))
    cols = [("rill, 1 core", base), (f"rill, {cores} cores", many),
            ("numpy, 1 thread", one), ("numpy, all threads", base)]
    print(f"{'benchmark':<12}" + "".join(f"{c + ' work':>21}" for c, _ in cols))
    print(f"{'':<12}" + "".join(f"{c + ' process':>21}" for c, _ in cols))
    print("-" * (12 + 21 * len(cols)))
    for b in BENCHES:
        exe = os.path.join(BIN, b + "_rill")
        exe_par = os.path.join(BIN, b + "_rill_par")
        # `--unchecked`, as the C suite is built: NumPy's kernels check no
        # index either — both libraries check shapes once at the door.
        sh([RILL, "build", os.path.join(DIR, b + ".rill"), "-o", exe, "--unchecked"])
        sh([RILL, "build", os.path.join(DIR, b + ".rill"), "-o", exe_par, "--unchecked", "--parallel"])
        work, wall, outs = {}, {}, set()
        samples = {name: ([], []) for name, _ in cols}
        # Round-robin, one run of each column per round: the machine's
        # clock and cache state drift over seconds, and a column measured
        # in one block would be measured in one state.
        for i in range(WARMUP + RUNS):
            for name, env in cols:
                cmd = ([exe] if name == "rill, 1 core" else [exe_par] if name.startswith("rill")
                       else [sys.executable, os.path.join(DIR, b + ".py")])
                w, ms, out = run_once(cmd, env)
                if i >= WARMUP:
                    samples[name][0].append(w)
                    samples[name][1].append(ms)
                outs.add(tuple(out))
        for name, _ in cols:
            work[name] = statistics.median(samples[name][1])
            wall[name] = statistics.median(samples[name][0])
        outs = list(outs)
        if not all(same(list(outs[0]), list(o)) for o in outs[1:]):
            sys.exit(f"{b}: OUTPUT MISMATCH {outs!r}")
        print(f"{b:<12}" + "".join(f"{work[n]:>21.1f}" for n, _ in cols))
        print(f"{'':<12}" + "".join(f"{wall[n]:>21.0f}" for n, _ in cols))
    print(f"\nms; work = the program's own clock around the array operations, process = the whole run; "
          f"median of {RUNS} after {WARMUP} warmup; answers agree to six digits")


if __name__ == "__main__":
    main()

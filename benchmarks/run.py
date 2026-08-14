#!/usr/bin/env python3
"""Builds and times the Rill / Go / OCaml benchmark suite.

Each binary runs WARMUP+RUNS times; the median of RUNS is reported.
Outputs are cross-checked across all languages.
"""

import os
import shutil
import statistics
import subprocess
import sys
import time

BENCH_DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(BENCH_DIR)
RILL = os.path.join(ROOT, "target", "release", "rill")
OUT = os.path.join(BENCH_DIR, "bin")
BENCHES = ["fib", "trees", "ring", "lcg"]
WARMUP = 3
RUNS = 25


def sh(args, **kw):
    r = subprocess.run(args, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"command failed: {' '.join(map(str, args))}\n{r.stderr}")
    return r.stdout


def langs():
    out = [
        ("rill", lambda b, exe: sh([RILL, "build", os.path.join(BENCH_DIR, f"{b}.rill"),
                                    "-o", exe])),
        ("go", lambda b, exe: sh(["go", "build", "-o", exe,
                                  os.path.join(BENCH_DIR, f"{b}.go")])),
    ]
    if shutil.which("ocamlopt"):
        def build_ml(b, exe):
            # ocamlopt writes .cmi/.cmx/.o next to the source; build in OUT.
            src = os.path.join(OUT, f"{b}.ml")
            shutil.copyfile(os.path.join(BENCH_DIR, f"{b}.ml"), src)
            sh(["ocamlopt", "-o", exe, src], cwd=OUT)
        out.append(("ocaml", build_ml))
    return out


def timed(exe):
    times, output = [], None
    for i in range(WARMUP + RUNS):
        t0 = time.perf_counter()
        r = subprocess.run([exe], capture_output=True, text=True)
        dt = time.perf_counter() - t0
        if r.returncode != 0:
            sys.exit(f"{exe} failed: {r.stderr}")
        output = r.stdout.strip()
        if i >= WARMUP:
            times.append(dt)
    return statistics.median(times), output


def peak_rss(exe):
    """Peak resident set size in bytes, median of 3 (macOS `time -l`)."""
    samples = []
    for _ in range(3):
        r = subprocess.run(["/usr/bin/time", "-l", exe], capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"{exe} failed under time -l: {r.stderr}")
        for line in r.stderr.splitlines():
            if "maximum resident set size" in line:
                samples.append(int(line.split()[0]))
                break
    return statistics.median(samples)


def fmt_mem(b):
    if b >= 1 << 20:
        return f"{b / (1 << 20):.1f}M"
    return f"{b / 1024:.0f}K"


def main():
    os.makedirs(OUT, exist_ok=True)
    table = langs()
    names = [n for n, _ in table]
    mems = {}
    sizes = {}
    print(f"{'benchmark':<10} " + " ".join(f"{n + ' (ms)':>12}" for n in names) + "   fastest")
    print("-" * (12 + 13 * len(names) + 10))
    for b in BENCHES:
        results = {}
        outputs = {}
        for name, build in table:
            exe = os.path.join(OUT, f"{b}_{name}")
            build(b, exe)
            ms, out = timed(exe)
            results[name] = ms
            outputs[name] = out
            mems[(b, name)] = peak_rss(exe)
            sizes[(b, name)] = os.path.getsize(exe)
        if len(set(outputs.values())) != 1:
            sys.exit(f"{b}: OUTPUT MISMATCH {outputs!r}")
        best = min(results, key=results.get)
        second = sorted(results.values())[1]
        margin = second / results[best]
        row = " ".join(f"{results[n] * 1000:>12.1f}" for n in names)
        print(f"{b:<10} {row}   {best} ({margin:.2f}x)")

    print(f"\n{'peak RSS':<10} " + " ".join(f"{n:>12}" for n in names) + "   smallest")
    print("-" * (12 + 13 * len(names) + 10))
    for b in BENCHES:
        vals = {n: mems[(b, n)] for n in names}
        best = min(vals, key=vals.get)
        row = " ".join(f"{fmt_mem(vals[n]):>12}" for n in names)
        print(f"{b:<10} {row}   {best}")

    print(f"\n{'binary':<10} " + " ".join(f"{n:>12}" for n in names))
    print("-" * (12 + 13 * len(names)))
    for b in BENCHES:
        row = " ".join(f"{fmt_mem(sizes[(b, n)]):>12}" for n in names)
        print(f"{b:<10} {row}")

    print(f"\noutputs verified identical; time = median of {RUNS} runs, RSS = median of 3")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Rill against C, program by program.

Every benchmark is a `.rill`/`.c` pair kept to the same algorithm and the same
data layout. Both are built at -O3, run WARMUP+RUNS times, and the median is
reported; outputs are compared before any time is (a float benchmark prints
its answer as an integer with nine decimals and is allowed to differ in the
last two units, since the two compilers may contract `a*b+c` differently).
`pip install tiktoken` adds the token columns.

    python3 benchmarks/c/run.py            # everything
    python3 benchmarks/c/run.py nbody sieve  # a subset
    python3 benchmarks/c/run.py --unchecked qsort  # Rill built without bounds tests
"""
import os
import statistics
import subprocess
import sys
import time

DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DIR))
TOK = os.path.join(ROOT, "benchmarks", "tokens")
RILL = os.path.join(ROOT, "target", "release", "rill")
BIN = os.path.join(DIR, "bin")
WARMUP, RUNS = 2, 15

# name -> (rill source, c source, stdin file, float-tolerant)
BENCH = {
    "fib":       ("fib.rill", "fib.c", None, False),
    "lcg":       ("lcg.rill", "lcg.c", None, False),
    "tuple":     ("tuple.rill", "tuple.c", None, False),
    "trees":     ("trees.rill", "trees.c", None, False),
    "pipeline":  ("pipeline.rill", "pipeline.c", None, False),
    "sieve":     ("sieve.rill", "sieve.c", None, False),
    "qsort":     ("qsort.rill", "qsort.c", None, False),
    "fannkuch":  ("fannkuch.rill", "fannkuch.c", None, False),
    "matmul":    ("matmul.rill", "matmul.c", None, False),
    "spectral":  ("spectral.rill", "spectral.c", None, True),
    "nbody":     ("nbody.rill", "nbody.c", None, True),
    "nbody_rec": ("nbody_rec.rill", "nbody.c", None, True),
    "hashmap":   ("hashmap.rill", "hashmap.c", None, False),
    "strbuild":  ("strbuild.rill", "strbuild.c", None, False),
    # the five from tokens/ that have a C twin
    "loop":      (f"{TOK}/loop.rill", f"{TOK}/loop.c", None, False),
    "mandel":    (f"{TOK}/mandel.rill", f"{TOK}/mandel.c", None, False),
    "algo":      (f"{TOK}/algo.rill", f"{TOK}/algo.c", None, False),
    "dijkstra":  (f"{TOK}/dijkstra.rill", f"{TOK}/dijkstra.c", f"{TOK}/big.graph", False),
    "json":      (f"{TOK}/json.rill", f"{TOK}/json_free.c", f"{TOK}/big.json", False),
}


def sh(args, **kw):
    r = subprocess.run(args, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"command failed: {' '.join(args)}\n{r.stderr}")


def timed(exe, stdin_path):
    times, out = [], None
    for i in range(WARMUP + RUNS):
        f = open(stdin_path, "rb") if stdin_path else None
        t0 = time.perf_counter()
        r = subprocess.run([exe], stdin=f, capture_output=True)
        dt = time.perf_counter() - t0
        if f:
            f.close()
        if r.returncode != 0:
            sys.exit(f"{exe} failed: {r.stderr.decode()}")
        out = r.stdout
        if i >= WARMUP:
            times.append(dt)
    return statistics.median(times), out


def peak_rss(exe, stdin_path):
    samples = []
    for _ in range(3):
        f = open(stdin_path, "rb") if stdin_path else None
        r = subprocess.run(["/usr/bin/time", "-l", exe], stdin=f, capture_output=True, text=True)
        if f:
            f.close()
        for line in r.stderr.splitlines():
            if "maximum resident set size" in line:
                samples.append(int(line.split()[0]))
    return statistics.median(samples)


def same(a, b, tolerant):
    if a == b:
        return True
    if not tolerant:
        return False
    la, lb = a.split(), b.split()
    return len(la) == len(lb) and all(abs(int(x) - int(y)) <= 2 for x, y in zip(la, lb))


def tokens(path):
    try:
        import tiktoken
    except ImportError:
        return None
    return len(tiktoken.get_encoding("o200k_base").encode(open(path).read()))


def mem(b):
    return f"{b / (1 << 20):.1f}M" if b >= 1 << 20 else f"{b / 1024:.0f}K"


def main():
    os.makedirs(BIN, exist_ok=True)
    if not os.path.exists(RILL):
        sys.exit("build the compiler first: cargo build --release")
    unchecked = "--unchecked" in sys.argv
    names = [a for a in sys.argv[1:] if a != "--unchecked"] or list(BENCH)
    if unchecked:
        print("Rill built with --unchecked: no bounds test on buffer accesses\n")
    rows = []
    print(f"{'benchmark':<10}{'rill ms':>9}{'c ms':>8}{'rill/c':>8}  {'rill rss':>9}{'c rss':>8}  {'rill tok':>9}{'c tok':>7}  verdict")
    print("-" * 92)
    for name in names:
        rsrc, csrc, stdin_name, tolerant = BENCH[name]
        rsrc = rsrc if os.path.isabs(rsrc) else os.path.join(DIR, rsrc)
        csrc = csrc if os.path.isabs(csrc) else os.path.join(DIR, csrc)
        if stdin_name and not os.path.exists(stdin_name):
            print(f"{name:<10} skipped: {os.path.basename(stdin_name)} missing (run tokens/gen_inputs.py)")
            continue
        rexe, cexe = os.path.join(BIN, name + "_rill"), os.path.join(BIN, name + "_c")
        sh([RILL, "build", rsrc, "-o", rexe] + (["--unchecked"] if unchecked else []))
        sh(["clang", "-O3", "-o", cexe, csrc, "-lm"])
        rt, rout = timed(rexe, stdin_name)
        ct, cout = timed(cexe, stdin_name)
        if not same(rout, cout, tolerant):
            sys.exit(f"{name}: OUTPUT MISMATCH\n rill: {rout[:200]!r}\n c:    {cout[:200]!r}")
        rm, cm = peak_rss(rexe, stdin_name), peak_rss(cexe, stdin_name)
        rtok, ctok = tokens(rsrc), tokens(csrc)
        ratio = rt / ct
        verdict = "rill" if ratio < 0.95 else ("c" if ratio > 1.05 else "tie")
        rows.append((name, rt, ct, rm, cm, rtok, ctok, verdict))
        print(f"{name:<10}{rt*1000:>9.1f}{ct*1000:>8.1f}{ratio:>8.2f}  {mem(rm):>9}{mem(cm):>8}  "
              f"{rtok or '-':>9}{ctok or '-':>7}  {verdict}")
    print(f"\noutputs verified; time = median of {RUNS} (after {WARMUP} warmup), RSS = median of 3, "
          f"tokens = o200k_base")
    with open(os.path.join(DIR, "last_run.md"), "w") as f:
        f.write("| benchmark | rill ms | c ms | rill/c | rill RSS | c RSS | rill tokens | c tokens | faster |\n")
        f.write("|---|---:|---:|---:|---:|---:|---:|---:|---|\n")
        for name, rt, ct, rm, cm, rtok, ctok, v in rows:
            f.write(f"| {name} | {rt*1000:.1f} | {ct*1000:.1f} | {rt/ct:.2f} | {mem(rm)} | {mem(cm)} | "
                    f"{rtok or '-'} | {ctok or '-'} | {v} |\n")


if __name__ == "__main__":
    main()

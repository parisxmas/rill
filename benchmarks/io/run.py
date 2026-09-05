#!/usr/bin/env python3
"""Disk and network against Go.

Seven programs, each a `.rill`/`.go` pair: a 32 MB file written and read
back, five thousand small files, `wc` over standard input, two million lines
to standard output, an echo round trip a hundred thousand times, five hundred
connections at once, and 64 MB down one socket. Go is timed on one processor
(`GOMAXPROCS=1`, the shape Rill runs in by default) and as it ships. Time =
median of RUNS after WARMUP; outputs cross-checked; peak RSS = median of 3.

    python3 benchmarks/tokens/gen_inputs.py   # once, for `wc`'s 31 MB input
    python3 benchmarks/io/run.py
"""
import glob
import os
import shutil
import statistics
import subprocess
import sys
import time

DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DIR))
RILL = os.path.join(ROOT, "target", "release", "rill")
BIN = os.path.join(DIR, "bin")
STDIN = os.path.join(ROOT, "benchmarks", "tokens", "big.graph")
WARMUP, RUNS = 2, 15
# name -> (stdin path or None, argument: a tag for file names, or a port)
BENCH = {
    "bigfile": (None, "tag"),
    "smallfiles": (None, "tag"),
    "wc": (STDIN, None),
    "emit": (None, None),
    "echo": (None, "port"),
    "conns": (None, "port"),
    "bulk": (None, "port"),
}
PORTS = {"rill": 19301, "go, 1 proc": 19302, "go, all procs": 19303}


def sh(args, **kw):
    r = subprocess.run(args, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        sys.exit(f"command failed: {' '.join(args)}\n{r.stderr}")


def run_once(cmd, stdin_path, env):
    f = open(stdin_path, "rb") if stdin_path else None
    t0 = time.perf_counter()
    r = subprocess.run(cmd, stdin=f, capture_output=True, env=env)
    dt = time.perf_counter() - t0
    if f:
        f.close()
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed: {r.stderr.decode()}")
    return dt, r.stdout


def peak_rss(cmd, stdin_path, env):
    samples = []
    for _ in range(3):
        f = open(stdin_path, "rb") if stdin_path else None
        r = subprocess.run(["/usr/bin/time", "-l"] + cmd, stdin=f, capture_output=True, text=True, env=env)
        if f:
            f.close()
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
    if not os.path.exists(STDIN):
        sys.exit("big.graph is missing — run benchmarks/tokens/gen_inputs.py")
    base = dict(os.environ)
    cols = [("rill", base), ("go, 1 proc", dict(base, GOMAXPROCS="1")), ("go, all procs", base)]
    print(f"{'benchmark':<11}" + "".join(f"{c + ' (ms)':>20}" for c, _ in cols) + "   fastest")
    print("-" * (11 + 20 * len(cols) + 14))
    rss = {}
    for b, (stdin_path, argkind) in BENCH.items():
        rexe, gexe = os.path.join(BIN, b + "_rill"), os.path.join(BIN, b + "_go")
        sh([RILL, "build", os.path.join(DIR, b + ".rill"), "-o", rexe])
        sh(["go", "build", "-o", gexe, os.path.join(DIR, b + ".go")])
        results, outs = {}, set()
        for name, env in cols:
            tag = name.split(",")[0]
            arg = [str(PORTS[name])] if argkind == "port" else ([tag] if argkind == "tag" else [])
            cmd = [rexe if name == "rill" else gexe] + arg
            if b == "smallfiles":
                os.makedirs(f"/tmp/rill-io-small-{tag}", exist_ok=True)
            # A socket benchmark leaves its connections in TIME_WAIT for a
            # while; too many runs back to back and the client side runs out
            # of ports. Fewer runs, a breath between them.
            warmup, runs = (1, 7) if argkind == "port" else (WARMUP, RUNS)
            times = []
            for i in range(warmup + runs):
                if argkind == "port":
                    time.sleep(0.5)
                dt, out = run_once(cmd, stdin_path, env)
                if i >= warmup:
                    times.append(dt)
            results[name] = statistics.median(times)
            outs.add(out)
            rss[(b, name)] = peak_rss(cmd, stdin_path, env)
        if len(outs) != 1:
            sys.exit(f"{b}: OUTPUT MISMATCH")
        best = min(results, key=results.get)
        second = sorted(results.values())[1]
        print(f"{b:<11}" + "".join(f"{results[n] * 1000:>20.1f}" for n, _ in cols)
              + f"   {best} ({second / results[best]:.2f}x)")
    for path in glob.glob("/tmp/rill-io-big-*.txt"):
        os.remove(path)
    for d in glob.glob("/tmp/rill-io-small-*"):
        shutil.rmtree(d, ignore_errors=True)
    print(f"\n{'peak RSS':<11}" + "".join(f"{c:>20}" for c, _ in cols))
    print("-" * (11 + 20 * len(cols)))
    for b in BENCH:
        print(f"{b:<11}" + "".join(f"{mem(rss[(b, n)]):>20}" for n, _ in cols))
    print(f"\noutputs verified identical; time = median of {RUNS} runs after {WARMUP} warmup (7 after 1 for the socket benchmarks), RSS = median of 3")


if __name__ == "__main__":
    main()

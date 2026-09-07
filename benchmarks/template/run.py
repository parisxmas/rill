#!/usr/bin/env python3
"""Renders the two pages the Rust template benchmark renders, and times them.

    cargo build --release
    python3 benchmarks/template/run.py

Both templates are compiled by `tools/tmplgen.rill` into the Rill in this
directory, which is checked in so that the generated code can be read. The
rendered bytes are compared against a reference before any time is reported.

To measure the Rust engines next to it, clone the suite the two workloads
come from, drop the harness in this directory into it, and point at the
binary it builds:

    git clone https://github.com/askama-rs/template-benchmark
    cp benchmarks/template/time.rs template-benchmark/src/bin/time.rs
    cd template-benchmark && cargo build --release \\
        --no-default-features --features sailfish,maud,askama,markup --bin time
    python3 benchmarks/template/run.py --rust path/to/time

The Rust harness times what this one times: the same page rendered over and
over into one buffer that is emptied between renders, with the clock read
once around the whole round rather than once around each render.
"""

import argparse
import os
import re
import subprocess
import sys

DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DIR))
RILL = os.path.join(ROOT, "target", "release", "rill")
BIN = os.path.join(DIR, "bin")
CASES = [("big-table", 2000), ("teams", 200000)]
ENGINES = ["sailfish", "askama", "markup", "maud"]


def sh(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def build():
    os.makedirs(BIN, exist_ok=True)
    env = dict(os.environ)
    env["RILL_PATH"] = os.path.join(ROOT, "lib") + ":" + DIR
    gen = os.path.join(BIN, "tmplgen")
    r = sh([RILL, "build", os.path.join(ROOT, "tools", "tmplgen.rill"), "-o", gen], env=env)
    if r.returncode != 0:
        sys.exit("tmplgen: " + r.stderr)
    # From the root with a relative path: the generator writes the name it is
    # given into the file, and the file is checked in.
    for name, out in [("big-table.tmpl", "big_table.rill"), ("teams.tmpl", "teams_tmpl.rill")]:
        here = os.path.relpath(DIR, ROOT)
        r = sh([gen, os.path.join(here, name), os.path.join(here, out)], cwd=ROOT)
        if r.returncode != 0 or "wrote" not in r.stdout:
            sys.exit(f"{name}: {r.stdout}{r.stderr}")
    bench = os.path.join(BIN, "bench")
    r = sh([RILL, "build", os.path.join(DIR, "bench.rill"), "-o", bench], env=env)
    if r.returncode != 0:
        sys.exit("bench: " + r.stderr)
    return bench


def each_ns(out):
    m = re.search(r"each (\d+) ns", out)
    return int(m.group(1)) if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust", help="the template-benchmark timing binary, to measure next to")
    ap.add_argument("--rounds", type=int, default=4)
    args = ap.parse_args()
    if not os.path.exists(RILL):
        sys.exit("build the compiler first: cargo build --release")
    bench = build()

    # What is rendered is checked before how long it takes: a page that is
    # wrong is not fast.
    for case, _ in CASES:
        got = sh([bench, case]).stdout
        want = open(os.path.join(DIR, f"{case}.expected")).read()
        if got != want:
            sys.exit(f"{case}: the page rendered is not the one expected")
    print("both pages render byte for byte as expected\n")

    for case, n in CASES:
        rows = {}
        for _ in range(args.rounds):
            t = each_ns(sh([bench, case, str(n)]).stdout)
            rows["Rill"] = min(t, rows.get("Rill", t))
            for e in ENGINES if args.rust else []:
                t = each_ns(sh([args.rust, case, e, str(n)]).stdout)
                if t is not None:
                    rows[e] = min(t, rows.get(e, t))
        print(f"== {case}   {n} renders a round, best of {args.rounds}")
        print(f"   {'engine':<12}{'ns a page':>12}")
        for name, t in sorted(rows.items(), key=lambda r: r[1]):
            print(f"   {name:<12}{t:12}")
        print()


if __name__ == "__main__":
    main()

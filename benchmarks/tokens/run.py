#!/usr/bin/env python3
"""Builds, checks and times the same five programs in eight languages.

Two of them — `json` and `dijkstra` — are the pair the token comparison is
about: what it costs to *write* the same algorithm. The other three are timed
only. Every language's output is compared against every other's before any
time is reported, because a benchmark that is quietly wrong is worse than no
benchmark.

    python3 benchmarks/tokens/gen_inputs.py     # once; 43 MB of input
    python3 benchmarks/tokens/run.py

A missing toolchain is skipped with a note rather than being an error, so this
runs with whatever is installed. `pip install tiktoken` adds the token counts;
without it the table is times only.
"""

import hashlib
import os
import shutil
import subprocess
import sys
import time

DIR = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(DIR))
RILL = os.path.join(ROOT, "target", "release", "rill")
BIN = os.path.join(DIR, "bin")
RUNS = 9

# name -> (stdin file or None, whether it is one of the two token tasks)
BENCH = {
    "json": ("big.json", True),
    "dijkstra": ("big.graph", True),
    "loop": (None, False),
    "algo": (None, False),
    "mandel": (None, False),
}
# The extra Rill entry: the same task written the plain way, with `map` and
# `join` instead of a builder threaded through by hand.
# Two more rows that are the same task written differently, kept because
# without them a number in the table means something it does not: nearly all
# of C++'s Dijkstra time is `std::cin >>` rather than anything about C++.
EXTRA = {
    "json": [("Rill · short", "json_short.rill"), ("C · frees", "json_free.c")],
    "dijkstra": [("C++ · scanf", "dijkstra_scanf.cpp")],
}


def sh(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def have(exe):
    return shutil.which(exe) is not None


def build_cs(src, exe):
    """NativeAOT, so what is timed is a program and not a runtime warming up."""
    proj = os.path.join(BIN, "cs-" + os.path.basename(exe))
    os.makedirs(proj, exist_ok=True)
    with open(os.path.join(proj, "p.csproj"), "w") as f:
        f.write(
            "<Project Sdk='Microsoft.NET.Sdk'><PropertyGroup>"
            "<OutputType>Exe</OutputType><TargetFramework>net11.0</TargetFramework>"
            "<Nullable>enable</Nullable><PublishAot>true</PublishAot>"
            "<AssemblyName>p</AssemblyName><InvariantGlobalization>true</InvariantGlobalization>"
            "</PropertyGroup></Project>"
        )
    shutil.copy(src, os.path.join(proj, "Program.cs"))
    r = sh(["dotnet", "publish", "-c", "Release", "-o", os.path.join(proj, "out"), proj])
    if r.returncode != 0:
        return r
    shutil.copy(os.path.join(proj, "out", "p"), exe)
    return r


LANGS = [
    ("Rill", "rill", lambda s, e: sh([RILL, "build", s, "-o", e]), lambda: os.path.exists(RILL)),
    ("C", "c", lambda s, e: sh(["clang", "-O2", "-o", e, s]), lambda: have("clang")),
    ("C++", "cpp", lambda s, e: sh(["clang++", "-O2", "-std=c++20", "-o", e, s]),
     lambda: have("clang++")),
    ("Rust", "rs", lambda s, e: sh(["rustc", "-O", "-o", e, s]), lambda: have("rustc")),
    ("Zig", "zig", lambda s, e: sh(["zig", "build-exe", "-O", "ReleaseFast", "-femit-bin=" + e, s],
                                   cwd=BIN), lambda: have("zig")),
    ("C#", "cs", build_cs, lambda: have("dotnet")),
    ("Go", "go", lambda s, e: sh(["go", "build", "-o", e, s]), lambda: have("go")),
    ("OCaml", "ml", lambda s, e: sh(["ocamlfind", "ocamlopt", "-package", "str", "-linkpkg",
                                     "-O3", "-o", e, s], cwd=BIN)
     if have("ocamlfind") else sh(["ocamlopt", "-o", e, s], cwd=BIN), lambda: have("ocamlopt")),
]


def run_once(exe, stdin_path):
    f = open(stdin_path, "rb") if stdin_path else None
    t0 = time.perf_counter()
    p = subprocess.run([exe], stdin=f, capture_output=True)
    dt = time.perf_counter() - t0
    if f:
        f.close()
    return dt, p.returncode, hashlib.md5(p.stdout).hexdigest()


def tokens(path):
    try:
        import tiktoken
    except ImportError:
        return None
    src = open(path).read()
    return (len(tiktoken.get_encoding("o200k_base").encode(src)),
            len(tiktoken.get_encoding("cl100k_base").encode(src)))


def main():
    os.makedirs(BIN, exist_ok=True)
    if not os.path.exists(RILL):
        sys.exit("build the compiler first: cargo build --release")
    skipped = [name for name, _, _, ok in LANGS if not ok()]
    if skipped:
        print("no toolchain, skipping:", ", ".join(skipped), "\n")

    for bench, (stdin_name, is_token_task) in BENCH.items():
        stdin_path = os.path.join(DIR, stdin_name) if stdin_name else None
        if stdin_path and not os.path.exists(stdin_path):
            sys.exit(f"{stdin_name} is missing — run gen_inputs.py")
        entries = [(name, ext, f"{bench}.{ext}") for name, ext, _, ok in LANGS if ok()]
        entries += EXTRA.get(bench, [])

        rows, digests = [], {}
        for item in entries:
            name, src = (item[0], item[2]) if len(item) == 3 else (item[0], item[1])
            path = os.path.join(DIR, src)
            if not os.path.exists(path):
                continue
            builder = next(b for n, _, b, _ in LANGS if n == name.split(" ·")[0])
            exe = os.path.join(BIN, f"{bench}-{name.replace(' · ', '-').replace('#', 'sharp').replace('+', 'p')}")
            r = builder(path, exe)
            if r.returncode != 0 or not os.path.exists(exe):
                print(f"  {name}: BUILD FAILED\n{r.stderr[-400:]}")
                continue
            times = []
            for _ in range(RUNS):
                dt, rc, h = run_once(exe, stdin_path)
                if rc != 0:
                    print(f"  {name}: exit {rc}")
                    break
                times.append(dt)
            else:
                digests[name] = h
                rows.append((name, min(times), os.path.getsize(exe), tokens(path)))

        # C contracts `a*b + c` into a fused multiply-add by default and Go
        # does too; Rill, Rust, Zig and C# do not. On a program that is
        # nothing but `a*b + c` that is most of the difference between them,
        # so C is built both ways here rather than leaving the reader to
        # wonder. It is a question of what rounding a language promises, not
        # of how fast it is.
        if bench == "mandel" and have("clang"):
            exe = os.path.join(BIN, "mandel-C-strict")
            r = sh(["clang", "-O2", "-ffp-contract=off", "-o", exe,
                    os.path.join(DIR, "mandel.c")])
            if r.returncode == 0:
                times = [run_once(exe, None)[0] for _ in range(RUNS)]
                rows.append(("C · no FMA", min(times), os.path.getsize(exe), None))

        agree = len(set(digests.values())) <= 1
        print(f"== {bench}   {'all outputs identical' if agree else 'OUTPUTS DIFFER: ' + str(digests)}")
        base = dict((n, t) for n, t, _, _ in rows).get("C")
        head = f"   {'language':<14}{'ms':>8}{'×C':>7}{'binary':>10}"
        if any(t for _, _, _, t in rows):
            head += f"{'o200k':>8}{'cl100k':>8}"
        print(head)
        for name, t, size, tok in sorted(rows, key=lambda r: r[1]):
            line = f"   {name:<14}{t * 1000:8.0f}{(t / base if base else 0):7.2f}{size / 1024:9.0f}K"
            if tok:
                line += f"{tok[0]:8}{tok[1]:8}"
            print(line)
        print()


if __name__ == "__main__":
    main()

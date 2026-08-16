#!/usr/bin/env python3
"""Writes the two inputs the timed programs read.

Both come out of the same linear congruential sequence the `algo` programs
generate their graph with, so nothing here depends on a language's own random
number generator and the files are byte-identical wherever they are made.

    big.graph  31 MB  200k nodes, 2M edges — the same graph `algo` builds in
                      process, written out as text so `dijkstra` has to read it
    big.json   11 MB  one line, a nested document with every kind of value in
                      it, including strings that need escaping
"""

import os
import sys

DIR = os.path.dirname(os.path.abspath(__file__))
N, M = 200_000, 2_000_000


class Lcg:
    """The sequence in `algo.c`, to the bit."""

    def __init__(self, seed=12345):
        self.s = seed

    def __call__(self):
        self.s = (self.s * 1103515245 + 12345) & 2147483647
        return self.s

    def below(self, n):
        return self() % n


def write_graph(path):
    r = Lcg()
    out = [f"{N} {M} 0 {N - 1}\n"]
    for _ in range(M):
        out.append(f"{r.below(N)} {r.below(N)} {r.below(100)}\n")
    with open(path, "w") as f:
        f.write("".join(out))


# One line of JSON, about eleven megabytes of it. The shape matters more than
# the size: a parser that only ever sees flat arrays of numbers is not being
# asked the same question as one that has to keep a stack, unescape strings and
# tell six kinds of value apart.
STRINGS = ['"leaf"', '"a\\tstring \\"quoted\\""', '"kısa"', '""', '"\\\\n"']


def value(r, depth, out):
    kind = r.below(10)
    if depth >= 6:
        kind = r.below(5)
    if kind == 0:
        out.append("null")
    elif kind == 1:
        out.append("true" if r.below(2) else "false")
    elif kind in (2, 3):
        n = r.below(1_000_000)
        out.append(str(-n if r.below(4) == 0 else n))
    elif kind == 4:
        out.append(STRINGS[r.below(len(STRINGS))])
    elif kind in (5, 6, 7):
        out.append("[")
        for i in range(r.below(7)):
            if i:
                out.append(",")
            value(r, depth + 1, out)
        out.append("]")
    else:
        out.append("{")
        for i in range(r.below(6)):
            if i:
                out.append(",")
            out.append(f'"k{i}":')
            value(r, depth + 1, out)
        out.append("}")


def write_json(path, target=11_000_000):
    r = Lcg()
    out = ["["]
    n = 1
    first = True
    while n < target:
        if not first:
            out.append(",")
            n += 1
        first = False
        before = len(out)
        value(r, 0, out)
        n += sum(len(p) for p in out[before:])
    out.append("]\n")
    with open(path, "w") as f:
        f.write("".join(out))


def main():
    graph, doc = os.path.join(DIR, "big.graph"), os.path.join(DIR, "big.json")
    for path, make in ((graph, write_graph), (doc, write_json)):
        if os.path.exists(path) and "--force" not in sys.argv:
            print(f"{os.path.basename(path)}: already there")
            continue
        make(path)
        print(f"{os.path.basename(path)}: {os.path.getsize(path) / 1e6:.1f} MB")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Slots of the state block claimed twice — the trap that only bites wasm.

    python3 tools/slots.py valley.rill

A state block is one array reached two ways. `peek_f32(p, i)` reads the byte
at 4i; `peek_ptr(p, i)` reads the byte at `i * sizeof(void*)` — eight on the
server, **four in the browser**. So on wasm32 float slot i and pointer slot i
are the same four bytes: a buffer parked on a pointer slot whose float twin
this file already uses is a buffer that a counter writes over, in the browser
only, while the server and every native test go on passing.

That is exactly how it went wrong. Four rings went in at 121 to 124, which
are the round's own clock and counters; the marks' cursor went on float 94,
which is `wd`, the pointer back to the world; and the second attempt landed
on 70 to 72, which is where the last broken crystal stood. None of it showed
up until a `Float32Array` refused an odd address in the browser.

The pairs below are older than that and are safe by accident of block: the
pointer is on the world and the float is on a player, and no block is ever
both. Nothing new should join the list — pick a slot this prints as free.
"""
import re
import sys

# pointer on the world, float on a player: different blocks, never met
KNOWN = {106, 113, 114, 115, 116, 117, 118, 119, 120}


def scan(path):
    src = re.sub(r"#[^\n]*", "", open(path).read())
    floats = set(int(m) for m in re.findall(r"\b(?:peek|poke)_f32\(\s*\w+\s*,\s*(\d+)", src))
    ptrs = set(int(m) for m in re.findall(r"\b(?:peek|poke)_ptr\(\s*[\w()]+\s*,\s*(\d+)", src))
    return floats, ptrs


def main(path):
    floats, ptrs = scan(path)
    clash = sorted((floats & ptrs) - KNOWN)
    for i in clash:
        print(f"slot {i}: claimed as a float and as a pointer — the same four "
              f"bytes in the browser")
    free = [i for i in range(64, 128) if i not in floats and i not in ptrs]
    if clash:
        print(f"{len(clash)} slot(s) claimed twice. Free instead: "
              + (", ".join(map(str, free)) or "none — take one from a retired buffer"))
        return 1
    print(f"{len(floats)} float slots, {len(ptrs)} pointer slots, none newly claimed twice")
    print("free for a pointer: " + (", ".join(map(str, free)) or "none"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))

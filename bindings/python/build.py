#!/usr/bin/env python3
"""Builds the `grid` package's library from `gridlib.rill`.

    python3 bindings/python/build.py

Needs the Rill compiler (`rill` on PATH, or the RILL environment variable
pointing at it; the repository's `target/release/rill` is found on its own)
and RILL_PATH pointing at the repository's `lib`, where `grid.rill` lives
(set on its own too, when run from inside the repository).
"""
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))


def main():
    rill = os.environ.get("RILL") or shutil.which("rill") or os.path.join(ROOT, "target", "release", "rill")
    if not os.path.exists(rill):
        sys.exit("build.py: no `rill` compiler found — build it with `cargo build --release`")
    env = dict(os.environ)
    env.setdefault("RILL_PATH", os.path.join(ROOT, "lib"))
    out = os.path.join(HERE, "grid", "_gridlib")
    args = [rill, "build", os.path.join(HERE, "gridlib.rill"), "--shared", "--unchecked", "-o", out]
    if "--checked" in sys.argv:
        args.remove("--unchecked")
    r = subprocess.run(args, env=env)
    if r.returncode != 0:
        sys.exit(r.returncode)
    print("built", out + (".dylib" if sys.platform == "darwin" else ".so"))


if __name__ == "__main__":
    main()

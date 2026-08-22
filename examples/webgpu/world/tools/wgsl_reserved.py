#!/usr/bin/env python3
"""Every name a WGSL shader declares, against the words WGSL keeps.

    python3 tools/wgsl_reserved.py shaders/*.wgsl

The list is the reserved-word table from the WGSL specification. These are
not errors a reader spots: `from` and `ref` are ordinary English and ordinary
variable names, and both of them shipped — the first blanked every pipeline
in the scene, the second killed one. A shader that fails to compile takes its
whole pipeline with it and the only symptom is something missing from the
picture, so the cheapest place to catch it is here.
"""
import re
import sys

RESERVED = set("""NULL Self abstract active alignas alignof as asm asm_fragment async
attribute auto await become binding_array cast catch class co_await co_return
co_yield coherent column_major common compile compile_fragment concept
const_cast consteval constexpr constinit crate debugger decltype delete demote
demote_to_helper do dynamic_cast enum explicit export extends extern external
fallthrough filter final finally friend from fxgroup get goto groupshared
highp impl implements import inline instanceof interface layout lowp macro
macro_rules match mediump meta mod module move mut mutable namespace new nil
noexcept noinline nointerpolation non_coherent noncoherent noperspective null
nullptr of operator package packoffset partition pass patch pixelfragment
precise precision premerge priv protected pub public readonly ref regardless
register reinterpret_cast require resource restrict self set shared sizeof
smooth snorm static static_assert static_cast std subroutine super target
template this thread_local throw trait try type typedef typeid typename typeof
union unless unorm unsafe unsized use using varying virtual volatile wgsl
where while writeonly yield""".split())

# `let x`, `var x`, `fn x(`, `struct X`, and a function's parameters.
DECL = re.compile(r"\b(?:let|var|const|fn|struct|override)\s+([A-Za-z_]\w*)")
PARAM = re.compile(r"@location\(\d+\)\s+([A-Za-z_]\w*)\s*:|(?:^|,|\()\s*([A-Za-z_]\w*)\s*:\s*[A-Za-z_]")


def check(path):
    src = open(path).read()
    # comments hold English prose, and `from` in a sentence is not a decl
    src = re.sub(r"//[^\n]*", "", src)
    bad = []
    for m in DECL.finditer(src):
        if m.group(1) in RESERVED:
            bad.append((m.group(1), src[: m.start()].count("\n") + 1))
    for m in PARAM.finditer(src):
        name = m.group(1) or m.group(2)
        if name and name in RESERVED:
            bad.append((name, src[: m.start()].count("\n") + 1))
    return bad


def main(paths):
    bad = 0
    for p in paths:
        for name, line in check(p):
            print(f"{p}:{line}: `{name}` is a WGSL reserved word")
            bad += 1
    if bad:
        print(f"{bad} reserved name(s) declared")
        return 1
    print(f"{len(paths)} shaders, no reserved names declared")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

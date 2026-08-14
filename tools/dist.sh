#!/bin/bash
# Packages a self-contained Rill toolchain into dist/rill/.
#
# Rust builds the compiler and the runtimes *here*, once — the same way a C
# compiler is built with a C compiler. What comes out needs no cargo and no
# rustup: `bin/rill` finds its runtimes and the wasi sysroot in `lib/rill/`,
# so a user builds native binaries and browser demos with nothing but this
# directory (plus `wasm-ld` for the wasm target, which comes with LLVM).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dist="$root/dist/rill"
wasm_triple=wasm32-wasip1

cargo build --release
cargo build --release --target $wasm_triple -p rill_runtime

sysroot=$(rustc --print sysroot)/lib/rustlib/$wasm_triple/lib/self-contained
[ -f "$sysroot/libc.a" ] || { echo "missing wasi-libc: rustup target add $wasm_triple" >&2; exit 1; }

rm -rf "$dist"
mkdir -p "$dist/bin" "$dist/lib/rill/$wasm_triple"
cp "$root/target/release/rill" "$dist/bin/"
cp "$root/target/release/librill_runtime.a" "$dist/bin/"
cp "$root/target/$wasm_triple/release/librill_runtime.a" "$dist/lib/rill/$wasm_triple/"
cp "$sysroot/crt1-command.o" "$sysroot/crt1-reactor.o" "$sysroot/libc.a" "$dist/lib/rill/$wasm_triple/"

echo "packaged: $dist"
du -sh "$dist" | cut -f1

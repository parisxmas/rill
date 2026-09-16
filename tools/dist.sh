#!/bin/bash
# Packages a self-contained Rill toolchain: dist/rill/, and a tarball of
# it named for the version and the machine it runs on.
#
# Rust builds the compiler and the runtimes *here*, once — the same way a C
# compiler is built with a C compiler. What comes out needs no cargo and no
# rustup: `bin/rill` finds its runtimes and the wasi sysroot in `lib/rill/`,
# the standard library in `lib/`, so a user builds native binaries and
# browser demos with nothing but this directory (the model editor is not
# in it; it runs at rillsdl.baltavista.com) — plus a C linker (`cc`:
# the Xcode command-line tools on a Mac, `gcc` on Linux), and `wasm-ld`
# for the wasm target, which comes with LLVM.
#
#     bash tools/dist.sh          # dist/rill/ and dist/rill-<version>-<os>-<arch>.tar.gz
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dist="$root/dist/rill"
wasm_triple=wasm32-wasip1
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)

cargo build --release
cargo build --release --target $wasm_triple -p rill_runtime

sysroot=$(rustc --print sysroot)/lib/rustlib/$wasm_triple/lib/self-contained
[ -f "$sysroot/libc.a" ] || { echo "missing wasi-libc: rustup target add $wasm_triple" >&2; exit 1; }

rm -rf "$dist"
mkdir -p "$dist/bin" "$dist/lib/rill/$wasm_triple"
cp "$root/target/release/rill" "$dist/bin/"
cp "$root/target/release/librill_runtime.a" "$dist/bin/"
# On a Mac the compiler links Homebrew's zstd, which a user need not have:
# the library travels with it, the binary is told to look beside itself,
# and is signed again, since a changed binary is one Apple silicon refuses.
if [ "$(uname -s)" = "Darwin" ]; then
  for lib in $(otool -L "$dist/bin/rill" | awk '/\/opt\/homebrew\//{print $1}'); do
    cp "$lib" "$dist/bin/"
    install_name_tool -change "$lib" "@executable_path/$(basename "$lib")" "$dist/bin/rill"
  done
  codesign -s - -f "$dist/bin/rill" 2>/dev/null
fi
cp "$root/target/$wasm_triple/release/librill_runtime.a" "$dist/lib/rill/$wasm_triple/"
cp "$sysroot/crt1-command.o" "$sysroot/crt1-reactor.o" "$sysroot/libc.a" "$dist/lib/rill/$wasm_triple/"
# the standard library, found beside bin/ by the compiler itself
for d in "$root"/lib/*/; do
  name=$(basename "$d")
  case "$name" in bench*|wt_*) continue;; esac
  mkdir -p "$dist/lib/$name"
  cp "$d"/*.rill "$dist/lib/$name/"
done
mkdir -p "$dist/examples"
cp "$root"/examples/*.rill "$dist/examples/"
cp "$root/LANGUAGE.md" "$root/README.md" "$root/CHANGELOG.md" "$dist/"
cat > "$dist/INSTALL.md" <<'DOC'
# Rill, packaged

    bin/rill                 the compiler; put bin/ on your PATH, or call it by path
    bin/librill_runtime.a    the runtime it links into every program
    lib/                     the standard library, found by the compiler beside bin/
    lib/rill/wasm32-wasip1   the runtime and libc for `--target wasm32-wasip1`
    examples/                programs to start from; `bin/rill run examples/hello.rill`
    LANGUAGE.md              the reference

Needs a C linker on the PATH: the Xcode command-line tools on a Mac
(`xcode-select --install`), `gcc` or `clang` on Linux. Nothing else.

On a Mac, a binary downloaded through a browser is quarantined and will
not run until `xattr -dr com.apple.quarantine <this directory>`; a
`curl` download is not.
DOC

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m); [ "$arch" = "arm64" ] && arch=arm64; [ "$arch" = "aarch64" ] && arch=arm64
[ "$os" = "darwin" ] && os=macos
tarball="$root/dist/rill-$version-$os-$arch.tar.gz"
(cd "$root/dist" && tar czf "$tarball" rill)
echo "packaged: $dist"
echo "tarball:  $tarball ($(du -h "$tarball" | cut -f1))"

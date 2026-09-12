#!/bin/sh
# lib/str.rill against C, on one 44 MB text. Prints both tables and then the
# strand functions on eight workers. Needs `rill` on the path and `cc`.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${TMPDIR:-/tmp}/rill-strings-bench
mkdir -p "$out"
cc -O3 -o "$out/strings_c" "$here/strings.c"
RILL_PATH="$root/lib" rill build "$here/strings.rill" -o "$out/strings_rill"
RILL_PATH="$root/lib" rill build --parallel "$here/strings.rill" -o "$out/strings_par"
echo "== C"; "$out/strings_c"
echo "== Rill"; "$out/strings_rill"
echo "== Rill, RILL_THREADS=8"; RILL_THREADS=8 "$out/strings_par" | tail -2

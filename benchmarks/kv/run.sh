#!/bin/sh
# lib/kv.rill against SQLite on one workload: N random keys put and
# committed, every key read back, a range scan, then 1000 one-key
# transactions committed one by one. Needs `rill` on the path and
# Homebrew's sqlite (`brew install sqlite`); the system libsqlite3 is an
# older build that does not honour `fullfsync`.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
n=${1:-200000}
out=${TMPDIR:-/tmp}/rill-kv-bench
mkdir -p "$out"
sq=$(brew --prefix sqlite)
cc -O2 -I"$sq/include" -L"$sq/lib" -o "$out/sqlite_bench" "$here/sqlite_bench.c" -lsqlite3
RILL_PATH="$root/lib" rill build "$here/kv_bench.rill" -o "$out/kv_bench"
echo "== SQLite $("$sq/bin/sqlite3" --version | cut -d' ' -f1)"
DYLD_LIBRARY_PATH="$sq/lib" "$out/sqlite_bench" "$n"
echo "== Rill lib/kv.rill"
"$out/kv_bench" "$n"

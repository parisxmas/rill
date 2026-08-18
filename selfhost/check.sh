#!/bin/bash
# Proves the self-hosted front end against the Rust one: for every Rill file
# in the repository, the output of the Rill-written formatter must match
# `rill fmt --stdout` byte for byte. One divergent file is one bug.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
rill="$root/target/release/rill"
"$rill" build "$root/selfhost/fmt.rill" -o "$root/selfhost/selffmt" || exit 1
ok=0; bad=0
for f in $(find "$root/lib" "$root/examples" "$root/benchmarks" "$root/selfhost" "$root/book/code" \
             -name '*.rill' 2>/dev/null | sort) "$root/crates/rill_syntax/src/prelude.rill"; do
  [ -f "$f" ] || continue
  if "$root/selfhost/selffmt" "$f" 2>/dev/null | diff -q - <("$rill" fmt "$f" --stdout 2>/dev/null) >/dev/null 2>&1; then
    ok=$((ok+1))
  else
    bad=$((bad+1)); echo "DIVERGES: $f"
  fi
done
echo "byte-identical: $ok   diverging: $bad"
[ $bad -eq 0 ]

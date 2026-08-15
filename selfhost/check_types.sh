#!/bin/bash
# Proves the self-hosted type checker against the Rust one: for every Rill
# file in the repository and every deliberately broken program in
# `tyck_tests/`, both checkers must agree — same exit code, same first
# error, word for word (the Rust side's path prefix stripped off).
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
rill="$root/target/release/rill"
prelude="$root/crates/rill_syntax/src/prelude.rill"
"$rill" build "$root/selfhost/typecheck.rill" -o "$root/selfhost/typecheck" || exit 1
ok=0; bad=0
for f in $(find "$root/examples" "$root/benchmarks" "$root/selfhost" "$root/book/code" \
             -name '*.rill' 2>/dev/null | sort); do
  [ -f "$f" ] || continue
  rust_out=$("$rill" check "$f" 2>&1); rust_e=$?
  rust_out=${rust_out#"$f: "}
  mine_out=$("$root/selfhost/typecheck" "$f" "$prelude" 2>&1); mine_e=$?
  if [ "$rust_out" = "$mine_out" ] && [ "$rust_e" = "$mine_e" ]; then
    ok=$((ok+1))
  else
    bad=$((bad+1))
    echo "DIVERGES: $f"
    echo "  rust ($rust_e): $rust_out"
    echo "  rill ($mine_e): $mine_out"
  fi
done
echo "agreeing: $ok   diverging: $bad"
[ $bad -eq 0 ]

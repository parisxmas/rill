#!/bin/bash
# Proves the self-hosted code generator against the Rust one, by behavior:
# every program in `rillc_tests/` is built both ways — `rillc` emitting LLVM
# IR linked by cc, and `rill build` — and must print the same bytes and exit
# the same way.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
rill="$root/target/release/rill"
runtime="$root/target/release/librill_runtime.a"
prelude="$root/crates/rill_syntax/src/prelude.rill"
work="${TMPDIR:-/tmp}/rillc-check-$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
"$rill" build "$root/selfhost/rillc.rill" -o "$root/selfhost/rillc" || exit 1
ok=0; bad=0
for f in "$root"/selfhost/rillc_tests/*.rill; do
  n=$(basename "$f" .rill)
  if ! "$root/selfhost/rillc" "$f" "$prelude" > "$work/$n.ll" 2>"$work/$n.rc.err"; then
    bad=$((bad+1)); echo "RILLC FAILS: $n"; sed 's/^/  /' "$work/$n.rc.err"; continue
  fi
  if ! cc -O2 -Wno-override-module "$work/$n.ll" "$runtime" -o "$work/$n.self" 2>"$work/$n.cc.err"; then
    bad=$((bad+1)); echo "IR REJECTED: $n"; sed 's/^/  /' "$work/$n.cc.err" | head -5; continue
  fi
  "$rill" build "$f" -o "$work/$n.rust" >/dev/null 2>&1 || { bad=$((bad+1)); echo "RUST BUILD FAILS: $n"; continue; }
  a=$("$work/$n.self" 2>&1); ae=$?
  b=$("$work/$n.rust" 2>&1); be=$?
  if [ "$a" = "$b" ] && [ "$ae" = "$be" ]; then
    ok=$((ok+1))
  else
    bad=$((bad+1)); echo "DIVERGES: $n"
    echo "  self ($ae): $(echo "$a" | head -3)"
    echo "  rust ($be): $(echo "$b" | head -3)"
  fi
done
echo "matching: $ok   diverging: $bad"
[ $bad -eq 0 ]

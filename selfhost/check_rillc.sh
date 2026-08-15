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
  la=$(RILL_DEBUG_ALLOC=1 "$work/$n.self" 2>&1 >/dev/null | grep "live allocations" || true)
  lb=$(RILL_DEBUG_ALLOC=1 "$work/$n.rust" 2>&1 >/dev/null | grep "live allocations" || true)
  # Clean exit must be leak-free; a strand still running at exit legitimately
  # holds its stack, so there both sides must simply agree on that fact.
  if [ -z "$la" ]; then
    : # the program left through exit(); no report to hold it to
  elif echo "$la" | grep -q "still running"; then
    echo "$lb" | grep -q "still running" || { bad=$((bad+1)); echo "LEAKS: $n (self abandons strands, rust does not)"; continue; }
  elif ! echo "$la" | grep -q ": 0"; then
    bad=$((bad+1)); echo "LEAKS: $n ($la)"; continue
  fi
  if [ "$a" = "$b" ] && [ "$ae" = "$be" ]; then
    ok=$((ok+1))
  else
    bad=$((bad+1)); echo "DIVERGES: $n"
    echo "  self ($ae): $(echo "$a" | head -3)"
    echo "  rust ($be): $(echo "$b" | head -3)"
  fi
done
echo "matching: $ok   diverging: $bad"

# The self-hosting proof: rillc compiles itself; the self-built compiler
# compiles rillc again; the two IR files must be byte-identical. Then the
# self-built compiler builds the formatter, which must agree with rill fmt
# over the whole corpus.
"$root/selfhost/rillc" "$root/selfhost/rillc.rill" "$prelude" > "$work/rillc2.ll" 2>&1 \
  && cc -O2 -Wno-override-module "$work/rillc2.ll" "$runtime" -o "$work/rillc2" 2>/dev/null \
  && "$work/rillc2" "$root/selfhost/rillc.rill" "$prelude" > "$work/rillc3.ll" 2>&1 \
  && cmp -s "$work/rillc2.ll" "$work/rillc3.ll" \
  && echo "fixpoint: rillc(rillc) == rillc2(rillc)" || { echo "FIXPOINT BROKEN"; bad=$((bad+1)); }
selfleak=$(RILL_DEBUG_ALLOC=1 "$work/rillc2" "$root/selfhost/rillc.rill" "$prelude" 2>&1 >/dev/null | grep "live allocations" || true)
echo "self-compilation heap at exit: ${selfleak:-unreported}"
echo "$selfleak" | grep -q ": 0" || { echo "SELF-COMPILATION LEAKS"; bad=$((bad+1)); }
"$work/rillc2" "$root/selfhost/fmt.rill" "$prelude" > "$work/fmt2.ll" 2>&1 \
  && cc -O2 -Wno-override-module "$work/fmt2.ll" "$runtime" -o "$work/fmt2" 2>/dev/null || { echo "FMT BUILD FAILS"; bad=$((bad+1)); }
fok=0; fbad=0
for f in $(find "$root/examples" "$root/benchmarks" "$root/selfhost" "$root/book/code" \
             -name '*.rill' 2>/dev/null | sort) "$prelude"; do
  [ -f "$f" ] || continue
  if "$work/fmt2" "$f" 2>/dev/null | diff -q - <("$rill" fmt "$f" --stdout 2>/dev/null) >/dev/null 2>&1; then
    fok=$((fok+1))
  else
    fbad=$((fbad+1))
  fi
done
echo "formatter built by self-built rillc: byte-identical: $fok   diverging: $fbad"
bad=$((bad+fbad))

# Every runnable example, both compilers, output- and leak-diffed.
# `callbacks` needs C function-pointer callbacks (not yet emitted) and
# `echo_server` blocks forever; both sit out.
eok=0; ebad=0
for f in "$root"/examples/*.rill; do
  n=$(basename "$f" .rill)
  case "$n" in callbacks|echo_server) continue;; esac
  "$rill" build "$f" -o "$work/e.$n.rust" >/dev/null 2>&1 || continue
  "$root/selfhost/rillc" "$f" "$prelude" > "$work/e.$n.ll" 2>/dev/null \
    && cc -O2 -Wno-override-module "$work/e.$n.ll" "$runtime" -o "$work/e.$n" 2>/dev/null \
    || { ebad=$((ebad+1)); echo "EXAMPLE FAILS TO BUILD: $n"; continue; }
  a=$(timeout 30 "$work/e.$n" 2>&1); ae=$?
  b=$(timeout 30 "$work/e.$n.rust" 2>&1); be=$?
  la=$(RILL_DEBUG_ALLOC=1 timeout 30 "$work/e.$n" 2>&1 >/dev/null | grep "live allocations" || true)
  if [ "$a" = "$b" ] && [ "$ae" = "$be" ]; then
    if [ -n "$la" ] && ! echo "$la" | grep -qE ": 0|still running"; then
      ebad=$((ebad+1)); echo "EXAMPLE LEAKS: $n ($la)"
    else
      eok=$((eok+1))
    fi
  else
    ebad=$((ebad+1)); echo "EXAMPLE DIVERGES: $n"
  fi
done
echo "examples, both compilers: $eok matching and leak-free, $ebad failing"
bad=$((bad+ebad))
[ $bad -eq 0 ]

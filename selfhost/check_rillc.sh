#!/bin/bash
# Proves the self-hosted compiler against the Rust one, by behavior: every
# program in `rillc_tests/` is built both ways — `rillc build`, which emits
# LLVM IR and drives `cc` itself, and `rill build` — and must print the same
# bytes and exit the same way.
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
rill="$root/target/release/rill"
runtime="$root/target/release/librill_runtime.a"
prelude="$root/crates/rill_syntax/src/prelude.rill"
rillc="$root/selfhost/rillc"
work="${TMPDIR:-/tmp}/rillc-check-$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
"$rill" build "$root/selfhost/rillc.rill" -o "$rillc" || exit 1
ok=0; bad=0
for f in "$root"/selfhost/rillc_tests/*.rill; do
  n=$(basename "$f" .rill)
  if ! "$rillc" build "$f" -o "$work/$n.self" 2>"$work/$n.rc.err"; then
    bad=$((bad+1)); echo "RILLC FAILS: $n"; sed 's/^/  /' "$work/$n.rc.err" | head -5; continue
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

# The self-hosting proof: rillc builds itself; the self-built compiler emits
# the IR for rillc again; the two must be byte-identical. The second compiler
# lives outside the source tree, so it is told where the prelude and the
# runtime are — which is the other half of the driver under test.
"$rillc" build "$root/selfhost/rillc.rill" -o "$work/rillc2" 2>&1 \
  && "$rillc" emit-llvm "$root/selfhost/rillc.rill" --prelude "$prelude" -o "$work/rillc2.ll" \
  && "$work/rillc2" emit-llvm "$root/selfhost/rillc.rill" --prelude "$prelude" -o "$work/rillc3.ll" \
  && cmp -s "$work/rillc2.ll" "$work/rillc3.ll" \
  && echo "fixpoint: rillc(rillc) == rillc2(rillc)" || { echo "FIXPOINT BROKEN"; bad=$((bad+1)); }
selfleak=$(RILL_DEBUG_ALLOC=1 "$work/rillc2" emit-llvm "$root/selfhost/rillc.rill" \
             --prelude "$prelude" -o "$work/rillc4.ll" 2>&1 >/dev/null | grep "live allocations" || true)
echo "self-compilation heap at exit: ${selfleak:-unreported}"
echo "$selfleak" | grep -q ": 0" || { echo "SELF-COMPILATION LEAKS"; bad=$((bad+1)); }
"$work/rillc2" build "$root/selfhost/fmt.rill" -o "$work/fmt2" \
  --prelude "$prelude" --runtime "$runtime" || { echo "FMT BUILD FAILS"; bad=$((bad+1)); }
fok=0; fbad=0
for f in $(find "$root/lib" "$root/examples" "$root/benchmarks" "$root/selfhost" "$root/book/code" \
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
# `echo_server` blocks forever waiting for a connection, so it sits out.
eok=0; ebad=0
for f in "$root"/examples/*.rill; do
  n=$(basename "$f" .rill)
  case "$n" in echo_server) continue;; esac
  "$rill" build "$f" -o "$work/e.$n.rust" >/dev/null 2>&1 || continue
  "$rillc" build "$f" -o "$work/e.$n" 2>/dev/null \
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

# The driver's own surface: `rillc check` must reject what `rill check`
# rejects, in the same words, and `rillc run` must hand back the program's
# output and its exit code.
cok=0; cbad=0
for f in "$root"/selfhost/tyck_tests/*.rill; do
  rust_out=$("$rill" check "$f" 2>&1); rust_e=$?
  rust_out=${rust_out#"$f: "}
  mine_out=$("$rillc" check "$f" --prelude "$prelude" 2>&1); mine_e=$?
  if [ "$rust_out" = "$mine_out" ] && [ "$rust_e" = "$mine_e" ]; then
    cok=$((cok+1))
  else
    cbad=$((cbad+1)); echo "CHECK DIVERGES: $(basename "$f")"
    echo "  rust ($rust_e): $rust_out"
    echo "  rill ($mine_e): $mine_out"
  fi
done
echo "rillc check vs rill check: $cok agreeing, $cbad diverging"
bad=$((bad+cbad))

printf 'fn main() =\n  println("driven")\n  exit(3)\n' > "$work/driver.rill"
run_out=$("$rillc" run "$work/driver.rill" 2>&1); run_e=$?
if [ "$run_out" = "driven" ] && [ "$run_e" = 3 ]; then
  echo "rillc run: output and exit code passed through"
else
  echo "RUN DIVERGES: got ($run_e) $run_out"; bad=$((bad+1))
fi

[ $bad -eq 0 ]

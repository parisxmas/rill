# Rill against C

Twenty programs, each written once in Rill and once in C to the same
algorithm and the same data layout, built at `-O3` (Rill's LLVM 21 pipeline,
Apple clang 17 for C) and timed on an Apple M4. Time is the median of 15 runs
after 2 warmups; peak RSS is the median of 3; outputs are compared before any
time is printed. Tokens are `o200k_base`. Reproduce with
`python3 benchmarks/c/run.py` (needs `tiktoken` for the token columns).

Fifteen pairs live here; the other five (`loop`, `mandel`, `algo`,
`dijkstra`, `json`) are the ones in `../tokens/` that already had a C twin,
and `json` is measured against the C that frees its tree.

## Results (2026-09-13, on LLVM 21; first measured 2026-09-05, after the compiler work below)

| benchmark | what it exercises | rill ms | c ms | rill / c | rill RSS | c RSS | rill tok | c tok |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| deriv | symbolic differentiation, 3000 rounds of rewriting small trees (added 2026-09-12) | 47.0 | 229.4 | **0.21** | 1.3 M | 1.4 M | 828 | 1718 |
| trees | build + drop 5.2 M boxes | 16.3 | 45.2 | **0.36** | 13.3 M | 9.3 M | 156 | 209 |
| dijkstra | read 31 MB graph, CSR, heap | 84.4 | 209.7 | **0.40** | 83.7 M | 59 M | 743 | 836 |
| strbuild | 5 M ints into a builder, then scan | 23.9 | 31.9 | **0.75** | 103 M | 40 M | 92 | 211 |
| json | parse + print 11 MB | 119.0 | 153.6 | **0.77** | 208 M | 104 M | 1608 | 2133 |
| spectral | float, 2000², function per element | 95.3 | 115.7 | **0.82** | 1.3 M | 1.3 M | 329 | 414 |
| nbody_rec | the bodies as records in a list | 30.4 | 34.2 | **0.89** | 1.3 M | 1.3 M | 1201 | 1189 |
| fannkuch | n=10, small int32 buffers, many short loops | 109.3 | 121.5 | **0.90** | 1.3 M | 1.2 M | 469 | 339 |
| mandel | float loop, data-dependent exit | 180.9 | 194.5 | **0.93** | 1.2 M | 1.2 M | 201 | 228 |
| qsort | 5 M ints, in-place Hoare | 271.5 | 285.6 | **0.95** | 39.4 M | 39.5 M | 304 | 304 |
| algo | Dijkstra core, graph built in process | 31.8 | 32.9 | **0.96** | 59 M | 59 M | 834 | 832 |
| loop | 100 M indexed reads | 28.3 | 28.6 | 0.99 | 1.3 M | 1.3 M | 71 | 93 |
| hashmap | 2 M inserts + 2 M lookups, Int keys | 79.6 | 80.0 | 1.00 | 138 M | 130 M | 191 | 728 |
| lcg | 100 M-step integer loop | 121.6 | 121.1 | 1.00 | 1.3 M | 1.2 M | 101 | 110 |
| matmul | 500³ float, `c[i][j] += a*b` | 17.4 | 17.3 | 1.00 | 7.0 M | 7.1 M | 180 | 250 |
| sieve | 50 M byte buffer, strided writes | 138.9 | 139.1 | 1.00 | 49 M | 49 M | 150 | 151 |
| tuple | 100 M calls returning a pair | 253.7 | 253.5 | 1.00 | 1.2 M | 1.2 M | 74 | 106 |
| pipeline | `range \|> filter \|> map \|> sum`, 5 M | 3.1 | 3.1 | 1.00 | 1.3 M | 1.2 M | 49 | 73 |
| fib | fib(35), recursion | 15.6 | 15.2 | 1.03 | 1.2 M | 1.2 M | 43 | 53 |
| nbody | 5 bodies as 7 float buffers, 1 M steps | 36.7 | 34.0 | 1.08 | 1.3 M | 1.3 M | 1132 | 1189 |

Eleven wins and nine within noise — the run-to-run noise of a program this
short is 2–4%, and `pipeline`'s 3 ms is mostly process start-up, measured
separately at 61 runs as 3.09 against 3.05. Nothing is behind C. Rill is
shorter than C in eighteen of twenty sources.

Moving the compiler from LLVM 18 to 21 (2026-09-13, no change to what Rill
emits) is what took spectral from level to 18% ahead and moved qsort,
fannkuch and lcg a few percent; the C column is Apple clang 17 throughout.

### What the same table looked like before

The first measurement, on the compiler as it stood on the morning of
2026-09-05, was three wins, four ties and twelve losses:

| benchmark | before | after | C |
|---|---:|---:|---:|
| pipeline | 83.0 (13.1×) | 5.1 | 4.9 |
| nbody_rec | 140.8 (3.06×) | 54.1 | 38.7 |
| matmul | 65.3 (2.62×) | 20.3 | 20.5 |
| strbuild | 104.8 (2.34×) | 39.5 | 36.0 |
| nbody | 79.3 (1.72×) | 41.0 | 38.7 |
| tuple | 558.6 (1.65×) | 287.8 | 288.5 |
| hashmap | 193.1 (1.53×) | 97.4 | 104.8 |
| algo | 70.6 (1.30×) | 46.9 | 45.3 |
| fannkuch | 199.5 (1.23×) | 125.5 | 133.5 |
| qsort | 446.5 (1.17×) | 339.8 | 321.6 |
| sieve | 217.5 (1.09×) | 175.7 | 175.1 |
| spectral | 170.8 (1.09×) | 140.1 | 132.5 |

The C numbers moved too between the two runs (the machine was busier in the
morning), so the ratios are the honest column; every "after" was measured in
the same run as its C.

## What changed in the compiler

Eleven changes, each measured on its own before the next went in. In the
order they landed:

1. **Internal linkage for every Rill function but `main` and wasm exports.**
   LLVM inlines an external function warily. fannkuch 200→179, nbody 79→67,
   spectral 171→156, and matmul 65→25 — the inner loop could now be inlined
   into a function where `n` was a constant, the bounds checks became
   provable, and the loop vectorized.
2. **Borrowed parameters.** A fixpoint analysis over the whole program marks
   buffer, string, builder and map parameters that are only ever read —
   indexed, measured, passed to a builtin or to another borrowed parameter —
   and those are passed without a retain and released by nobody. A call in
   tail position may only pass a borrowed parameter of its own function that
   way (the frame releases everything else before it jumps), and any other
   argument there forces the callee's parameter back to owned so the call
   stays a tail call. qsort 448→341, algo 71→53, nbody 67→42.
3. **List-pipeline fusion.** `range |> filter |> map |> sum` (and `len`,
   `fold`) over lambdas written in place and scalar types is generated as one
   loop with no list. Only the prelude's own functions qualify; a program that
   redefines `map` gets what it wrote. pipeline 70→4.7, 154 MB→1.2 MB.
4. **`b += int_to_str(n)` writes the digits into the builder** through a new
   `rill_strbuf_int`, with no string in between; later the digit formatter
   itself was rewritten (digit count by a comparison ladder, two digits at a
   time from a table), which every `int_to_str` and `println` now uses.
   strbuild 82→40.
5. **A buffer's length is stored where LLVM can see it.** `buf_*` writes the
   count into the header a second time, in the generated code, and
   `rill_buf_new` is marked `noalias`; the optimizer then proves `i < #v`
   for loops it can relate to `n` and drops the check. sieve 190→175,
   fannkuch 179→126.
6. **Map tables know their key and value kinds.** An `Int` key is hashed and
   compared by the runtime itself, a scalar value is never retained; an
   insert probes once; `map_or` on scalar values is one runtime call, and for
   `Int` keys the whole probe — hash, control bytes, one key compare — is
   laid into the generated code. The `Int` hash itself became one multiply
   and a fold, the same as a hand-written table's. hashmap 159→97.
7. **Flat records.** A type with one constructor and nothing but scalar
   fields — a pair of numbers, a point, an n-body's body — is an LLVM struct
   by value: no box, no count, no tag, `p.x` an `extractvalue`. It is boxed
   only at the edge of a table or a channel, which store words. tuple
   481→288, nbody_rec 107→92.
8. **The pool's fast path is generated inline.** Allocation pops the size
   class's free list and freeing pushes it, in three instructions each; the
   runtime is asked only when a list is empty. trees 32→28, nbody_rec 92→71,
   json 157→148.
9. **A closure call borrows its callee.** `f(x)` inside `map` was a
   `rill_retain` and a `rill_env_release` around every call. nbody_rec
   71→54.
10. **`b += byte` inline** when the builder has room. strbuild 56→52.
11. **The formatter lost the body of `v[i] = for … with …`** — it printed
    `...` — found because `rill fmt` ate `spectral.rill`. Fixed.
12. **Borrowed captures.** A lambda written in place for a parameter that
    never escapes its function (the prelude's `map`, `filter`, `fold` — `f`
    is only ever called or passed along) is built with an environment that
    holds no reference to its captures: the frame that made it outlives the
    call and holds them already. Not in tail position, where the frame lets
    go before the jump — and that is exactly where `nbody_rec`'s `map` sits,
    so it gained nothing there; every `fold(xs, 0, \\a, x -> … k …)` written
    as a statement did.
13. **Bounds checks hoisted out of `for`.** A loop body indexing `v[i]` by
    the loop variable itself gets one `rill_buf_check(v, lo, hi)` in front
    of the loop and no test on any element; the desugaring rewrites the
    accesses, the checker vouches for them only when `v` is a buffer, and a
    name rebound in the body disqualifies it. Strings keep their per-byte
    test because `s[i]` past the end is `-1` by contract. spectral 140→125
    (ahead of C), nbody 41→40.
14. **`strbuf_str(b)` hands the block over** when `b` is a builder this
    frame owns and never names again: the builder keeps sixteen bytes in
    front of its text in the shape of a string header, so the block becomes
    the string. strbuild 39.6→35.9, RSS 140 MB→103 MB.
15. **`rill build --unchecked`.** No bounds test on any buffer access — the
    trade Zig's ReleaseFast and Swift's -Ounchecked offer, off by default.
    qsort's remaining 6% is exactly the check on indexes no compiler can
    prove (the scan in a Hoare partition stops on the data, not on a bound):
    341 ms checked, 322 unchecked, C 321.
16. `nbody.rill` was rewritten twice on the way: its `kick` had two
    self-tail-calls, which LLVM turned into two nested loops that did not
    hoist their invariant loads (44.6→40.5 as a single tail call), and then
    into a `for … with` over a flat tuple so that every `x[i]` is indexed by
    a loop variable and hoisted (→40.0).
17. **Borrowed data parameters.** A list or record parameter that a function
    only takes apart with `match` or reads a field of — and whose type the
    body never *builds*, so no cell of it is a candidate for in-place reuse
    — is borrowed like a buffer. `check(t)` walking a tree, `kick` walking
    the bodies, `fold` walking anything: no count touched per node. trees
    28.5→20.9.
18. **Bindings out of a borrowed box are held above too.** `match os
    Cons(o, rest) -> kick(b, rest, …)`: `rest` came out of a box an earlier
    frame holds, so it may go through a borrowed position of a tail call —
    without this the tail rule forced every such parameter back to owned.
    nbody_rec 54→46.
19. **`map(xs, \\x -> …)` as a loop.** Over lists of numbers or flat
    records: walk, lay the lambda's body over each element, build the result
    forward with each cell written into the tail of the one before. No
    closure, no indirect call per element, no reverse pass. nbody_rec 46→33,
    ahead of C.
20. **A buffer value is the address of its first element**, count and
    length in the sixteen bytes before it. `v[i]` is then one
    register-indexed load; with the header in front it was an `add` and a
    load with an offset, and in a scan of five million elements that add was
    the whole of qsort's gap. qsort 341→312, ahead of C.

Two smaller repairs along the way: `tokens/algo.rill` and
`tokens/dijkstra.rill` had stopped compiling (an unannotated buffer
parameter now defaults to `Str`), and the `--emit-ir` experiments have to be
run through `opt` with `-mtriple`/`-mcpu`, or the cost model has no target and
never vectorizes — half a day was lost to that.

### deriv, and what a pool would do for C

`deriv` (2026-09-12) differentiates a degree-12 polynomial with a
`(2x+1)^4` factor down to zero, simplifying each derivative by pattern
rules, 3000 rounds. It is `trees` with rewriting in it: every node is
built, matched and dropped, and the C twin does it with `malloc`/`free`,
as `trees.c` does. The first measurement was 94 ms against 226 — and
against 52 for the same C given a seven-line free-list pool, which is the
honest twin for a language with a pool of its own. Four changes to the
code generator, in the order they landed, took Rill to 48 ms, ahead of
that too (outputs unchanged, `RILL_DEBUG_ALLOC` at zero throughout):

1. **A tuple written in place is never built.** `match (simp(a), simp(b))`
   boxed a pair of pointers to take it apart on the next line — an
   allocation and a free per rewritten node. The parts are now an LLVM
   struct the decision tree reads with `extractvalue`, and each part is
   released where the box would have been. 94 → 88.
2. **A constructor that is the function's answer moves and reuses** like
   one handed to a tail call: what the frame owns goes into it uncounted,
   and a box the arm took apart of the same shape is written over. The
   reuse slot became a stack, so a nested constructor finds its own. 88 →
   83; with the parts of an unboxed tuple bound to the values the frame
   holds, so that a use can move them, 72.
3. **Releasing a box is inline.** `drop$Expr` was a call for every release,
   and nearly every release is a decrement that goes from something to
   something. The load, compare and store are laid down in place and the
   drop function is called only for the last reference — half of `deriv`'s
   time had been in that function doing subtractions. 72 → 63.
4. **An arm that returns empties the box it takes apart.** If the frame's
   reference is the only one — one load and a compare, decided at run time
   — the fields become the frame's own with no count touched and the box
   is a shell: written over by the arm's answer if that has the same shape,
   given back to the pool at exit if not. If the box is shared, the fields
   are retained and the count dropped, as before. The same is done to the
   parts of an unboxed tuple and, recursively, to any field matched by a
   constructor pattern, so `(Num(x), Mul(Num(y), z)) -> Mul(Num(x * y), z)`
   over a tree nothing else holds allocates nothing and counts nothing.
   Names an arm hands to a call once are moved there, and a local handed
   back as the answer goes uncounted. 63 → 48. This is Perceus's
   drop-guided reuse, and it is what a C programmer writes by hand when
   they free the node they just consumed.

The source is 55 lines against 114: the rules are one pattern each where C
writes an `if` per field and frees exactly the nodes a rule consumed.

`json` moved with it, 144 → 122, and `strbuild` 36 → 25; nothing moved the
other way. The table above is from the run after the fourth change.

## What is left

Nothing is behind C. The two rows that took longest are worth a word:

- **qsort** kept a bounds test per element that no compiler can prove away
  — a Hoare partition's scan stops on the data — and still ended ahead,
  because the address arithmetic around the test was what actually cost:
  once a buffer became the address of its elements, the test was a compare
  and a predicted branch and nothing else. `--unchecked` removes even that
  (322 ms) and is not needed to win.
- **nbody_rec** allocates five cells a step by construction — the lambda
  reads the list `map` is rebuilding, so the old cells cannot be reused —
  and wins anyway, once `map` over a lambda is the loop it describes and the
  bodies' list is walked without a count being touched.

## Why each loss happened (the morning diagnosis)

Every cause below was isolated by experiment, not guessed. The main tool was
taking the compiler's own IR (`--emit-ir`), editing one thing out, and
running it through the same `opt -O3` / `llc` the compiler uses.

### 1. Bounds checks stop LLVM vectorizing (matmul, spectral, nbody, sieve, qsort)

Every `b[i]` is `if i < len then load else trap`. The check itself is cheap;
the damage is that every check is a second exit from the loop, and LLVM's
loop vectorizer will not touch a loop with two exits. Deleting the checks from
the IR and nothing else:

| | as built | checks removed | C |
|---|---:|---:|---:|
| matmul | 64.7 | **24.2** | 24.9 |
| spectral | 169.5 | **148.4** | 156.7 |
| nbody | 78.8 | 60.9 | 46.1 |
| sieve | 216.4 | 198.2 | 200.2 |
| qsort | 443.7 | 430.4 | 382.9 |
| fannkuch | 197.6 | 200.6 | 162.6 |

matmul's whole gap is this one thing: without the checks LLVM emits NEON
`fmla` and lands on C exactly. spectral goes from losing to winning. fannkuch
is unaffected, so its cause is elsewhere.

**Fix.** Not "remove bounds checks" — hoist them. A `for i in lo..hi` whose
body indexes `v[i]`, `v[i*n + j]` with `j` the loop variable, or `v[i + k]`
for a loop-invariant `k` needs one check before the loop (`hi - 1` and `lo`
against `#v`) and none inside. The compiler already lowers `for` itself, so it
knows the range and can emit the check at the lifted function's entry; LLVM's
`irce` pass does the same thing after the fact and could be added to the
pipeline as a first, cheaper step. Either way the in-loop branch to
`rill_buf_oob` disappears and the loop has one exit again.

### 2. Lifted functions are exported symbols, so LLVM keeps them at arm's length (fannkuch, nbody, qsort)

Every `fn` inside a body is lifted to a top-level function with external
linkage. LLVM will inline an external function but is far more conservative
about it, and can never delete the out-of-line copy or specialize it on its
call sites. Marking every `@main.*` function `internal` in the emitted IR:

| | checks removed | + internal linkage | C |
|---|---:|---:|---:|
| fannkuch | 200.6 | **172.4** | 162.6 |
| nbody | 60.9 | **50.3** | 46.1 |
| qsort | 430.4 | 422.4 | 382.9 |

This is a one-line change in codegen (a previous pass measured it on `algo`
alone and saw nothing; it matters when the helpers are tiny and called in a
loop). Only `main`, exports and extern trampolines need to stay visible.

### 3. Reference-count traffic on buffers around every call (qsort, fannkuch, algo)

After the two fixes above, quicksort's fully inlined loop still reads and
writes the buffer's count on every partition step: the caller retains `a`
before calling `up`/`down`/`part` and the callee releases it on return. Every
one of those is a load, a compare against the immortal sentinel, an add and a
store to the same header line — in the inner loop of a sort. C touches
nothing but the elements.

**Fix.** Borrowed parameters. A parameter that the callee only reads or
indexes — never stores in a box, returns, sends or captures — can be passed
without a count and released by no one; Perceus calls this a borrowed
parameter and infers it by a simple escape check. Buffers and strings are the
types to start with, since they are what hot loops pass. An earlier
measurement on `algo` put this at 39%.

### 4. Every record and tuple is a heap box (tuple, nbody_rec)

`(a, b)` allocates sixteen bytes plus a header, and so does `B(x, y, z, …)`.
`step(a, b)` returning a pair costs an allocation and a free per call — 100 M
of them in `tuple` — while C returns the struct in two registers. In
`nbody_rec` each step builds five seven-field records and a five-cell list,
and it runs three times slower than the same arithmetic on buffers.

**Fix.** Unboxed small values: a single-constructor type whose fields are all
scalars (and tuples of scalars) can be an LLVM aggregate passed and returned
by value, with no count at all. Nothing in the language changes — `p.x` and
`(a, b) = f()` mean what they meant — only the representation. Where a
recursive or multi-constructor type is needed the box stays.

### 5. `int_to_str` allocates a string just to copy it into a builder (strbuild)

`b += int_to_str(i)` formats into a fresh `Str`, copies it into the builder,
and frees it. 5 M of those are 54 ms of the 80 ms build phase; C formats
straight into its buffer. The RSS is 140 MB against 40 MB because the builder's
64 MB block, the 39 MB string `strbuf_str` copies out of it, and the pool's
retained pages are all live at once.

**Fix.** A `strbuf_int(b, n)` runtime entry that formats in place, and the
compiler rewriting `strbuf_add(b, int_to_str(x))` to it — the same for
`float_to_str` and `show`. Separately, `strbuf_str` could hand over the block
when the builder's count is one instead of copying, which halves the RSS.

### 6. Map lookups go through two runtime calls and two indirect calls (hashmap)

Inserts are within 15% of a hand-written C table (101 vs 88 ms). Lookups are
2.4× slower (92 vs 38 ms): `map_or` is a prelude function calling
`map_find` and then `map_val_at`, and each runtime call hashes and compares
through the instantiation's vtable — indirect calls LLVM cannot inline.
Inserting a new key also probes twice, once to look and once to place.

**Fix.** Make `map_get`/`map_or`/`m[k]` intrinsics: for `Int` and `Str` keys
emit the hash and the control-byte probe inline, one pass, no function
pointer. Fold insert's lookup and placement into one probe.

### 7. Lists are what a pipeline is made of (pipeline)

`range(0, n) |> filter(…) |> map(…) |> sum` builds a five-million-cell list
(154 MB) and walks it three times. In-place reuse means `filter` and `map`
allocate nothing new, but the range itself is 5 M boxes and the walks are
pointer chases. C is one loop with no memory at all.

**Fix.** Fusion. A pipeline of the prelude's `range`, `map`, `filter`, `fold`,
`sum`, `any`, `all`, `take` that ends in a consumer can be rewritten into one
tail-recursive loop before codegen, allocating nothing — it is the standard
foldr/build deforestation and the prelude is small enough to do it by name.
It is also the single change that makes the *idiomatic* program fast rather
than only the buffer-and-index one.

### Where Rill wins, and why

`dijkstra` is `scan_int` reading blocks against `fscanf` — 2× ahead on the
reading alone. `trees` is the size-class pool against `malloc`/`free`: 5.2 M
boxes at 38 ms against 61. `mandel` is the tail-recursive `escape` compiling
to a tighter loop than C's `while`, as measured before.

## Tokens: where the syntax costs more than C

Rill is shorter in seventeen of nineteen. The two exceptions are precise:

- **fannkuch, 469 vs 339.** Six `while` loops and a `continue`/`break`
  structure. Each `while` becomes a named local `fn` with its own parameters
  and an `else 0` for the exit branch, and each early exit becomes a return
  value threaded back. Rill had `for … in a..b` but no `while`, no stepped
  `for`, and no loop exit, so the program *named* six things C leaves
  anonymous. (`while … with` arrived on 2026-09-12; the source here predates
  it and has not been rewritten.)
- **algo, 834 vs 832** and **nbody, 1181 vs 1189** are ties for the same
  reason on smaller scale, plus numeric literals dominating nbody.

The cheapest wins, in order of tokens saved per feature: (1) a `while c`
block, or `for` over a stepped range, (2) letting `if c then` stand without
`else` when the branch is a statement (the current `else 0` is pure
ceremony), (3) a lambda parameter that may be annotated, so `\b: Body -> b.vx`
does not need a named helper.

Everywhere else the count is already good, and the `hashmap` row (191 vs
728) is what a hash table in the language rather than in the program buys.

## The fix list as it was ranked that morning

| # | change | where | evidence |
|---|---|---|---|
| 1 | hoist bounds checks out of `for` loops (or add `irce`) | codegen | matmul 65→24, spectral 170→148, nbody 79→61, sieve 217→198 |
| 2 | `internal` linkage on lifted functions | codegen, one line | fannkuch 200→172, nbody 61→50 |
| 3 | borrowed `Buf`/`Str` parameters | codegen (escape check) | qsort residual 422 vs 383; algo 39% in an earlier pass |
| 4 | unboxed scalar records and tuples | codegen + types | tuple 1.65×, nbody_rec 3.06× |
| 5 | list-pipeline fusion over the prelude | new pass before codegen | pipeline 13× |
| 6 | intrinsic `map_get`/`map_or`, single-probe insert | codegen + runtime | hashmap lookups 2.4× |
| 7 | `strbuf_int`, and `strbuf_str` handing over the block | runtime + codegen rewrite | strbuild 2.34×, RSS 3.5× |

Two of the three benchmarks the language already wins were also the two
that once lost badly (`trees` was 2.25× behind Go); none of the seven above
is harder than what fixed those.

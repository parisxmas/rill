# The self-hosted front end

Rill's lexer, parser and canonical printer, written in Rill. Together they
are `rill fmt` reimplemented in the language it formats — the first stage of
a compiler that does not need Rust.

| file | what it is |
|---|---|
| `token.rill` | the token set, with numbers and strings kept as raw source text |
| `lexer.rill` | the layout algorithm: an indent stack, silent inside parentheses |
| `ast.rill` | the syntax tree, `&&`/`||`/`!` already desugared to `if` as in Rust |
| `parser.rill` | recursive descent, `PR(result, rest)` threaded through |
| `printer.rill` | the canonical form, comments carried by line anchoring |
| `fmt.rill` | the entry point |
| `check.sh` | the proof: byte-for-byte against `rill fmt` over every `.rill` in the repo |

The claim is checkable in one command:

```sh
./selfhost/check.sh    # formats ~150 files with both implementations, diffs each pair
```

Two things made the port interesting. State threads through single
constructors (`Lx`, `FS`, `PR`) because the language has no mutation, and
every loop is a self-tail call because that is what a loop is. And one
polymorphic `die_any` handles every parse error: its recursive tail call —
never reached past `exit` — is what lets one function fail at every type.

## The type checker

The second stage: Rill's Hindley–Milner checker, in Rill. `typecheck.rill`
loads a program the way the compiler does — imports resolved depth-first,
aliased modules qualified, the prelude merged with shadowing — and runs the
same judgements: unification with occurs check, trait constraints,
dependency-ordered generalization, Maranget's usefulness test for match
exhaustiveness. First error only, in the Rust checker's exact words.

| file | what it is |
|---|---|
| `tyck_core.rill` | the type language, persistent substitution, unification |
| `tyck_ty.rill` | resolution, generalization and instantiation |
| `tyck_decl.rill` | ADTs, traits, impls, externs, provisional signatures |
| `tyck_infer.rill` | the judgement per expression form; exhaustiveness |
| `tyck_load.rill` | multi-file loading, aliasing, the prelude merge |
| `typecheck.rill` | the driver: silent on success, first error otherwise |
| `check_types.sh` | the proof, against `rill check` |
| `tyck_tests/` | deliberately broken programs, one error path each |

The proof is the same shape as the formatter's:

```sh
./selfhost/check_types.sh   # both checkers over every file + the error suite
```

Every repository file and every broken test must produce the same exit code
and the same first error, word for word. The only known divergence is path
canonicalization: Rust resolves symlinks through the OS, this port folds
`.`/`..` textually, so messages naming files can differ when symlinks are
involved.

## The code generator

The third stage: `rillc_gen.rill` compiles Rill — data types and `match`,
generics by monomorphization, lambdas and closures, tail calls, the scalar
built-ins — to LLVM IR text, ready for `cc out.ll librill_runtime.a`. It
runs the self-hosted checker first, so emission starts from a program known
to be well-typed; generic instantiations are recovered by matching the
checker's generalized signatures against argument types at each call, so no
typed tree is needed. Every value fits an 8-byte slot; `match` and `if`
results meet in a stack slot rather than a phi, which keeps arms the
checker left polymorphic (a `die` that never returns) well typed.

```sh
./selfhost/check_rillc.sh   # behavior parity, then the self-hosting proof
```

The proof has three tiers:

1. **Behavior**: thirty-nine programs build under both compilers and print
   the same bytes — among them a five-million-iteration tail loop, a
   million-deep mutual recursion and a million-element list fold.
2. **Fixpoint**: `rillc` compiles itself; the self-built compiler compiles
   `rillc.rill` again; the two IR files are byte-identical.
3. **Full circle**: the self-built compiler builds the self-hosted
   formatter, and that binary matches `rill fmt` byte-for-byte over all
   ~200 files in the repository. It builds the self-hosted type checker
   too, which then matches `rill check` word-for-word on the error suite.

Memory is precise: every box carries a reference count, every use of a
variable retains, every binding releases when its scope ends, and each
monomorphized data type gets a generated release function whose last field
frees by tail call — a million-long list unwinds in constant stack. The
harness runs every test and the self-compilation itself under
`RILL_DEBUG_ALLOC` and requires `live allocations: 0` at exit.

The language is covered: traits dispatch from the receiver's concrete
type to the impl's mangled function; `==` on data types goes through a
generated structural equality whose last field compares by tail call;
`show` uses the user's impl when one exists and a generated structural
form when none does; `Buf` maps to the runtime's counted buffers; and
channels, `spawn` and `select` speak the strand runtime's ABI directly —
one 8-byte word per message, an environment box per strand, the scheduler
entry point flipped on when any of it appears. C callbacks cross as the
address of a trampoline emitted while compiling — C keeps no environment, so
which function it is has to be settled by then, and a helper that forwards a
callback is emitted once per callback it is handed. A fourth proof tier runs
every runnable example under both compilers and requires identical output
and a clean heap; `echo_server`, which waits forever for a connection, is the
only one that sits out. The runtime stays Rust, as runtimes do.

## The driver

`rillc.rill` is the part that makes the above a compiler rather than a
library: it parses a command line, finds the prelude and the runtime, writes
the IR, and calls `cc`.

```sh
rillc build main.rill -o main    # check, emit IR, link — no Rust in the loop
rillc run main.rill              # and then run it, exit code and all
rillc emit-llvm main.rill        # the IR on stdout
rillc check main.rill            # the checker alone
```

The prelude and `librill_runtime.a` are looked for beside `rillc` and one
level up in the tree it was built in; `$RILL_PRELUDE`, `$RILL_RUNTIME`,
`--prelude` and `--runtime` override that in the usual order. Libraries come
out of the program's own `extern "lib"` declarations, in the linker's three
forms — `-lname`, `-framework Name`, and anything starting with `-` passed
through as written.

Reaching the linker at all takes four C functions the language has no
built-in for — `system`, `getenv`, `access`, `remove` — declared as plain
`extern`s, which is to say the driver uses nothing a Rill program could not.
Everything handed to the shell is single-quoted. `-O2` is load-bearing: the
emitted IR is unoptimized and it is the optimizer that turns `tail call`
into a jump, without which deep recursion overflows the stack. Success
returns from `main` rather than calling `exit`, so the runtime's allocation
report still runs — the leak proof depends on it.

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

The third stage, begun: `rillc.rill` compiles the scalar subset of the
language — Int, Float, Bool, Str, functions, blocks, `if`, tail calls and
the scalar built-ins — to LLVM IR text, ready for
`cc out.ll librill_runtime.a`. It runs the self-hosted checker first, so
every type is concrete before a line is emitted; only functions reachable
from `main` are generated, which is what lets the generic prelude sit
unused in a scalar program. Anything outside the subset dies with its name:
data types and `match`, lambdas, channels, `spawn`, generics. Heap strings
are not yet released — Perceus insertion arrives with the data-type
milestone.

```sh
./selfhost/check_rillc.sh   # both compilers over rillc_tests/, output-diffed
```

The proof here is behavioral: every test program builds both ways and must
print the same bytes and exit the same way. The suite includes a
five-million-iteration tail loop and a million-deep mutual recursion, so
`tail call` emission is load-bearing, not decorative.

With this, all three compiler stages exist in Rill: parse (proven
byte-identical), type-check (proven error-identical), and code generation
(proven behavior-identical on its subset). The runtime stays Rust, as
runtimes do.

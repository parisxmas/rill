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

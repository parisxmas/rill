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

## Paused: the type checker

`tyck_core.rill`, `tyck_ty.rill`, `tyck_decl.rill` and `tyck_infer.rill` are
the beginnings of the next stage — the type checker in Rill. Unification,
declaration processing and the inference engine are written and compile;
the module loader, prelude merge, driver and the parity harness are not.
Nothing here is verified yet. Resume by writing `tyck_load.rill` (a port of
`loader.rs` + `prelude.rs`) and a `typecheck.rill` driver, then diffing
against `rill build`'s type errors the way `check.sh` diffs the formatter.

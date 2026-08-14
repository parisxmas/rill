# Rill for Visual Studio Code

Syntax highlighting and editing support for [Rill](../../README.md) — a strict,
statically typed functional language that compiles to native executables.

## What it gives you

- **Highlighting** for every construct in the language: `fn`, `type`, `trait`,
  `impl`, `extern`, `import ... as`, `match` arms, `select` arms with
  `default`, lambdas (`\x, y -> ...`), the pipeline operator `|>`, `#`
  comments, and string escapes (an unknown escape such as `\q` is marked
  invalid, matching what the lexer rejects).
- **Distinct colours** for the parts the type system distinguishes: built-in
  types (`Int`, `Float`, `Bool`, `Str`, `Unit`, `Ptr`, `Chan`), prelude types
  and constructors (`Option`/`Some`, `Result`/`Ok`, `List`/`Cons`), built-in
  functions (`println`, `recv`, `str_find`, …), prelude functions (`map`,
  `fold`, `range`, …), user constructors, and module aliases in qualified
  names (`m.label`, `m.Meters`).
- **Layout-aware editing**: two-space indentation, spaces only (Rill rejects
  tabs), and automatic indent after a line ending in `=`, `->`, `select`, or
  `match x`, and after `type` / `trait` / `impl` headers.
- **Snippets**: `main`, `fn`, `fnb`, `type`, `match`, `if`, `trait`, `impl`,
  `select`, `extern`, `import`, `lam`.

## Installing

From this directory:

```sh
# development: symlink into your extensions folder and reload the window
ln -s "$PWD" ~/.vscode/extensions/rill

# or package it and install the .vsix
npx --yes @vscode/vsce package
code --install-extension rill-0.2.10.vsix
```

Cursor, Windsurf, and VSCodium read the same folder under their own
`~/.cursor/extensions`, `~/.windsurf/extensions`, `~/.vscode-oss/extensions`.

Reload the window (`Developer: Reload Window`) and open any `.rill` file. If
the language is not detected, pick it from the status bar or run
`Change Language Mode → Rill`.

## Files

| file | what it is |
|---|---|
| `package.json` | the manifest: language id `rill`, `.rill` extension, grammar, snippets, per-language editor defaults |
| `language-configuration.json` | comments, brackets, auto-closing pairs, word pattern, indentation rules |
| `syntaxes/rill.tmLanguage.json` | the TextMate grammar |
| `snippets/rill.json` | snippets |

## Working on the grammar

`Developer: Inspect Editor Tokens and Scopes` shows the scope under the cursor,
which is the fastest way to see why something is coloured as it is. The
keyword, built-in and prelude name lists come from `crates/rill_syntax/src/lexer.rs`,
`crates/rill_types/src/lib.rs` (`is_builtin_fn`, `is_builtin_type`) and
`crates/rill_syntax/src/prelude.rill`; when the language gains a name, update
the grammar to match.

`examples/showcase.rill` in the repository root exercises most of the language
and makes a good visual test.

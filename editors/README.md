# Editors

Three things live here. `vscode/` is the extension that colours Rill and
knows how it is indented. `web/` is the model editor — a page drawing a
file's `system` and `machine`s as SDL drew them, beside the code, editing
them as code written back through the formatter, and replaying a
scenario's run on them with the sequence chart beside, with a server in
Rill that runs the compiler for it; `rill run
editors/web/server.rill` starts it on <http://127.0.0.1:8765> — this
machine alone, since it runs programs on request — the current directory
as its root; `--port`, `--root`, `--rill` and `--host` say otherwise. It
keeps a `rill lsp` alive beside it for hover and go-to-definition.
The third is not a file at all: `rill lsp` is a language server built into
the compiler, and any editor that speaks the protocol can start it without
an extension of its own.

## What the server answers

| | |
|---|---|
| diagnostics | every error one pass can find, on the buffer as it is — nothing has to be saved |
| hover | the type of the name under the cursor: a function's signature with the requirements its body imposed, a constructor, a local, a parameter |
| go to definition | a name declared in this file |
| outline | the file's functions, types and traits |
| format | `rill fmt` over the buffer |

Hover is the one worth having. Rill infers everything, so a signature is
usually somewhere the program does not say it, and this is where it is said —
`fn twice(n: Int) -> Int` over a line that reads `fn twice(n) = n * 2`, and
`fn merge(a: List('a), b: List('a)) -> List('a)    # where 'a is ordered` over
one that says nothing about being ordered at all.

`rill types <file.rill>` prints the same signatures for a whole file, for
when the question is about a file rather than a name.

## Wiring it up

**Neovim** — no plugin needed:

```lua
vim.filetype.add({ extension = { rill = "rill" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "rill",
  callback = function(args)
    vim.lsp.start({
      name = "rill",
      cmd = { "rill", "lsp" },
      root_dir = vim.fs.dirname(args.file),
    })
  end,
})
```

**Helix** — in `languages.toml`:

```toml
[language-server.rill]
command = "rill"
args = ["lsp"]

[[language]]
name = "rill"
scope = "source.rill"
file-types = ["rill"]
roots = []
language-servers = ["rill"]
indent = { tab-width = 2, unit = "  " }
```

**Emacs**, with eglot:

```elisp
(add-to-list 'auto-mode-alist '("\\.rill\\'" . prog-mode))
(add-to-list 'eglot-server-programs '(prog-mode . ("rill" "lsp")))
```

**VS Code** needs a client extension to start a server, which the extension in
`vscode/` does not yet include — the colours work, the server does not start
itself. Until it does, `rill check` and `rill types` from a terminal are the
same answers a keystroke later.

## What it does not do

Completion, rename, and finding every use of a name are not there.

Neither is a column. The compiler records the line a thing is on and not where
on the line it sits, so a diagnostic underlines the whole line and hover works
out what is under the cursor by reading the identifier written there rather
than by looking up a position in the tree. That is exact for anything declared
at the top level, and right for a local wherever one name in a function means
one thing — which is nearly always, and the honest limit is a name bound twice
in one body, where hover shows the first.

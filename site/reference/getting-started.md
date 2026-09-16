Rill comes as one directory: the compiler, the runtime it links into every
program, the standard library, the examples, the model editor and this
reference. It needs one thing from the machine: a C toolchain's linker —
the Xcode command-line tools on a Mac (`xcode-select --install`), `gcc` or
`clang` on Linux. The compiler produces an object file and hands the last
step to `cc`, which joins it with Rill's runtime, the system's C library
and its start-up code into an executable of the platform's own format —
the same step Rust, Go with C, and every C compiler leave to the system,
because the C library and the start-up objects are the platform's and come
with it. Nothing else is needed: no Rust, no LLVM, no runtime to install.

### Download

```
$ curl -O https://rill.baltavista.com/dl/rill-0.13.0-macos-arm64.tar.gz    # Apple silicon
$ curl -O https://rill.baltavista.com/dl/rill-0.13.0-linux-x86_64.tar.gz   # Linux, x86-64
$ tar xzf rill-0.13.0-*.tar.gz
$ rill/bin/rill
usage:
  rill build <file.rill> [-o <out>] [--debug] [--parallel] [--unchecked] [--shared] [--emit-ir] [--target <triple>] [--sandbox[=caps]]
  rill run <file.rill> [--debug] [--parallel] [--unchecked] [--sandbox[=caps]]
  ...
```

On a Mac, a file downloaded through a browser rather than `curl` is
quarantined and the compiler refuses to start; `xattr -dr
com.apple.quarantine rill` after unpacking lifts it. Put `rill/bin` on the
`PATH` or call the compiler by its path — the rest of this page assumes
`rill` is on the `PATH`:

```
$ export PATH="$PWD/rill/bin:$PATH"
```

The Mac build wants macOS 11 or later; the Linux build wants glibc 2.36 or
later, which is Debian 12, Ubuntu 22.04 and anything newer. Checksums are
in `dl/SHA256SUMS`. There is no Windows or Intel Mac build yet; both are
targets of the compiler, not hosts for it.

### Hello, world

A program is a file with a `main`:

```
fn main() = println("hello, world")
```

`rill run` compiles it to a binary and runs it in one step; `rill build`
keeps the binary, which is a native executable of about 50 KB that needs
nothing installed to run:

```
$ rill run hello.rill
hello, world
$ rill build hello.rill -o hello
$ ./hello
hello, world
$ ls -l hello
-rwxr-xr-x  1 you  staff  50936 hello
```

### A second program

Types are lists of the shapes a value can have, `match` takes them apart,
and the compiler checks that every case is covered. Everything is inferred;
`area` says nothing about what it takes or returns.

```
type Shape
  Circle(r: Float)
  Rect(w: Float, h: Float)

fn area(s) = match s
  Circle(r) -> 3.14159 * r * r
  Rect(w, h) -> w * h

fn main() =
  shapes = Cons(Circle(1.0), Cons(Rect(2.0, 3.0), Nil))
  each(shapes, \s -> println(area(s)))
  println(fold(shapes, 0.0, \total, s -> total + area(s)))
```

```
$ rill run shapes.rill
3.14159
6
9.14159
```

Leave out `Rect` in `area` and the program does not compile; the error
names the case:

```
shapes.rill: type error at line 5: match is not exhaustive; no arm matches `Rect(_, _)`
```

`rill fmt shapes.rill` rewrites the file in the one spelling every Rill
program has, and `rill check shapes.rill` type-checks without building,
saying everything it finds rather than the first thing.

### The examples

`rill/examples/` has one program per idea — `list.rill`, `maps.rill`,
`records.rill`, `traits.rill`, `pingpong.rill` for strands and channels,
`exchange.rill` and `abp.rill` for machines — each of them run by the
test suite, so each of them works:

```
$ rill run rill/examples/pingpong.rill
$ rill test rill/examples/abp.rill
ok    test_a_text_goes_through
ok    test_a_lost_frame_is_resent_and_delivered_once
ok    test_texts_wait_their_turn
3 tests: 3 passed, 0 failed
```

A test is a function named `test_...` that takes nothing; `assert` and
`assert_eq` end it where they fail, and each test runs in a process of its
own.

### The model editor

`exchange.rill` is a telephone exchange as SDL would draw it: a `system` of
routes and a `machine` with states, signals and timers. The editor draws
it, edits it as diagrams that are written back into the file, and runs a
scenario with the machines lighting up as it plays:

```
$ rill run rill/editors/web/server.rill      # then open http://127.0.0.1:8765
```

The same editor, with the examples, runs at
[rillsdl.baltavista.com](https://rillsdl.baltavista.com/) — nothing to
install, nothing saved. `rill diagram rill/examples/exchange.rill` prints
the diagrams as Mermaid, `--svg` as SDL's symbols; `RILL_TRACE=1 rill run
rill/examples/exchange.rill 2>trace.txt` records what the machines did,
and `rill msc trace.txt` draws the run as a sequence chart.

### Other targets

```
$ rill build hello.rill --parallel                       # strands across cores: RILL_THREADS=8 ./hello
$ rill build hello.rill --sandbox                        # refused if it would read files, open sockets, call C
$ rill build hello.rill --target wasm32-wasip1 -o hello.wasm   # needs wasm-ld, from LLVM (`brew install lld`, `apt install lld`)
```

### Where next

The chapters that follow are the whole language: [Program
structure](#1-program-structure) for files, imports and layout; [Types](#2-types)
and [Expressions](#3-expressions) for the everyday; [Concurrency](#7-concurrency)
for strands, channels and machines; [The toolchain](#13-the-toolchain) for
every command. The book, *Functional Programming with Rill*, starts from
what functional programming is and ends with a modelled protocol.

# Chapter 3. First Programs

This chapter covers the parts of Rill you need before anything interesting can
happen: how a program is shaped, how values are named, and how the compiler
talks to you when you get it wrong. It is short, and almost everything in it
will be familiar from other languages. The unfamiliar parts are flagged.

---

## 3.1 The smallest program

```rill
# book/code/ch03/hello.rill
fn main() = println("Hello, Rill")
```

```
$ rill run hello.rill
Hello, Rill
```

Four things are worth noticing in one line of code.

**`fn name(args) = expression`** is the whole function syntax. There is no
`return`, because the body *is* the value. There are no braces, because Rill
uses layout.

**Every program needs `main`.** It takes nothing and returns nothing; the
program exits when it returns.

**`#` starts a comment**, running to the end of the line.

**`rill run` compiles and runs.** It is not an interpreter — a native binary is
produced and executed. `rill build hello.rill -o hello` keeps the binary, which
is about 33 KB.

---

## 3.2 Expressions

```rill
# book/code/ch03/arithmetic.rill
fn main() =
  println(2 + 3 * 4)
  println(7 / 2)
  println(7 % 2)
  println(7.0 / 2.0)
  println(to_float(7) / 2.0)
  println("a" + "b" + "c")
  println(3 < 4 && !(1 == 2))
```

```
$ rill run arithmetic.rill
14
3
1
3.5
3.5
abc
true
```

The operators are the usual ones: `+ - * / %` for arithmetic, `== !=` for
equality, `< <= > >=` for ordering, `&& || !` for logic, `+` for joining
strings. Precedence is conventional — `2 + 3 * 4` is 14.

Two things differ from what you may expect.

**`/` on integers truncates** (`7 / 2` is 3) and `%` is integers only. Float
division is written on floats.

**There are no implicit conversions.** This is a type error, and it is the
first error most newcomers meet:

```rill
# book/code/ch03/mixing.rill
fn main() = println(1 + 2.0)
```

```
$ rill run mixing.rill
mixing.rill: type error at line 1: operands of a binary operator: expected Int, found Float
```

`1` is an `Int`, `2.0` is a `Float`, and Rill will not quietly widen one to the
other. `to_float` and `to_int` cross between them and you write them down.

This is a small nuisance about twice a week and it removes an entire class of
silent bug — the kind where a currency total is a hair off because something
became a float three call frames ago. The rule is: *numeric types never mix by
accident.*

> **Float literals need a decimal point**
>
> `2.0` is a `Float`; `2` is an `Int`. There is no context in which `2` becomes
> a float on its own. When a float prints as a whole number Rill shows it
> without the point — `float_to_str(72.0)` is `"72"` — which the next listing
> demonstrates.

---

## 3.3 Bindings and blocks

A function body can be an indented block. Each line is evaluated in order, and
the block's value is its last line:

```rill
# book/code/ch03/bindings.rill
fn celsius(f) = (f - 32.0) * 5.0 / 9.0

fn report(f) =
  c = celsius(f)
  warm = c > 20.0
  float_to_str(f) + "F is " + float_to_str(c) + "C" + (if warm then " (warm)" else " (cold)")

fn main() =
  println(report(72.0))
  println(report(50.0))
```

```
$ rill run bindings.rill
72F is 22.2222C (warm)
50F is 10C (cold)
```

`c = celsius(f)` is a **binding**, not an assignment. It introduces a name for
a value, visible for the rest of the block. What it cannot do is change one:

```
c = celsius(f)
c = c + 1.0        # not "c becomes bigger" — a second binding of the same name
```

There is no assignment operator in Rill. This is the point of Chapter 1 made
concrete: `c` means one thing from where it is written to the end of the block,
and no call in between can change what it means.

**`if` is an expression.** Notice where it appears in the listing — in the
middle of a string concatenation. `if c then a else b` produces a value, so it
goes wherever a value goes. There is no statement form and no `elif`; chains
are written by putting another `if` in the `else`.

Both branches must have the same type, which is why an `else` is never
optional.

---

## 3.4 Layout

Blocks are indentation, two spaces per level, spaces only. There are no braces
and no semicolons.

```rill
fn report(f) =
  c = celsius(f)          # this block is two spaces in
  warm = c > 20.0
  ...
```

Two rules cover the cases where this could be ambiguous:

**A construct may continue on the next line only where the grammar allows it.**
`then` and `else` may begin a line, so a long conditional can be broken up.

**Inside parentheses, indentation is ignored.** Anything written between
brackets stays on one line as far as the layout rules are concerned.

If you are unsure how something should be laid out, do not guess — run
`rill fmt` on the file. Rill has exactly one canonical formatting, the
formatter produces it, and every listing in this book has been through it.

```sh
rill fmt program.rill            # rewrite in place
rill fmt --check program.rill    # exit non-zero if it is not canonical
```

---

## 3.5 Printing and converting

| | |
|---|---|
| `println(x)` / `print(x)` | print any value, with or without a newline |
| `show(x) -> Str` | render any value as a string |
| `int_to_str` `float_to_str` `bool_to_str` `str_to_int` | conversions to and from `Str` |
| `to_float(n)` `to_int(x)` | between `Int` and `Float`; `to_int` truncates toward zero |

`println` takes any value type, including data types you define — Chapter 5
shows what that prints and how to change it.

---

## 3.6 The REPL

For trying one expression, there is no need for a file:

```sh
$ rill repl
> 2 + 3 * 4
14
> fn double(x) = x * 2
> double(21)
42
```

Definitions persist for the session; bindings do not. Under the hood each entry
is compiled to a native binary and run, which takes a few hundred milliseconds
— slower than an interpreter, and it means the REPL and the compiler can never
disagree about what the language means.

---

## 3.7 Exercises

1. Write a program that prints the area and circumference of a circle of radius
   2.5. Do it without writing `3.14159` twice.
2. `rill run` the following and read the error before fixing it:
   `fn main() = println(str_len(42))`
3. Write `fahrenheit(c)` as the inverse of `celsius(f)` from §3.3, and print
   `fahrenheit(celsius(72.0))`. Explain the last digit of what you get.
4. Try writing a block whose lines are indented three spaces, then run
   `rill fmt` on it.

---

Chapter 4 introduces the part that makes the style work: functions that take
and return other functions.

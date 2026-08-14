# Chapter 8. Traits

A trait is an interface: a set of operations that a type can provide. Rill uses
them for two jobs — letting your own types plug into the language's own
operators, and writing functions that work across unrelated types.

This is the last chapter of the language tour. After it you have seen
everything, and the rest of the book is about using it.

---

## 8.1 Plugging into the language

Two traits are built in, and implementing them changes how the language treats
your type.

```rill
# book/code/ch08/traits.rill
type Money
  Money(Int)

impl Show(Money)
  fn show(m) = match m
    Money(c) -> "$" + int_to_str(c / 100) + "." + pad(c % 100)

fn pad(n) = if n < 10 then "0" + int_to_str(n) else int_to_str(n)

impl Ord(Money)
  fn compare(a, b) = match a
    Money(x) -> match b
      Money(y) -> x - y

fn main() =
  println(Money(1250))
  println(Money(305))
  println(max(Money(1250), Money(305)))
  println(Money(500) < Money(750))
  prices = Cons(Money(1250), Cons(Money(305), Cons(Money(999), Nil)))
  println(prices)
```

```
$ rill run traits.rill
$12.50
$3.05
$12.50
true
Cons($12.50, Cons($3.05, Cons($9.99, Nil)))
```

**`Show`** replaces derived printing. Without it, `println(Money(1250))` prints
`Money(1250)`; with it, `$12.50`. And notice the last line — the list printed
its elements through your `show`, without being told. Derived printing calls
`show` on each field, so an impl anywhere composes everywhere.

**`Ord`** enables `<`, `<=`, `>`, `>=`. You write one function, `compare(a, b)`,
returning a negative number, zero, or a positive number, and the four operators
follow from it. So does `max`, and so will the sort in Chapter 10 — anything
written against "an ordered type" now accepts `Money`.

That is the leverage: two small impls, and a type you invented behaves like a
built-in one everywhere in the language and the prelude.

> **What you get without any impl**
>
> Every data type already prints structurally, and `==` already compares
> structurally, field by field. You only write `Show` when the derived form is
> not what you want to read, and `Ord` when there is an ordering worth having.

---

## 8.2 Traits of your own

```rill
# book/code/ch08/own_trait.rill
type Dog
  Dog(name: Str)

type Robot
  Robot(id: Int)

trait Speak(a)
  noise(a) -> Str
  loudness(a) -> Int

impl Speak(Dog)
  fn noise(d) = match d
    Dog(n) -> n + " says woof"
  fn loudness(d) = 7

impl Speak(Robot)
  fn noise(r) = match r
    Robot(i) -> "unit " + int_to_str(i) + " says beep"
  fn loudness(r) = 3

fn announce(x) = println(noise(x) + " (" + int_to_str(loudness(x)) + "/10)")

fn main() =
  announce(Dog("Rex"))
  announce(Robot(7))
```

```
$ rill run own_trait.rill
Rex says woof (7/10)
unit 7 says beep (3/10)
```

`trait Speak(a)` declares the interface: any type `a` that implements it
provides `noise` and `loudness`. Each `impl` supplies them for one type.

`announce` is the payoff. It was never told which types exist. Its body calls
`noise` and `loudness`, so its inferred type is *"for any `a` that can Speak,
`a -> Unit`"*, and it works for `Dog`, for `Robot`, and for any type someone
adds later. You did not write the constraint down; it came from the body, the
same way `biggest` picked up its ordering requirement in Chapter 4.

Method names are global, like constructor names. There is no `dog.noise()` —
`noise` is a function, and which implementation runs is decided by the type of
its argument.

---

## 8.3 What "resolved at compile time" means

Dispatch in Rill is **static**. During monomorphization the compiler knows the
concrete type at every call site, looks up the implementation, and emits a
direct call.

The consequences are worth spelling out:

- **No vtables, no dictionaries, no type tags.** A trait call costs exactly
  what a normal call costs, and is usually inlined away entirely.
- **A missing implementation is a compile error**, naming the trait, the method
  and the type. It cannot be a runtime failure, because there is no runtime
  lookup to fail.
- **You cannot make a heterogeneous list** of "things that Speak". There is no
  boxed trait object, no `dyn Trait`, no `interface{}`. A `List(a)` has one
  element type.

That last point is a real limitation, and it is the trade for the first two. In
Rust you would reach for `Box<dyn Speak>`; in Java, an interface reference. In
Rill, when you need a collection of mixed things, you declare a sum type
listing them:

```
type Speaker
  ADog(Dog)
  ARobot(Robot)
```

Which is more work up front, and closes the set — and which the compiler will
then check you have handled exhaustively everywhere. Chapter 5's table of the
expression problem is the right way to think about the choice.

---

## 8.4 Implementations for generic types

An `impl` may target a generic type by naming its parameters, in which case it
covers every instantiation at once:

```rill
# book/code/ch08/generic_impl.rill
type Box(a)
  Box(a)

impl Show(Box(a))
  fn show(b) = match b
    Box(x) -> "[" + show(x) + "]"

impl Show(List(a))
  fn show(l) = "{" + join(map(l, \x -> show(x)), " ") + "}"

fn main() =
  println(Box(42))
  println(Box("hi"))
  println(Cons(1, Cons(2, Cons(3, Nil))))
  println(Box(Cons(1, Cons(2, Nil))))
  println(Cons(Box(1), Cons(Box(2), Nil)))
```

```
$ rill run generic_impl.rill
[42]
[hi]
{1 2 3}
[{1 2}]
{[1] [2]}
```

This is the listing to study. Two impls were written; five different types are
printed, including two that nest one inside the other.

The mechanism is that `show(x)` **inside** the `List` impl is itself a trait
call, resolved when the element type becomes known. So `List(Box(Int))` uses
the list impl on the outside and the box impl on the inside, composing without
any declaration that they should. In Haskell this needs a `where` clause
(`instance Show a => Show [a]`); in Rill the requirement is inferred from the
body and checked at monomorphization.

The rule the compiler enforces is that a generic impl must genuinely be
generic. An `impl Show(List(a))` whose body only works when `a` is `Int` is
rejected with a "not general enough" error, rather than silently working for
one case.

Note also that the third line replaced the built-in list printing everywhere in
the program — `{1 2 3}` instead of `Cons(1, Cons(2, Cons(3, Nil)))`. If the
`Cons`-chains in earlier chapters have been bothering you, six lines fix them.

---

## 8.5 The whole language, in one page

That is all of it. For reference, everything Chapters 3–8 introduced:

```rill
import "other"                  # and: import "other" as o

type Shape                      # sum type, optionally generic
  Circle(r: Float)
  Rect(w: Float, h: Float)

trait Speak(a)                  # interface
  noise(a) -> Str

impl Speak(Shape)               # implementation
  fn noise(s) = "..."

fn area(s) = match s            # function, pattern match
  Circle(r) -> 3.14159 * r * r
  Rect(w, h) -> w * h

fn main() =
  x = 3 + 4                     # binding (immutable)
  f = \a, b -> a * b            # lambda
  if x > 5 then ... else ...    # expression
  xs |> map(area) |> sum        # pipeline
  spawn worker(ch)              # strand (Chapter 16)
```

Nine constructs. There is no `for`, no `while`, no `return`, no `null`, no
exception, no class, no inheritance, no assignment, no macro, no attribute
system, and no module system beyond `import`. Everything in Chapters 9 to 14 is
built out of the list above and nothing else.

---

## 8.6 Exercises

1. Give `Shape` from Chapter 5 a `Show` impl that prints `circle r=1.0`, and an
   `Ord` impl that orders by area. Then sort a list of shapes with `max` and a
   fold.
2. Write a `Describe` trait with a default-ish helper: a plain function
   `label(x) = "<" + describe(x) + ">"` that works for every implementing type.
3. Write `impl Show(Option(a))` printing `Some 3` and `-`. Does it apply to
   `Option(Option(Int))`? Predict, then check.
4. Try implementing a trait for a type that does not satisfy it — call
   `compare` on a type with no `Ord` impl — and read the error.

---

Part II ends here. Chapter 9 starts Part III, where the language stops being
the subject and algorithms are.

# Chapter 1. What Functional Programming Is

There is a version of this chapter that defines functional programming in one
sentence — *programming with pure functions over immutable data* — and then
moves on. It is accurate and it teaches nobody anything, because every word in
it is doing work you cannot see yet.

So we are going to take the long way. By the end of this chapter you will know
what the definition means, which parts of it are essential and which are
fashion, what the style costs, and why a language would give up assignment —
the single most familiar operation in programming — to get it.

You will not write any Rill until Chapter 3. That is deliberate. Every idea in
this chapter predates Rill by decades and outlives any particular language, and
a reader who understands them will pick up the syntax in an afternoon.

---

## 1.1 Two ways to say the same thing

Here is a program that adds up the numbers from 1 to 10, skipping the odd ones.
In Python, written the way most people write Python:

```python
total = 0
for i in range(1, 11):
    if i % 2 == 0:
        total = total + i
print(total)                       # 30
```

And here is the same calculation, written as an expression:

```python
print(sum(i for i in range(1, 11) if i % 2 == 0))    # 30
```

Both produce 30. The difference is not brevity — you could make the first one
shorter and the second one longer. The difference is what each one *is*.

The first program is a **sequence of commands**. It says: make a box called
`total` and put 0 in it. Make a box called `i`. Repeat the following, changing
what is in `i` each time: look in `total`, add something to what you found, and
put the result back in `total`. When you are done, look in `total` and print it.

The second program is a **description of a value**. It says: 30 is the sum of
the even numbers between 1 and 10. There are no boxes. Nothing is put anywhere.
There is no "when you are done", because there is no *doing* — there is a thing
that the expression is equal to, and the machine's job is to work out what.

This is the whole idea. Everything else in this chapter is a consequence.

> **The word "functional" is a poor name**
>
> It suggests the subject is functions, and every language has functions. The
> subject is really *expressions*: building programs out of things that have
> values rather than out of things that have effects. "Value-oriented
> programming" would have been a better name, and it is too late.

---

## 1.2 The trouble with boxes

Assignment looks harmless. `total = total + i` is the first line of code most
people ever write, and it does exactly what it appears to do.

The trouble is not what it does. The trouble is what it *prevents you from
knowing*.

Consider this fragment, in any language you like:

```python
def report(order):
    tax = compute_tax(order)
    total = order.subtotal + tax
    return format_money(total)
```

Read it and answer a question: **what is `order.subtotal` when line 3 runs?**

You cannot answer. `compute_tax` was handed the order, and for all you know it
adjusted the subtotal on the way past. To find out you must open
`compute_tax`, and then everything `compute_tax` calls, and so on until you
reach the bottom or run out of patience. The three lines in front of you are
not enough. In a large system they are never enough, and this is the practical
reason that reading unfamiliar code is so much harder than writing it.

Now suppose the language did not permit `compute_tax` to change anything. Not
by convention, not by code review — by construction. Then the question has an
answer, and the answer is visible: `order.subtotal` is whatever it was on line
2, because there is nowhere else it could have come from.

That is the trade. You give up assignment, and in exchange **the meaning of an
expression stops depending on the history of the program**. What a name refers
to is decided where the name is introduced, and stays decided.

### Aliasing, the sharp edge

The subtler version of the same problem is aliasing — two names for one piece
of memory. Here is a bug that every working programmer has shipped at least
once:

```python
def add_tag(tags, tag):
    tags.append(tag)
    return tags

defaults = ["draft"]
a = add_tag(defaults, "urgent")
b = add_tag(defaults, "internal")

print(a)          # ['draft', 'urgent', 'internal']
print(b)          # ['draft', 'urgent', 'internal']
print(defaults)   # ['draft', 'urgent', 'internal']
```

Three names, one list. The function looked like it was returning a new value
and was in fact editing a shared one. Nothing here is exotic; this is Tuesday.

With immutable data the bug is not fixed — it is *unspellable*. `add_tag`
cannot append to `tags`, because there is no operation that changes a list.
The only thing it can do is produce a new list with one more element, and then
`a`, `b` and `defaults` are necessarily three different values.

> **"Immutable means copying everything, so it must be slow"**
>
> This is the objection everyone raises, and it is wrong for a specific
> reason: values that cannot change can be **shared** without risk. Adding an
> element to the front of an immutable list allocates one cell that points at
> the old list; the old list is not copied, because nothing can ever modify it
> and be noticed. A tree with one node replaced shares every untouched
> subtree. The technical name is *persistent data structures*, and the cost is
> usually a small constant factor, not the disaster the objection imagines.
>
> Chapter 15 has the measurements for Rill specifically, including the cases
> where the constant factor does bite.

---

## 1.3 Purity and referential transparency

A function is **pure** when two things hold:

1. Its result depends only on its arguments.
2. Calling it does nothing else — no writing to disk, no mutating a parameter,
   no incrementing a counter somewhere.

`math.sqrt` is pure. `random.random()` is not (it depends on hidden state).
`print` is not (its whole purpose is the effect). `list.append` is not.

The property that purity buys is called **referential transparency**: an
expression can be replaced by its value, anywhere it appears, without changing
what the program means. If `f(3)` is 9 once, it is 9 always, so:

```
f(3) + f(3)      ≡      2 * f(3)      ≡      let x = f(3) in x + x
```

These are all the same program. That sounds like a triviality until you notice
how much depends on it:

- **You can reason locally.** Understanding an expression requires reading the
  expression, not the program's history.
- **The compiler can rewrite freely.** Common subexpressions can be shared,
  loop-invariant work hoisted out, dead calls deleted. A compiler for an
  impure language must first prove that a call has no effects, and usually
  cannot.
- **Tests are cheap and stable.** A pure function needs no fixture, no
  database, no clock, no mocks. Give it inputs, compare outputs. There is no
  setup because there is no state to set up.
- **Caching is free.** If a result depends only on arguments, remembering it
  is always safe.
- **Concurrency stops being frightening.** Two threads that only read
  immutable data cannot race. There is nothing to lock, because a data race
  requires a write.

That last point deserves emphasis, because it is the reason the industry
became interested in this style around 2005 and not before. When processors
stopped getting faster and started getting more numerous, "shared mutable
state" changed from a manageable nuisance into the central difficulty of the
field. Functional programming had spent forty years accidentally preparing for
exactly that.

### Nobody is pure all the way down

A program with no effects computes something and then cannot tell you what.
Every real program prints, writes files, answers sockets. The functional
discipline is not *avoiding* effects, it is **pushing them to the edges**:

```
                 ┌─────────────────────────────────┐
   input ───────►│  parse   (effectful, thin)      │
                 ├─────────────────────────────────┤
                 │                                 │
                 │   decide   (pure, thick)        │
                 │                                 │
                 ├─────────────────────────────────┤
   output ◄──────│  perform  (effectful, thin)     │
                 └─────────────────────────────────┘
```

The middle does the thinking and can be tested by calling it. The edges do the
talking and are kept small enough to eyeball. Languages differ enormously in
how much ceremony they demand at the boundary — Haskell tracks it in the type
system with `IO`, ML and Rill simply let you call `println` — but the
architectural advice is identical, and it is good advice in Java too.

---

## 1.4 Functions as values

In most languages, functions and data live in separate worlds. You can put a
number in a list; you cannot as easily put "adding" in a list.

In a functional language, a function is an ordinary value. It can be stored,
passed, returned, and built at run time. A function that takes or returns a
function is called **higher-order**, and three of them replace most of what
loops are used for:

| You want to | Loop version | Higher-order version |
|---|---|---|
| Do something to each element | `for x in xs: out.append(f(x))` | `map(xs, f)` |
| Keep some elements | `for x in xs: if p(x): out.append(x)` | `filter(xs, p)` |
| Combine into one value | `acc = 0; for x in xs: acc = acc + x` | `fold(xs, 0, add)` |

The gain is not that `map` is shorter. It is that `map` **says what is
happening in its name**. When you read a `for` loop you must read the whole
body to discover whether it is a map, a filter, a fold, a search, or three of
those tangled together. When you read `map(xs, f)` you already know the shape
of the answer: same length, same order, each element transformed. You only
need to read `f` to know the details.

This is the real argument for the style, and it is an argument about reading:

> **A loop tells you the mechanism and hides the intent. A combinator tells
> you the intent and hides the mechanism.**

Pipelines compose these into a description of a calculation that reads in the
order it happens:

```
range(1, 100)
  |> filter(is_prime)
  |> map(square)
  |> sum
```

Four steps, each one a noun-phrase you could say out loud. The imperative
version is a single loop with four things happening inside it, and separating
them again is work the reader has to do.

### Closures

A function value that captures variables from where it was written is a
**closure**. This is what makes higher-order functions useful rather than
merely possible:

```python
def multiplier(k):
    return lambda x: x * k      # captures k

triple = multiplier(3)
triple(5)                       # 15
```

`triple` is a function that carries a 3 around with it. Closures are how you
build behaviour at run time — a comparison function that knows which field to
sort on, a callback that knows which connection it belongs to, a parser that
knows which keyword it is looking for.

---

## 1.5 Recursion, and why loops go away

If nothing can be assigned, a `while` loop cannot exist: its counter would have
to change. So how does anything repeat?

By calling itself. Here is a countdown, written as a loop and as recursion:

```python
# loop                             # recursion
i = 10                             def go(i):
while i > 0:                           if i == 0: return
    print(i)                           print(i)
    i = i - 1                          go(i - 1)
```

The loop changes `i` in place. The recursion never changes anything: each call
gets its own `i`, one smaller than the last. Same countdown, no assignment.

Newcomers reasonably object that recursion is slower and blows the stack. For
a *general* recursion that is true. But the shape above is special:

```
    go(i - 1)          ← the last thing the function does
```

Nothing happens after the recursive call returns. There is therefore nothing
in the current stack frame worth keeping, and a compiler can reuse it instead
of pushing a new one. This is **tail-call elimination**, and it turns that
recursion into a machine-code loop — same instructions, same speed, constant
stack.

The rule to learn is simple: *if the recursive call is the last thing that
happens, it is a loop*. The accumulator pattern is how you get there:

```
sum(list)                         # not a tail call: the + happens after
  = head + sum(tail)

sum(list, acc)                    # tail call: nothing happens after
  = sum(tail, acc + head)
```

The second version carries the running answer forward as an argument. That
argument *is* the loop variable, and every functional programmer learns to see
it that way.

> **Where tail calls are guaranteed and where they are not**
>
> Scheme, ML, OCaml, Haskell, Erlang, Elixir and Rill guarantee it — it is
> part of the language, not an optimization you hope for. Java, Python and
> JavaScript do not, which is why recursion carries a stigma among programmers
> who learned there. C compilers usually do it at `-O2` but promise nothing.
>
> Chapter 7 shows how to check that Rill really has done it, rather than
> taking anyone's word.

---

## 1.6 Types that describe the problem

The second half of functional programming — and the half that changed most
between 1975 and today — is the type system.

Most working programmers have met types as a tax: a thing you write down to
keep the compiler quiet. In the ML tradition types are the opposite. They are
the design notation, and the compiler is a machine that checks your design
against your code.

### Sum types: the missing half of the toolkit

Every language has **product types** — a struct, a record, a class, a tuple.
"A point has an x *and* a y."

Far fewer have **sum types** — "a shape is a circle *or* a rectangle *or* a
triangle." Where they are missing, people simulate them: a class hierarchy, a
tagged struct, a dictionary with a `"kind"` key, an enum plus a union you must
remember to read correctly.

```
type Shape
  Circle(radius)
  Rect(width, height)
  Triangle(base, height)
```

Three things follow, and they are worth more than the syntax suggests.

**One: illegal states become unrepresentable.** The classic example is a
network request:

```java
class Response {
    int status;
    String body;      // null unless status == 200
    String error;     // null unless status != 200
}
```

Four fields, and only two of the sixteen combinations are meaningful. Every
reader must remember the rule; every writer must maintain it; and nothing
enforces it.

```
type Response
  Ok(body)
  Failed(status, error)
```

Now the meaningless combinations cannot be constructed. Not "are discouraged"
— cannot be typed. A whole category of bug stops being a thing you test for
and starts being a thing you cannot write.

**Two: `null` becomes unnecessary.** Tony Hoare called null references his
billion-dollar mistake, and the fix is a sum type:

```
type Option(a)
  None
  Some(a)
```

`Option(Int)` is either nothing or an integer, and the difference is in the
type. A function returning `Option(User)` announces in its signature that the
user may be absent, and the compiler will not let you skip the case. There is
no `NullPointerException` because there is no null — absence is a value with a
name.

The same shape handles failure without exceptions:

```
type Result(a, e)
  Ok(a)
  Err(e)
```

A function that can fail returns one of these. The caller cannot use the
success value without saying what happens on failure, because the two live in
different branches. Errors travel as data, along the same paths as everything
else, and no control flow jumps out from under you.

**Three: the compiler can check that you covered every case.** This is
**exhaustiveness checking**, and it is the feature that converts more
imperative programmers than any other. Add a fourth shape to the type, compile,
and the compiler lists every place that now has a hole. Not a runtime
`default:` that logs and continues — a compile error, before the program runs.

Rill does this, and the message names what is missing:

```
$ rill run colour.rill
colour.rill: type error at line 6: match is not exhaustive; no arm matches `Blue`
```

That is a real message from the compiler this book is about, and it is the
single most useful sentence it can say to you.

### Pattern matching

Sum types would be tedious without a way to take them apart. Pattern matching
is that way: it tests the shape and binds the pieces in one step.

```
fn area(s) = match s
  Circle(r)    -> 3.14159 * r * r
  Rect(w, h)   -> w * h
  Triangle(b, h) -> b * h / 2.0
```

Compare the object-oriented arrangement, where each shape carries its own
`area` method. Both are reasonable, and the choice between them is a real
design decision rather than a matter of taste:

|  | Add a new **shape** | Add a new **operation** |
|---|---|---|
| **Methods on classes** | easy — one new class | hard — edit every class |
| **Match on a sum type** | hard — edit every match (but the compiler lists them) | easy — one new function |

This is the *expression problem*, and neither column wins. Sum types suit
domains where the cases are known and the operations keep coming: compilers,
protocols, state machines, anything with a fixed vocabulary. Classes suit
domains where new kinds of thing keep arriving. Knowing which you are in is
most of the skill.

### Inference: types without typing

The last piece is that you rarely write any of this down. Hindley–Milner
inference, which ML introduced in 1973 and every language in this family has
kept, works out the most general type of an expression from its use.

```
fn twice(f, x) = f(f(x))
```

The compiler reasons: `f` is applied to `x`, so `f` takes `x`'s type; the
result is fed to `f` again, so `f` returns that same type. Therefore
`twice : (a -> a, a) -> a`, for any `a`. Nobody wrote a type and the function
is fully checked, at every type it is ever used at.

This is why "statically typed" and "verbose" are not synonyms, and why the
comparison people usually make — Java's ceremony against Python's freedom — is
a false choice. There is a third option, and it has been around for fifty
years.

---

## 1.7 A short history, because it explains the landscape

**1958 — Lisp.** John McCarthy builds a language on expressions, recursion and
lists. Programs are data; garbage collection is invented because the style
demands it. Everything after this is a refinement of ideas Lisp had.

**1973 — ML.** Robin Milner needs a language for writing proof tactics and
builds one with type inference, sum types, pattern matching and guaranteed
tail calls. Nearly every idea in Section 1.6 dates from here.

**1978 — Backus's Turing Award lecture.** John Backus, who led the FORTRAN
team, uses his acceptance speech to argue that assignment is the bottleneck of
programming — that the "von Neumann style" makes programs hard to reason about
and hard to run in parallel. The field mostly ignores him for twenty-five
years.

**1990 — Haskell.** A committee unifies the lazy functional languages. By
enforcing purity absolutely, Haskell discovers what a program looks like when
effects are tracked in the type system, and produces a generation of ideas —
monads, type classes, QuickCheck — that spread far beyond it.

**1996–2005 — the industrial ML branch.** OCaml, and later F#, keep ML's types
and drop its academic reputation. Erlang, built at Ericsson for telephone
switches, arrives at immutability and lightweight processes for entirely
practical reasons: hardware fails and calls must not drop.

**2005 onwards — absorption.** Processors stop getting faster. Multi-core
becomes the only way forward, shared mutable state becomes the central
difficulty, and the industry goes looking for ideas. Between 2008 and 2020 the
mainstream acquires lambdas and closures (Java 8, C++11, C# 3), map/filter/fold
in every standard library, sum types and pattern matching (Rust, Swift, Kotlin,
Scala, and eventually Java 17), immutability by default (Rust, and every
Java style guide since), and `Option`/`Result` instead of null and exceptions
(Rust, Swift, and a hundred libraries elsewhere).

The interesting thing about that list is what is *not* on it. The mainstream
took the types, the values and the combinators. It did not take laziness, and
it did not take effect tracking. Those turned out to be the parts of Haskell
that most programmers were unwilling to pay for — which tells you something
about which ideas here are load-bearing.

---

## 1.8 What it costs

An honest chapter has to include this section.

**Some algorithms genuinely want mutation.** In-place quicksort, union-find
with path compression, a hash table with open addressing, a dynamic-programming
table filled in place — these have functional equivalents, and the equivalents
are often a logarithmic factor slower or a good deal more intricate. A pure
array update is O(log n) with a persistent tree, not O(1). This is a real cost
and it is unevenly distributed: it barely touches business logic and it lands
hard on numerical kernels.

**Performance is harder to predict.** Allocation happens where an imperative
programmer sees none. Lazy languages add a second layer of unpredictability —
Haskell's classic beginner disaster is a space leak from thunks piling up in an
accumulator. Strict functional languages, Rill among them, avoid that
particular trap by evaluating when they are told to.

**The learning curve is real, and it is in an unusual place.** The syntax is
small; you will have it in a day. What takes weeks is the *rearrangement*:
learning to reach for a fold instead of a loop, to thread state through
arguments instead of storing it, to make illegal states unrepresentable instead
of validating at the door. Programmers coming from imperative languages
routinely describe six weeks of feeling slow, then a step change.

**The ecosystems are smaller.** For any functional language you will find fewer
libraries, fewer answers, fewer people who have hit your bug. This matters more
than any language feature on a real project and it is the reason most teams
sensibly stay where they are.

**Some of it is genuinely awkward.** Threading state through twenty call
frames is worse than a global. Building a graph with cycles in an immutable
language is a puzzle. Interacting with a fundamentally stateful world — a GPU,
a socket, a file handle — requires a bridge, and the bridge is never as
pretty as the core.

The people who get the most from this style are the ones who take the ideas
and skip the fundamentalism: pure functions where purity is free, immutability
by default and mutation where it pays for itself, sum types everywhere,
effects at the edges. That is the position this book argues from, and it is the
position Rill's design takes.

---

## 1.9 What to carry into Chapter 2

Six ideas. The rest of the book is these ideas made concrete:

1. **Expressions, not commands.** A program describes a value rather than a
   sequence of steps.
2. **Immutability.** Names do not change meaning; sharing is therefore safe;
   aliasing bugs become unspellable.
3. **Purity at the core, effects at the edges.** The thick middle can be read
   and tested locally.
4. **Functions are values.** Map, filter and fold say the intent that a loop
   hides.
5. **Recursion is repetition, and tail calls make it a loop** with no stack
   growth and no cost.
6. **Types describe the problem.** Sum types make illegal states
   unrepresentable; pattern matching takes them apart; the compiler proves you
   handled every case; inference means you barely write any of it down.

The next chapter asks the obvious follow-up question: given that all of this is
fifty years old and several excellent languages already implement it, what is
Rill for?

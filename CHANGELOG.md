# Changelog

## Unreleased — LLVM 21

The compiler now builds against LLVM 21 (`brew install llvm@21`; the prefix is
wired in `.cargo/config.toml`, and `inkwell`'s feature is `llvm21-1`). Not a
line of the code generator changed: the IR Rill emits, the `tailcc` +
`musttail` promise it rests on, and the wasm32-wasip1 target all carried over
unchanged, and every test passes as it did.

What changed is what comes out the other end. Three years of AArch64 work sit
between LLVM 18 and 21, and the C table moved the way one would hope: spectral
123 → 95 ms (ahead of C by 18%, where it was level), qsort 290 → 272, fannkuch
118 → 109, lcg 128 → 122; nothing moved the other way. The idiom that started
this — `x % 3 == 0` in `pipeline`'s filter, which LLVM 18 spelled in six
instructions and Apple's newer clang in five — is now five in Rill too, and
that row is a tie to the hundredth of a millisecond.

## Unreleased — a string library, a `while`, and boxes that empty themselves

`lib/str.rill` (`import "str" as s`): 145 functions — searching, transforming,
padding, strict parsing, a hash, an edit distance, every encoding a text
arrives in (UTF-8 validation and repair, UTF-16/32 in either order, Latin-1,
cp1252, cp1254, Unicode case with the Turkish rule, `ascii_fold`, base64,
hex, percent, HTML entities, C escapes), and `par_count` / `par_index_of` /
`par_lines`, which cut a text across strands. Against C on a 44 MB text
(`benchmarks/strings/`): the searches two to three times ahead, `replace`
and `upper` ahead, the rest level.

`while cond with (a, b: T) = init`: the loop that stops on the data rather
than on a count, opened like `for` into a tail-recursive function. The state
is what the condition and the body see, the body's value is the next state,
and the loop is worth the state the condition first refused. A tuple state
travels as separate parameters wherever the body ends in a tuple written in
place; an annotated one types the loop's result as well. No `break`, on
purpose: a loop with one exit is the loop that vectorizes. Rewriting the
string library on it took it from 180 functions to 145 at the same speed.

And the code generator learned to take a box apart without counting: an arm
that returns empties the box it matches when the frame's reference is the
only one, its fields become the frame's own and the box a shell for the
answer to be built in; a tuple written as a scrutinee is never built; a
constructor that is the answer moves what it is given. `deriv`, a symbolic
differentiator against C, went from 94 ms to 48 — ahead of the C given a
free-list pool, and 4.8× ahead of the C with `malloc`.

## Unreleased — instructions the machine has and C has no name for

`asm fn` declares a run of instructions the way `extern fn` declares a symbol:

```
asm fn outb(port: U16, v: U8) = "outb $1, $0" : "{dx},{al}"
asm fn read_cr3() -> U64      = "mov %cr3, $0" : "=r"
asm fn barrier()              = "dmb sy"       : "~{memory}"
```

Found trying to say what an operating system in Rill would be missing. The
answer was mostly this: `cli`, `lgdt`, `invlpg`, `wrmsr`, `in`/`out`, `cpuid`,
`wfi`, and the barriers — none of which libc has a name for, and all of which
are one instruction. A language that can reach C but not the instruction under
it can drive a machine only as far as somebody else already wrapped it.

The parameter list is `extern`'s, deliberately: the question of what may cross
into an instruction is the question of what may cross into C, and the widths
(`I8`…`U64`, `USize`, `Ptr`) already answer it. What is new is the constraint
string, and it is LLVM's rather than ours — a template and its constraints go
through as written, the way `extern "-L/opt/homebrew/lib"` goes to the linker
as written. Omitting them asks for `=r` and `r`, which is what most
instructions want; naming them is for the register the instruction insists on.

A constraint string that does not describe the signature is a compile error
with a line number. This is not politeness: LLVM answers a mismatched one with
an assertion, and the compiler dies without saying where.

Side effects are always assumed. Two identical calls stay two calls, and a
result nothing reads still runs — a device register read twice was meant
twice. The optimizer is otherwise free, and the emitted code is the
instruction and nothing around it: `add x0, x8, x9` on arm64, `addq %rax,
%rdi` on x86-64.

`naked asm fn` is the other half: the instructions are the whole function, and
LLVM puts the name in front of them with no prologue and no epilogue. An
interrupt handler ends in `iretq` because that is what it is for, and there was
never a caller to return to.

```
naked asm fn isr_timer() = "push %rax\n  ...\n  pop %rax\n  iretq"
naked asm fn _kstart()   = "mov $0x1000, %rsp\n  call kmain\n  hlt"
```

No parameters, no result, no constraints — refused with a line number rather
than compiled into something wrong at the one moment it runs. Nothing in Rill
may call one, and the error says why. The address is what a program has use
for, and taking it is an ordinary `asm fn`. Inside a naked body `$` is the
assembler's literal rather than an operand reference, so an immediate is
written the way the manual has it.

wasm refuses either kind outright.

## Unreleased — bytes back from C, and a name two libraries wanted

`str_from_ptr(p, n)` copies `n` bytes out of C memory into a Rill string.
`from_cstr` was the only way back and it stops at the first zero, which is
right for a C string and wrong for everything a library hands back by filling
a buffer and returning a count — a `read`, a decompression, an `SSL_read`. The
alternative was a loop appending one byte at a time into a `StrBuf`, which is
sixteen thousand calls for a sixteen-kilobyte read and is not what a bridge to
C should cost.

Found writing TLS for a message broker in Rill: OpenSSL is reached entirely
through `extern`, with no runtime change at all, once there is a way to take
the plaintext out of the buffer it decrypted into.

`lib/http.rill`'s `render` is now `response_bytes`. `render` is the method
`lib/tmpl.rill`'s `Render` trait owns, so a program that serves pages built
from templates could not have both files at once — which is the ordinary shape
of a web program and so the one case both libraries were written for.

## Unreleased — a wait that the write before it was ending

`sock_wait(fd, ms)` answered false in under a millisecond if the same socket
had just been written to. The deadline had not remotely passed; the wait had
simply been woken by something that was not what it was waiting for, and
reported that as the time running out.

A descriptor is registered for reading and for writing separately, and the
poller names a registration by the descriptor alone. A socket written to a
moment ago is writable, and that edge is delivered to whatever strand is
parked on that descriptor — which the runtime already knew could happen, since
`wait_ident` exists to drop an event whose descriptor is not the one being
waited for. It cannot tell a read from a write on the *same* descriptor, and
that is the case that was getting through.

So the wait is a loop against a deadline rather than one sleep, and only the
clock ends it. Anything that wakes it and is not something to read puts it back
to sleep for what is left. This is what a wait on a condition has always had to
be, and the reason it was not is that the wake looked reliable.

It is `sock_wait` that carried it, so what was affected is everything with a
deadline on it: a server reading a request after writing a response, which is
every keep-alive connection, and any client that writes and then waits. Found
by writing a client that sends a command and waits for the answer, which is the
smallest program that can hit it.

## Unreleased — a refused connection that left its registration behind

`connect_to` to a port nothing is listening on answers `Err` and closes the
descriptor it made. It did not take that descriptor back out of the poller
first, and the number then goes back to the operating system to hand out
again.

The next socket given that number, waited on by the same strand, is taken for
one already registered — on a single worker the poller keeps a table of who is
waiting on what, and finding the same strand there already, it does not
register anything. So the wait never ends. Two refused connections in a row
were enough, and the second one hung for ever.

Found by a broker whose nodes dial each other at startup: the first peer being
down was fine and the second one being down was a hang.

`sock_close` had always done this; a failed connect is the other path that
closes a descriptor the poller knows about, and it now does the same.

## Unreleased — a `select` arm that let go of what it took

A value received by a `select` arm comes out of the channel owned: the channel
has given up its reference and somebody has to have it. An arm that hands it on
does — `x = recv(ch) -> Some(x)` moves it — but an arm that only *reads* it
hands it to nobody, and the reference was dropped on the floor.

```
got = select
  v = recv(ch) -> str_len(v)      # one string leaked, every time round
  recv(done) -> 0
```

A loop receiving a million strings and asking each one its length ended with a
million of them still allocated. So did a loop that bound a value and ignored
it. What made it hard to see is that the obvious spelling is the safe one:
anything that passes the value to a function or puts it in a constructor moves
it, and moving is what most arms do.

The fix is the rule a block's own bindings already follow. A block notes what
the frame owed before it began and releases whatever is left over at the end
that was not there before; a move takes the entry out of that list on the way
past, so nothing is released twice. A `select` arm now does the same, and the
arm's own answer is the one thing excused, because that leaves with the
`select`.

Found by measuring a message broker written in Rill, which was holding
seventeen allocations per message and giving none of them back. It was this,
all of it: with the arm releasing what it bound, twenty thousand messages
published, delivered and acknowledged leave two allocations at exit rather
than three hundred and forty thousand, and the broker's memory per message
went from about eight hundred bytes to a hundred and sixty-four.

## Unreleased — a deadline on a wait

`select` takes a `timeout(ms)` arm, ready when nothing else has been for that
long.

```
r = select
  v = recv(ch) -> "answered " + int_to_str(v)
  timeout(ms) -> "gave up"
```

A `select` that had to give up needed a strand and a channel to be its clock,
and the reference is right that a ticker is three lines — for a *rate*. For a
deadline it is the wrong price: a server putting a limit on every request in
flight pays a strand and a channel per request, for a clock each of them looks
at once. The arm parks in the poller instead, beside the channels it is waiting
with, and costs nothing while it waits.

The interesting part is the race. A strand in a `select` with a deadline is
registered in two places that can both come ready, and queueing a strand twice
corrupts the run queue. The poller already solved this for a socket armed with
a deadline — whoever gets there first claims the strand and the second finds
`on_io` already false — so the deadline arm uses the same claim and nothing new
had to be invented. What did have to change is that the poller stamped case
zero on a strand it woke, which a `select` cannot tell from its first arm
having won; it stamps a mark of its own now, far above any case number. Four
thousand rounds with the deadline swept across the arrival, half answered by
the value and half by the clock, leave no allocation behind on one worker or on
four.

The shortest deadline is the one that answers, so a wait may carry a warning
and a limit at once. `timeout(0)` is ready immediately, which makes it a poll —
`default` by another name, and the better spelling where the number is the
point. Writing `default` and `timeout` in the same `select` is refused rather
than silently ignoring the deadline. `examples/timeouts.rill` is the five
shapes of it.

## Unreleased — and a way to be told to

`wait_signal()` parks until the program is asked something and answers which:
`sig_hup()`, `sig_int()` or `sig_term()`. It is the other end of `close`, and
the two are written together — one strand waits and tells the others, so the
waiter never has to know how many others there are.

```
fn shutdown(done) =
  wait_signal()
  close(done)
```

A signal arrives in the worst context there is: on whichever thread the
operating system picked, between any two instructions, possibly while a strand
holds the channel lock. Nothing a program has written may run there. So the
handler does one thing — writes a byte to a pipe — and `wait_signal` is a
strand parked on that pipe, through the same poller a strand waiting on a
socket parks in. Waiting for a signal costs a worker nothing, which is the
whole point when waiting is all the strand does, and a program whose only
strand is waiting is not reported as a deadlock because it is waiting on
something that answers.

The pipe is only the wakeup. Which signal it was, and whether it is a new one,
come from a counter the handler bumps and the strand compares against what it
noted before parking — so a byte left in a pipe by an earlier signal is
harmless rather than something bookkeeping has to be right about. Each waiting
strand gets a pipe of its own, because two strands parked on one descriptor
would be two registrations of the same descriptor and only the second would
wake; sixteen may wait at once and the seventeenth is told to have one wait and
close a channel the others watch.

`SIGUSR1` and `SIGUSR2` are deliberately absent. They are 30 and 31 on a Mac
and 10 and 12 on Linux, and a number that means one thing where it was written
and another where it runs is worse than not having it; `SIGHUP` is the
traditional "read your configuration again" and is what a server would have
used them for. `examples/signals.rill` raises one on itself so that there is
something to catch.

## Unreleased — a way to stop

A strand parked on `recv` was parked until a value arrived. There was no other
outcome. So tearing down whatever was going to send one left the strand where
it was, holding its stack, for as long as the program ran — and a server with a
strand per connection has as many of those as it has had connections. The
language could start things and could not stop them.

```
close(ch)      # nothing more will be sent, and everyone parked on it wakes
closed(ch)     # whether it has been
recv_opt(ch)   # Some(v) while there is something, None once closed and empty
```

Closing is a **broadcast**, which is the thing sending a value cannot be. It
reaches everyone parked on the channel at once, however many that is, so a
program tearing down does not have to know the number — which is the whole
difficulty, because the number is usually the thing it is least sure of.
Closing twice is not an error, so a teardown that arrives from two directions
does not have to arrange which of them gets there first. And what is already
buffered survives the close and comes out first, so a producer may close the
moment it has nothing more to say without the consumer losing anything in
flight.

`recv` still answers the element type, and that is a decision rather than an
oversight. Most channels never end — a worker pool, a pipeline, a ticker — and
making every one of them answer an `Option` would have put a `match` in front
of every receive in the language to pay for the few that do. `recv` on a closed
channel ends the program and says to use `recv_opt`, which is the answer an
index outside a buffer gets and is the same reason: there is no value to hand
back that would not be a lie about a string or a list.

Sending into a closed channel ends the program too, and the rule that avoids it
is the one Go arrived at: whoever sends is whoever closes. Where a strand
really is parked on a send nobody will ever take, `select` is how it hears:

```
r = select
  send(out, n) -> push(out, stop, n + 1)
  recv(stop) -> "gave up"
```

A closed channel makes its receive arm ready, so one `close(stop)` reaches that
strand whatever the other arm is doing. The arm binds nothing, and that is
exactly why it works — there is no value for a closed channel to produce. An
arm written `x = recv(stop) ->` would have to produce one, and ends the program
saying so rather than binding a word that means nothing.

`chan_wait` and `chan_taken` are the two builtins `recv_opt` is written from,
the way `map_get` is written from `map_find` and `map_val_at`. The answer has
two shapes and a call has one, so the wait and the value are separate calls and
the prelude puts them back together.

**`sock_shutdown(fd)`**, for the other half of the same problem. The reference
already said that a strand parked on a read cannot be got out of it by closing
its socket from elsewhere: the reader is inside the poller, closing does not
wake it, and the number can be handed to the next connection while the old
registration is still armed. Shutting down acts on the connection rather than
on the descriptor. The number stays this program's until it closes it, and the
socket becomes readable-at-end-of-file at once — a readiness event, so the
poller wakes the strand and its `sock_read` answers `""`. The reader then
closes the descriptor itself, on its own strand, where that has always been
safe. A server putting a limit on how long it holds a connection, or dropping
all of them on the way down, now has something to call.

`examples/closing.rill` is the three shapes of it: a producer that runs out, a
worker waiting on two things at once, and one close reaching four listeners.

## Unreleased — a file it does not have to hold, and a sync that does not stop the world

`read_file` and `write_file` were the whole of what a program could do with a
file, and both of them are "all of it, now". A log is not that shape, and
neither is a journal, or a store several readers are at different places in.
So a file is now a descriptor — the same `Int` a socket is, deliberately, so
that a program holding both is not holding two ideas of what an open thing is.

```
fd = file_open(path, "a")     # "r" "w" "a", and each with "+" to read as well
at = file_size(fd)            # where this record will start
file_write(fd, record)
file_sync(fd)                 # now it is on the disk, and not before
file_close(fd)
```

`file_read` and `file_write` where the descriptor is, `file_pread` at an
offset without moving it, `file_seek`, `file_pos`, `file_size`,
`file_truncate`, `file_remove`, `file_rename`, `file_exists`, and `dir_make`,
`dir_list` and `dir_sync` for the directory around them. The prelude has
`open_file` as a `Result`, `dir_names` as a list, and `replace_file` for the
whole write-sync-rename-sync dance. §10 of the reference is the surface and
`examples/files.rill` is it working.

`file_pread` is the one worth naming twice. Reading at an offset leaves the
descriptor where it was, so several readers of one file do not have to take
turns, and none of them disturbs whatever is appending to the end of it. That
is the difference between one open file answering a hundred consumers and a
hundred open files.

**The sync does not hold the worker.** This is the part that took the design
rather than the code. A write that has returned is in the operating system's
cache, and `fsync` is the wait for the disk to say otherwise — a millisecond
on a good drive and ten on a bad one, which on a single worker is the whole
program standing still. So the strand hands the request to a helper thread and
parks on a pipe, which is the same manoeuvre a strand waiting on a socket
makes and goes through the same poller. Four small syscalls to hide a wait a
thousand times longer.

The measurement is not how fast a sync is, because that is the disk's business
and not the language's. It is whether anything else can run during one: a
strand computing beside a strand syncing takes 173 ms against the 180 ms it
takes alone, while a hundred syncs happen next to it and take about three
hundred. Held the worker, it would have had to wait for every one of them.

The rule the whole thing rests on is that the request is on the heap and not
on the strand's stack. A parked strand's frames are copied away while it waits
— that is how one worker runs many strands on one stack — so an address into
them means nothing to another thread. Before the scheduler exists at all, a
sync is simply done where it stands: there is no worker to spare and nothing
to be an improvement on.

Two smaller things that are easy to get wrong and are therefore not left to
the caller. On macOS the sync is `F_FULLFSYNC` and not `fsync`, because
`fsync` there hands the bytes to the drive and returns without waiting for the
drive to keep them, which is fast and is not durability. And `dir_sync` exists
because syncing a file says the bytes are there and does not say the *file*
is: creating and renaming both change the directory, and that change is cached
like any other, so a program can sync a file it has just made and come back
after a power cut to a directory that has never heard of it.

## Unreleased — a page, and the appends underneath it

`tools/tmplgen.rill` turns an HTML template into Rill, before the program that
uses it is compiled. There is no template at run time and nothing to
interpret: `<%= e %>` becomes a call, the text between the tags becomes
appends, and a loop in the template becomes a loop in the language.

```
%% import "shop"
%= product(p: Item, seen)
<h1><%= p.name %></h1>
<% if seen > 0 %><p><%= seen %> people looked</p><% end %>
<ul><% each tag, i in p.tags %><li><%= i %>: <%= tag %></li><% end %></ul>
```

`<%= %>` escapes, `<%! %>` does not, `<% %>` carries `for`, `each`, `if` /
`elif` / `else`, `let`, `include` and `end`, `<%# %>` is a comment, and `<%-` /
`-%>` eat the whitespace around a tag so a template can be laid out to read
rather than to print.

A page may be built out of several templates. `<% include row(it, i) %>` is
the call `row(b, it, i)` — the same builder, and whatever the including
template has in scope handed over as arguments — and the template it names is
reached with a `%%` import like any other function in another file. There is
no separate notion of a partial or a layout, and nothing is looked up by name
at run time: a template is a function, and a page made of four of them is four
calls. `examples/template/` is that page. What is inside a tag is Rill — `<% if seen > 0 %>` is that
expression, not a second language that happens to look like it.
`lib/tmpl.rill` is the half that stays: the `Render` and `Raw` traits, so
`<%= %>` works for anything with an impl, and an escape that copies the text
whole until it meets a byte that needs a name.

`benchmarks/template` measures it against the Rust engines on the two pages
the askama-rs suite renders, byte for byte identical output: **11 455 ns**
against sailfish's 11 546 on a 292 KB table, and **29 ns** against its 32 on a
small page — ahead of markup, askama and maud by two to three times, and of an
interpreted engine by sixty.

`examples/oxidb/catalogue.rill` is the first thing written on it. Its page was
markup built by hand — nine functions concatenating strings, and no way to see
what the page looked like without reading them; it is now four templates — the
page including the form, the table and the pager — with `catview.rill` beside
them saying what a page is given. What comes out
carries the same content on every route, the server answers the same 5,600
requests a second at a hundred users, and the HTML is laid out to read rather
than run together, which costs about a fifth more bytes on the wire.

`lib/http.rill` is a good deal less of a toy. It reads a body that arrives in
its own packet rather than with the headers, and one sent in pieces
(`Transfer-Encoding: chunked`); it keeps what a read took past the end of one
request, so a pipelined pair gets two answers rather than one; and it decides
keep-alive from the headers of the request in hand rather than from the bytes
in the buffer, which may hold the next one.

It also says no. A connection that goes quiet for fifteen seconds is let go, a
header block over 64 KB and a body over a megabyte are refused before they are
read, and a request that tells two stories about its own length — a
`Content-Length` beside a `Transfer-Encoding`, two lengths, a length that is
not a number — is refused outright: the danger there is not that this server
picks the wrong story but that the proxy in front picks the other one, which is
what request smuggling is.

Plus the things a server needs to be one: `Date` on every answer (which wanted
a wall clock, so `now_millis()` is a builtin now — `now_nanos` measures how
long something took, this says when it happened), `HEAD` answered without a
body, twenty-four status texts instead of six, percent-decoding and query
parsing (`path_of`, `query_of`, `param`, `params`, `decoded`, `encoded` — the
catalogue had written five of those for itself, without the decoding, so a
filter with a space in it did not work), cookies in and out, and a client that
can send a method, headers and a body rather than only `GET`.

`lib/http.rill` learned headers, both ways. A `Request` carries the ones it
arrived with, folded to lower case because HTTP says the case does not matter
and a proxy will one day disagree about it; `header(req, name)` is the lookup.
A `Response` carries whatever it wants to say beyond its type, and
`with_header` adds one. That is what the two things below are made of.

**Middleware.** A handler answers a request; a middleware is one layer around
it, handed the request and the handler underneath, free to answer without
passing it on. `wrap` stacks them outermost first and hands back a handler
like any other, so `serve`, `serve_n` and `accept_forever` take it unchanged.
`rate_limit(n)` is the first: n writes a second per client, 429 for the rest,
reads never counted — and a client is whoever `X-Forwarded-For` names, since
that is all a server behind a proxy can see. `examples/http/middleware.rill`
is the whole of it in forty lines.

**What the catalogue serves twice, it now sends once.** The nine modules a
page pulls in are read from disk once and kept, and go back with
`Cache-Control` and an `ETag`; a request that already has that version is told
304. A repeat visit that fetched 123 KB fetches nothing at all — measured in
the browser, fifteen files, zero bytes.

It is also where the template engine meets a client one. The page arrives
whole and works with JavaScript off; `static/catalogue.js` then takes it over
with [zap](https://github.com/barisakin/zap) — signals, no virtual DOM — and
its components: `Select` and `Button` in the toolbar, `DataGrid` for the
table, which sorts, filters and pages a window of rows with nothing leaving
the browser, and shows what the rows cost to produce — the database's time
and the writing's — beside its row count. The table is editable: a cell opens
the editor its column's type asks for, and what comes out is posted back to
the document it came from, one field at a time. The rules are written on both
sides deliberately — the grid stops you where the column says, and the server
checks the same limits again before it writes, because a request need not have
come from the page — and a write that lands says how long the database took,
in a notice that shows for three seconds. Markup the first paint needs stays in the templates, state moves
to signals, and the JSON the server already had for that page is in the page,
so there is nothing to fetch before the first paint and nothing to swap in
after it. Narrowing a filter is a question for the database, which has an
index behind every field the toolbar offers; sorting a column is not.

Almost none of that is the generator. A page is a run of appends, and the
appends are what changed:

- **An append is written where it stands.** `b += s` was a call into the
  runtime that loaded the string's length out of its header and branched on a
  width the caller already knew; a literal now goes in as whole words, its
  length a constant, and `b += byte` and `b += int_to_str(n)` are laid down
  the same way.
- **The runtime is handed the block, not the builder.** `rill_sb_grow` takes
  the text's block and gives back a bigger one; the pointer, the length and
  the capacity are written by the caller. That is what lets a loop of appends
  keep all three in registers instead of reading the header back after every
  write — the store-to-load turnaround was most of what an append cost.
- **The header and the text are told apart.** The bytes a builder holds live
  in a block of their own, so a byte written into the text can never land on
  the length; saying so in the IR is what makes the above hold across an
  append that grows.
- **Digits go in directly.** How many there are is read off the top bit rather
  than found by dividing, and they are written two at a time out of a table.
  The magnitude is carried unsigned, so the smallest integer needs no case of
  its own.
- **A builder may be borrowed by an impl method.** `render(b, x)` was taking a
  reference and giving it back on every interpolation — a load, a compare and
  a store on the header, in the middle of the tightest loop a page has.
- **`strbuf_clear(b)`** empties a builder and keeps the room it took, for text
  built over and over into the same one.
- **The range test a `for` over a buffer makes is written out** rather than
  called for, with the call left for the case that ends the program. Both it
  and the growth path are marked cold, so what is laid down in the loop is the
  appends and nothing else.

Together these are about **four times** off the cost of building a page, and
they show up anywhere a program writes text: the JSON benchmark in
`benchmarks/tokens` builds its output through a builder, and every server in
`examples/` writes its responses through one.

### The catalogue's grid groups, and the database does the grouping

Drag a column header onto the panel above the catalogue's table and the rows
fold into groups, one level per header dropped — `role › region › stock` is
three drops, and a chip dragged back onto the table takes its level out.
A group opens on a click to the level under it, and the last level opens to
the documents. Every level is one question to the server and the groups are
paged the way documents are, so a hundred thousand groups is four thousand
pages of them, not a screen that stops answering.

`/rows?group=<field>` is the question. The filters stay what they were, and
the answer is what the matching documents come to under each value of that
field: the count, the average score and the stock in total, as one pipeline
in OxiDB — `$match`, `$group`, and `$facet` to split the result into the
page asked for and the count of groups, so the pager has its total without a
second trip. `lib/oxidb.rill` gains `aggregate`, which is the one call it
takes. Nothing on the server reads a document, and nothing in the browser
counts anything: what a group says is about the whole data set, not the
window the flat grid holds.

The grid's side of it is three props — `groupable`, `groupBy` and a
`source` that answers a level given the path down to it — and a column may
say what it shows on a group row with `aggregate`. `static/components/grid.js`
carries the change until it goes back to zap.

## Unreleased — a shape over a buffer, and NumPy behind it

`lib/grid.rill`: arrays of numbers with a shape, the way NumPy has them. A
grid is one flat `Buf(Float)` and the shape laid over it — `shape`,
`strides`, `off` — so `transpose`, `rows`, `col` and `slice1` are views that
cost nothing, and `copy`, `reshape` and every operation that makes a grid
lay it out contiguously. Making: `zeros`/`ones`/`full`, `arange`,
`linspace`, `of`, `from_fn1`/`from_fn2`, `identity`. Elementwise: `add`,
`sub`, `mul`, `div`, `scale`, `shift`, `power`, `neg`, `gabs`, `gsqrt`,
`gexp`, `glog`, `maximum`, `minimum`, and `map`/`zip` with a lambda.
Comparisons make a `Mask` — a byte per element — `gt lt ge le eq ne` and
`gtk ltk gek lek eqk nek` against one number, with `where`, `mcount`,
`any`, `all`, `mask_and/or/not`, `to_grid`. Reductions: `sum`, `mean`,
`prod`, `gmax`, `gmin`, `argmax`, `argmin`, `var`, `std`, `dot`, `norm`,
`cumsum`, `sum_axis`, `mean_axis`. Linear algebra: `matmul`, `matvec`,
`outer`, `trace`. Elements: `at1`/`at2`, `put1`/`put2` in place.
`println(g)` prints it. On a `--parallel` build with `RILL_THREADS` set,
anything over a few million elements is split across the workers in
strands.

`bindings/python`: the same library from Python — `import grid as g`, then
`a = g.arange(6).reshape(2, 3)`, `a * 2.0 + 1.0`, `a @ a.T`, `a[a > 2]`,
`.sum(axis=0)`, `np.asarray(a)`. `gridlib.rill` is a set of `export fn`s
built with `rill build --shared`; the package is a `ctypes` layer of
operators, indexing and shape checks. Nothing is computed in Python.

The rest of what an array library is expected to have, all of it checked
against NumPy value by value:

- **The maths**: `gsin`, `gcos`, `gtan` and the inverses, the hyperbolics,
  `gexp2`, `gexpm1`, `glog2`, `glog10`, `glog1p`, `gcbrt`, `gfloor`,
  `gceil`, `gtrunc`, `ground` (to the even one on a half, as NumPy rounds),
  `gsign`, `gsquare`, `greciprocal`, `gdegrees`, `gradians`, `gclip`,
  `gmaxk`/`gmink` against one number, and `gatan2`, `ghypot`, `gfmod`,
  `gcopysign`, `gpowg` over two grids.
- **Shape and joining**: `zeros_like` and its family, `squeeze`, `ravel`,
  `logspace`, `geomspace`, `concat`, `vstack`, `hstack`, `stack`, `tile`,
  `repeat`, `diag`, `tril`, `triu`, `meshgrid`, `fliplr`, `flipud`, `flip`,
  `roll`, `take`.
- **Order**: `sort` (quicksort, median-of-three, the larger half left to
  the tail call, so the stack is logarithmic), `argsort` (a stable merge
  sort over the positions), `distinct`, `searchsorted`, `nonzero`,
  `count_nonzero`, `median`, `percentile`, `quantile`, `ptp`, `diff`,
  `cumprod`, and the `nan`-skipping reductions `nansum`, `nanmean`,
  `nanmax`, `nanmin`, `nanargmax`, `nanargmin`, with `isnan`, `isinf`,
  `isfinite` and `isclose` giving masks.
- **Solving**: `lu` factors a matrix once with partial pivoting, sixty-four
  columns at a time, its trailing update and the substitutions afterwards
  going through the same `Float2` kernel `matmul` uses; `solve` (a vector or
  a matrix of right-hand sides), `inv`, `det`, `matrix_power` (by squaring),
  `lstsq` (through the normal equations), `norm1`, `norm_inf`, `norm_fro`,
  `cond`, `kron`, `cross`, `inner` and `vdot` are written on it. A singular
  matrix stops a program with a message and hands a host an empty grid
  instead, which `grid.solve` in Python raises `LinAlgError` on.

`solve`, `det` and `inv` on a 400 × 400 matrix went from 43 ms to 6.3,
which is level with LAPACK through NumPy on one thread. Four things did it,
in the order they mattered: the trailing update of the factorisation goes
through the same `Float2` kernel as `matmul` and runs at 51 GFLOP/s; the
block of columns being factored is copied out transposed first, because a
column of a row-major matrix is one number per cache line and the pivot
search, the scaling and the rank-one update all walk one; `inv` exploits
the identity's shape rather than solving against it, which is a third of
the forward pass rather than all of it, at the price of putting the
answer's columns back in order at the end; and what is left is divided
between the workers in pieces sized by the arithmetic in them, since
starting ten strands costs thirty-five microseconds and four costs five.
On all cores OpenBLAS is still ahead, 5.3 ms against 6.2.

`sort` above a few thousand elements sorts by the bits rather than by
comparing: a float's order is its bit pattern's order once the sign is
folded in, so the numbers go into place a byte at a time, in eight counting
passes over two buffers, with one histogram pass serving all eight. There is
nothing to mispredict, which is what a comparison sort spends its time on,
and it runs 2.3× faster than NumPy's sort where the quicksort it replaces
ran 1.4× slower. Below that it is still a quicksort, since the passes and
the two buffers are not worth it for a short range. `median` and
`percentile` select rather than sort, as NumPy's do.

`benchmarks/numpy/` measures the library against NumPy on twelve programs,
on one core and on all of them. On one core Rill is ahead on eight and
level on four: two hundred thousand small operations 11×, sorting 2.3×,
reductions and moving numbers about 1.9×, a mask and a `where` 1.8×, order
statistics and a matrix product 1.1×, and level where both sides stream
memory at the machine's limit, call the same maths library, or solve a
linear system. On all cores NumPy keeps two — a threaded matrix product and
a threaded factorisation. The whole process is 2–12× shorter on all twelve.
`benchmarks/numpy/README.md` has the table and the reasoning.

The language and compiler, on the way there:

- **`Float2`**: two floats in one register, with `+ - * /` on both lanes,
  `float2`, `splat2`, `lane0`/`lane1`, `f2_load`/`f2_store` and `fma2`. A
  kernel written in it says its vectors; the grid's matrix product keeps a
  4 × 10 block of answers in twenty of them and runs at the speed of a BLAS.
- **`unique(x)`**: whether this frame holds the only reference. A value
  asked about is passed owned, so a caller still using it keeps its own
  reference and the answer is `false`. What lets an operation write into a
  temporary's storage.
- **A binding's last use moves it into the call** — no retain before, no
  release after, the callee the one holder — on the statement's
  unconditional path, when the name is not spoken again.
- **Tuples hold up to eight** values (`Tuple5`–`Tuple8`), so a reduction can
  carry eight partial sums in eight registers.
- **Flat values cross a call as their fields**, one parameter each, instead
  of one aggregate: a loop carrying a tuple of sums is a function tail-calling
  itself with one, and LLVM's aggregate `phi` for it hid the sums from the
  vectorizer.
- **`sqrt`, `fabs`, `floor`, `ceil`, `trunc`, `round`, `rint`, `fma`,
  `fmin`, `fmax`, `copysign` declared from libm are the instruction**, not
  the call: C's `sqrt` may set `errno`, which is why a loop of them never
  vectorized.
- **A `for` loop whose body calls nothing is not preemption-checked**: the
  check is a volatile load and a branch per element, and a program that
  spawns anything had it in every loop.
- **`workers()`**, **`buf_float_uninit(n)`** (a buffer about to be written
  in full, no clearing pass), and a runtime cache of the last few big blocks
  freed, so an array library's eighty-megabyte results are not page-faulted
  in afresh every time.
- **`rill build --shared`**: a shared library whose `export fn`s a C or
  Python host calls. Counted values cross as handles, lent in and handed
  out, with `rill_drop_<Type>` for the host to let go by; `main` runs as
  `rill_init`.
- **`--emit-ir` honours `--unchecked`.**
- **A worker with nothing to do no longer takes every other worker's queue
  lock** on every turn of its idle loop; it asks the queued count first. Six
  idle workers were spending a busy worker's cache line on a question whose
  answer was no.
`examples/oxidb/catalogue.rill` puts the two together: a hundred thousand
documents seeded into OxiDB's document engine and served over HTTP. `/` is a
page a browser can use — a form for the filters, a table, and a pager that
carries them from page to page — and the same server answers `/count`,
`/doc/42`, `/find?role=eng&limit=3` and `/top?by=score&limit=5` as JSON. A
thousand documents to a frame puts them in in 624 ms; four indexes take 466
more; then a page renders in about half a millisecond, where without the
indexes finding one document takes 15 ms and sorting by a field 471. Paging
by offset is what a URL wants and what it costs: page 1 in 0.4 ms, page
1000 in 1.4, page 3900 in 95. Neither library knows about the other — what
is written is the joining.

Under a hundred concurrent searchers the first version of it fell over, and
the reason is worth the example: it opened a connection to the database per
request, and at four thousand requests a second the closed ones piled up in
TIME_WAIT until the machine had no ports left — nine replies in ten were
502. The connections are now sixteen, handed round on a channel, and a
request that finds none waits. A hundred strands on one worker then answer
5,600 searches a second with half of them inside 18 ms and none failed; five
hundred at once is the same throughput with the queue in the latency.
`examples/oxidb/search.lua` is the mix `wrk` was given.

`http.rill` learned what a reply is as well as what it says: `Response`
carries a content type, and `text_response`, `html_response` and
`json_response` name the three a small server needs. A browser shown
`text/plain` renders the tags instead of the page. `oxidb.rill` gained
`count_where` and `find_page` — how many match, and one page of them — both
of which the server had all along.

- **`hello` reads the reply the server actually sends.** The handshake is
  the one command answered before the server has looked at what format the
  request was written in, so it comes back as JSON whether or not the rest
  of the connection is binary, and in the server's own shape rather than the
  usual envelope. The client read it as OxiWire and refused it. It now reads
  either, and `server_name`, `server_version`, `wire_version` and
  `server_features` say what came back. Found by running all five clients
  against a real `oxidb-server` rather than the stand-ins.
- **`sock_port(fd)`**: the port this end of a socket is on. `listen_on(0)`
  asks the kernel for a free one, and until now nothing could ask which it
  gave — so a program that wanted a port had to name one, which is a promise
  about a machine rather than about a program. `examples/http/server.rill`
  and `examples/echo_server.rill` now let the kernel choose; both used to
  answer with whatever else happened to hold 8080 or 7878.
- **`rill fmt` no longer invents a blank line after a statement written
  across several lines.** What it prints is anchored to the line a construct
  starts on, so the lines a multi-line statement left behind looked like a
  gap the author had left; it now asks whether any of them was actually
  blank. Three files in `lib/` that the formatter had been unable to leave
  alone are formatted.
- **`!=` on floats is now unordered**, as C, Python and IEEE 754 have it:
  `x != x` is true of a NaN, where it used to be false. `==` is unchanged —
  a NaN is equal to nothing, itself included — and nothing else about
  comparison moved. It is how a program asks whether a number is a number.

Building it found four things in the compiler:

- **A record's field indexed in a loop** — `g.data[k]` — was retained and
  released on every element, and the release's branch kept the loop from
  vectorizing. A field of a box the frame holds is now read without a count
  taken for the look.
- **A `for` body that indexes a record's field** reads the field into a name
  of its own before the loop, and **an index `v[e + i]` with a
  loop-invariant `e`** — `a[i*n + j]` in a matrix loop — is bounds-checked
  once for the whole loop, as `v[i]` already was.
- **Bindings inside a non-tail `if` branch's block were released at the
  function's exit**, on a path where they never existed. A block's bindings
  are now released when the block ends — by identity, since a call in the
  block may have been handed one of the frame's own.
- **Three faults a module imported under an alias showed**: an `impl` inside
  it did not find its own type; a type annotation in it (`g: Grid`) was taken
  for a type parameter, the alias being lowercase; and a `for` body that read
  a field of a captured record (`p.x`) did not capture `p`. All three fixed.

## Unreleased — bytes in, bytes out

Seven disk and network programs beside their Go twins (`benchmarks/io/`),
and what the first run found:

- **Standard output is buffered**, the way C's is: a line at a time when it
  is a terminal, 64 KB at a time otherwise. It is written out before
  anything that waits — the program ending (through `atexit`, so a C
  callback that prints after `main` is not lost), `exit`, a read of standard
  input, a strand parking on a socket or a timer — and before an abort, so
  the message that explains the abort follows what the program said. Two
  million `println`s were four million `write` calls; they are now 2× faster
  than Go's buffered writer. A `--parallel` build with several workers
  writes as before, unbuffered, since the workers share no buffer.
- `read_line` no longer copies each line through a scratch block: the line is
  nearly always whole in the block last read, so it is one `memchr` and one
  copy. Standard input is read 256 KB at a time.
- Copies of up to sixteen bytes are two overlapping word moves instead of a
  byte loop; every short string in the language got quicker for it (json
  144 → 124 ms, dijkstra 105 → 91).
- `sock_read` keeps one 64 KB scratch per worker instead of allocating and
  freeing one per call.
- `s[i]` inside `for i in 0..#s` is read without its per-byte test — it is in
  range by construction — while every other string index keeps its `-1` past
  the end.
- A socket's registration in the poller outlives the wait that made it. On
  one worker a descriptor is registered once, edge-triggered, for the strand
  that waits on it, and re-registered only when a different strand comes;
  before, every wait was a `kevent` to arm and a `kevent` to wait. An event
  for a descriptor the strand has since left is recognised and dropped, and a
  closed descriptor is forgotten so its number can come round again. Sound
  because every wait follows an `EAGAIN` from the operation itself. Linux
  keeps its one-shot `epoll` registrations for now; several workers keep the
  one-shot path everywhere.
- `sock_buffered(fd)` and `sock_flush(fd)`: an opt-in 64 KB write buffer per
  socket. `sock_write` queues; the queue is written when it fills, on
  `sock_flush`, whenever the strand parks (a read, a channel, a timer — so
  nothing a program wrote waits on the program), on `sock_close`, and at
  exit. A stream of small writes is one system call for many; an echo loop
  is unchanged. Nothing is buffered on several workers.
- The accept loop in the language reference spawned a strand *around* the
  accept and so never waited: `spawn handle(tcp_accept(s))` spins. It reads
  `conn = tcp_accept(s)` first now, and the concurrency section says why.

## Unreleased — as fast as the C

A day spent putting nineteen programs beside their C twins
(`benchmarks/c/`) and closing what it found. The morning's table was three
wins, four ties and twelve losses; the evening's is eight wins and eleven
within run-to-run noise, nothing behind. What changed, each measured alone:

- Every Rill function but `main` and a wasm export is `internal` to its
  module, so LLVM inlines it freely — which is what let a matrix multiply's
  inner loop lose its bounds checks and vectorize (65 ms → 20, C's 20).
- **Borrowed parameters.** A buffer, string, builder or map parameter that a
  function only reads is passed without a reference of its own and released
  by nobody; the retain the call took and the release its callee made were a
  load, a compare and a store each on the same header, in the inner loop of
  every sort and heap. Tail calls stay tail calls: only a borrowed parameter
  of the calling function may go through a borrowed position there, and
  anything else forces the callee's parameter back to owned.
- **Pipeline fusion.** `range |> filter |> map |> sum` (and `len`, `fold`),
  over lambdas written in place and scalar types, is one loop with no list:
  83 ms and 154 MB became 5 ms and 1 MB.
- **Flat records.** A type with one constructor and only scalar fields is a
  struct in registers — no box, no count, no tag. A function returning
  `(q, r)` used to allocate; 100 million of them ran in 559 ms and now in
  288, exactly C's. Such a value is boxed only at the edge of a table or a
  channel.
- **The allocator's fast path is generated inline**: a free-list pop or push,
  three instructions, with the runtime asked only when a list is empty.
- **Tables** know when their key is an `Int` and their value a scalar, hash
  and compare in place, probe once on insert, and answer `map_or` in a single
  call — for `Int` keys, in generated code with no call at all. The `Int`
  hash is one multiply and a fold. Two million inserts and lookups: 193 ms →
  97, against a hand-written C table's 105.
- `b += int_to_str(n)` writes the digits straight into the builder, and the
  decimal formatter behind every `int_to_str` and `println` counts digits by
  comparison and writes two at a time; `b += byte` is inline when there is
  room. A buffer's length is written where LLVM can read it, so loops it can
  relate to `n` lose their checks. A closure call no longer retains and
  releases its callee.
- A lambda written in place for a parameter that never escapes — the
  prelude's `map`, `filter`, `fold` — borrows its captures: its environment
  holds no reference of its own, since the frame that made it outlives the
  call. Not in tail position, where the frame has already let go.
- **Bounds checks hoisted out of `for`.** A body that indexes `v[i]` by the
  loop's own variable gets one test in front of the loop — `lo >= 0` and
  `hi <= #v` — and none on the elements. The check is emitted only when `v`
  is a buffer; a string keeps its per-byte test, since `s[i]` past the end
  is `-1` by contract. An out-of-range loop now stops before it starts, with
  the index it would have reached.
- `strbuf_str(b)` hands the builder's block over as the string when `b` is
  never named again in its block, instead of copying it; the builder is left
  empty and usable. A builder read twice is copied as before.
- `rill build --unchecked` (and `rill run --unchecked`): no bounds test on
  buffer accesses, for a program that has been run checked first. The trade
  Zig's ReleaseFast and Swift's -Ounchecked make; off by default.
- **A buffer value is the address of its first element**; its count and
  length sit in the sixteen bytes before it. `v[i]` is one register-indexed
  load, where the header in front cost an `add` on every element of every
  scan. Foreign code that took a buffer as a `Ptr` sees exactly what it saw.
- **Borrowed data parameters.** A list or record parameter that a function
  only matches on or reads fields of — and whose type the body never builds,
  so in-place reuse has nothing to lose — is passed without a count of its
  own; so is a binding a `match` takes out of it. Walking a tree or a list
  no longer touches a count per node.
- **`map(xs, \\x -> …)` over numbers or flat records is a loop**: no
  closure, no call per element, the result built forward with no second pass.
- `rill fmt` lost the body of `v[i] = for … with …` — it printed `...`. Found
  when it ate a benchmark. Fixed.

## Unreleased — a socket that does not wait to be asked twice

`sock_nodelay(fd)` turns Nagle's algorithm off, and says whether the kernel
took it. It is a call rather than a default because it is a real trade: Nagle
holds a small segment until the previous one has been acknowledged, which is
right for a file being copied and wrong the moment a program sends small
messages on a clock faster than the round trip — every message then waits on
the one before it, and the wait is the difference between the two.

Found while measuring a game server: nothing in the language could reach the
option, so the delay was neither visible in the code nor measurable from
inside the program. On the valley's own numbers it costs nothing today — the
tick is fifty milliseconds and the round trip thirty-six, so each write is
acknowledged before the next is built — and it starts costing the moment the
tick goes under the round trip, which is exactly the change that was being
considered.

The two constants agree across the Unixes and are written out once, unlike
`SOL_SOCKET` and its neighbours; the call is verified on macOS and on Linux,
where a descriptor that is not a socket answers 0 rather than reporting
success.

## Unreleased — a scratch handed back

The binary wire's float-to-bytes conversion took an eight-byte `alloc_bytes`
scratch per message — and `alloc_bytes` is deliberately uncounted, so every
snapshot, every input and every event broadcast left eight bytes (and a
malloc header) on the floor for ever. Invisible at a glance and unmissable in
a soak: sixteen players for nineteen minutes walked RSS from three megabytes
to sixteen in a perfectly straight line, about half a megabyte per ninety-
second session, while an idle server and six hundred join/leave cycles both
held flat — which is what pointed the finger at the traffic rather than the
connections. Every scratch is now `free_ptr`'d before its function returns,
and the same soak holds level after warm-up. The test rides along as
`wt_load.rill` (sixteen concurrent players) and `wt_churn.rill` (three
hundred joins and leaves), both against a live server, both read with `ps`.

Two production faults in one evening, each found by somebody standing at the
door of a live server.

- **A ring cursor outlived its match.** The server's tick reads every ring
  cursor before the world steps and sends the difference after; `new_match`
  empties the rings. When the step that ends a match falls between those two,
  the old cursor is a count the new match may never reach, and the send loop —
  which walks upward until it *equals* the cursor — walks for ever,
  broadcasting as it goes. The hub never comes back: nobody new is admitted,
  nobody quiet is dropped, and the process reads as one pegged core with no
  syscalls, which is how `perf` found it (`broadcast`, `strbuf_byte`, `offer`,
  round and round). `since(from, now)` clamps a cursor from behind a reset to
  nought, where the new match's events genuinely begin.
- **`ws.rill` read header names with their case on.** Header names are
  case-insensitive (RFC 7230) and always were; what changed is who was asking.
  HTTP/2 lowercases every header it carries, and a handshake that has been
  through Cloudflare or an h2-fronted nginx arrives as `sec-websocket-key:` —
  which a case-sensitive `str_find` does not find, so the upgrade was refused
  by silence and nginx turned the silence into a 502. The name is now matched
  case-blind and the value still read from the original bytes, because a
  base64 key is exactly the kind of value case matters to.

## 0.5.0 — the words a program reaches for

The release where the language grew its everyday vocabulary. `Map(k, v)`,
`p.x`, a guard after a pattern, `?` for the error that only wants passing on,
`add(2, _)` for the argument left for later, and a character layer over the
byte strings — none of them new ideas, all of them words a program reaches for
in its first ten lines, and each arrived because a real program had just
reached for it and found it missing. The sections below this one tell each of
them properly.

Underneath them, two promises were made hard. A tail call is `musttail` on its
own calling convention now — a jump by contract rather than by an optimizer's
mood — which is what lets the valley's server recurse for a day without a
stack. And the compiler stopped being a closed box: `rill check` reports
everything one pass can find, `rill types` says what was inferred, and
`rill lsp` hands both to an editor.

The valley kept being the proving ground, and the release closes with two
things it asked for:

- **The wire is binary.** One message per frame, little-endian, and the first
  byte is the letter the text protocol used — a snapshot still starts with an
  `s` in a hexdump. The layouts live in one place, `server.rill`'s "bytes on
  the wire" section, and `net.js` mirrors them field for field; a snapshot
  went from about 70 bytes a player to 37, and neither end pays for
  `float_to_str` or the parse back. `ws.rill` learned to send binary frames
  (`ws_write_bin`, `client_frame_bin`) without giving up text, and
  `wt_wire.rill` is a client that checks the whole door sequence against a
  running server, byte by byte.
- **A shot-at wall settles.** Stones above the hole used to keep their height,
  hanging in the air on nothing. Now every break packs the column down — each
  standing stone drops to sit on however many standing stones are left below
  it, on the ground if there are none. Computed from the broken set alone,
  like the rubble's resting place, so a client that arrives an hour late and
  replays the history ends with the same wall as everybody who watched it come
  down. Settling happens after the whole blast has had its say: settling
  mid-loop fed the survivors downward into the blast sphere one course at a
  time, and one rocket ate the column to the top.

## Unreleased — an argument left for later

`\x -> add(2, x)` is a lot of notation for very little thought.

- **`add(2, _)`** is the function you get by leaving that argument to be given
  later. Written out as the lambda it means, in `lift`, so everything after that
  pass sees an ordinary closure.
- **Any position**, not only the ones at the end: `bol(_, 2)` divides *by* two,
  which currying cannot express without writing the lambda out. Two underscores
  make a function of two arguments, in the order they were left.
- **Constructors and builtins too**: `Daire(_)` and `int_to_str(_)`. The second
  matters — a builtin was not a value at all before, so `map(l, int_to_str)`
  said "unknown variable" and there was nothing to write instead but a lambda.
- **Too few arguments is still an error.** `add(2)` says so. That is the reason
  for the underscore rather than bare under-application: otherwise every
  miscounted argument list becomes a value of the wrong type and the complaint
  arrives several functions away.

Not yet: calling the result immediately. `add(2, _)(5)` does not parse, because
calling what a call returned is not a shape the parser has.

## Unreleased — a field is answered at the end, not where it was written

`p.x` used to be resolved the moment it was read, so it depended on the order
the statements were in: `a = uzak(p)` then `p.x` compiled, and the same two
lines the other way round did not, though the second line says exactly as much
about `p` as the first. Reordering two lines changed whether a function
compiled.

- **`.x` is deferred.** The field name is carried on the node and an obligation
  is recorded against the type; both are settled once the whole dependency group
  is inferred, which is the first moment the answer can exist. Settling happens
  before the numeric and ordering requirements are discharged, since `a = p.x`
  followed by `a + 1` leaves a requirement on whatever `a` is and only the field
  says what that is.
- **The annotation is still needed** where inference never learns the type — a
  function meant to work on any record — and that is what the error says now,
  at the end, having actually looked.
- Record update is *not* deferred: it needs the whole field list, and the shape
  of its expansion depends on the answer, so `{ p | x = 1 }` still wants the
  type known where it is written.

## Unreleased — three things a match could not say

- **`1 | 2 -> ...`**: arms that go the same way say so once. `lift` turns one
  arm holding alternatives into one arm per alternative before anything else
  sees it, so exhaustiveness, reachability and guards go on working unchanged.
  Alternatives must bind the same names — the body is written once, so a name it
  uses has to mean something whichever alternative was taken. Top of an arm
  only.
- **`hepsi @ Daire(r) -> hepsi`**: names the whole of what a pattern matched, so
  an arm can take a value apart to ask about it and hand back the value it was
  given rather than building an equal one. Nests, unlike `|`.
- **`{ p | x = 9 }`**: every field `p` had, apart from those named. Becomes a
  call to the constructor; the base is bound first, so it is read once however
  many fields are carried over. Needs a single-constructor type, the rule `.x`
  already has.

A name is a binding and not a test, so everything that asks a pattern what it
tests for reads through the `@` first. One place deliberately does not: the
optimisation that lets an arm build over the box it took apart, which an `@`
name is precisely something that can still be holding.

Also: the rejected-source test helpers all wrote to one file per process, so two
of them running at once — the harness runs tests in parallel — could read back
each other's source. Latent until three more callers made it show.

## Unreleased — a byte is not a character

`#s` is ten for `"günaydın"`, which is eight letters, and `s[0:2]` cut the `ü`
in half and handed back something that was not text. Bytes were all there were.

- **A character layer in the prelude**: `char_count`, `char_at`, `char_sub`,
  `char_take`, `char_drop`, `chars`, `from_char`, `from_chars`, and
  `char_decode` / `char_wide` / `char_offset` for stepping through byte offsets
  without counting from the start each time.
- **`#s`, `s[i]` and `s[i:j]` still mean bytes**, deliberately. A lexer wants
  bytes and a wire protocol wants bytes, and the compiler that compiles the
  prelude is one of them: changing what `#` counts would change what all of
  them count. The characters say what they are in their names instead.
- **`from_char` spells a codepoint in UTF-8**, one to four bytes. `chr` writes a
  single low byte and still does — right for building bytes, wrong for every
  character past 127.
- Written in Rill on top of `str_get` and `str_sub`, so the runtime is
  unchanged; malformed input terminates and never reads past the end of the
  string; and nothing that does not call it pays for it — a wasm build of the
  valley came out byte-identical.

## Unreleased — a tail call is a jump, and now it is a promise

The valley server segfaulted three times in two hours, disconnecting everybody
in it each time. The core said what it was: eight thousand stacked frames of
`reader`, `gate_on`, `handle_line`, `route` — the loop that reads one
connection, every call in it in tail position, every one of them supposed to
be a jump.

- **Rill's own functions use `tailcc`**, LLVM's guaranteed-tail-call
  convention, and a tail call is emitted as `musttail`. The C convention could
  not keep the promise this language makes about tail calls: `tail` is a hint
  there, and the optimizer may take it away. It does so in a shape that is not
  rare at all — when every path through a loop returns the same value, the
  returns are folded into one block, and a call that no longer ends its
  function is not one the backend will jump to. That is what every loop
  written for its effects looks like, and it cost one frame per message until
  the strand ran off the end of its stack. `musttail` is an invariant instead
  of a hint: no pass may break it, and under `tailcc` the two functions need
  not even share a signature — which is what a loop written as several
  functions calling each other needs.
- **Everything the runtime calls keeps the C convention**: `main`, whose
  address goes to the scheduler; the body of a `spawn`; an environment's drop
  function; a map's vtable; and every trampoline C itself calls. A closure is
  reached through a pointer, so lambdas, the shims that wrap named functions,
  and the indirect call itself all use the one convention and can be sure of
  each other. On wasm nothing changes: a tail call there needs a feature the
  module may not be allowed to declare.
- Measured on the machine it mattered on: before, the reader cycle compiled to
  four calls and five jumps on x86-64, and the server died at about eight
  thousand messages on one connection. After, it is one call — the one that
  enters the loop — and eight jumps, and the deployed valley took twenty-four
  thousand messages on one connection without growing.
- `a_tail_call_cycle_runs_in_constant_stack` is the test that would have
  caught it: three functions calling each other in tail position, two hundred
  thousand turns, every path returning the same thing.

## Unreleased — the compiler, kept open

A language that infers everything is a language whose types are somewhere the
program does not say them. Three things say them now.

- **`rill check` reports everything one pass can find** rather than the first
  thing. A function whose body does not check is recorded and stepped over —
  its signature stays provisional, which is a fresh variable at every call, so
  the functions after it mostly check as though it were fine and each says its
  own first problem. Three mistakes take one run to learn about.
- **`rill types <file.rill>`** prints the signature the checker worked out for
  every function in the file, with the requirements the body imposed: `fn
  merge(a: List('a), b: List('a)) -> List('a)    # where 'a is ordered`, over a
  source line that says nothing about being ordered.
- **`rill lsp`** is a language server, built into the compiler and speaking the
  protocol on stdin and stdout. Diagnostics on the buffer as it stands —
  nothing has to be saved, since the loader will take the entry file's text
  from memory — the type of the name under the cursor, go to definition, an
  outline, and formatting. `editors/README.md` has the three lines each that
  Neovim, Helix and Emacs need; VS Code still wants a client extension, which
  is not here yet.
- What the server answers is worked out from **names rather than positions**,
  because the AST records the line a thing is on and not the column. So a
  diagnostic underlines a line and hover reads the identifier written under the
  cursor. That is exact for anything declared at the top level and right for a
  local wherever one name in a function means one thing; a name bound twice in
  one body shows the first. Columns in the tree would fix that, and are a
  bigger change than this one was.
- The protocol is spoken without a dependency: `crates/rill_cli/src/json.rs` is
  a value, a parser and an escaper, which is as much JSON as `initialize` and
  `textDocument/hover` need between them. This toolchain has one dependency and
  it is LLVM.

## Unreleased — a loop inside a loop counted the wrong one

- **Nested `for` loops were wrong, and had been since `for` existed.** Both
  the end of the range and the loop function it expands to were named the same
  thing every time — `for.hi` and `for` — so an inner loop's end answered for
  the outer one's: `for i in 0..3` around `for j in 0..5` ran the outer body
  five times, and every count above it was wrong by whatever the inner loop
  said. Each loop takes its names from the same pool the lifted functions use
  now, so each is its own. Three levels deep, `with` inside plain, and two
  loops both counting with `i` all give the answers they read as.
- `examples/loops.rill` gained the `for` form it never had and the nested
  loop that would have caught this, and stopped saying Rill has no `for`.

## Unreleased — guards

An arm can say what a pattern cannot.

- **`pattern if cond ->`**: the arm is taken when the pattern matches and the
  condition holds, and a false one falls through to the arms below — which the
  value is then matched against as though the guarded arm were not written.
- **A guarded arm covers nothing.** `Cons(x, _) if x > 0` leaves `Cons(_, _)`
  to be answered, and a match that does not answer it is not exhaustive; the
  checker names the case as it always did. The other side of the same rule:
  nothing below a guarded arm becomes unreachable, while an unguarded arm above
  still makes things unreachable.
- **The decision tree keeps its shape.** A guarded leaf carries what to do when
  the condition is false — the rest of the match, built in the same value
  context — so an unguarded match compiles exactly as it did, and a guarded one
  pays by testing the arms below it again rather than by going linear. A guard
  is asked at the leaf, with the pattern's bindings borrowed out of the box the
  way the arm's own are, which is what keeps a tail call in a guarded arm a
  tail call: two million of them run in constant stack.
- `examples/patterns.rill` gained three functions and the sentence that goes
  with them.

## Unreleased — fields have names, and names can be read

A constructor's fields could always carry names — `P(x: Int, y: Int)` says
which number is which — and nothing but a human reader could use them. They
are a read now.

- **`p.x`**, on any type with a single constructor. That is the whole
  condition, and it is the one a binding already had: `P(x, y) = p` reads as a
  binding because it cannot fail, and `p.x` for the same reason. A type with
  several constructors has no field that is certainly there, and the error says
  so, naming them.
- **Anywhere, not only on a name.** `mk(1).y`, `p.pos.x`, `find(n)?.y`. A dot
  written against a name is still part of that name as far as the lexer is
  concerned — `geo.area` is one too — and the checker tells a module alias from
  a value with fields; a dot after anything else is a token of its own now,
  which is what makes `f(x).y` parse.
- **A field is read, never written.** `p.x = 3` used to parse as a binding
  called `p.x` and quietly shadow nothing at all; it is refused now, and says
  what to do instead: build the record you want, which is what changing a value
  means.
- **The one thing it asks** is that the type be known where the dot is written.
  Inference does not work backwards from a field name — two types may both have
  an `x` — so a parameter read this way is annotated, and the error names it.
- `examples/records.rill` is a player, a position inside it, and the four
  lines that make a hurt one; `CtorSig` keeps field names for it.

## Unreleased — `?`

Errors are `Result` values and always were. What that cost was a `match` per
call, two of whose three lines were the same two lines every time.

- **`e?` is the value inside the `Ok`, or else the whole function's `Err`.** It
  is a `match` and nothing else: `lift` takes the rest of the block as the `Ok`
  arm and adds `Err(e) -> Err(e)`, before the checker sees the program. Nothing
  is added at run time, an error travels no further than it did, and a tail
  call written after a `?` is still a tail call — two million of them run in
  constant stack.
- **Anywhere in an expression.** `number(field(text, "port")?)?` names the
  inner result first, in the order the expression evaluates. A statement of its
  own (`field(text, "host")?`) and a binding that takes the value apart
  (`(a, b) = pair()?`) work too.
- **One rule, and it is the one the desugaring implies**: a `?` belongs where
  the block it is in is what the function returns, since that is the door its
  `Err` leaves by. In a branch or an arm that *is* the function's answer, it
  works. Written where the block is worth something else, the error names that
  something else — the generated `match` never appears in a message, because a
  program that did not write it should not have to read it.
- **On anything but a `Result`**, the error says what to write instead:
  `match` on an `Option`, or `unwrap_or` for a default. There is no `From`
  conversion between error types either, and none is wanted: `Err(e) -> Err(e)`
  carries the same value out.
- `examples/results.rill` is the whole of it — a four-step parse where every
  step can fail, the failures coming out of the middle with nothing written to
  carry them. And `examples/showcase.rill`'s interpreter lost its `eval2`: the
  five lines that evaluated both sides of a binary operator and passed the
  first error along are three now, and print what they always printed.

## Unreleased — a buffer is its address where C wants one

`buf_data(b)` said the same thing at fifty call sites: *the first element's
address*. It is now what handing the buffer over means.

- **A `Buf` decays to a `Ptr` where one is wanted** — a foreign parameter, a
  `peek` or a `poke`, a Rill parameter annotated `Ptr`. This is C's own rule
  for an array, and it is the only place in the language where a value is not
  the type it was written as. `glGetShaderiv(sh, status_name(), status)` reads
  the way the C does, and `status[0]` reads it back.
- **`buf_data` is gone**, and the fifty calls to it with it. Nothing else
  changed at those sites: the buffer stays counted, stays the caller's, and
  the address it lends is still only good for the call.
- **A buffer built in the argument no longer dangles.** `strlen(buf_u8(4))`
  used to hand C an address and release the buffer before the call — the block
  went back to the pool with C still reading it. A buffer decayed at a call is
  released *after* the call now, which is the only reason the shorter spelling
  is safe to encourage. `examples/cocoa/cocoa.rill` had a comment warning
  about exactly this; `frame_of` hands back the buffer instead of a pointer
  into it, and the warning is gone.
- The one thing decay does not do is make a buffer a pointer anywhere else: a
  binding, a branch, a data field and a channel all still say which they hold.

## Unreleased — hash tables

Rill could look a thing up by walking a list or by keeping a sorted tree, and
both are in `examples/showcase.rill` because there was nothing else. `Map(k, v)`
is the thing that was missing: open addressing, linear probing, and a key type
the compiler works the hashing out for.

- **`Map(k, v)`**, the second thing in the language that is storage rather than
  a value. `map_new()` makes one, `m[k] = v` inserts or replaces, `m[k]` answers
  `Some(v)` or `None` — a key may not be there, and absence is an `Option` here
  as it is everywhere else — `#m` is how many entries, and `map_del`,
  `map_has`, `map_or`, `map_keys`, `map_values`, `map_items`, `map_of` and
  `map_clear` are the rest of it. Both type arguments are inferred; unlike a
  buffer's width they are ordinary types.
- **Any value type is a key.** A number, a string, a tuple, a data type, nested
  as deep as it likes. The compiler generates `hash$T` beside the `eq$T` it
  already generated, walking the same fields in the same order — which is what
  makes the two agree. A function or a channel is refused where the key is
  written, not somewhere later.
- **One table for every instantiation.** The probing loop is in the runtime and
  knows nothing about what it holds; what it cannot know — hashing a key,
  comparing two, counting a key's and a value's references, and how wide a
  value is — is compiled per instantiation into a constant vtable beside the
  program.
- **Seventeen bytes an entry.** One control byte says whether a slot holds
  anything and, if it does, carries seven bits of the key's hash; the key and
  the value are sixteen more, side by side. A probe walks the control bytes —
  two megabytes for two million slots, which stays in cache while the slots
  cannot — and touches a slot once, for the one key it believes it has found.
  A key is one word, because what has structural equality fits in one; a value
  is one too unless it is a closure, which is the only Rill value needing its
  code and its environment, and which of the two comes from the vtable. Not
  keeping the whole hash costs a rehash of every key when the table grows,
  which is about a tenth of the time it takes to fill one.
- **Counted like a string.** What a table holds is released when the table is,
  including the entries a `map_del` or a `map_clear` drops. Every map program in
  the tests ends on `live allocations: 0`, including one that inserts, deletes
  and reinserts five hundred keys.
- **Ahead of Go's map on all three.** A million `Int` keys: 26 ms to insert
  against Go's 61, 22 ms to read back against Go's 23, peaking at 73 MB against
  Go's 85, on an M4. Half a million `Str` keys, string building included: 28 ms
  against 58, 20 ms against 37, 53 MB against 67. `map_or(m, k, d)` is the read
  for a hot loop — `unwrap_or(m[k], 0)` builds a `Some` to take apart again and
  costs about a third more.
- **Two errors say what to do.** `println(m)` is refused like a buffer, and
  points at `map_items(m)`. `m[k] += 1` expands to `m[k] = m[k] + 1`, where an
  `Option` meets a number — the message names `unwrap_or(m[k], 0)` rather than
  leaving `Option(Int)` against `Int` to be worked out.
- **wasm found an ABI sloppiness native had tolerated.** A vtable's value
  retain and release are called through a pointer, so they take two words
  whatever the value is stored in — an extra argument a native call ignores is
  a signature mismatch to wasm, and traps. Both are two words now, and the
  first `wasm32-wasip1` build of `examples/maps.rill` printed what the native
  one does.
- Codegen's own type substitution never looked inside a `Chan(a)`, so a
  generic function taking one was monomorphized with the element type left as a
  parameter. Nothing had ever depended on it — releasing a channel is the same
  whatever it carries — but a map's key type decides how the key is hashed, and
  found it at once. Both shapes substitute now.

## 0.4.0 — a valley with other people in it

The release where a Rill program was handed to strangers. `examples/webgpu/world`
went from one browser walking around its own valley to one server holding a
single world for everybody in it: the crystals, the rounds in the air, the
craters, the shark and every player's health are the server's, and a browser is
told about them rather than deciding any of it for itself. It is a few hundred
lines of Rill on a socket, published at a name, with the word at the door and
people who are not us behind it.

Nothing sharpens a runtime like that. The list below is mostly the language
being found out by a program that could not be restarted quietly: a write to a
peer that had gone raised `SIGPIPE` and killed the process, a read was assumed
to be a frame and was not, a socket read had no deadline so a client that
connected and said nothing held a strand for ever, a per-second budget stopped
parsing but not reading and starved everybody honest, and — the one that took
longest — a strand parked on a channel and a strand parked on a socket looked
identical to the poller, so an event that had outlived its wait delivered a
value that had never been sent. That last one read as heap corruption for weeks
and was nothing of the kind.

The rest is the game: a skinned figure with a rifle that points where it is
aimed, damage that depends on where the round lands, a scope, a roster, spawns
that are not all the same patch of grass, and three defences against a modified
client — a shot has to go roughly where its owner is looking, a name belongs to
a token rather than to whoever typed it first, and a client is no longer told
where somebody it cannot see is standing. The last of those is the only one a
client cannot work around, because it is not a rule it is asked to follow.

- **A client is no longer told where it cannot see.** The snapshot carried
  everybody's position to everybody, and no amount of care in the renderer can
  unsay that: a browser that has been given a coordinate can draw a figure
  through a hill. The server now works the line of sight out per reader —
  eye to eye, sampled against the terrain every three metres — and a player
  behind the ground arrives with their position blanked and a zero on the end
  of their block.

  They stay in the roster: who is in the valley is not a secret from anyone
  standing in it, and health and the two counts are not positions. The cost is
  the one-line broadcast, which becomes one string per reader; measured with
  everybody running about it does not show, 20 snapshots a second either way
  and one per cent of a core at twenty-four players. What it buys is the
  terrain's to give, and this terrain is a bowl — about one player in twenty is
  blanked at any moment. It is also the only one of the three defences a
  modified client cannot work around, because it is not a rule it is asked to
  follow.

- **A name belongs to somebody now.** The server mints a token on a browser's
  first visit, hands it back in `hello`, and the browser sends it in the query
  from then on; `who.txt` holds one `<token> <name>` line per person and the
  name is refused to everybody else, including while its owner is away. The
  entropy is half the clock and half the sixteen random bytes a browser puts in
  `Sec-WebSocket-Key` — the clock so one client cannot get the same token
  twice, the key so nobody else can work out what it was.

  It is not an account: never typed, not recoverable, and whoever copies it out
  of a browser is that player. It buys two things that were impossible while
  everybody was anonymous — a name that stays yours, and `banned.txt`, read on
  every knock so that saying somebody is not welcome does not need a restart.

- **`write_file` could not create a file on Linux, and never could.** The
  three flags it hands `open` were the numbers a Mac uses, and the difference
  is not that Linux calls them something else — it is that the same numbers
  mean other things there. `O_CREAT | O_TRUNC` from BSD reads on Linux as
  `O_TRUNC | O_APPEND`, with nothing saying create. Writing over a file that
  existed worked, which is why nothing noticed for so long; the first program
  to want a file that was not there yet found out. `examples/io.rill` now
  writes to a path nothing has used before, so creating is what the test tests.

- **`fn bind` in a Rill program takes the name away from the C library.** Top
  level functions are emitted under their own names, so a program that defines
  one the runtime calls through libc replaces it — and the failure is a socket
  call landing in your own function, which is a segfault at startup with a
  stack that makes no sense. Renamed here; the compiler should be the one
  saying it, and does not yet.

- **The server used to fall over about one arrival in ten, and it was the
  scheduler.** A strand parked on a socket and a strand parked on a channel
  both had `selected` set to -1, and the poller took that one flag to mean
  "waiting on me". A registration can outlive the wait that made it — the
  strand was woken by the other half of a two-part wait, or the descriptor was
  handed on — and when that event was finally delivered, the strand named in it
  had usually gone on to `recv` on a channel. The poller made it runnable, and
  `recv` returned the two zero words its waiter was initialised with: a null
  string, handed to the first thing that asks a received line for its length.

  It read as heap corruption and it was not: nothing was corrupt, a value was
  delivered that had never been sent. Strands now say which of the two waits
  they are in, so an event with no owner is dropped. Eight crashes in ten runs
  of a connect-shoot-hang-up loop, then none in ten. The count of dropped
  events is in the `RILL_DEBUG_ALLOC` report, because it is a real thing that
  happens; what would be an error is acting on one.

- **A run of shots that go nowhere near where the shooter is looking now costs
  the connection.** Refusing them was the defence; counting them is what makes
  it worth having. Our client cannot produce one — it works the direction out
  from the same camera matrix it just sent the look from — so twelve in a row
  is not a bad connection, it is something other than our client on the socket.
  Out the same door as anyone who has gone quiet. It is a streak, not a total,
  and it is never logged as it climbs: a client can ask to fire a hundred and
  fifty times a second, and a line each would be a way to fill the disk from
  outside.

- **A round has to go roughly where its owner is looking.** The look and the
  shot arrive on separate lines and the server kept only the latest of each, so
  a client could face north and put a round through somebody standing south of
  it while everybody else watched it stare into the distance. Twenty degrees of
  slack now — enough for a frame's lag on a fast turn, not enough to shoot
  behind yourself. It does not stop anybody aiming perfectly, which is between
  a player and their own mouse and which no server can see; it makes them turn
  to do it, and turning is the part everybody else can see. `check.rill` fires
  one with the shooter facing the other way and it is refused.

- **Keys are masked to the seven that mean anything.** What arrives is whatever
  somebody typed into a socket, and a key that is not a key should not become
  one the first time something new is hung off the eighth bit.

- **Everybody comes in somewhere else.** The spawn was a spiral round one patch
  of grass, which put everybody within sight of each other the moment they
  arrived and put whoever had just been shot back where they fell, in front of
  whoever shot them. It is a hash of who is arriving and when now, tried until
  it lands on grass between the waterline and the rocks — dry, walkable, and
  never the same twice.

- **The top of a crystal is a crystal.** The four spheres that stood in for one
  left two holes: from 1.68 to 1.87 of its height, and from 2.13 to the tip at
  2.40. The second one is the one anybody would notice, because the top tenth
  of a crystal is the part that stands clear of everything around it and is
  therefore exactly what people shoot at. Six spheres now, each at least as
  wide as the crystal is there and at least half the distance to the next, so
  the chain cannot have a hole in it.

- **A log down the bottom left.** Everything that happened, oldest at the top,
  each line fading on its own clock rather than the list scrolling — a line
  still being read should not move because a new one arrived. And an
  announcement across the middle when somebody arrives or leaves, in the same
  shape as the hit line: in from half again its size, out by shrinking.

- **Where a round lands is now the whole of it.** Three spheres stacked and
  tested from the top down, because the smallest is worth the most: the head
  ends it outright, the body takes a third, the legs take a tenth. The
  difficulty of this rifle has moved from how many times it is fired to where
  it is pointed, which is the difference between a shot and a sequence — and it
  is what the scope is for. Rounds stop at a leg on the page as well, from the
  same three spheres, so a man behind a rock is hit where he is showing.

- **And a voice.** Being hit says so out loud: four of qubodup's CC0 recordings
  of himself straining, from OpenGameArt, a second each end to end in one 33 KB
  file and played by offset the way the glass breaks are. Louder for a bigger
  hit, and a hair of detune each time — a leg and a head are different things
  to have happen to you and should not sound the same, and the same file twice
  in a row is the one thing that gives a sample away.

  The line pops in and shrinks away rather than appearing: a line that simply
  appears is read a beat later than one that moves, and a beat is all there is.
  It goes after a second, since it is one sentence about something that has
  already happened and it is sitting over the middle of the screen.

- **Who shot you, and who you shot.** Low and in the middle, where the eye goes
  when something has happened *to* you rather than in front of you: the name in
  the valley's amber and the number bigger than the words around it. The two
  people a round concerns each get their own sentence — one of them has been
  shot and the other has hit something, which are not the same news — and the
  outgoing one is green rather than red, so which of the two it is does not
  have to be read to be known. A kill gets a skull and stays up half a second
  longer.

  Whether a hit was fatal is the server's to say and rides with it. The shooter
  cannot work it out: the snapshot that says somebody is down says nothing
  about who put them there. "Hit" is a
  different thing to be told than "hit for a third of you", and in a valley
  where four people are firing there is otherwise no way to know whether the
  round that took you down was aimed at you.

  It is a ring on the world and a cursor that counts up, the same shape as
  every other thing that happens between two ticks. Sent to everybody rather
  than to the one it happened to — it is three numbers — because a client that
  knows what happened to the person beside it can say so, which is the
  difference between a valley where things happen and one where they happen
  only to you.

- **Hold the right button to aim.** The world narrows to five times: the far
  ridge is four hundred metres off and somebody standing on it becomes a figure
  rather than a suggestion. The rifle comes eleven centimetres nearer the eye
  and straightens out of its cant — close enough that the eyepiece is most of
  what there is to see, which is what an eye against a scope sees — the sway
  drops to a quarter, and the crosshair steps back to a quarter of itself,
  because behind the sights the rifle has its own and two at once is one more
  than anybody needs.

  Looking down the tube of the rifle's own scope was honest and useless: the
  far lens is a centimetre across and the eye is twenty behind it, so what
  could be seen through it was a keyhole. So at the top of the aim the rifle
  goes and the scope becomes the screen — a clear circle two thirds of the
  screen high and black outside it. It opens as the aim comes up and closes as
  it drops, and it is a radial gradient over the canvas rather than a pass that
  would have to touch every pixel, because it is in the eye and not in the
  valley.

  In the middle of it is a reticle rather than the page's crosshair: a duplex
  with a mil scale, which is what a rifle scope has and what makes one useful
  for anything besides pointing. The thick posts carry the eye to the middle
  from any background, the thin cross is what you aim with, and the ticks are a
  ruler — two mils apart, so a figure of a metre eighty subtends one of them at
  nine hundred metres and four at two hundred, and holding over is counting
  rather than guessing. Black glass over a pale edge, because a reticle that is
  only black vanishes against dark water and one that is only white vanishes
  against the ridge. The crosshair fades as the rifle comes up and is gone
  before the scope arrives: two opinions about where the middle is, is one too
  many.

  All of it eased and framerate-independent: nine tenths of the way in about
  seventy milliseconds and settled inside an eighth of a second, measured at
  sixty frames and at a hundred and forty-four. A field of view that changes
  between one frame and the next reads as a cut rather than as somebody raising
  a rifle; one that takes a sixth of a second — which is where this started —
  reads as the game thinking about it. The mouse slows with the zoom, or
  the same movement of the hand covers two and a half times the angle and the
  sights are unusable. Letting go of the pointer lets go of the sights, since a
  held button cannot report itself released once the page has stopped hearing
  about it.

  The left button still fires while the right is held, which is the whole point
  of holding it. The rifle was always drawn with its scope on the view axis, so
  none of this is a new pose — it is the same rifle, closer, steadier, and
  looking further.

- **A quarter off both speeds.** Five and a half metres a second, and twelve
  with the sprint key, down from seven and a half and sixteen. At the old pace
  the valley went past faster than anything on the ground in it could account
  for, and a shot at somebody crossing the meadow was a lead nobody could
  judge. The run clip's playback follows — √(5.625/4) and √(12/4) — so the feet
  still land under the body.

- **Who has hit whom, down the left.** Two numbers a row beside the health bar:
  rounds landed in green and rounds taken in red, the same two colours the bar
  uses, so neither needs a label. Both are the server's count and both are
  written down at the moment a round connects, because the second cannot be
  worked out from the first — a client can see somebody flinch but has no way
  of knowing whose round it was. `check.rill` shoots somebody three times and
  reads both sides.

- **A word at the door.** The shared valley asks for one before it lets anybody
  in — `135` for now. It travels in the query of the address the socket is
  opened to, so the server can refuse before it answers the handshake and
  before anything is allocated for whoever is asking, and nginx is told not to
  log that line: a password in an access log outlives the password. It is a
  latch and not a lock — one word for everybody, in plain sight inside the TLS,
  and anybody who has it can pass it on. It keeps the valley to the people who
  were told about it, which is all it is for.

  The word has to end where it ends, too: `135` matching as a prefix would have
  admitted `1350`, and a word of `1` would have admitted everything.

  Giving the wrong one leaves you in a valley of your own, and now it says so
  rather than dropping you there quietly to wander about wondering where
  everybody went.

- **Nobody was being interpolated.** The position between two snapshots was
  measured from the *older* of the two, so by the first frame after one arrived
  the sum was already past the newer one and the clamp pinned it at the end.
  Everybody stood still for a twentieth of a second and then jumped, twenty
  times a second — which on a fast screen reads as a flicker between positions
  rather than as walking. Measured from the newer one, which is what the
  comment above it always claimed, every frame moves: at 60 Hz not one frame in
  three hundred stood still, and the largest step was 1.3× the median.

  The interval between snapshots is smoothed as well, so one late packet does
  not make everybody sprint to catch up and one early one does not leave them
  standing.

- **Nobody walks in this valley.** Moving is seven and a half metres a second,
  which is faster than anybody has ever run, and the walk clip under it made
  the legs stroll while the body flew. The run clip covers both speeds now, at
  two rates: the clip is a run at about four metres a second, and cadence goes
  about as the square root of speed — √(7.5/4) and √(16/4) — so the feet stay
  under the body at both. The clip's own time is carried forward frame by frame
  rather than worked out from when it started, or pressing sprint would throw a
  runner four seconds down the cycle mid-stride.

- **And the run looks like running.** The armed pose was a flag: every joint
  from the spine up came wholly from the armed idle, so the legs pumped
  underneath a torso that did not move at all — the rifle was steady and the
  man carrying it was a shop dummy on rails. It is a weight now, one number a
  joint, baked from the rig: the arms and hands keep the weapon, the shoulders
  keep a little of the stride, the spine and head keep a third of it, and the
  pelvis keeps about half its lean. The barrel dips twelve to eighteen degrees
  through a running stride instead of nought or twenty-nine, and the chest
  moves twenty centimetres through the cycle instead of standing still.

- **A player on a fast screen watched their own ping climb past half a minute.**
  144 fps, and the page sent one input a *frame* — 144 messages a second against
  an allowance of ninety. Fifty-four a second went unread, waited in the kernel
  buffer, and everything sent after them waited behind: the ping is a message
  like any other, so what came back was the backlog and not the network. It read
  36 seconds and was still rising.

  Two things were wrong. The page now sends sixty a second whatever the screen
  is doing — already three times the tick rate, since the server walks from the
  last thing it heard, so an input between two ticks is the one that counts at
  the next. And the allowance is a hundred and fifty rather than ninety, with
  room above the sixty for the shots and the pings and for any client that does
  not know about the cap.

  Measured before and after with a client sending a hundred and twenty a second:
  the round trip read 1,014 ms and climbing, and now reads 0. A flood of a
  million lines a second is still held off — the honest client keeps its full
  twenty snapshots a second through it.

- **The published page joins the published valley.** `?join` with nothing after
  it used to mean `ws://<host>:8091/`, which on a page served over TLS is two
  things a browser will not do: a plain socket from an encrypted page, and a
  port that is not open. It now follows the page it is on — `wss://<host>/valley`
  over https, and straight at the port over plain http, which is how it runs
  while it is being worked on. One path, no port, and the certificate is the
  one the page already came over.

- **Behind a proxy, the valley knows who is really there.** Put nginx in front
  — which is what `wss://` requires, since the page is served over TLS and a
  browser will not open a plain socket from an encrypted one — and every
  connection arrives from 127.0.0.1. The limit of sixteen per address would
  then have counted the whole internet as one house and turned away the
  seventeenth person in the world.

  So the address is taken from `X-Real-IP` when, and only when, the socket
  itself is the loopback: on that machine nothing but nginx can reach the port,
  and a header is something the client writes — one that could name its own
  address would invent a new one per connection and walk round the limit
  entirely. Checked both ways: seventeen through the proxy from one machine and
  the seventeenth is refused by its real address, eighteen straight at the port
  with a different invented address each and every one of them still counts as
  the socket it came from.

  The per-address gate moved from `accept` to after the request is read, since
  that is when there is a request to read it from. The total gate stays at the
  door, so a flood of half-open handshakes still cannot get past
  `max_connections()`.

- **A dead socket killed the server.** Writing to a peer that has gone raises
  SIGPIPE, and nothing in a Rill binary handles it: the process died where it
  stood, no panic, no message, exit 141. Rust's own runtime turns SIGPIPE off
  before `main`, but a Rill binary's main is generated code and never goes
  through it. So any client that walked away between one snapshot and the next
  could take the valley down with it — which is what a public port is full of,
  and it is almost certainly the unreproducible death seen once during testing
  a while back.

  It is turned off per socket rather than for the process, because a program
  piping into `head` should still stop when `head` does and that is a pipe.
  BSD says it once with `SO_NOSIGPIPE`; Linux has no such option and says it on
  every send with `MSG_NOSIGNAL`.

- **A flood of messages stopped starving everybody else.** The per-second
  allowance stopped the hub being bothered by the ten-thousandth message and
  did not stop the reader from working: the frame walk and the unmask happen
  before anything can decide a line is unwanted, so a client sending a million
  lines a second was still being read a million times a second. An honest
  player in the same valley got **no snapshots at all** for as long as it went
  on. Over the allowance the reader now sleeps out the rest of the second
  instead, which leaves the frames in the kernel buffer where they belong, the
  window fills, and the sender's own writes start blocking. Measured: the
  honest client went from 0.0 snapshots a second under the flood to a full
  20.0, and the flooder got 32,000 lines away instead of a million.

- **Renaming stopped being an amplifier.** Every accepted name goes out to
  everybody in the valley, so one client renaming as fast as it was allowed to
  speak was ninety lines a second times everybody here — the cheapest
  amplifier in the protocol. A name that is not different is not news now, and
  nobody may change theirs more than once every three seconds. Measured: 54.2
  broadcast lines a second down to 0.6, with the same client still asking 108
  times a second.

- **Ten seconds on the ground.** Running out of health in a shared valley used
  to be a frame: the server stood you back up on the next tick, so the die clip
  never played and being killed was a teleport with a health bar behind it. Now
  you go over, you stay there for ten seconds, and then you are back at the
  spawn — long enough that it is an event, that your rifle is in the grass
  beside you and that whoever shot you walks past, short enough that it is not
  a punishment.

  The camera goes down with the body. The eye falls from a metre seventy-five
  to a metre thirty-five below that, the horizon rolls a quarter turn, and it
  goes over on the side the last round threw the head — so the fall finishes
  the motion being shot started. A camera that cut to ankle height would read
  as a bug and one that stayed at eye level while the figure fell would read as
  a ghost, so it eases over three quarters of a second, smooth at both ends.

  On screen: **YOU ARE DEAD**, and the seconds counting down under it in the
  largest numerals on the page. The count is the point of the screen — without
  it, being dead is indistinguishable from being broken. It counts against the
  same clock the server decides on, rather than a timer of its own that agrees
  for a while.

  The rifle goes with them, and it goes on the ground. No viewmodel while you
  are down — a gun held out at arm's length in front of a camera lying in the
  grass is the one thing left on that screen still behaving as though nothing
  had happened — and in its place a rifle lying in the grass a pace ahead of
  where you fell, on its side, turned a little off the way you were facing,
  because one that landed square with its owner reads as placed rather than
  dropped. Everybody else's does the same: the die clip puts the hand on the
  ground and a rifle still gripped in it is a body holding its weapon at
  attention while lying dead.

  This player's own is the one rifle in the valley with nobody to hang it off —
  you are the camera, there is no figure and so no hand — so the skinner keeps
  one spare matrix past the last person for it.

  Somebody on the ground does not walk, does not look about and cannot fire.
  All three are in `walk`, which both ends run, so the keys a dead client keeps
  sending are ignored the same way in both places rather than only where
  somebody thought to check. Getting up is a decision and so is the server's;
  the clock is kept by both, because a client needs it for the camera and for
  the number.

  Alone in your own valley none of this happens: there, running out of health
  is the end of the thing and there is already a screen that says so.

- **The rifle stopped lying on its side.** The grip was levelled against the
  bind pose, which is the one pose in the whole library where nobody is holding
  anything: a bind pose has the palms turned down, every armed pose turns the
  wrist ninety degrees to grip, and a rifle squared up against the first
  therefore spends the entire game on its side. It is levelled against the pose
  people actually stand in now, and the barrel is laid along the direction the
  *figure* faces rather than the direction the forearm points — those differ by
  ten degrees, and ten degrees is the difference between a rifle aimed at what
  its owner is aimed at and one that is not. Everywhere else it follows the
  wrist, which is what a wrist is for. Standing and firing, the barrel now
  comes out exactly level and exactly forward.

- **Walking with it up.** The library has a walk, a run and a crouch, and an
  armed idle, and no armed anything else — so in those three the arms swung
  like the empty arms they were animated as, and the rifle swung with them. The
  legs now take the clip somebody is actually doing and everything from the
  spine up takes the armed idle. The hips are below the split, so the pelvis
  still sways with the stride and carries the torso rather than nailing it to
  the world; that is why this is a mask on the skeleton and not two clips faded
  together. Walking, the barrel went from forty-nine degrees into the ground to
  five, and level.

  The pelvis is masked with the arms, and it is the interesting one. Its
  rotation is what pitches a running body forward and the whole torso rides on
  it — masked from the spine alone, the rifle came out level and pointing
  twenty-five degrees into the ground, tipped by a joint underneath the mask.
  Its children stay behind, because the legs hang off the pelvis rather than
  off the spine, so they still run: the knee still swings sixty-four
  centimetres at a run and twenty-three at a walk, on an upright pelvis rather
  than a leaning one. The stride's rise and fall is a translation and is not
  masked at all, so a crouching player is still down at a metre and a running
  one still bobs fourteen centimetres. Standing, walking, running, crouching —
  the barrel now comes out level and forward in all four.

  Which is why the mask goes into the file a joint at a time rather than as one
  index and a subtree: the rule has an exception in it, and the place to know
  about the exception is the tool that is reading the rig, not a page that has
  never heard of a pelvis.

- **A round stops at the person it hit.** It stopped on the server, which is
  where the damage is decided, and carried on through them on every screen,
  which is where it is watched. A client has no player blocks but its own —
  everybody else arrives as a snapshot and is drawn from it — so there was
  nothing in its world for a round to hit. The page now writes down where it is
  drawing people, and rounds stop there.

  It is not a decision and there is no health in that table: what a hit costs is
  still the server's word. And it is where they are *drawn*, interpolated
  between the last two snapshots, so a round stops at the body you can see. That
  is why it is not done off the health going down in the next snapshot instead —
  at a hundred and twenty metres a second, one tick of waiting is six metres of
  tracer past somebody's back.

- **DISCONNECTED, in the middle, in the largest letters on the page.** Losing
  the socket looked like nothing at all: the valley carried on being drawn, the
  wind carried on blowing, and this player carried on walking around a world
  that had stopped existing for everybody else. Nothing is torn down — what is
  on screen is the last thing everybody agreed about — but it says so.

- **A round throws your head about.** Being shot moved a bar in the corner and
  nothing else. Now the camera takes it: a roll, mostly, with a little pitch
  and yaw on top, settling out over four hundred milliseconds — three degrees
  of tilt reads as a head snapping sideways in a way the same angle spent on
  pitch reads only as recoil. The rifle rides it for nothing, being placed
  relative to the eye.

  Nothing announces the hit. Health only ever goes down, so the frame it goes
  down on is the frame somebody was hit — which is how the page already picks
  the hit clip for everybody else, and it means the throw works the same
  whether the damage came from a round, a blast, or a server saying so. It goes
  on the camera and not on the yaw and pitch, because a round should throw the
  view about and not turn the player: what you were aiming at is still what you
  are aiming at when it settles. `check.rill` shoots somebody and looks at the
  matrix, which is where a roll cannot hide.

- **The rifle in their hands is the size of a rifle.** It was a toothpick. The
  scale had been taken from `VM_SCALE`, which is the viewmodel's number — the
  viewmodel is placed in camera space and projected through its own frustum, so
  what looks right there says nothing about how long a rifle is in the valley.
  It is now 1.10 m, measured against the 1.83 m mannequin holding it, and the
  scale that gets it there is worked out from `rifle.mesh`'s own bounding box
  rather than written down beside it. Where the hand grips is a fraction along
  the stock rather than an offset in metres, so moving it is one number.

- **The sky is the server's too.** The sun ran on each browser's own clock, so
  two people standing next to each other were in different parts of the
  afternoon — one of them shooting into a glare the other could not see. It now
  runs on the world clock, which is the number the server sends in every
  snapshot and which the wind and the water already ride on. Alone in a valley
  that clock is this browser's own count of the frames it has drawn, which is
  what it always was: a day does not pass while a tab is hidden.

- **A rifle in everybody's hands.** The figure was firing an empty fist. What
  it holds now is the same `rifle.mesh` the viewmodel draws — one rifle in this
  valley, not two — instanced once per person off a matrix the page works out
  each frame: the hand joint's own world matrix times a grip.

  The grip is derived at bake time rather than guessed. `bake_figure.py` finds
  the right hand in the bind pose, takes the direction its own forearm points
  as the barrel's, world up as the rifle's up, and writes the result into the
  header along with which joint it hangs off — so the weapon follows the arm
  through every clip and the page knows nothing about the rig beyond one index.
  The bolt and the round do not animate out here: they belong to the gun in
  your own hands, ten centimetres from the eye, and at the distance somebody
  else is a bolt is two pixels.

  Standing still is now the armed idle rather than the empty-handed one,
  because somebody standing still is who you look at, and they are holding a
  rifle.

- **Everybody stopped walking sideways.** The figures were placed at yaw plus
  half a turn, which mirrors rather than turns — so a person walked at a right
  angle to where they were aiming, and on a mannequin that is symmetric until
  it moves the mirror was invisible. The rotation that takes the model's
  forward to the direction Rill means by a yaw is π − yaw, and nothing else
  does.

- **Who else is in here.** A roster down the left: everybody in the valley,
  the colour their figure is drawn in, what they are called, and a bar for how
  much of them is left. The tag over a head answers "who is that"; this answers
  "who am I in here with", which is a different question and not one you can
  answer by turning round. It is rebuilt only when a row arrives or leaves, so
  the bars slide rather than step.

- **How far away the server is.** A round trip, once a second, shown top right
  and coloured at the thresholds where a player would start noticing one. The
  token is the browser's own clock handed straight back, so nothing has to be
  remembered on either side. It is answered from the hub, queued behind the
  ticks and the inputs like a snapshot is, rather than from the reader strand —
  a reply sent from there would time the socket and the kernel, and the socket
  is not what anybody is waiting on.

- **The day is six minutes long.** Ninety seconds was a good number for somebody
  who would look at this for a minute and a poor one for somebody living in it:
  the shadows visibly crawled and the sun set while you were crossing the
  meadow. Three hundred and sixty is still nothing like a day and still shows a
  whole one inside a session somebody stays in, and the sky has stopped being
  something you can watch move — the light changes while you are busy and you
  notice it afterwards, which is what daylight does.

- **Other people are people now.** The blocky figure of stacked cones is gone;
  what stands in the valley is Quaternius' mannequin off the Universal
  Animation Library (CC0), with seven of that library's clips — standing,
  walking, running, crouching, firing, taking a round, and going down.

  `tools/bake_figure.py` reads the glTF and writes `figure.mesh`: fifty-three
  joints, 13,744 triangles, seven clips, 383 KB. Three facts about the source are
  what keep it there rather than at a megabyte and a half, and the baker checks
  each rather than assuming it — nothing in the rig is ever scaled, exactly one
  joint translates, and the clips are already authored on a thirty-a-second
  grid. So a frame is a root position and one rotation per joint in snorm16,
  and everything else is stored once.

  The skinning is split the way the rest of this is. The page works out one
  matrix per joint per person — fifty-three small matrix products each, which
  is nothing for eight people, and it keeps the whole animation somewhere it
  can be read — and the shader is four matrices and their weights. The matrices
  arrive already in world space, so the vertex stage has no model transform, no
  bind pose and no per-instance rotation in it. They cast shadows through the
  same skin.

  Two of those seven are events rather than states and are decided on the
  page, off what the snapshot already says. Firing is one. Being hit is the
  other, and it is not in the protocol at all: a health that went down since
  the last snapshot and has not reached zero is somebody taking a round, which
  is the same fact the server would have had to send, arriving for nothing.

  Which clip somebody is playing comes from the server, in a new field on each
  player in the snapshot. It has the keys and the health, and two people
  watching the same person should not disagree about whether they are running.
  Firing is the exception because it is an event rather than a state: a round
  leaving somebody's rifle plays the shot over whatever their legs are doing.
  **C** crouches, which changes what everybody sees and not where anybody goes
  — the honest half of it to ship first.

- **The crystals hold their fire in a shared valley.** A meteor is thrown at
  where somebody is standing, and standing about is most of what people do
  while there are others to look at — thirty seconds of not moving used to cost
  most of a life. `meteors()` in `server.rill` is the switch, and it is a
  switch on the world rather than a constant in `valley.rill`, because a
  browser alone in its own valley is the demo the fireballs were written for
  and still gets them. Only the starting is gated: a crystal already winding up
  finishes its swell and settles back, since one cut off mid-swell would stay
  at two and a half times its size with nothing to bring it down.

- **Names over heads.** A browser joining somebody else's valley is asked who
  it is before it connects, and everybody else sees that over the player's
  head. The name is sent once and comes back as a `who`, rather than riding in
  every snapshot twenty times a second: it changes about as often as a person
  arrives. Somebody walking in is told the whole roster; everybody already
  there hears about them.

  What arrives is somebody's to choose, so the server cuts it down to letters
  and digits, eight at most, and hands out `player7` to anyone who says
  nothing. A space would have split the line it is sent on, which is the first
  reason not to pass it through as it came.

  And it has to be free. Two people called the same thing is not a cosmetic
  problem — the tag over a head and the row in the roster are the only way
  anybody tells who shot them, and a valley with two `baris` in it cannot be
  reasoned about by the people in it. A taken name gets a number, and the
  number eats into the eight rather than pushing past it. That is done on the
  server rather than refused back to the client: a join that fails on its last
  step, after the world is built and the socket is open, is a worse thing to
  hand somebody than a name with a 2 after it. They are told what they ended
  up with on the same `who` line everybody else gets, so nobody is guessing.

  The tags are elements over the canvas rather than geometry in it. Text in a
  3D scene means a glyph atlas, a quad each and a pipeline to put them through;
  the browser already has a text engine that hints and kerns and knows about
  every script there is, and a tag wants to face the camera and stay legible at
  any distance, which is what it is good at. All that is ours is where it goes:
  a point above their head, through the same view-projection the valley was
  drawn with, which is sitting in the first sixteen floats of the state. They
  fade over the last twenty metres and go amber as somebody runs out of health.
  They are not occluded by the terrain, which is a choice rather than an
  oversight.

- **A limit per address, not just in total.** Sixty-four connections was no
  defence against one machine opening sixty-four of them: it could still take
  the whole valley. Sixteen from any one address now — generous for a household
  or an office behind one, and still three-quarters of the room left for
  everyone else. Measured: twenty-four attempts from one address, sixteen
  accepted and eight refused.

  Which needed `sock_peer(fd)`, the address at the other end as one number.
  A number rather than a dotted quad because what a server does with it is
  count, and the server keeps its rows in the same shared block as the total —
  one row per connection is more than enough, since a row is only in use while
  somebody from that address is.

- **A read can be given a deadline: `sock_wait(fd, ms)`.** A socket read had
  none, so a peer that opened a connection and then said nothing held a strand
  and a descriptor for as long as it cared to, and the valley server's answer
  was a cap on how many of those it would hold. Closing the socket from
  somewhere else is not the fix and this is worth writing down: the reader is
  parked on that descriptor inside the poller, closing it from another strand
  does not wake it, and the number can be handed to the next connection while
  the old registration is still armed.

  So the descriptor and a deadline are armed together — two kevents in one call
  on the BSDs, a socket and a timerfd in the same epoll set on Linux — and
  whichever arrives first wakes the strand. What it fired on is not asked of
  the poller: a one-byte `MSG_PEEK` answers it directly, and a peek cannot be
  wrong the way a race between two registrations can.

  Which uncovered a hazard worth fixing on its own: a strand registered on two
  things could be made runnable twice, and queueing the same strand twice
  corrupts the run queue. Whoever claims it first now wins, and the second
  event finds it claimed and leaves it alone.

  The valley server gives a connection five seconds to get its handshake in,
  measured against the clock rather than per read so that dribbling a byte at a
  time cannot extend it. Eighty sockets that connect and say nothing used to be
  held indefinitely; they are all let go now, and a real client connects
  afterwards.

- **`ws.rill` stops assuming a read is a frame.** TCP hands over bytes: one
  read can carry half a frame or three of them. The reader took the first frame
  out of whatever a single read produced and threw the rest away — so a client
  sending two messages in one write lost the second, and a message split across
  two segments was truncated, after which every following frame was read from
  the middle of the last one and the connection quietly turned to noise. That
  is what a large frame really did: it did not stall the server, it desynced
  the stream and the connection died.

  `ws_take(conn, acc)` hands back the message and the bytes after it, and they
  go back in next time. Measured on the valley server: three frames in one
  write all arrive, a frame split down the middle is reassembled, and a
  65,000-byte frame is answered on the next tick with the connection intact —
  all three of which used to fail. The echo example now sends both its messages
  in one write, so the test covers it.

  Unmasking was also quadratic — `acc + chr(...)` copies everything so far,
  every byte — which for the largest frame this reads came to two gigabytes of
  copying and about sixty-five milliseconds of one core, for one message. It
  goes through a `StrBuf` now.

- **The valley server stops taking a client's word for anything.** It was a
  demo on a trusted network and read like one, and asking what an unfriendly
  client could do turned up four things.

  Numbers off the wire are numbers somebody chose. `str_to_float` hands back a
  NaN for `"nan"` and an infinity for `"1e400"` as happily as it hands back
  2.75, and NaN is the one that spreads: it compares false against everything,
  so a player at a NaN position is hit by nothing, seen by nothing, always far
  enough away to be worth throwing at, and puts NaN into every snapshot
  everybody else reads. Measured before the fix: `i 0 nan nan` and the
  snapshot said `nan nan`. Everything arriving is now checked finite, and
  pitch is held to the same limits `walk` holds it to.

  The direction of a shot was multiplied by the muzzle velocity and believed.
  A client sending a direction a million long got a round leaving the muzzle at
  a hundred and seventy million metres a second, which crosses the whole valley
  inside one step and sweeps a segment that takes out every crystal along a
  line through the world. It gets worse on the way: the drag term is
  `1 - 0.00011 * speed * dt`, which at that speed is not a fraction under one
  but a factor of minus nine hundred, so a tenth of a second later the round
  was measured at 158 billion metres a second and going backwards. A rifle, not
  a rail gun: the direction is made a unit vector on arrival, and one too short
  to normalise is not a direction, so the shot is dropped. The same message now
  leaves at 120 m/s, where the drag term is the fraction it was meant to be.

  One client could stop the world for everybody. The hub takes its tick off
  the same channel as the key presses, and against an unbounded sender the
  tick never wins — modelled on its own, four busy senders and the ticker got
  through **zero times in eight seconds**. Each connection now has ninety
  messages a second, which is comfortably above what a browser sends and far
  below what a machine can; over the budget a line is read and dropped without
  being parsed. Under three flooding clients the tick now holds at a full 20 a
  second, where before the server fell over.

  The timeout then threw out the one client it was meant to protect. Input is
  sent once a frame and a browser stops running frames the moment its tab is
  hidden, so a tab left in the background said nothing at all and was dropped
  from a valley it was still connected to — measured, and then measured again
  after the fix, because the first attempt at testing it was served a cached
  `index.html` and proved nothing. There is a heartbeat on a timer now: the
  last look with no keys held, every two seconds, which says "still here" and
  "standing still". Standing still matters — the server walks from the last
  input it was given, so a tab hidden mid-stride would otherwise keep walking.
  The silence a connection is allowed is thirty seconds rather than ten, since
  a browser throttles the timers of a hidden tab.

  And there was no limit on open sockets. Eighty connections that said nothing
  at all held eighty strands and eighty descriptors for ever; there are
  sixty-four places now and the eighty-first is closed on arrival. That is a
  cap, not a timeout — a slow client still holds its place until it speaks,
  because a read cannot be given a deadline in this language yet.

- **A `select` in a loop is a loop again.** It was not corrupting the heap,
  which is what it looked like: it was leaking the stack.

  `select` builds a table of its cases and a slot for the result, and both were
  `alloca`s emitted where they were first needed. An `alloca` in the entry
  block is free — LLVM folds it into the frame layout and gives it a fixed
  offset — but anywhere else it is a real adjustment of the stack pointer, made
  every time control reaches it. Worse, a function holding a dynamically sized
  `alloca` cannot have its frame popped before a call, so LLVM quietly drops
  the `tail` marker and a Rill loop written as tail recursion stops being a
  loop.

  A hundred and sixty bytes a turn, measured: the stack pointer moved exactly
  sixteen megabytes between the hundred-thousandth `select` and the two
  hundred-thousandth. Two hundred thousand turns is thirty-two megabytes, which
  is the main strand's whole stack, and the next push went past the bottom into
  the guard page of the mapping next door. That is a *bus* error rather than a
  segmentation fault, landing wherever the program happened to be — usually
  inside `free`, once inside `strtod` — which is why it read as heap
  corruption for so long. Five lines of Rill, no sockets and no valley, five
  crashes out of five.

  Both allocas are taken in the entry block now, where they cost nothing and
  happen once however many times control comes back round. The reproduction
  runs a hundred million times with the stack flat, and the same loop is a
  test. This was what the valley server kept dying of, and it explains the
  shape of it: nothing to do with what a client sent, everything to do with how
  many times the hub had been round.

- **An aliased import no longer renames the module's own local names.**
  `import "lib" as v` qualifies everything `lib` defines, and it was doing that
  to *references* by name alone — so a local binding, a parameter, a loop's
  accumulator or a name a pattern bound was rewritten into a reference to the
  module function it happened to share a name with. A file that compiled
  perfectly well on its own stopped compiling the moment somebody imported it
  under an alias, and the error pointed at the innocent line: `ground` bound
  from `height(x, z)` came out as "expected Float, found (Float, Float) ->
  Float", which is the module's `ground` function standing where the number
  was.

  Found by splitting the valley in two, which is the first thing in this
  repository to import anything under an alias in earnest.

- **The valley holds more than one person.** `examples/webgpu/world/server.rill`
  is a WebSocket server that owns one world and steps it twenty times a second,
  and browsers pointed at `?join` walk about in it together.

  The point of it is which file it is made of. `valley.rill` is now the valley
  and nothing else — no exports, no `main` — and it is compiled twice: through
  `world.rill` into the wasm the browser loads, and through `server.rill` into
  a native binary. Both run the same walk, the same collisions, the same
  ballistics, because they are the same code. A server that reimplemented any
  of that would be a server that disagreed with the client about where a round
  went, and this one cannot.

  A player is a block of state pointing at a world, and the world's block
  points at itself, so `wd(s)` is the world whichever kind of block it is
  handed and one player is the same arithmetic as eight. `frame` came apart
  into `walk`, `step_world` and `pick_up` — one per player, one per tick, one
  per player again, in the order `frame` always ran them, so a browser alone in
  the valley behaves exactly as it did.

  Everything shared is decided once, on the server. The crystals throw at
  somebody chosen from the people actually in range; the shark hunts the
  nearest swimmer and bites whoever that is; a blast hurts everyone near it;
  and a round now carries whose it is, in the slot the tracer shader has always
  called `spare`, so a hit can be credited and nobody shoots themselves in the
  back of the head at the muzzle. Three rounds take a player down.

  What goes on the wire is text, one line per event, and the events cost the
  world nothing to produce: the rounds, the throws and the craters already live
  in rings whose cursors count up, so the difference between the cursor at the
  start of a tick and the cursor at the end *is* the log. Those cursors used to
  be stored already wrapped — 31, 32, 1 — which was fine for finding the newest
  entry, useless as a log, and quietly ate one crater's sound every time round.

  The client still walks itself, on the same `walk`, and eases toward what the
  server says rather than snapping to it; between snapshots it runs
  `step_visuals`, which moves everything and decides nothing. Its aim is never
  corrected. Other people are drawn as instances of a figure built the way the
  pines and crystals are, interpolated between the last two snapshots and
  casting a shadow like anything else standing in the sun.

  Three things the first cut of the server did not handle, all found by asking
  what happens when somebody's connection dies rather than closes:

  A connection that ends cleanly was already handled — the player leaves the
  valley, their block is freed and their id retired. But the strand *writing*
  to them was not: it sat waiting on an outbox nothing would ever be sent to
  again, one leaked strand and channel per disconnect, which measured 1,584 KB
  over two thousand connections. The socket is now closed first, so anything
  still queued fails at once, and then an empty line goes into the outbox as
  the one thing that can reach a strand waiting on a channel. The same two
  thousand connections now cost 112 KB.

  A connection whose far end fell asleep or was unplugged said nothing at all,
  and the player stood in the valley for ever: in the snapshot, in the count,
  in the way, a target for whatever the crystals threw, and holding one of the
  thirty-two places. The tick now drops anybody not heard from for ten seconds.
  Their socket is left alone on purpose — a strand is parked on that descriptor
  in the poller, closing it from elsewhere does not wake it, and the number
  could be reused by the next connection while the old registration is still
  armed.

  And a client turned away from a full valley used to decide it was player
  zero. The refusal is words, `str_to_int` of a word is zero, and the id was
  being read out of whatever arrived — so the thirty-third arrival could drive
  whoever player zero was. The welcome is now recognised by being a welcome.

  Also fixed while in there: `free_world` never freed the rounds, the shards,
  the fireballs, the chargers or the blasts, and the host asks for the valley
  at three or four resolutions while it settles on one.

- **`sleep_ms` — a strand can wait on the clock without holding a worker.**
  There was no way to wait at all: a program that wanted a rate had to spin,
  which costs a core to do nothing, and a server tick could not be written.

  It is the fifth operation on the poller rather than a call to the operating
  system's sleep, and that is the whole point — the strand goes into the same
  place a strand waiting on a socket goes, and the worker leaves to run
  something else. kqueue has a filter that counts time; epoll has only
  descriptors, so on Linux the wait is a `timerfd` that becomes readable and
  is closed by the sleeper on its way out. A thousand strands asleep at once
  cost 2% of one core and finish together, in as long as the longest.

  A sleeping strand counts as an I/O waiter, so a program in which every
  strand is asleep is not reported as a deadlock: it is waiting on something
  that always answers. There is no ticker type, because with a channel it is
  three lines of Rill; `LANGUAGE.md` shows those, and the deadline-driven loop
  that keeps a rate despite the slack every operating system's timers have.

  Refused on wasm, beside the sockets and the strands. A sleep that returned
  instantly there would be a program silently running at the wrong speed.

- **The wind stops making the trees shiver.** Everything that sways read its
  phase as `sin(t * rate(wind))` — a frequency that rises with the wind,
  multiplied by the clock. Differentiate that and the angular velocity is
  `rate + t * rate'`: a change in the wind moves the phase by that change
  times however long the page has been open. Five minutes in, the pines were
  turning 3.4 radians a frame at 60 fps — past π, so past what the frame rate
  can even represent, which is what the shivering was. Ten minutes in, 6.3.
  It also ran backwards, which is the part that looked wrong before it looked
  broken.

  The phase is integrated now, a step at a time, in `frame` where the wind is
  already computed: a change in the wind changes only what happens next. The
  pines, the grass and the water's ripples all read it, so they still agree
  about their cause. Steady 0.08 radians a frame, at any wind and any hour.

- **`poke_*` and `peek_*` are a store and a load, not a call.** They are one
  instruction each — the address is `p + i * width`, unaligned as the
  runtime's were — and they went through the runtime every time. For a
  program filling a vertex buffer that is one call per float, and filling
  buffers the host will read is the whole of what a wasm module does. Worth
  about 2% on the WebGPU valley; the rest of what follows is the example
  itself.

- **The WebGPU valley rebuilds in 3.8 ms instead of 8.5.** It sampled the
  terrain at each cell's four corners, and a corner belongs to four cells;
  the heights come off a lattice computed once now — 9409 samples where there
  were 36864, and each is twelve sines deep. It is also the more correct way
  round: two cells sharing an edge read one number for it instead of each
  working out its own in floating point. And a candidate pine, crystal or
  grass tuft is now rejected on its seed before the ground under it is worked
  out, which settles most of them.

  The mesh is unchanged — all 497,664 floats agree with the old one to five
  decimal places, and the three instance counts are the same. The loops that
  were self-recursion through a helper returning 1 are `for` loops.

## 0.3.0 — measured, and changed for it

The release where the language stopped being argued about and started being
counted. Two algorithms — a JSON parser and a Dijkstra — were written in eight
languages, and the token cost of the Rill ones decided what the language grew
next: `b[i]`, functions inside functions, `for` and `+=`, tuples, `#s`,
`s[i:j]`, `for … with`, string builders and `scan_int` are all here because a
count said where the words were going. The Dijkstra went from 1579 tokens to
740, which is under Go's.

Then the same programs were put on a clock, and the answer to every gap was
looked up rather than guessed at. What came back was thirteen changes to the
runtime and the code generator and four ideas measured and left unmade —
bounds-check elimination twice, `noalias`, internal linkage, and inlining the
allocator. The JSON benchmark went from 6.5 seconds to 149 ms, the end-to-end
Dijkstra from 10.6 seconds to 97, and the whole suite is in
`benchmarks/tokens/` so the numbers can be checked rather than believed.

It is also the release where Rill reached the browser: WebAssembly with
`export fn`, a triangle through WebGPU, and a world to walk around in.

- **Float arithmetic contracts.** A multiply and the add that consumes it can
  become one fused instruction now, rounding once instead of twice. The answer
  is nearer the true one, not further, but it is not the answer two separate
  roundings give — which is exactly the trade C, C++ and Go make by default
  and Rust and Zig decline. Rill follows the first group, and nothing else is
  loosened: no reassociation, no assumptions about NaN or infinity, no
  reciprocals, just `contract`.

  It is worth six percent on a Mandelbrot — 194 ms to **182**, which is past
  the 196 of the C it is measured against, and that C is the one with three
  fused instructions to Rill's one. The same benchmark is what found it: C,
  C++ and Go sat at 201 ms and Rust, Zig and C# at 249, and the whole of that
  gap was this one flag.

- **A local function that captures nothing is polymorphic.** It was not,
  and the reason turned out to be narrower than it looked: a lifted function
  is tied to the one it was written inside so that a captured buffer keeps the
  width it was captured with, since a `Buf` width is never inferred from use.
  That tie is what costs it a scheme — and a function that captured nothing
  never needed it. It is no longer made. `fn id(x) = x` inside `main` now
  works at `Int` and at `Str`, and so does the `die` that ends a branch at two
  different result types.

  What still has one type is a local function that closed over something,
  which is the case where the tie is doing real work. The error for that used
  to be a plain mismatch that said nothing; it now says which function the
  names came from, and that one which captures nothing would have generalized.

- **`List` or `Buf`.** Not something the compiler can decide, so LANGUAGE.md
  says when each is right, with the JSON parser as the worked example: it
  builds arrays as lists, and that one choice is half of what it still gives
  away to the C it is measured against.

- **A list can be rebuilt in the list it came from.** Perceus reuse: when a
  `match` arm takes a box apart, the frame is what holds it, and nothing left
  in the arm can say its name, the next constructor of exactly that shape is
  built *in that box* rather than in a fresh one. The count decides at run
  time — at one, the box's own references to its fields are let go, exactly as
  its drop would have done, and the memory is kept; above one, the count comes
  down and a fresh box is taken.

  `rev_onto`, which `map`, `filter`, `take`, `append`, `split`, `join` and
  `reverse` all end in, now allocates nothing: every link is written over the
  link it was read from. Those functions are **8–14% faster** — `map` 33.4 ms
  to 28.7 over two million elements, `filter` 34.2 to 30.7, `append` 29.8 to
  26.8, `join` 5.5 to 4.9. `range` and `len`, which build from nothing and
  build nothing, are unchanged, which is the shape of the result one would
  want.

  It buys **no memory**, and that is worth saying: the pool's free list is
  LIFO, so the block a link was freed from was already the block the next link
  was allocated in. Reversing ten million elements peaks at 321 MB either way.
  What reuse removes is the pair of calls, not the footprint.

  It is also worth saying that the four benchmarks in the report do not move
  at all — they spend their time elsewhere. This one was measured on what it
  actually touches.

- **`read_line` reads runs, not bytes.** It pulled one byte at a time out of
  the input buffer and grew its own buffer around them, so an eleven-megabyte
  line was eleven million calls and two copies of itself. It finds the newline
  with `memchr` and takes what is before it in one copy now: that line goes
  from 7 ms to **2**, against 1 for the `fgets` it is measured against.

- **A `match` stops counting, and a field can be moved into a box.** Two
  places where reference counting was doing work that cancels.

  Matching on a local took a reference of its own, which meant a retain on
  the way in and a release on the way out of every `match` in the language —
  once per link for a list walked one link at a time. Whatever keeps that
  local alive keeps it alive across the match too, and the arms already
  retain their bindings as they use them, so the match borrows it now.

  And a binding handed straight to a constructor is moved into it, the way
  one handed to a tail call already was: `Cons(x, acc)` in an accumulating
  loop no longer retains `acc` for the box and releases it for the frame.
  Only where the box is a tail call's own argument, and only for a field
  written as a name — a move under a branch is a move on one path and not the
  other, which the first attempt at this got wrong and the leak check caught.

  `rev_onto`, which every `map`, `filter`, `split` and `reverse` ends in, goes
  from three retains and three recursive drops per link to two and one. JSON
  drops from 176 ms to **154** and the short JSON from 312 to **269**.

- **Releasing a counted value stops being a call.** Handing a buffer or a
  string to a function retains it and the function releases it on the way
  out, and the release was a call into the runtime every time. Nearly always
  there is nothing to do there — the count goes from something to something,
  not to zero — so the decrement is emitted at the site now, with the call
  kept for the one time in many that actually frees. What that buys is not
  the call itself but what it uncovers: a `+1` in the caller and a `-1` in
  the callee, once inlining has brought them together, are now plainly a pair
  and cancel.

  It shows up wherever a program passes storage around. The Dijkstra
  algorithm benchmark, which threads six buffers through a heap's worth of
  small functions, goes from 46 ms to **42** (1.20&times; the C it is written
  against, from 1.32); JSON from 176 to **165**; end-to-end Dijkstra from 121
  to **113**. A `--parallel` build keeps the call, where the count has to be
  atomic.

  Two other candidates were measured and left unmade. Bounds checks in that
  benchmark's heap loop cost 3 ms in 720 — the third time this year that
  removing them has been measured and found not to be worth it. Giving
  generated functions internal linkage, so LLVM knows every caller, does turn
  the `% n` in a lifted loop from a division into a multiply, and changes the
  running time of nothing at all.

- **The prelude's list functions cost constant stack now, and are faster for
  it.** `map`, `filter`, `take`, `append`, `range` and `len` were written the
  way the definitions read — `Cons(f(x), map(rest, f))` — which leaves work
  after the recursive call and so costs a stack frame per element. Half a
  million of them was a segmentation fault. They build the result backwards
  and turn it round at the end instead, which costs a second cell per element
  and buys the whole list back: five million elements go through every one of
  them without trouble.

  The extra pass was expected to cost something. It does not: at a hundred
  thousand elements, where the old ones still survived, `map` is 29% faster,
  `range` 33% and `len` 38%. Deep recursion is more expensive than a second
  walk.

  `join` and `repeat` had the same shape and a second problem — they built
  their result with `+`, which copies everything assembled so far at every
  step. Through a builder they are linear: joining twenty thousand pieces
  goes from 19.7 ms to 1.9.

  The last of those shows up where it matters most. The short, idiomatic JSON
  benchmark — the one written with `map` and `join` rather than a builder
  threaded by hand, and 255 tokens cheaper for it — went from **6.5 seconds
  to 300 ms**. Writing it the plain way no longer costs twenty times the
  running time.

- **`str_find_from`, and a `split` that walks the line once.** Searching a
  string from anywhere but the beginning meant slicing off the part already
  read, which copies it — so `split` copied the whole remainder of the line at
  every separator. A line of a hundred thousand fields took two seconds, and
  one of two hundred thousand overflowed the stack, because `split` was not
  tail-recursive either. `str_find_from(s, needle, from)` starts partway
  along, `split` is written on top of it, and both problems go with it: the
  hundred-thousand-field line now takes 6 ms and five million fields take 117.

- **`str_find` stops looking one byte at a time.** It compared the needle
  against every position in turn, which is how a search reads when you write
  it out and is not how anyone's C library does it. It now jumps to each place
  the first byte occurs — `memchr`, which is vectorized everywhere this
  runtime is built — and compares from there. Searching a 20 MB text twenty
  times goes from 1020 ms to **27**, which is also seven times faster than the
  same searches through macOS's own `memmem`. `contains`, `starts_with`,
  `split` and `lines` all go through it.

  This was the answer to a wider question: does Rill get SIMD at all? It does,
  from LLVM, and it gets it for the loops people actually write — summing a
  `Buf` and counting bytes in a `Str` both compile to the same NEON as the C
  they were written against, whether the loop is spelled `for … with` or as
  tail recursion. What was left out was the runtime's own byte loops, and
  `str_find` was the one that mattered.

- **Four things the runtime was doing the slow way.** The JSON benchmark was
  the last place Rill lost badly to C, so it was profiled rather than guessed
  at, and every one of the four costs turned out to be the language's fault
  rather than the program's. `int_to_str` formatted through `snprintf`, which
  walks a format string and takes a lock to ask the locale about a decimal
  point it will not use; it now writes the digits itself. A string builder's
  bytes came from `malloc`, so a parser that builds one short string per token
  asked `malloc` for one per token; they come from the same pool as everything
  else now. `#s` and `s[i]` were calls into the runtime, which is why they
  could not be hoisted out of a loop — a string is `{ rc, len, bytes… }`, so
  both are a load, and both are emitted as one. And pieces like `","` were
  copied with `memcpy`, where working out how to go fast costs more than
  copying three bytes.

  Together: the benchmark goes from 215 ms to **174**, against 113 for the C
  it is written against and 148 for a C that frees what it allocated. Nothing
  about the language changed — the same program is faster.

  A fifth idea was measured and dropped. The pool's fast path can be laid down
  at the allocation site, where the size is a constant and the size class
  folds away; it is worth 3% on the JSON benchmark and nothing at all on the
  self-hosted compiler, which is not enough to justify generated code that
  knows the allocator's layout by heart.

- **`scan_int`, and two optimizations that turned out not to be worth
  making.** Chasing the last of the distance to C, the plan was to drop bounds
  checks where the loop proves the index is in range. Measuring first said
  not to: removing every bounds check from the Dijkstra benchmark takes it
  from 47 ms to 46, and marking buffer parameters `noalias` changes nothing at
  all. Both were dropped unmade. The same measurement said where the time
  actually was — 88% of that program's end-to-end run is reading its input,
  where every line becomes a list of strings through `split`, `filter` and
  `map`, about fifteen allocations a line. `scan_int()` scans a
  whitespace-delimited number straight out of the input buffer and allocates
  nothing, stopping on the delimiter without swallowing it. The benchmark goes
  from 495 ms to **120 ms** — past the C it is written against, which spends
  221 ms in `scanf`.

- **A loop can carry something.** `for i in a..b with acc = init` runs the
  body for each `i` with `acc` in scope, takes the body's value as the next
  `acc`, and is worth the last one. It costs nothing new: like the plain form
  it becomes the tail-recursive local function a loop is, and what it carries
  is a parameter. That is the point — a parameter is a register, and a program
  without this had to keep a running value in a one-element buffer, which is
  memory reached through a pointer on every touch. Measured on the Dijkstra
  benchmark, whose graph is made by a linear congruential sequence: that phase
  goes from 16 ms to 6 ms, which is faster than the C it is written against,
  and the whole program from 47 ms to 37 ms against C's 31.

- **String builders, and standard input read in blocks.** Building a string
  by `+` copies what is already there every time, so a `StrBuf` is what a
  `Buf` is, for bytes on their way to becoming a string: `strbuf()`,
  `b += s` (a string) or `b += byte` (a number), `strbuf_str(b)`, `#b`. `+=`
  on a name is the one assignment whose target is not an index, and it changes
  no binding — what it adds to is the storage the name points at. Written with
  one, the JSON benchmark's printing went from 44% of its time to 3%.
  Measuring the rest of it found something worse and simpler: `read_line` was
  a `read` syscall per byte, which on an eleven-megabyte input was almost the
  whole program. Standard input is now read in blocks. Together: JSON 6.4 s to
  **0.22 s**, level with Go and past OCaml; Dijkstra end to end 10.3 s to
  **0.51 s**, past Go. All four benchmarks now sit within about twice of C.
  **The self-hosted compiler does not know `StrBuf` yet**, as with slicing. A tight loop over a buffer ran
  twenty-four times slower than the same loop in C, and the emitted IR said
  why: six retains and eight releases per iteration, none of which changed
  anything. Two rules removed all of them. An operation that only reads
  through a reference — a buffer's element or length, a string handed to the
  runtime — **borrows** it when it is a local, since whatever owns it outlives
  the expression, so neither the retain nor the release that undoes it is
  emitted. And a binding this frame owns and hands to a tail call is
  **moved**: the retain the argument takes and the release that follows it
  cancel exactly, which is the whole of what a loop written as a tail call was
  paying. `v[i] += 1` also stopped naming its own base and index when they are
  already names. Measured: the tight loop 707 ms to 31 ms, within a tenth of
  C; Dijkstra's algorithm 236 ms to 66 ms, past OCaml and within twice of C.
  Seventy programs still end with `live allocations: 0`, and a test now reads
  the IR of a loop body to keep it that way.

- **`s[i:j]` slices a string.** The bytes from `i` up to but not `j` — the
  half-open range `for` already counts over — with either end omissible for
  the string's own: `s[3:]`, `s[:3]`, `s[1:#s - 1]`. It is `str_sub` with an
  end instead of a length, which is what makes leaving an end out mean
  anything, and it is a node of its own rather than a rewrite so that each
  part is evaluated once and the error says `[:]` rather than naming the
  builtin it lowers to. **The self-hosted compiler does not know this one
  yet** — no file in the repository uses it, so every oracle still holds, but
  `rillc` will refuse a program that does until it is taught. `#` measures the two things `[]`
  indexes: a string's bytes and a buffer's elements, `str_len` and `buf_len`
  under the notation. It costs the language one rule, stated where comments
  are: a `#` opens a comment when a space or the end of the line follows it,
  and is the operator when something does. Every comment ever written in this
  repository already reads `# ...`, so nothing moved. JSON reaches 1361
  tokens, 1.02&times; Go; Dijkstra 829, 1.11&times;.

- **Tuples, and bindings that take a value apart.** `(a, b)` hands back more
  than one thing, `(A, B)` is its type, and `(q, r) = divmod(17, 5)` reads it
  back. None of it is new machinery: a tuple is the prelude's `Tuple2`,
  `Tuple3` or `Tuple4` under a notation short enough to use, so it compares,
  shows and matches like the data type it is — and the notation is what
  `show` prints and what the formatter puts back. A binding may take apart
  any type with exactly one constructor, which is what makes it a binding and
  not a branch; it is the one-armed `match` it has always been, over
  everything that follows it. Written this way, the JSON benchmark falls from
  1525 to 1396 tokens, 1.14&times; to 1.04&times; Go.

- **`for i in a..b`, and `+=` on a buffer element.** Counting over a range now
  has a form of its own, and a buffer element can be written in terms of what
  it held (`+=`, `-=`, `*=`, `/=`, with the index evaluated once). Neither is
  a new idea in the language: `for` is spelled out as the tail-recursive local
  function it always was, and `v[i] += 1` as the read and the write it always
  was — both before the checker sees a program, so the formatter is the only
  thing besides the parser that knows they exist. Lifting had to learn one
  thing along the way: a function nested in another block is now lifted after
  the enclosing block's calls are rewritten, since what a sibling call passes
  is part of what the nested one captures. With this the Dijkstra benchmark
  reaches 833 tokens — 1.12&times; the Go it is written against, level with C
  and past OCaml, from 1579 three changes ago.

- **Functions inside functions.** A `fn` written in a body sees the names
  around it and may call itself and the functions beside it, which is what a
  loop over local state wants: only the loop's own variables are parameters.
  Each one is lifted out to the top level with the names it uses as leading
  parameters, so nothing is allocated and a tail call stays a tail call — a
  local function is a loop, not a closure, and for the same reason it can only
  be called, never passed. It is inferred together with the function it was
  written inside, so a captured buffer keeps the type it was captured with;
  that needed one further change, which is worth on its own: the functions of
  a mutually-dependent group are now inferred callers first, so what a caller
  says about its callee's parameters is known before the callee's body is
  looked at. Measured: the Dijkstra benchmark falls from 1438 to 1002 tokens,
  2.12&times; to 1.35&times; Go across both changes.

- **`b[i]` and `b[i] = v`.** A buffer element is now written the way it is
  read everywhere else, and a string indexes to its bytes with `s[i]`. Both
  mean exactly what `buf_get`/`buf_set`/`str_get` mean — the same types, the
  same bounds check, the same errors — so nothing new happens at run time;
  `b[i] = v` is the one assignment the language has, and only to a buffer,
  which was always storage. Indexing binds tighter than every operator. A
  base whose type is still open cannot be a buffer, since a `Buf` width is
  never inferred, so it settles as a string: `fn f(s, i) = s[i]` needs no
  annotation. Measured on two benchmark programs rewritten to use it: a
  Dijkstra with a hand-written heap loses 9% of its tokens, a JSON parser 3%.

- **The self-hosted compiler calls back.** `rillc` emits C-ABI trampolines,
  the last thing it could not compile: `Fn(...)` in a foreign signature is a
  code pointer with nowhere to keep an environment, so the trampoline widens
  C's arguments to Rill's, calls the function, and narrows the answer back.
  Which function it is has to be settled while compiling, which is what the
  new fixpoint over the program works out — every parameter that reaches a C
  callback slot, directly or by being handed on, is marked, and a function
  with one is emitted once per callback it is called with. `examples/
  callbacks.rill` now joins the oracle's example tier: twenty-seven of
  twenty-seven build under both compilers, print the same bytes and leave a
  clean heap. The self-hosted compiler now covers the language.

- **Long lists no longer crash on the way out.** The generated drop for a
  data type released its heap fields and *then* freed the box, so tearing
  down a list cost a stack frame per link and a half-million-element one
  segfaulted at scope end. It now reads the fields out, frees the box, and
  releases the last of them by a tail call — the shape the self-hosted
  compiler already used — so a five-million-element list unwinds in constant
  stack, optimized build or not. One field can have the last word, so a
  chain running through an earlier field of a multi-field constructor still
  recurses.

- **`rillc` is a command, not a library.** The code generator moved to
  `rillc_gen.rill` and `rillc.rill` became the driver every compiler needs:
  `build`, `run`, `emit-llvm` and `check`, with `-o`, `--keep-ir`,
  `--prelude` and `--runtime`. It finds the prelude and `librill_runtime.a`
  beside itself or one level up, collects link flags from the program's own
  `extern "lib"` declarations in all three of the linker's forms, writes the
  IR and calls `cc` — through C's `system`, `getenv`, `access` and `remove`,
  declared as ordinary `extern`s, so the driver uses nothing a Rill program
  could not. Every path reaches the shell single-quoted, and success returns
  from `main` rather than exiting, which keeps the runtime's leak report
  running. The oracle now drives the whole proof through the new command,
  including a tier holding `rillc check` to `rill check` and one that
  watches `rillc run` hand back a program's exit code.

- **The self-hosted compiler covers the language.** `rillc` now compiles
  traits (receiver-typed dispatch to impl functions), structural `==` and
  `show` for data types (generated per monomorphized type, user impls
  honored down to fields, last-field comparisons by tail call), `Buf`, and
  the whole strand story: channels, `spawn`, `select`. Messages cross a
  channel as one 8-byte word; a spawned strand's free variables ride in a
  runtime-owned environment box; a `select` builds the runtime's case
  table and runs exactly the arm it is told to. A channel bound without
  annotations recovers its element type the way a reader does — from the
  signature of the worker it is handed to, or the payload it is sent.
  The oracle grew a fourth tier: every runnable example now builds under
  both compilers and must print the same bytes and leave a clean heap —
  26 of 26 do. Still out: C function-pointer callbacks.

- **The self-hosted compiler stops leaking.** `rillc` now emits reference
  counting: boxes carry a count with the static-constant marker the runtime
  already understands, every variable use retains, bindings release at
  scope end, arguments are consumed by the callee and the runtime borrows
  what it is passed. Pattern matches retain their bindings only after the
  whole pattern has matched, so a failing arm touches nothing; each
  monomorphized data type gets a generated release function whose final
  field is freed by tail call, so a million-long list unwinds in constant
  stack. The proof is the runtime's own live-allocation counter: every
  behavior test, the compiler-built formatter on the largest source file,
  and the self-compilation of `rillc` by its own output all end with
  `live allocations: 0` — and the fixpoint still holds.

- **Rill compiles itself.** The self-hosted code generator grew from the
  scalar subset to the real language: data types and `match` (nested
  patterns, tag dispatch, shared static nullary constructors), generic
  functions cloned per instantiation by a worklist from `main`, lambdas
  lifted to top-level functions over environment boxes, and an 8-byte-slot
  value model that lets a `die` arm the checker left polymorphic coexist
  with proven types. The proof is a fixpoint: `rillc` compiles itself, the
  self-built compiler compiles it again, and the two IR files are
  byte-identical. The self-built compiler then builds the self-hosted
  formatter — byte-identical to `rill fmt` over ~190 files — and the
  self-hosted type checker — word-identical to `rill check` on the whole
  error suite. Reference counting is the one big absence: the generated
  code leaks, which a batch compiler can afford until Perceus lands.

- **A code generator in Rill.** The third compiler stage, begun:
  `selfhost/rillc.rill` compiles the scalar subset — Int, Float, Bool, Str,
  functions, blocks, `if`, tail calls, the scalar built-ins — to LLVM IR
  text that `cc` links against `librill_runtime.a`. It runs the self-hosted
  type checker first, so every emitted type is concrete, and generates only
  what `main` reaches. `selfhost/check_rillc.sh` proves it behaviorally:
  fourteen programs built by both compilers print the same bytes, including
  a five-million-iteration tail loop and a million-deep mutual recursion.
  All three stages of the front end now exist in Rill, each with its own
  oracle against the Rust implementation.

- **The type checker, self-hosted.** The second compiler stage written in
  Rill: module loading (`tyck_load.rill` — depth-first imports, aliased
  modules qualified to `alias.name`, the prelude merged with transitive
  shadowing), unification with occurs check over a persistent substitution,
  traits and impls, dependency-ordered generalization, and Maranget's
  usefulness test for match exhaustiveness. A new `rill check` subcommand
  runs the Rust checker alone (load + type check, no codegen), and
  `selfhost/check_types.sh` holds the two implementations to the same
  verdict over every file in the repository plus a suite of deliberately
  broken programs: same exit code, same first error, word for word —
  161 cases, zero divergence.

- **Block bodies for match arms, select arms and `if` branches.** The first
  thing writing the compiler in its own language taught was where the
  language pinched: an arm's body had to be one expression, so real logic
  became chains of tiny helper functions, and a long `if` chain became one
  long line. Now an arrow or a `then`/`else` may end its line and take an
  indented block of statements, exactly the way a function body does —
  `else` stands level with its `if`, and `else if` chains stay flat. No new
  machinery below the parser: a block was already an expression everywhere
  else, so the type checker and the code generator took the change without
  a single edit.

  The formatter learned the layout form in both of its implementations — the
  Rust one and the self-hosted one — and the byte-for-byte oracle now runs
  over 125 files including `examples/blocks.rill`, which exercises the new
  forms. Zero divergence.

- **The front end, self-hosted: Rill's lexer, parser and formatter, in Rill.**
  `selfhost/` holds a complete port of `rill fmt` — the layout-sensitive
  lexer with its indent stack, the recursive-descent parser, and the
  canonical printer with comment anchoring and blank-line preservation. The
  proof is not a test suite but an oracle: `selfhost/check.sh` formats every
  `.rill` file in the repository with both implementations and diffs each
  pair. One hundred and twenty files, byte-identical, including the prelude,
  every example and book chapter, and the self-hosted source itself — which
  it formats exactly as the Rust formatter does, parenthesization choices
  and all.

  The port shaped itself around what the language is. State threads through
  single constructors because there is no mutation; every loop is a
  self-tail call because that is what a loop is; numbers and strings keep
  their raw source text, which is both the honest way to reproduce bytes and
  the reason no float printer had to be written. One `die_any` carries every
  parse error — its recursive tail call, never reached past `exit`, is what
  makes one function polymorphic enough to fail at every type. This is the
  first stage of a compiler that does not need Rust; the type checker is the
  natural next piece.

- **A Rill user needs no Rust.** `tools/dist.sh` packages the toolchain the
  way compilers ship: `bin/rill` with its native runtime beside it, and
  `lib/rill/wasm32-wasip1/` carrying the wasm runtime and the wasi sysroot.
  The compiler looks in its own bundle before falling back to cargo's and
  rustup's paths, so the development tree keeps working unchanged. Proven
  with an environment where `rustc` and `cargo` do not exist: the packaged
  binary built the whole WebGPU valley and a native hello-world. Rust builds
  the toolchain once, the way C builds a C compiler — after that it is not
  the user's concern.

- **The valley got weather, and the trees got in the way.** One wind for the
  whole world — a direction that wanders, a moderate strength that gusts —
  computed by Rill each frame like everything else the world knows about
  itself. The lake reads it: its ocean swell became what a lake actually has,
  short frequent chop clustered around the wind's direction, amplitude
  following the gusts, foam breaking on the windward shore. The pines read
  it: crowns lean with the square of the height while trunks stand, and the
  flutter quickens as the wind does. The grass reads it too. Three effects
  agreeing about their cause is what makes it weather rather than animation.

  And the player now collides with what stands in the world: trunks and
  crystals are circles, overlap pushes back to the boundary, and pressing
  into a tree slides you around it the way a shoulder would. Measured in the
  harness: three hundred frames of walking straight at a pine, closest
  approach exactly the trunk radius, to the millimetre.

- **`rill build` generates the embedding, not just the module.** A wasm
  reactor is made to be hosted, so the toolchain now writes the host's
  plumbing beside it: `rill build app.rill --target wasm32-wasip1 -o app`
  produces `app.wasm` and `app.js` — a generated loader carrying the WASI
  shim browsers do not have, instantiation, and the init calls. A page that
  embeds a Rill module now writes only its own code: both WebGPU demos
  dropped their hand-written copies of exactly that plumbing and import the
  generated file instead.

- **A world you can walk through, in `examples/webgpu/world/`.** The proof
  that the wasm target and `export fn` carry real weight: a low-poly valley
  at dusk — terrain from three octaves of value noise with a lake carved out
  of the middle, two hundred and sixty pines and a scatter of glowing
  crystals placed by the same hash, a first-person camera that walks, falls
  and jumps against the terrain, and every matrix the GPU consumes — all
  computed in Rill. The mesh is fifty-five thousand flat-shaded vertices
  written straight into linear memory in seven milliseconds, through loops
  whose bodies return how far to advance, because on wasm the call that comes
  back around must be a tail call and nothing else.

  The renderer half exercises most of what WebGPU has: a shadow pass under a
  fixed golden-hour sun with PCF sampling, a 4x MSAA forward pass, instanced
  drawing fed directly from the buffers Rill wrote, alpha-blended animated
  water, additive crystal halos, a compute-shader swarm of twelve hundred
  fireflies, and a procedural sky with the sun and stars in it. Two lessons
  the debugging left behind: a texture cannot be sampled in the pass that
  renders into it, which poisons every frame silently unless an error scope
  is listening — and `textureSampleCompare` must be reached in uniform
  control flow, so the shadow lookup samples first and selects after.

  The demo sizes itself to whatever machine opens it. There is no API that
  says how fast a GPU is, so it is not asked: the adapter's description picks
  a starting tier, and measured frame times do the rest — the world is
  regenerated denser or sparser until the display's own frame budget is
  spent, which `init` taking the grid and the forest densities as arguments
  makes cheap enough to do live. The camera survives the world being rebuilt
  around it. On an Apple M4 the loop settles at 160x160 cells, nine hundred
  pines, five thousand grass tufts and four thousand fireflies, with the
  GPU reporting under three milliseconds a frame through `timestamp-query`.

  The water became water. Gerstner trochoids rather than summed sines — the
  horizontal displacement is what sharpens crests — with phase speed from
  the gravity relation, amplitude that dies in the shallows, ripple octaves
  in the fragment stage for the sun's glitter, Schlick fresnel against the
  same procedural sky the sky pass draws, absorption with depth, and foam
  where the depth runs out and where the trochoids pinch. Every water vertex
  carries the exact terrain depth beneath it, asked of Rill's own height
  function point by point — a 32-bit re-derivation in the shader would have
  put the shoreline foam where f32 thought the shore was.

- **`export fn`, and a triangle drawn through WebGPU.** The wasm target could
  run a program; now it can *be* a program the host keeps calling. `export fn
  frame(t: Float)` makes `frame` a wasm export, and a module with any export
  links as a reactor — no `_start`, no exit, the host drives it. `main`
  becomes `rill_init`, called once by the host before anything else, which is
  what `main` already meant with the process boundary taken away.

  The signature rules are the boundary's own: parameters `Int`, `Float`,
  `Bool` or `Ptr`, returns those or `Unit`, no generics — an export is one
  entry point, not a family waiting for a call site to pick its types. Wider
  data crosses through a `Buf` and `buf_data`, which the host reads straight
  out of linear memory. Each rule is refused with the reason, at the export's
  own line, and `export` on a native target is refused too: exports are
  functions the embedding host calls, and a native executable has no host.

  In a reactor, an `extern fn` that no library satisfies becomes a wasm
  import for the host to fill in — which turns out to be the entire browser
  FFI. `examples/webgpu/` draws a spinning triangle: WebGPU's setup is
  promise-based and a wasm module cannot await, so the JavaScript host does
  the ceremony once and supplies two imports, and Rill computes the frame —
  corners from `cosf`/`sinf` (resolved against wasi-libc, like any C call)
  into a `Buf(F32)`, handed over as a pointer. The host file is also the
  needed WASI shim, all sixty lines of it, because browsers ship none —
  `println` in a canvas tab arrives in the console by way of `fd_write`.
  Five million frames leave the module's memory byte-for-byte the same size:
  each frame's buffer recycles through the pool, and the 16 MB the pool maps
  up front is all it ever maps.

- **A strand that only computes no longer holds its worker to the end of the
  program.** It could before, and everything else on that worker waited for
  good: a started strand is pinned to the stack its frames live on, so no other
  worker may pick it up. Two strands and one worker was enough to hang — one
  spinning, one that had already run and could not be moved.

  Rill has no `while`, so a loop is a function calling itself in tail position,
  and that is where the check goes: a byte read, a branch that is not taken,
  and a call into the scheduler when it is. A timer thread raises the flag once
  a slice and the strand hands its worker back at the next turn of the loop,
  exactly as if it had blocked on a channel — the stack is saved the same way,
  at a point the compiler chose, so none of this needs a map of which words are
  references.

  Taking a turn away from a strand is worth nothing if nobody is waiting for
  one, and a server spends most of its life with nobody waiting, so the timer
  looks ten times less often while the queues are empty. It costs the first
  strand to start hogging a worker one slow look before it is noticed, and it
  is the difference between an idle program waking a sleeping core a thousand
  times a second and thirty times: three seconds of doing nothing measured
  10 ms of system time before, and none after.

  Two things keep it from costing anything. It is only emitted in programs that
  can reach a strand at all, decided by walking the call graph out from `main`
  before a line is generated, so `fib`, `trees` and `lcg` contain no trace of
  it. And within those programs it goes only on tail calls that come back
  round, which is direct recursion and, through the same call graph, two
  functions calling each other — a check on only the first would leave the
  second able to spin. Measured over 25 runs: `ring` 18.9 ms against 18.9 ms
  without, with identical fastest runs.

  The load has to be volatile, or it would be lifted out of the loop it exists
  to interrupt.

- **What abandoned strands were holding is released.** `main` returning ends
  the program whether or not the strands it started have finished, which is the
  semantics we want — but a strand that never reaches the end of its body never
  reaches the `rill_env_release` there, so its environment and everything the
  environment captured stayed held. `spawn`ing a worker and letting `main`
  return was enough to see it: three live allocations in `examples/loops.rill`,
  four in `showcase`.

  Nothing knew where the strands were. A parked one is in a channel's wait
  queue, a fresh one in a run queue, and neither is reachable from the other —
  the `next` field is already the link for whichever queue the strand is
  sitting in. So strand records now go on a second, permanent list as they are
  allocated, and the shutdown path walks it and releases the environment of
  anything that had not finished. They are recycled through their worker's free
  list and never handed back, so the list is only ever as long as the most
  strands that existed at once. A strand that did finish has already released
  its own environment and is marked dead, so nothing is released twice.

  Walking that list is a cache miss per strand, though, and `ring` makes a
  hundred thousand of them and leaves none of them running — it paid 6 ms to
  find nothing. A count of the strands that have started and not finished
  answers that without looking, and the walk happens only when it is not zero.

  What a strand that had *started* left on its stack stays. Those references
  are owned by that stack, and finding them would need a map of which words are
  references — precisely what the copy-on-block scheduler is built to do
  without. So the report names them instead of implying a mistake: `live
  allocations: 2 (1 strand(s) were still running at exit)`. Every program that
  abandons nothing now reports a clean zero, which is what makes the number
  worth asserting on again.

  With more than one worker the sweep first has to know that nobody is still
  looking at a strand. `STOP` became an atomic and is now read at the top of
  every scheduler iteration rather than only when the queue runs dry, which
  also settles a difference that should never have been there: one worker
  already abandoned everything the moment `main` died, and several workers
  went on running however many strands happened to be queued. They now do the
  same thing. Each worker records that it has left its loop, and shutdown wakes
  them and waits for that — a join in all but name, and not `pthread_join` for
  the reason joining was rejected before: a strand that never blocks would
  never let go of its worker. So the wait is bounded and giving up is a normal
  outcome, leaving the strands exactly as they were.

- **Rill compiles to WebAssembly.** `rill build foo.rill --target wasm32-wasip1`
  writes a `.wasm` that runs under any WASI host. Codegen needed nothing: the
  data layout already comes from the target machine, so a 32-bit pointer is
  simply what LLVM lays out. The runtime grew a third arm beside the unix and
  windows ones — wasm has one thread and no way to make another, so the
  scheduler's primitives are all the same answer, and `mmap` is `malloc`
  because the pool only ever bump-allocates chunks and never gives one back.

  Three things had to agree that never had to before. wasi-libc's startup
  calls `__main_argc_argv`, not `main`, and there is no shimming around it —
  clang renames a C `main` the same way, so a hand-written forwarder forwards
  to itself. `rill_rt_run_direct` took its entry as a one-argument function
  pointer and called `rill_main`, which takes none; a call is just a jump on
  every other target, but wasm checks the signature of an indirect call
  against the callee's and traps. And `rill_env_release` found the closure's
  drop function by counting one `usize` past the refcount, which is eight
  bytes only where a pointer is eight bytes wide — the environment is
  `{ i64 rc, ptr drop_fn, ... }`, so the offset is now written in bytes. The
  same assumption was in `rill_peek_ptr` and `rill_poke_ptr` as a literal `8`.
  All three were correct on every 64-bit target and wrong on the first 32-bit
  one; nothing that shipped was affected.

  Strands do not come along, and cannot. A context switch means saving the
  registers and the stack pointer, and wasm exposes neither — the call stack
  belongs to the engine, and the copy-on-block model has nothing to copy. The
  saving grace is that `needs_scheduler()` already decides this per program:
  one that never spawns links `rill_rt_run_direct` and drops the scheduler
  entirely, so single-threaded wasm is a subset rather than a fork.

  A program that does spawn is refused, and refused where it spawns rather
  than at the abort the runtime's stubs would otherwise reach. Finding the
  line meant knowing which functions are the author's: the prelude is merged
  in ahead of them, so `Module` and `TModule` now record how many of their
  functions came from it, and a channel reached only through a prelude wrapper
  is still refused but without citing a line the author cannot go and read.
  A named library is refused the same way — a wasm module imports from its
  host rather than linking a shared object — while a bare `extern fn` is left
  alone, because it resolves against wasi-libc like any other C symbol.

  What that last allowance lets through is a declaration that disagrees with
  the C function it names, which is easy to write when `size_t` is 32 bits
  here and Rill's `Int` is 64. `wasm-ld` calls it a warning and emits a stub
  that traps when called, so the link is run with `--fatal-warnings` and it
  becomes an error naming the function and both signatures.

  Sixteen of the twenty-seven examples run. The other eleven are refused with
  a diagnostic — nine by the compiler, two by the linker — and none of them
  reaches a trap at run time. Stripped, `hello.wasm` is 28.5 KB against
  32.9 KB for the native binary.

- **Live graphs in the task manager.** A strip per CPU core and one for
  memory, drawn by Rill: a view class of our own with `drawRect:`, which is a
  fifth callback shape — one that receives a struct by value, since an NSRect
  on arm64 is four doubles in v0-v3, the same reasoning `send_rect` uses in
  the other direction. Bars are `NSRectFill` in the system's own colours, so
  they follow the light and dark appearances. Memory also gets a real
  `NSProgressIndicator` for where it stands now.

  Two things had to be worked out along the way. `drawRect:` is handed the
  rectangle that needs repainting, which can be *larger* than the view — the
  whole window's, in practice — so it cannot be used as the bounds; and Rill
  cannot receive a struct as a result, so a view cannot simply be asked how
  big it is. KVC answers it: `[v valueForKey:@"frame"]` is an NSValue, an
  object Rill can hold, and `getValue:size:` writes the struct into memory the
  caller owns. Overriding `setFrameSize:` looked like the way to be told about
  a resize, but autoresizing does not route through an added method — nothing
  ever fired. `windowDidResize:` on the window delegate does, continuously,
  including while a corner is being dragged.

  The two halves sit in an `NSSplitView`, so the divider between them can be
  dragged, and a wider window adds the same number of points to each side
  rather than the proportion springs would have chosen. Its divider position
  has to be set after the window is on screen: asked before that, a split view
  has not worked out its own geometry and clamps it to nearly nothing.

- **A process monitor, in `examples/cocoa/taskman.rill`.** A native AppKit
  window listing every process on the machine with its CPU share and resident
  memory, refreshed on a timer, with a button that sends SIGTERM to the
  selected one. `procs.rill` reads the kernel's table through libproc —
  `proc_listpids` into a `Buf`, then `proc_pidinfo` per pid — and the window's
  table pulls its contents from a data source written in Rill. It exists
  because it needs everything below at once: a callback that answers with a
  count, one that answers with an object, both carried into `class_addMethod`
  as parameters. Three things worth knowing are written down where they bit.
  The CPU counters are in mach absolute time units rather than nanoseconds, so
  elapsed time is measured with the same clock and the ratio needs no
  conversion. The memory figure is `ri_phys_footprint` and not the resident
  size — that is the number Activity Monitor shows, and the resident one puts
  Chrome at 481 MB where the system says 339. And a process owned by another
  user cannot be read at all, not even its name, while leaving the buffer
  holding the previous process's numbers: a refusal means leave it out, not
  read zeroes. About a third of the machine is missing for want of the
  privileged helper Activity Monitor has, and the status line says so rather
  than quietly showing a short list. Clicking a column header sorts by it and clicking again
  turns it around, through `tableView:didClickTableColumn:` — a fourth
  callback shape (two objects, no answer) on the same class that already
  carries the data source. Re-sorting is not re-reading: the loop keeps the
  rows it last built, so the numbers on screen do not move when the order
  does, and every process is sorted rather than only the hundred and twenty
  on screen.

- **Callbacks can answer.** A foreign signature may now write `Fn(Ptr, Ptr) ->
  I32`, and the trampoline carries the result back out, narrowed to the
  declared width the same way an argument is narrowed on the way in. Without
  the arrow a callback still hands nothing back, so every existing declaration
  means what it did. The result has to be something C can hold: a heap value
  would be discarded with nothing left to release it, and a callback returning
  another callback would need a trampoline built at run time. This unblocks
  the signatures that were previously unspellable — a `qsort` comparator, a
  table view asking how many rows it has, a `windowShouldClose:` that can say
  no. `examples/callbacks.rill` sorts an array through one.
- **Callbacks travel as parameters.** A callback no longer has to be named at
  the `extern` call itself, so a helper can wrap the registration instead of
  every call site repeating it. What crosses is still a bare code pointer, so
  the function is settled while compiling: a helper that forwards a callback
  is emitted once per callback it is called with, each copy with its own
  trampoline built in — the same monomorphization the language already does
  for type parameters, keyed on a function instead of a type. Only parameters
  that actually reach C are treated this way, so nothing else duplicates. A
  lambda, or a function picked at run time, is still refused, and now the
  diagnostic names the parameter it travelled under rather than the C
  signature.
- **The AppKit demo uses both.** `examples/cocoa/` grew the parts that were
  previously unreachable: a `windowShouldClose:` delegate that refuses to let
  the window close until the button has been pressed, and an `NSTableView`
  whose rows come from a data source written in Rill — one method answering
  with a count, the other with an object. `cocoa.rill` now names the four
  method shapes it can install (`does`, `answers`, `counts`, `supplies`),
  each a helper taking the callback as a parameter, and the three-step
  `responder_class` / `void_sig` / `class_addMethod` dance at the call site
  is gone.

## 0.2.11 — multi-core servers

Two bugs made `--parallel` unusable for anything that allocates or talks to a
socket. Both were found while chasing the HTTP benchmark's tail latency, which
turned out not to be a Rill problem at all.

- **The allocator was not thread-safe.** One global set of free lists and one
  bump pointer, mutated without synchronization: two workers allocating at the
  same time corrupted the free list and the program died with a segmentation
  fault. Each worker now owns its pool, so the fast path still takes no lock,
  and a block freed on another worker simply joins that worker's list.
  `benchmarks/parallel_alloc.rill` is the regression test — it crashes on the
  old runtime at four workers and passes now.
- **Network wakeups went to the wrong worker.** A strand that has run is
  tied to its worker's stack, but every worker shared one poller, so a
  readiness event woke whichever worker happened to be asleep rather than the
  one that could resume the strand. The real owner then waited out the
  20 ms poll timeout — on nearly every request. Each worker now has its own
  poller and is woken directly; spawning also wakes a sleeper so it can steal.
  The HTTP server went from 2,219 req/s on four workers to 39,158, next to
  40,903 on one.
- The connection-per-request tail latency that started all this is a
  measurement effect, not a defect: at matched connection rates Rill's p99 is
  lower than Go's (10.9 ms against 15.7 ms), and the spikes happen while the
  server sits completely idle. `benchmarks/BENCHMARKS.md` shows the evidence
  and the README claim that Go held an advantage there has been corrected.
- Recorded ring memory corrected to 53.4 MB; the old 45.7 MB no longer
  reproduces on either the current or the previous runtime.

## Unreleased — typed buffers

`alloc_bytes` and `poke_f32` are how C's memory is reached, but they are a
poor thing to write a program in: no length, no element type, and a `free` to
remember. `Buf(w)` is the same memory with all three.

- **`Buf(w)`** is a counted block of elements of one C width — `Buf(F32)`,
  `Buf(U8)`, `Buf(Ptr)` and the rest. `buf_f32(n)` and its siblings allocate,
  `buf_get`/`buf_set` read and write one element, `buf_len` reports the count
  and `buf_data` hands the first element's address to C.
- **Indexing is checked.** An index outside the buffer ends the program naming
  the index and the length, instead of reading past the end. The comparison is
  unsigned, so a negative index fails the same test.
- **Counted like a string**, header and all (`{rc, len, elements}`), so it is
  released when the last reference goes. `examples/gl/texgen.rill` lost its
  `free_ptr` and generates its textures into a `Buf(U8)`.
- The width is part of the type rather than a type variable, so each width has
  its own constructor and a function taking a buffer annotates it. The error
  when it cannot be inferred says so.
- **`examples/gl/` was rewritten on top of it**, and there is now no manual
  memory anywhere in it: the maze grid and its search stack are `Buf(U8)` and
  `Buf(I32)`, the matrices and every mesh are `Buf(F32)`, textures and pixel
  readback are `Buf(U8)`, and the out-parameters C writes handles into are
  one-element buffers. Two `free_ptr` calls remain in the whole directory, both
  for strings `to_cstr` allocated.
- **The maze's world became records.** Where it kept forty untyped slots in a
  raw block, it now has `Scene`, `Mesh`, `Torches`, `Overlay`, `Eye` and a
  `World` holding them: every field has a type the compiler checks, and the
  buffers among them are *held* rather than pointed at, which is what makes
  the counting correct. The mutable parts — the camera, the two matrices, the
  cursor reading — stay buffers inside those records, which is the division
  the language is built around: immutable structure, explicit mutable memory.

## Unreleased — a maze to walk around in

- **`examples/gl/maze.rill`**: a first-person maze, lit by torches, with
  nothing loaded from disk. The layout is carved by depth-first search over a
  raw-buffer grid with its own stack; the brick and flagstone textures are
  generated pixel by pixel; up to sixteen torches light the walls, each
  flickering at its own rate, drawn as quads that turn to face the camera;
  distance fades to the clear colour. **W A S D** walks, the mouse looks,
  walls are checked one axis at a time so a glancing collision slides.
- **Lighting with nothing made up in it.** The ambient is a thousandth, the
  wrap-around fill is gone, and every torch is a point source falling off with
  the inverse square of its distance, lit by the cosine of the surface's angle
  to it. Corridors out of reach of a flame are dark, and light along a wall
  rakes across the brick.
- **Fog that behaves like air, not like a filter.** Mixing towards a fixed
  colour lifted the far dark to grey. It is now absorption on what a surface
  sends you, plus in-scattering of the torches' light computed from where the
  *eye* is and saturating within a few strides. Unlit distance stays black.
- **A gait.** The camera bobs twice a stride and sways once, on distance
  covered rather than on time, easing in and out with a smoothed speed; the
  hand torch swings a little out of phase.
- **Bump mapping for free**: the textures carry their height in the alpha
  channel, and the shader tilts the normal by that channel's slope. Four extra
  samples turn flat quads into brickwork with edges that catch the light.
- **`to_float` and `to_int`**, compiled to a single instruction each. The two
  numeric types still never mix on their own, but crossing between them no
  longer means going through a string.
- **Sound**, through OpenAL: a looping fire on every torch, positioned in the
  world so it pans and fades as you move, and music under all of it —
  Rachmaninoff conducting his own *Isle of the Dead*, which is the painting
  hanging on one of the walls. OpenAL was chosen
  because it wants integer handles and raw pointers and never calls back from
  its own thread, which is the only shape safe next to a non-atomic reference
  counter.
- **A bloody steel edge.** The axe's head is repainted as steel — the patch of
  its palette sheet the edge points at, found from the mesh rather than by
  hand — and the blood over it is worked out per fragment from a noise field
  that creeps down the blade, because nine pixels of texture cannot hold a
  smear. Both catch the torchlight through the same gloss the wet blood on the
  walls uses.
- **Space swings the axe**: a wind-up, a fast chop that overshoots, and a slow
  settle back, with the arm reaching forward and dropping as it goes. The
  whole state is one float — when the swing started — and a second swing is
  refused until the first is done.
- **The flame in your hand is bigger and leans away from where you are going**
  — only its top two corners move, so the fire still leaves the wood where it
  was, and the amount is the walk's own, so it stands up when you stop. It
  also stands a little way towards you rather than through the middle of the
  torch: a billboard down the axis of the stick has the near half of the head
  in front of it, which cut the bottom of the fire off.
- **The torches are models now**, in your hand and on all twelve walls — the
  painted quads that stood in for them are gone, and the wood is lit by the
  maze's own shader like everything else. The one on the wall is stood at the
  foot of its flame with its upright axis pointed out of the stone, which the
  mesh loader gained a second kind of copy for.
- **An axe in one hand and the torch in the other.** The torch moved to the
  left, with its shaft turned to run from the fist back towards the eye rather
  than away from it, and the axe is a model carried in the right — the one
  thing in the maze that moves with the eye. The maze's vertex shader gained a
  `model` uniform for it, the identity everywhere else.
- **Dirt and wear on the flagstones too**, and twice as many drag marks over
  them: stones from different quarry faces, corners broken into hollows, and
  dirt gathering against the joints and walked off the middle of each stone,
  which is what a floor nobody has swept looks like.
- **Dirt and wear on the brick**, generated like the brick itself: soot in
  patches with streaks running down out of them, bricks from light and dark
  batches, corners knocked off into the mortar behind — a hollow, since the
  fourth channel is height — and salt blooming through. A wall that is only
  brick and mortar reads as a pattern; what is wrong with a real one is what
  tells you it is one.
- **Models**, loaded and drawn: what is left of people on the floor of the
  maze — a body, a hand, a heart, kidneys, a slab of flesh — five glTF models
  converted by `tools/fetch_bodies.py` into `bodies.rmsh`
  — a table of meshes and then vertices in the maze's own eight-float layout,
  with the four textures packed onto one sheet and the texture coordinates
  rewritten into it. `mesh.rill` copies a mesh into the world's buffer with
  its rotation already applied rather than carrying a model matrix, so every
  remain in the maze is one buffer, one texture and one draw call. Their
  textures are treated on the way in: the ones that arrived as a single flat
  colour — measured, two of them — are given a generated flesh instead, and
  all of them are stained and given height in the alpha channel, so the bump
  mapping has something to work with. The atlas tile was 128 pixels for
  everything, which threw away most of what the good textures had. Models that
  are modelled standing are laid down on the way in; nothing in this maze is
  upright.
- **Blood**, in three kinds and all of it photographed rather than drawn
  (`tools/fetch_blood.py`, all CC0): splatter stains on wall faces, runs down
  the wall cut from strips of a blood-stained wall, and drag marks on the
  floor cut from a forensic photograph of a wipe trace, laid along the
  corridor. The runs were drawn by hand first and looked like candle wax —
  blood is a millimetre of liquid, not a rope of it. The photographs have no
  alpha channel, so one is keyed out of how much redder than everything else
  each pixel is, and every crop is chosen by measuring rather than by eye.
  Which kind a tile is comes from its shape, so the counts are not written
  down twice.
- **Wet blood.** The same alpha that gives a decal its relief drives a
  specular highlight off the same tilted normal, so a run catches the torch
  along its ridge and its dry thin edges do not. It costs the brick nothing:
  `decal` is zero over every wall fragment and the branch is skipped.
- Windows open **in the middle of the screen** rather than wherever the window
  manager last left one.
- **The resident set, in the top right corner** of the maze, beside the frame
  rate in the left — both labelled, in block letters, since seven segments
  cannot make an M: megabytes as the kernel counts them, through one Mach
  `task_info` call (`mem.rill`), sampled once a second rather than per frame.
  Checked against `ps`: the same number to the kilobyte.
- **Three cries**, played at intervals and from bearings the maze's own
  generator decides, quiet and falling off fast — a scream three corridors
  away should be a doubt rather than an event.
- **MP3 through libmpg123** (`mp3.rill`) for every recording: 336 KB of file
  where the samples would have been 2.6 MB, decoded once at startup into a
  `Buf(I16)`. The first read reports the format it settled on rather than a
  sample, which is not an end — reading it as one is why the first attempt
  decoded nothing. Fed from memory rather than opened by path, so the bytes
  come straight out of the pack.
- **One file for all the data** (`assets.rpak`, `pack.rill`,
  `tools/pack_assets.py`): a directory of names and a concatenation, holding
  the atlas and the five recordings. Everything in it arrives compressed, so
  it adds none of its own; it is read whole and handed out as pointers into
  that one block.
- Five public-domain recordings and `tools/fetch_sound.py` to reproduce them,
  measuring which window of each to keep and handing them to `lame`. The music
  is under a minute of the piece, cut at the quietest second near the minute
  mark and cross-faded into its own opening so that looping it has no seam;
  mono at 22050 Hz and forty kilobits, since that is all the program mixes at.
  Encoding costs neither loop its join: lame writes down its own delay and
  mpg123 reads it, so what comes back is sample for sample what went in.
- A third generated texture for the ceiling — boards across beams — with the
  floor and the ceiling built as two runs of one mesh so each can wear its own.
- **The torches have brackets**, drawn from the same buffer in an opaque pass
  before the fire's additive one, with the flame seated on the tip.
- **The torches burn in a shader.** The flat tapered quad is gone: each flame
  is now value noise scrolling upward through a narrowing shape, threshold-cut
  so its edge flickers, coloured red to white from rim to core, seeded per
  torch and blended additively.
- Paintings are spread by counting the wall faces first and hanging one every
  so many, rather than by a hash of the coordinates — which, with a modulus
  that divided one of its own coefficients, had been ignoring the x axis
  entirely and lighting up whole rows. A stone carries one fixture and its
  neighbours along the same wall carry none, so torches and canvases never
  share a face and two canvases never sit side by side.
- **A texture packer** (`tools/pack_art.py`): shelf packing, with every sheet
  width worth trying tried and the smallest area kept, so twenty 160×160 tiles
  come out as a 640×800 sheet with no waste. The result is deflated into one
  `.rtex` file — 2 MB of pixels in 917 KB, losslessly — and read back with a
  file read, one call into the system's zlib through the FFI, and one upload.
  Sharing a sheet also collapses twenty draw calls into one. (PNG-style row
  filtering was measured first: 1.6% over deflate alone on these images, not
  worth the loop on the reading side.)
- **Paintings on the walls.** Twenty public-domain works — Goya, Caravaggio,
  Bruegel, Gentileschi, Géricault, Rembrandt and more — hang on wall faces the
  corridors can see, each exactly once, lit by the torches through the maze's
  own shader. There is no image decoder in the language, so `tools/fetch_art.py`
  fetches the scans from Wikimedia Commons, frames them and writes the raw
  160×160 RGBA bytes; `art.rill` reads a file and hands the bytes straight to
  `glTexImage2D`, which is what a counted string being binary-safe buys.
- `examples/gl/mazemap.rill` prints the carved maze as text, with no window
  and no OpenGL, which is how the generator is tested.

## Unreleased — smaller binaries

A program that uses no concurrency was still paying for all of it. Measured
against the same program written in C (`examples/gl/reference/triangle.c`),
which links the same GLFW and OpenGL: **54.6 KB → 37.6 KB, against C's
36.4 KB**. The gap went from 18 KB to 1.2 KB.

- **No scheduler unless something can wait.** If nothing in the emitted module
  calls into `spawn`, a channel, `select` or a socket, `main` runs straight on
  the process stack through a new `rill_rt_run_direct`, and the linker drops
  the worker loop, the run queues, the poller and the stack switching with it.
  The question is asked of the finished module, so a channel in a function
  nothing reaches costs nothing.
- **The worker table no longer sits in the executable.** One non-zero byte —
  a statically initialised mutex, a `-1` sentinel — put all 9 KB of it in
  `__DATA`, which is written to the file. The statics start zeroed and
  `rill_rt_run` fills in what it uses, so the table lives in `__bss` instead
  and costs nothing on disk.
- **String literals and nullary constructors are read-only.** They were
  writable only because reference counting wrote to them, so counting now
  skips anything already at the immortal count. Being constants, they move out
  of `__DATA` entirely — which is what removes the segment, and with it a
  whole 16 KB page from every binary.
- The skip is a branch, not a conditional store address. The first attempt
  redirected the store to a scratch word to stay branchless and cost **28%**
  on `trees`: a store through a selected pointer takes away what the optimizer
  knew about what it touches. Written as a branch, the common path is
  byte-for-byte what it was, and `trees`, `fib` and `lcg` all measure
  unchanged.

## Unreleased — C widths and raw buffers

The FFI could only describe C functions written in 64-bit numbers, which is
almost none of them. Both halves of that gap are closed.

- **Foreign signatures can name a width.** `I8`, `U8`, `I16`, `U16`, `I32`,
  `U32`, `I64`, `U64`, `F32` and `F64` are spellable inside an `extern`, and
  the boundary converts: arguments truncate or `fptrunc` on the way in,
  results sign- or zero-extend on the way back according to the declared
  signedness. `Int` and `Float` keep meaning the 64-bit forms. This matters
  most for `float`: a C function declared `Float` and handed a double read the
  wrong half of the register and quietly returned nonsense — `sqrtf(2.0)` came
  back 0. Integer arguments happened to work by accident before, since the
  callee reads the low half of the register; they are now correct by
  construction.
- **Raw buffers**: `alloc_bytes`, `ptr_offset`, `poke_i8/i16/i32/i64`,
  `poke_f32/f64`, `peek_i8/u8/i16/u16/i32/u32/i64` and `peek_f32/f64`. Plain
  C memory outside the reference counter, freed with `free_ptr`, indexed by
  element rather than byte, and unaligned-safe so an offset pointer stays
  usable. This is what lets a program hand an array — vertex data, a pixel
  buffer, a matrix — to a C library.
- **Pointer slots**: `poke_ptr` and `peek_ptr`, for the `char**` a C API takes
  and the handle it writes back through an out-parameter. `ptr_offset` may now
  be applied to null, because a byte offset carried in a pointer-shaped
  argument is how OpenGL and friends describe a location in a buffer.
- **Linking**: a library string starting with `-` goes to the linker verbatim,
  so a program can say where its libraries live, and `framework:Name` links a
  macOS framework. Search paths are emitted before the libraries that need
  them, whichever declaration carried them.
- **Callbacks**: `Fn(...)` in a foreign signature is a C function pointer, and
  a top-level Rill function can be passed as one — codegen emits a C-ABI
  trampoline per (function, instantiation, signature) that widens C's
  arguments and calls through. Closures are refused with a message that says
  why (an environment C cannot hold), a callback returning a heap value is
  refused because nothing would release it, and `Fn` as a *return* type is
  refused too. This closes the last gap that forced polling: GLFW key and
  resize events are delivered now.
- `extern fn f(x: Unit)` used to panic the compiler instead of being rejected.
- `examples/ffi_buffers.rill` covers all of it, with an e2e test; the first
  line it prints is the one that fails on the old compiler.
- **`examples/gl/` drives OpenGL with no C anywhere**: GLFW opens a window,
  shaders are compiled, vertex data goes up in a raw buffer, and a triangle
  spins with a seven-segment frame counter drawn as rectangles. A headless
  variant renders the same scene through CGL into a renderbuffer and prints
  the pixels as text, which is what the test suite checks — no display, no
  window server, same GPU path. Escape and window resizing arrive as
  callbacks rather than being polled.
- `examples/callbacks.rill`: C calling back into Rill, during `main` through a
  signal handler and after it through `atexit`.

## Unreleased — editor support

- **A Visual Studio Code extension** (`editors/vscode/`): a TextMate grammar
  that highlights every construct in the language — declarations, `match` and
  `select` arms with `default`, lambdas, the pipeline operator, `import ... as`
  aliases and the qualified names they introduce — and separates built-in
  types, prelude types and constructors, built-in functions and prelude
  functions, so what the compiler treats differently also looks different. It
  marks the string escapes the lexer rejects as errors, sets two-space
  spaces-only indentation with automatic indent after `=`, `->`, `select`,
  `match x` and block headers, and ships snippets.
- The grammar has a test: `npm test` in `editors/vscode/` tokenizes
  `test/fixture.rill` — a compilable file exercising the whole language — and
  checks 43 scope expectations, that nothing falls through unscoped, and that
  nothing valid is flagged invalid.

## 0.2.10 — HTTP keep-alive, and an HTTP benchmark

- **Keep-alive in the HTTP server** (`examples/http/http.rill`): a connection
  now serves request after request until one side asks to stop, following
  HTTP/1.1's default and the client's `Connection:` header. The client side
  gained `get_all`, which sends every path down one connection and reads each
  response by `Content-Length` rather than by end of stream.
- **An HTTP benchmark against Go** (`benchmarks/http/`): the same one-route
  service on Rill's module and on Go's `net/http`, driven by `wrk` in both
  keep-alive and connection-per-request modes. Rill serves 213.8k req/s to
  Go's 197.8k with keep-alive, 42.9k to Go's 13.1k without, in 7× less memory
  and a 52 KB binary against 7.2 MB. Go keeps the better tail latency under
  connection churn, which the write-up says plainly.

## 0.2.9 — YAML

- **A YAML reader in Rill** (`examples/yaml/`) for what configuration files
  actually use: nested mappings by indentation, `- ` sequences, lists of
  mappings, comments, quoted and bare scalars. Anchors, aliases, multiple
  documents, flow collections and block scalars are out of scope, and said so.
- It parses into the JSON module's value type, so a YAML document prints as
  JSON without conversion — and the two modules import across directories.

## 0.2.8 — JSON

- **A JSON parser and printer in Rill** (`examples/json/`): a recursive-descent
  parser returning value-and-position, escapes including `\uXXXX` for the
  Latin-1 range, a printer that round-trips, and lookups (`get`, `path`,
  `items`, `as_str` / `as_float` / `as_bool`).
- `str_to_float`, which JSON numbers need.

## 0.2.7 — bit operations, and WebSocket

- **Bit operations**: `bit_and`, `bit_or`, `bit_xor`, `bit_not`, `shl`, `shr`,
  compiled to single instructions. Without them no binary protocol could be
  written in the language.
- **SHA-1 and base64 in Rill** (`examples/http/sha1.rill`), matching the
  published test vectors, including RFC 6455's example key.
- **A WebSocket server in Rill** (`examples/http/ws.rill`): the handshake and
  text frames both ways, masking included. Verified against an independent
  client for the accept-key computation and for both the 7-bit and 16-bit
  length paths.

## 0.2.6 — sockets and an HTTP server

- **TCP sockets integrated with the scheduler.** `tcp_listen`, `tcp_accept`,
  `tcp_connect`, `sock_read`, `sock_write` and `sock_close`, with `Result`
  wrappers in the prelude. Sockets are non-blocking underneath: a strand
  that would block parks and its descriptor is handed to `kqueue`, which the
  scheduler polls when it runs out of runnable strands. One strand per
  connection therefore costs nothing while it waits, and an idle server is not
  mistaken for a deadlock.
- **Functions are now checked in dependency order.** Mutually recursive
  functions are inferred as a group, and a function whose type is settled by a
  later definition comes out right — previously that quantified its return type
  into a variable nothing could satisfy, and the program failed to compile.
- **An HTTP/1.1 server written in Rill** (`examples/http/`): request parsing,
  routing, responses, and a client, in about 100 lines on top of the socket
  builtins. It answers `curl`, and serves 30 concurrent connections with a
  strand each.
- Strings understand `\r`, which HTTP needs.

## 0.2.5 — calling C

- **A foreign function interface.** `extern "m" fn sqrt(x: Float) -> Float`
  declares a C symbol and the library to link; the compiler passes `-lm` to the
  linker and calls it directly. Signatures accept the shapes the C ABI carries
  (`Int`, `Float`, `Bool`, `Ptr`, `Unit`) and reject the rest with an
  explanation.
- **`Ptr`**, an opaque C pointer that reference counting leaves alone, plus
  `to_cstr` / `from_cstr` / `free_ptr` / `null_ptr` / `is_null` for moving
  strings across the boundary — a Rill `Str` is a counted heap object, not a
  `char*`.
- An `extern` declaration shadows a prelude function of the same name, exactly
  as a definition does.

## 0.2.4 — source-level debugging, qualified imports, matching in parentheses

- **`--debug` builds carry DWARF line information**, so `lldb` stops on Rill
  source lines (`b fib` resolves to `fib.rill:2`) instead of raw symbols. The
  object file is kept next to the binary and `dsymutil` runs when available,
  which is how macOS reaches the DWARF.
- **`import "x" as m`** qualifies a module: everything it defines is reached as
  `m.name`, so two files may define the same function, type or constructor.
  Unaliased imports keep working exactly as before.
- **`match` and `select` now work inside parentheses**, where their arms follow
  one another on the line: `println(match o None -> 0 Some(n) -> n)`. The
  formatter prints nested matches in that form and no longer refuses to format
  a file that contains one.
- **The scheduler no longer shares one lock.** Each worker guards its own run
  queue and recycles its own strand records, so workers that are simply
  running strands never contend; channel operations keep a single lock,
  always taken before a run queue's. Deadlock detection now confirms the
  quiet state over many rounds rather than trusting one snapshot.

## 0.2.3 — select

- **`select`** waits on several channels at once, with `recv` arms that bind
  what arrives, `send` arms, and an optional `default` for polling. Arms are
  tried in order, so an earlier one wins a tie.
- Channel wait queues now hold per-case waiter nodes rather than strands, so
  one strand can wait in many queues; the first channel to reach it claims
  it and the rest are unlinked when it wakes.
- **Layout fix**: a `match` or `select` used as a non-final statement in a block
  no longer swallows the line break that separates it from the next statement.

## 0.2.2 — generic trait impls, buffered channels

- **`impl Show(List(a))`** and friends: a trait implementation may target a
  generic type by naming its parameters, covering every instantiation. Method
  bodies stay generic and resolve their own trait calls per element type, so
  `List(Box(Int))` composes two impls with no `where` clauses. An implementation
  that only works for one instantiation is rejected with an explanation.
- **Buffered channels**: `channel(n)` carries a ring buffer of `n` values in
  the same allocation as the channel, so a producer can run ahead of its
  consumer. `channel()` is unchanged — capacity zero is still a rendezvous.

## 0.2.1 — multi-core scheduler

- **Strands can run on several cores.** `rill build --parallel` makes
  reference counting multi-thread safe, and `RILL_THREADS=N` then spreads
  strands over N worker threads. Eight independent computations run 3.6×
  faster on this machine; a binary built without the flag says so and stays on
  one worker rather than running unsoundly.
- Workers steal strands that have not started yet. One that has already run
  is pinned to its worker, since the copy-on-block stack model restores its
  frames to that worker's addresses.
- Deadlock detection is now exact — no runnable strand anywhere and none
  executing — instead of the "every worker looks idle" snapshot, which could
  fire spuriously at startup.
- Reference-count updates are inlined into generated code, so single-worker
  programs no longer call into the runtime to retain a value.

## 0.2.0 — nested patterns, constrained generics, trait-driven operators

**Language**

- **Patterns nest freely.** `Cons(Some(0), rest)` and deeper now work anywhere a
  pattern does. Matches compile to a decision tree, so each value is tested once
  and no arm's body is duplicated.
- **Exhaustiveness checking names a missing case.** A non-exhaustive match
  reports an actual unmatched value — `no arm matches Cons(None, Cons(_, _))` —
  and arms that earlier arms already cover are rejected.
- **Constrained generics.** Requirements a body imposes (`<`, `+`, `println`)
  travel with the function's type instead of pinning it to `Int`. `fn sort(l)`
  is generic over ordered types; `fn announce(x) = println(x)` works for every
  printable type. Using a function at a type that doesn't satisfy a requirement
  is a compile error naming both.
- **Built-in traits `Show` and `Ord`.** `impl Show(T)` replaces a type's derived
  printing (including inside other values); `impl Ord(T)` gives it `<`, `<=`,
  `>`, `>=`. Strings are ordered without any impl.
- **`if` may span lines** — `then` and `else` can start a continuation line.
- A type and one of its constructors may share a name (`type Pair` with
  `Pair(x, y)`).

**Fixes**

- The formatter no longer drops the arms of a `match` nested inside an `if`
  branch, and refuses to format rather than corrupt a construct it cannot
  reproduce.
- `||` and `&&` chains no longer gain redundant parentheses when reformatted.

## 0.1.0 — the language

- **M0** Native pipeline: lexer with indentation layout, parser, LLVM backend,
  linked executables.
- **M1** Type inference, algebraic data types, `match` with exhaustiveness.
- **M2** Let-polymorphism, generic ADTs, lambdas and closures, monomorphization,
  tail-call elimination. C-sized binaries (~33 KB) via a `no_std` runtime.
- **M3** Perceus-style compile-time reference counting: no GC, no pauses,
  verified leak-free by a live-allocation counter.
- **M4** Go-style concurrency: `spawn`, typed rendezvous channels, a green-thread
  scheduler with hand-written arm64 context switching.
- **M5** A standard library written in Rill, and counted heap strings.
- **M6** Derived `show` and structural `==` for every data type;
  user-defined `trait` / `impl` with fully static dispatch.
- **M7** File and process IO, string processing, logical operators, a module
  system (`import`), `rill fmt`, `rill repl`, and `--debug` builds.

**Performance work** (see `benchmarks/BENCHMARKS.md`): arena strand stacks and
release-before-tail-call codegen took the 100k-strand ring from 321 ms to
13 ms; nullary-constructor singletons and a size-class pool allocator took
binary-trees from 125 ms to 34 ms. Rill leads Go and OCaml on all four
benchmarks in time, peak memory and binary size.

# Chapter 17. Machines

A strand that waits for a signal, does what the signal asks in the state
it is in, and waits again is what the telephone exchanges called a
*process*, and the language they modelled them in — SDL, the ITU's
Specification and Description Language — drew each one as a diagram: a
state at the top, the signals it answers below, what each does, and the
state it leads to. Chapter 16 gave you the strand and the channel. This
chapter gives you the process, written as the diagram it is, and the
tools that made SDL a modelling language rather than a notation:
the block diagram checked against the code, the trace of a run drawn as
a sequence chart, and a clock the model owns.

The program throughout is `examples/abp.rill`: the alternating-bit
protocol, which is the first thing anyone models — a sender that numbers
each frame with one bit and keeps sending it until the acknowledgement
with that bit comes back, a medium that loses what it is told to lose,
and a receiver that delivers each text once and acknowledges every frame.

---

## 17.1 A machine

```rill
type Signal
  Send(text: Str)
  Frame(bit: Int, text: Str, back: Chan(Mail(Signal)))
  Ack(bit: Int, back: Chan(Mail(Signal)))
  Deliver(text: Str)
  Lose(count: Int)
  Retry
  Report
  Status(text: Str)

type ReceiverState
  Expecting(bit: Int)

machine receiver(me: Chan(Mail(Signal)), env: Chan(Mail(Signal)))
  state Expecting(bit)
    Frame(b, text, back) if b == bit ->
      output(env, Deliver(text))
      reply(Ack(b, back))
      Expecting(1 - bit)
    Frame(b, _, back) ->
      reply(Ack(b, back))
      stay
```

The signals are a type, one constructor a signal, with the fields a
signal carries. The states are a type too. The machine is a function
whose first parameter is its mailbox and whose last, unwritten, is the
state; it is started with its initial state as one more argument —
`spawn receiver(r, env, Expecting(0))` — and from then on it is the
loop you would have written by hand: receive, match the state, match
the signal, call yourself with the next state. That loop is what the
parser makes of it, and `rill explain` shows the calls as the tail calls
they are, so a machine runs in constant stack for as long as the program
does.

Each `state` lists the signals it answers as the arms of a match, with
patterns and guards, and an arm's body ends in the next state — or
`stay`, the state it is in, or `stop`. A signal no arm names is consumed
and the state kept, which is what SDL did with an unexpected signal.

What arrives is a `Mail`: the signal, and the mailbox it came from. So
every arm has **`sender`**, `reply(sig)` answers it, and `output(to,
sig)` sends to any mailbox, both carrying this machine as the sender. A
signal that wants an answer no longer has to carry a channel. The
receiver above replies to the frame's `back` field rather than to
`sender`, because between it and the sender stands the medium, and the
acknowledgement is for the sender.

From outside a machine, `post(to, from, sig)` says where the answer
goes:

```rill
fn delivered(n: Net) = match recv(n.env).signal
  Deliver(text) -> text
  _ -> "?"

fn main() =
  n = start()
  post(n.s, n.env, Send("hello"))
  println(delivered(n))
```

In SDL the environment is a process too, and the program's `env` is its
mailbox. A mailbox is made with `mailbox()`, which grows rather than
making a sender wait — SDL's input port never refuses a signal, and two
machines that send each other bursts at once on a bounded channel hold
each other up for ever.

---

## 17.2 Timers, and a signal kept for later

```rill
machine sender(me: Chan(Mail(Signal)), medium: Chan(Mail(Signal)), env: Chan(Mail(Signal)))
  state Ready(bit, sent)
    Send(text) ->
      output(medium, Frame(bit, text, me))
      set(Retry, 100)
      Sending(bit, text, 1, sent)
    Report ->
      reply(Status("ready, " + int_to_str(sent) + " sent"))
      stay
  state Sending(bit, text, tries, sent)
    save Send(_)
    Ack(b, _) if b == bit ->
      reset(Retry)
      Ready(1 - bit, sent + 1)
    Retry ->
      output(medium, Frame(bit, text, me))
      set(Retry, 100)
      Sending(bit, text, tries + 1, sent)
```

`set(Retry, 100)` starts a timer that delivers `Retry` in 100 ms — a
timer is a signal the machine sends itself later, named by the signal,
moved by setting it again, stopped by `reset`, and it keeps running
across states until it runs out, when whatever state the machine is in
answers it like any other signal. That is the retransmission: every
100 ms without an acknowledgement, the frame goes again.

`save Send(_)` keeps a `Send` that arrives while one is in flight, in the
order it came, for the state that can answer it: when the sender is
`Ready` again the saved text is taken before the mailbox is read. With
a guard, `save Send(_) if !ready`, it is SDL's `input Send provided
ready`. Two more forms belong to the same family: `priority Lose(_)`
answers that signal before any that came earlier, and `when cond ->`
is a transition taken with no signal at all, once nothing pending can be
answered and the condition holds. `state *` holds the arms every state
answers, a `Report` or a `Kill`, after each state's own.

A machine can also be a procedure. Called inside another machine's
transition, on the same mailbox, it runs in that strand with states of
its own, and `done(v)` at an arm's tail is its return:
`if confirm(me, log, Asking(name)) then …`.

---

## 17.3 The block diagram

```rill
system abp
  env -> sender: Send, Report
  sender -> env: Status
  sender -> medium: Frame
  medium -> receiver: Frame
  receiver -> medium: Ack
  medium -> sender: Ack
  receiver -> env: Deliver
  env -> medium: Lose
```

A `system` names the routes between the kinds of machine and the
environment, with the signals each carries, and the compiler checks it
against the machines the way SDL's tools checked a block diagram against
its processes. A signal a machine sends that no route out of it carries
is refused, naming both; so is a signal it answers that no route into it
brings, a route to something that is not a machine, and a signal that
does not exist. Take `Ack` off the route from the medium to the sender
and the build says so at the line that sends it. Instances are not
declared — the diagram is of kinds, and `spawn` makes as many as the
program wants.

`rill diagram examples/abp.rill` draws the system as a flowchart and
each machine as SDL drew a process: the state at the top, a column per
signal with the input symbol, the outputs, a task with the hourglass
where a timer is set, a decision where the arm branches, and the next
state at the bottom. `--svg` gives the same in SDL's own symbols. The
diagram is read off the code and never kept beside it, so it cannot fall
behind.

`rill check` adds the report SDL's tools gave of a state's *implicit
consumption*: for each state, the signals some other state answers and
this one neither answers nor saves, and once per machine the signals no
state answers at all — a `Status` the sender only ever sends, or a
mistake in every state. A build says nothing of it; the notes are for
reading a model.

---

## 17.4 A run, drawn

```sh
rill build examples/abp.rill -o abp
RILL_TRACE=1 ./abp 2>trace.txt
rill msc trace.txt              # Mermaid
rill msc --svg trace.txt -o abp.svg
```

With `RILL_TRACE=1` every machine writes one line an event on standard
error — received, in what state, from whom; the next state; sent, to
whom; a timer set or reset; a signal saved or dropped; a stop — numbered
by the runtime in the order events happened across every strand, each
machine named by the number of its mailbox. `rill msc` draws those lines
as the message sequence chart SDL's simulators drew: a lifeline per
machine and per mailbox of the environment, an arrow per signal, a note
per state change.

This is not decoration. The first version of the receiver above replied
`Ack(b, me)` — its own mailbox as the return address — and the program
hung. The chart showed the medium sending the acknowledgement back to
the receiver at event 16, and the fix was one word. A run without the
variable pays one load per event and says nothing.

---

## 17.5 The model's own clock

```sh
RILL_VIRTUAL_TIME=1 rill test examples/abp.rill
```

```
ok    test_a_text_goes_through
ok    test_a_lost_frame_is_resent_and_delivered_once
ok    test_texts_wait_their_turn
3 tests: 3 passed, 0 failed
```

With `RILL_VIRTUAL_TIME=1` the program runs on the clock a simulator
keeps: time does not pass while anything runs, and when every strand is
waiting and the soonest wait is a timer, the clock jumps to it. The
sender's `Retry` fires the moment nothing else can happen; three lost
frames and their retransmissions take no time; and the trace of a run is
the same every run, which is what makes a model's behaviour something a
test can state. `now_millis` tells the virtual time, and a
`sleep_ms(1)` in a test means "once everything in flight has landed":

```rill
fn test_a_lost_frame_is_resent_and_delivered_once() =
  n = start()
  post(n.m, n.env, Lose(3))
  post(n.s, n.env, Send("two"))
  assert_eq(delivered(n), "two")
  assert_eq(status(n), "ready, 1 sent")
```

Sockets and file syncs still wait in real time, with the virtual clock
standing still meanwhile. In real time the same tests pass in under a
second; on the virtual clock, in a few milliseconds, and identically.

---

## 17.6 What SDL had that this has not

Blocks nested in blocks, and instance sets with a count: the `system`
here is one level, of kinds. Model checking — every interleaving
explored, every state reached — which the deterministic trace is the
first step towards and not the last. And the graphical editor, which is
the one thing deliberately left out: here the code is the model, and the
diagram is read from it.

---

## 17.7 Exercises

1. Give the medium a second fault: `Delay(ms)`, holding the next frame
   for that long before passing it on. Which of the tests still pass,
   and what does the chart show at the moment the sender gives up
   waiting?
2. Add a `Cancel` signal to the sender that abandons the text in flight.
   Where does `priority` belong, and what does `rill check` say about
   the states that do not answer it?
3. Write a lift controller: floors as signals, a door with a timer, a
   `system` with the buttons as the environment. Draw it, then run it
   on the virtual clock with a hundred calls and read the chart.
4. Take the exchange from `examples/exchange.rill` and let a line call
   a line that is busy. What should happen? Model it, and check that
   the trace says so.

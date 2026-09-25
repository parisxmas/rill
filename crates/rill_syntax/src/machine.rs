//! `machine`, lowered: the tail-recursive function a state machine is.
//!
//! ```text
//! machine line(me: Chan(Signal), sw: Chan(Signal))
//!   state Idle
//!     OffHook -> Dialling(Nil)
//!   state Dialling(digits)
//!     Digit(n) -> Dialling(Cons(n, digits))
//!     after 10000 -> Idle
//! ```
//!
//! becomes
//!
//! ```text
//! fn line(me, sw, machine.state) = match machine.state
//!   Idle ->
//!     machine.sig = recv(me)
//!     match machine.sig
//!       OffHook -> line(me, sw, Dialling(Nil))
//!       _ -> line(me, sw, machine.state)
//!   Dialling(digits) ->
//!     machine.got = select
//!       machine.s = recv(me) -> Some(machine.s)
//!       timeout(10000) -> None
//!     match machine.got
//!       None -> line(me, sw, Idle)
//!       Some(machine.sig) -> match machine.sig
//!         Digit(n) -> line(me, sw, Dialling(Cons(n, digits)))
//!         _ -> line(me, sw, machine.state)
//! ```
//!
//! An arm's body is taken at its tail: the next state goes into the call
//! that comes round again, `stop` becomes `0` and the strand ends, and an
//! `if` or a `match` at the tail is followed into each of its branches.
//! The names with a dot in them are ones no program can write, as the
//! loop functions `for` and `while` become are named.

//!
//! Two more things a state may do, each costing only the machines that use
//! them. Named timers — `set(Retry, 200)`, `reset(Retry)`, `active(Retry)`
//! in an arm's body, as SDL has them — live in a table the machine
//! carries, `machine.timers`, a signal under its name with the moment it
//! is due; the wait is a `select` whose deadline is the soonest of them,
//! and a timer that has run out is answered as the signal it is. `save
//! Sig` keeps a signal for a later state: the machine carries
//! `machine.pending`, the saved signals in the order they came, and in
//! each state the first of them the state does not save is taken before
//! the mailbox is read. A machine with either is two functions: the one
//! named as declared, which makes the table and the list, and `name.run`,
//! the loop, with the two as parameters no program can name.
//!
//! `state *` is the arms every state answers, put after each state's own
//! so the state's own win; `stay` at an arm's tail is the state the
//! machine is in, SDL's `nextstate -`. A composite state — one with
//! `state`s under it — is the same thing for its substates alone: its
//! arms go after each substate's own, then a composite's around it, then
//! `state *`'s, and only the substates are states the machine is in.
//!
//! `done(v)` at an arm's tail ends the machine with a value, as `stop`
//! ends it with none: a machine called from a transition of another, on
//! the same mailbox, is a procedure with states, and `done` is its
//! return.
//!
//! What arrives is a `Mail`: the signal and the mailbox it came from. An
//! arm has `sender`, `reply(sig)` sends to it, `output(to, sig)` to any
//! mailbox, both with this machine as the sender; a timer's signal comes
//! from the machine itself.
use crate::ast::*;
use crate::SyntaxError;

pub fn lower(m: &MachineDef) -> Result<Vec<FnDef>, SyntaxError> {
    let line = m.line;
    let state = "machine.state".to_string();
    let mailbox = m.params[0].name.clone();
    let plan = plan_of(m);
    let shared: Vec<&StateDef> = m.states.iter().filter(|st| matches!(st.pattern, Pattern::Wildcard)).collect();
    if shared.len() > 1 {
        return Err(SyntaxError::new(shared[1].line, 0, "a machine has one `state *`: the arms every state answers".to_string()));
    }
    let mut arms = Vec::new();
    for st in leaf_states(&m.states, &[]) {
        let merged = merged_state(&st, shared.first().copied());
        arms.push(Arm { pattern: st.pattern.clone(), guard: None, body: state_body(m, &merged, &mailbox, &state, &plan)?, line: st.line });
    }
    if arms.is_empty() {
        return Err(SyntaxError::new(line, 0, "a machine needs a `state` of its own besides `state *`".to_string()));
    }
    let body = Expr::Match { scrutinee: Box::new(var(&state, line)), arms, line };
    let mut params = m.params.clone();
    for h in plan.hidden() {
        params.push(Param { name: h.to_string(), ann: None });
    }
    params.push(Param { name: state.clone(), ann: None });
    let mut out = Vec::new();
    if plan.timers || plan.saves {
        // The loop, and the function named as declared that starts it with
        // a fresh table and an empty list.
        let mut args: Vec<Expr> = m.params.iter().map(|p| var(&p.name, line)).collect();
        if plan.timers {
            args.push(call("map_new", Vec::new(), line));
        }
        if plan.saves {
            args.push(var("Nil", line));
        }
        args.push(var(&state, line));
        let mut outer = m.params.clone();
        outer.push(Param { name: state, ann: None });
        out.push(fndef(&m.name, outer, call(&plan.loop_name(&m.name), args, line), line));
        out.push(fndef(&plan.loop_name(&m.name), params, body, line));
    } else {
        out.push(fndef(&m.name, params, body, line));
    }
    Ok(out)
}

/// A state with what `state *` gives every state: its arms after the
/// state's own, so the state's own win; its `after` where the state has
/// none; its saves besides.
fn merged_state(st: &StateDef, shared: Option<&StateDef>) -> StateDef {
    let mut out = st.clone();
    if let Some(all) = shared {
        merge_into(&mut out, all);
    }
    out
}

/// What `outer` gives `st`: its arms after `st`'s own — a shared arm the
/// state's own already answer whole is left out, since an arm that can
/// never be reached is refused — its `after` where `st` has none, and
/// its saves, whens and priorities besides.
fn merge_into(st: &mut StateDef, outer: &StateDef) {
    let covered = |p: &Pattern| st.arms.iter().any(|a| a.guard.is_none() && covers(&a.pattern, p));
    let more: Vec<Arm> = outer.arms.iter().filter(|a| !covered(&a.pattern)).cloned().collect();
    st.arms.extend(more);
    if st.after.is_none() {
        st.after = outer.after.clone();
    }
    st.saves.extend(outer.saves.iter().cloned());
    st.whens.extend(outer.whens.iter().cloned());
    st.priorities.extend(outer.priorities.iter().cloned());
}

/// The states the machine is in — every plain state, and every substate
/// of a composite, each carrying its composites' arms after its own,
/// the innermost composite's first — `state *` left out, for the caller
/// to merge last.
pub fn leaf_states(states: &[StateDef], outers: &[&StateDef]) -> Vec<StateDef> {
    let mut out = Vec::new();
    for st in states {
        if matches!(st.pattern, Pattern::Wildcard) {
            continue;
        }
        if st.is_composite() {
            let mut around: Vec<&StateDef> = vec![st];
            around.extend(outers.iter().copied());
            out.extend(leaf_states(&st.substates, &around));
        } else {
            let mut leaf = st.clone();
            leaf.substates = Vec::new();
            for outer in outers {
                merge_into(&mut leaf, outer);
            }
            out.push(leaf);
        }
    }
    out
}

/// Whether `own` takes everything `shared` would: a wildcard, or the same
/// constructor with nothing but names under it.
fn covers(own: &Pattern, shared: &Pattern) -> bool {
    match (own, shared) {
        (Pattern::Wildcard | Pattern::Var(_), _) => true,
        (Pattern::Ctor { name: a, args, .. }, Pattern::Ctor { name: b, .. }) => {
            a == b && args.iter().all(|p| matches!(p, Pattern::Wildcard | Pattern::Var(_)))
        }
        (Pattern::Or(alts, _), _) => alts.iter().any(|p| covers(p, shared)),
        _ => false,
    }
}

fn fndef(name: &str, params: Vec<Param>, body: Expr, line: usize) -> FnDef {
    FnDef {
        name: name.to_string(),
        exported: false,
        params,
        ret_ann: None,
        body,
        line,
        lifted_from: None,
        wheres: Vec::new(),
        private: false,
        fast: false,
    }
}

/// What this machine carries besides its state. A `when` needs the list
/// of what is pending too, since a signal looked at and not yet answered
/// waits there.
struct Plan {
    timers: bool,
    saves: bool,
}

impl Plan {
    fn hidden(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.timers {
            v.push("machine.timers");
        }
        if self.saves {
            v.push("machine.pending");
        }
        v
    }

    fn loop_name(&self, name: &str) -> String {
        format!("{name}.run")
    }
}

/// Whether a body sets, resets or asks after a timer.
fn uses_timers(e: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if let Expr::Call { name, args, .. } = e {
            if timer_call(name, args).is_some() {
                *found = true;
            }
        }
        crate::lift::each_child_all(e, &mut |c| walk(c, found));
    }
    walk(e, &mut found);
    found
}

/// `set(T, ms)`, `set(T(x), ms)`, `reset(T)`, `active(T)`: the timer's
/// name, when the call is one of these on a constructor.
fn timer_call(name: &str, args: &[Expr]) -> Option<String> {
    let ok = (name == "set" && args.len() == 2) || ((name == "reset" || name == "active") && args.len() == 1);
    if !ok {
        return None;
    }
    match &args[0] {
        Expr::Var(n, _) if starts_upper(n) => Some(n.clone()),
        Expr::Call { name: n, .. } if starts_upper(n) => Some(n.clone()),
        _ => None,
    }
}

fn starts_upper(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// An arm's body with the machine's own words rewritten: `output(to, s)`
/// is a send with this machine as the sender, `reply(s)` the same to the
/// sender of the signal being answered, and the timer calls go onto the
/// machine's table with the timer's signal wrapped as from itself. Each
/// says what it did to the trace first.
fn rewrite(e: Expr, m: &MachineDef, me: &str) -> Expr {
    let e = match e {
        Expr::Call { name, args, line, piped, stacked } => match timer_call(&name, &args) {
            Some(t) => {
                let mut a = vec![var("machine.timers", line), Expr::Str(t.clone())];
                let mut rest = args.into_iter();
                let signal = rest.next().expect("checked");
                let (fname, note) = match name.as_str() {
                    "set" => {
                        let ms = rest.next().expect("checked");
                        let note = concat(vec![Expr::Str(format!("\t{t}\t")), call("int_to_str", vec![ms.clone()], line)], line);
                        a.push(ms);
                        a.push(mail(me, signal, line));
                        ("machine_set", Some(("set", note)))
                    }
                    "reset" => ("machine_reset", Some(("reset", Expr::Str(format!("\t{t}"))))),
                    _ => ("machine_active", None),
                };
                let done = Expr::Call { name: fname.to_string(), args: a, line, piped, stacked };
                match note {
                    Some((kind, rest)) => Expr::Block(vec![Stmt::Expr(note_of(m, me, kind, rest, line)), Stmt::Expr(done)]),
                    None => done,
                }
            }
            None if name == "output" && args.len() == 2 => {
                let mut it = args.into_iter();
                let to = it.next().expect("two");
                let sig = it.next().expect("two");
                let text = crate::fmt::expr_text(&sig);
                let note = note_of(m, me, "send", concat(vec![Expr::Str(format!("\t{text}\t")), call("machine_id", vec![to.clone()], line)], line), line);
                Expr::Block(vec![Stmt::Expr(note), Stmt::Expr(call("send", vec![to, mail(me, sig, line)], line))])
            }
            None if name == "reply" && args.len() == 1 => {
                let sig = args.into_iter().next().expect("one");
                let text = crate::fmt::expr_text(&sig);
                let note = note_of(m, me, "send", concat(vec![Expr::Str(format!("\t{text}\t")), call("machine_id", vec![var("sender", line)], line)], line), line);
                Expr::Block(vec![Stmt::Expr(note), Stmt::Expr(call("send", vec![var("sender", line), mail(me, sig, line)], line))])
            }
            None => Expr::Call { name, args, line, piped, stacked },
        },
        other => other,
    };
    crate::lift::map_children(e, &mut |c| rewrite(c, m, me))
}

/// `machine_note(kind, name, me, rest)`: one event of the trace.
fn note_of(m: &MachineDef, me: &str, kind: &str, rest: Expr, line: usize) -> Expr {
    call("machine_note", vec![Expr::Str(kind.to_string()), Expr::Str(m.name.clone()), var(me, line), rest], line)
}

/// Strings joined with `+`.
fn concat(parts: Vec<Expr>, line: usize) -> Expr {
    let mut it = parts.into_iter();
    let mut acc = it.next().expect("one part");
    for p in it {
        acc = Expr::Binary { op: BinOp::Add, lhs: Box::new(acc), rhs: Box::new(p), line, newline: false };
    }
    acc
}

/// A statement said before a body.
fn noted(note: Expr, body: Expr) -> Expr {
    match body {
        Expr::Block(mut stmts) => {
            stmts.insert(0, Stmt::Expr(note));
            Expr::Block(stmts)
        }
        other => Expr::Block(vec![Stmt::Expr(note), Stmt::Expr(other)]),
    }
}

/// `Mail(me, signal)`: a signal as this machine sends it.
fn mail(me: &str, signal: Expr, line: usize) -> Expr {
    call("Mail", vec![var(me, line), signal], line)
}

fn field(base: &str, name: &str, line: usize) -> Expr {
    Expr::Field { base: Box::new(var(base, line)), name: name.to_string(), line }
}

/// What a state does: take a saved signal it can answer, or receive, or
/// wait with a deadline; then answer the signal, or keep it.
fn state_body(m: &MachineDef, st: &StateDef, mailbox: &str, state: &str, plan: &Plan) -> Result<Expr, SyntaxError> {
    let line = st.line;
    let state_text = crate::fmt::pattern_text(&st.pattern);
    let sig = "machine.sig".to_string();
    let got = "machine.got".to_string();
    let mut sig_arms = Vec::new();
    // What the state saves never reaches this match — it is kept before
    // it — but the arms say so first, guarded so they take nothing and
    // shadow nothing, for the report of what a state drops to read them
    // as answered.
    for (p, g, l) in &st.saves {
        let guard = g.clone().unwrap_or(Expr::Bool(false));
        sig_arms.push(Arm { pattern: p.clone(), guard: Some(guard), body: again(m, var(state, *l), *l), line: *l });
    }
    let mut catches_all = false;
    for a in &st.arms {
        if a.guard.is_none() && matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)) {
            catches_all = true;
        }
        let pat = crate::fmt::pattern_text(&a.pattern);
        let recv = note_of(m, mailbox, "recv", concat(vec![Expr::Str(format!("\t{state_text}\t{pat}\t")), call("machine_id", vec![var("sender", a.line)], a.line)], a.line), a.line);
        sig_arms.push(Arm { pattern: a.pattern.clone(), guard: a.guard.clone(), body: next(m, noted(recv, rewrite(a.body.clone(), m, mailbox)), state)?, line: a.line });
    }
    if !catches_all {
        // a signal no arm names is consumed, and the state kept
        let drop = note_of(m, mailbox, "drop", concat(vec![Expr::Str(format!("\t{state_text}\t")), call("machine_id", vec![var("sender", line)], line)], line), line);
        sig_arms.push(Arm { pattern: Pattern::Var("machine.other".to_string()), guard: None, body: noted(drop, again(m, var(state, line), line)), line });
    }
    // The signal answered: its sender and itself, out of the mail.
    let mail_name = "machine.mail".to_string();
    let answer = Expr::Block(vec![
        Stmt::Bind { name: "sender".to_string(), value: field(&mail_name, "from", line), line },
        Stmt::Bind { name: sig.clone(), value: field(&mail_name, "signal", line), line },
        Stmt::Expr(Expr::Match { scrutinee: Box::new(var(&sig, line)), arms: sig_arms, line }),
    ]);
    let after_body = match &st.after {
        Some((ms, body, l)) => {
            let note = note_of(m, mailbox, "after", concat(vec![Expr::Str(format!("\t{state_text}\t")), call("int_to_str", vec![ms.clone()], *l)], *l), *l);
            next(m, noted(note, rewrite(body.clone(), m, mailbox)), state)?
        }
        None => again(m, var(state, line), line),
    };
    let after_ms = st.after.as_ref().map(|(ms, _, _)| ms.clone());
    let aline = st.after.as_ref().map(|(_, _, l)| *l).unwrap_or(line);

    // The wait: `Option` of a signal, `None` when the deadline came first.
    let s = "machine.s".to_string();
    let wait = match (plan.timers, &after_ms) {
        (false, None) => call("Some", vec![call("recv", vec![var(mailbox, line)], line)], line),
        (false, Some(ms)) => select_wait(mailbox, &s, ms.clone(), line, aline),
        (true, _) => {
            // the soonest of the state's own wait and the timers; on a
            // deadline, a timer that has run out is the signal
            let ms = call(
                "machine_wait",
                vec![var("machine.timers", line), after_ms.clone().unwrap_or(Expr::Int(-1))],
                line,
            );
            let sel = select_wait(mailbox, &s, ms, line, aline);
            Expr::Match {
                scrutinee: Box::new(sel),
                arms: vec![
                    Arm { pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(s.clone())], line }, guard: None, body: call("Some", vec![var(&s, line)], line), line },
                    Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line }, guard: None, body: call("machine_expired", vec![var("machine.timers", line)], line), line },
                ],
                line,
            }
        }
    };

    if !plan.saves {
        let outcome = Expr::Match {
            scrutinee: Box::new(var(&got, line)),
            arms: vec![
                Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line: aline }, guard: None, body: after_body, line: aline },
                Arm { pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(mail_name.clone())], line }, guard: None, body: answer, line },
            ],
            line,
        };
        return Ok(Expr::Block(vec![Stmt::Bind { name: got, value: wait, line }, Stmt::Expr(outcome)]));
    }

    // With saves: whether this state keeps a signal, as a function, used
    // both to pick from what was kept and to judge what arrives.
    let x = "machine.x".to_string();
    let mut save_arms: Vec<Arm> = st
        .saves
        .iter()
        .map(|(p, g, l)| Arm { pattern: p.clone(), guard: g.clone(), body: Expr::Bool(true), line: *l })
        .collect();
    save_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: Expr::Bool(false), line });
    let saved = "machine.saved".to_string();
    let pred = Expr::Lambda { params: vec![x.clone()], body: Box::new(Expr::Match { scrutinee: Box::new(field(&x, "signal", line)), arms: save_arms, line }), line };
    let pending = "machine.pending".to_string();
    let rest = "machine.rest".to_string();
    // With a `priority`: everything that has arrived joins the pending
    // list first, and a signal the state gives priority to is taken from
    // it before any other, whatever its place.
    let (pending_now, first) = if st.priorities.is_empty() {
        (var(&pending, line), None)
    } else {
        let mut prio_arms: Vec<Arm> = st
            .priorities
            .iter()
            .map(|(p, g, l)| Arm { pattern: p.clone(), guard: g.clone(), body: Expr::Bool(true), line: *l })
            .collect();
        prio_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: Expr::Bool(false), line });
        let px = "machine.px".to_string();
        let is_prio = Expr::Lambda { params: vec![px.clone()], body: Box::new(Expr::Match { scrutinee: Box::new(field(&px, "signal", line)), arms: prio_arms, line }), line };
        (call("machine_drain", vec![var(mailbox, line), var(&pending, line)], line), Some(is_prio))
    };
    let take = match first {
        None => call("machine_take", vec![pending_now.clone(), var(&saved, line)], line),
        Some(is_prio) => call("machine_take_first", vec![pending_now.clone(), is_prio, var(&saved, line)], line),
    };
    // (the signal if any, the pending list without it)
    let source = Expr::Match {
        scrutinee: Box::new(take),
        arms: vec![
            Arm {
                pattern: Pattern::Ctor { name: "Some".into(), args: vec![tuple_pat(vec![Pattern::Var(s.clone()), Pattern::Var(rest.clone())], line)], line },
                guard: None,
                body: tuple(vec![call("Some", vec![var(&s, line)], line), var(&rest, line)], line),
                line,
            },
            Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line }, guard: None, body: tuple(vec![wait, pending_now], line), line },
        ],
        line,
    };
    let keep = Expr::Block(vec![
        Stmt::Expr(note_of(m, mailbox, "save", Expr::Str(format!("\t{state_text}")), line)),
        Stmt::Bind { name: pending.clone(), value: call("append", vec![var(&pending, line), call("Cons", vec![var(&mail_name, line), var("Nil", line)], line)], line), line },
        Stmt::Expr(again(m, var(state, line), line)),
    ]);
    let judged = Expr::If {
        cond: Box::new(call(&saved, vec![var(&mail_name, line)], line)),
        then: Box::new(keep),
        els: Box::new(answer),
        newline: false,
    };
    let outcome = Expr::Match {
        scrutinee: Box::new(var(&got, line)),
        arms: vec![
            Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line: aline }, guard: None, body: after_body, line: aline },
            Arm { pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(mail_name.clone())], line }, guard: None, body: judged, line },
        ],
        line,
    };
    let normal = Expr::Block(vec![
        Stmt::PatBind { pattern: tuple_pat(vec![Pattern::Var(got), Pattern::Var(pending.clone())], line), value: source, line },
        Stmt::Expr(outcome),
    ]);
    if st.whens.is_empty() {
        return Ok(Expr::Block(vec![Stmt::Bind { name: saved, value: pred, line }, Stmt::Expr(normal)]));
    }
    // A `when`: what has arrived is looked at without waiting and joins
    // what is pending; if nothing pending can be answered, the first
    // condition that holds is taken; otherwise the state goes on as it
    // would.
    let polled = Expr::Select {
        arms: vec![SelectArm {
            op: SelectOp::Recv { chan: var(mailbox, line), bind: Some(s.clone()) },
            body: call("Cons", vec![var(&s, line), var("Nil", line)], line),
            line,
        }],
        default: Some(Box::new(var("Nil", line))),
        line,
    };
    let idle = "machine.idle".to_string();
    let mut chain = normal;
    for (cond, body, l) in st.whens.iter().rev() {
        let note = note_of(m, mailbox, "when", Expr::Str(format!("\t{state_text}\t{}", crate::fmt::expr_text(cond))), *l);
        let taken = next(m, noted(note, rewrite(body.clone(), m, mailbox)), state)?;
        chain = Expr::If {
            // `idle && cond`, as the parser writes `&&`
            cond: Box::new(Expr::If { cond: Box::new(var(&idle, *l)), then: Box::new(cond.clone()), els: Box::new(Expr::Bool(false)), newline: false }),
            then: Box::new(taken),
            els: Box::new(chain),
            newline: false,
        };
    }
    Ok(Expr::Block(vec![
        Stmt::Bind { name: saved.clone(), value: pred, line },
        Stmt::Bind { name: pending.clone(), value: call("append", vec![var(&pending, line), polled], line), line },
        Stmt::Bind { name: idle, value: call("machine_idle", vec![var(&pending, line), var(&saved, line)], line), line },
        Stmt::Expr(chain),
    ]))
}

/// `select` on the mailbox with a deadline: `Some(signal)`, or `None`.
fn select_wait(mailbox: &str, s: &str, ms: Expr, line: usize, aline: usize) -> Expr {
    Expr::Select {
        arms: vec![
            SelectArm {
                op: SelectOp::Recv { chan: var(mailbox, line), bind: Some(s.to_string()) },
                body: call("Some", vec![var(s, line)], line),
                line,
            },
            SelectArm { op: SelectOp::Timeout { ms }, body: var("None", aline), line: aline },
        ],
        default: None,
        line,
    }
}

fn tuple(items: Vec<Expr>, line: usize) -> Expr {
    let name = crate::parser::tuple_name(items.len(), line, 0).expect("two to eight");
    Expr::Call { name, args: items, line, piped: false, stacked: false }
}

fn tuple_pat(items: Vec<Pattern>, line: usize) -> Pattern {
    let name = crate::parser::tuple_name(items.len(), line, 0).expect("two to eight");
    Pattern::Ctor { name, args: items, line }
}

/// An arm's body with its tail turned into the next turn of the machine:
/// the next state handed to the call, `stop` made `0`, a branch at the
/// tail followed into its branches.
fn next(m: &MachineDef, body: Expr, state: &str) -> Result<Expr, SyntaxError> {
    Ok(match body {
        Expr::Var(n, line) if n == "stop" => noted(note_of(m, &m.params[0].name, "stop", Expr::Str(String::new()), line), Expr::Int(0)),
        // `done(v)`: the machine's answer, for one called as a procedure
        Expr::Call { name, mut args, line, .. } if name == "done" && args.len() == 1 => {
            let v = args.remove(0);
            let text = crate::fmt::expr_text(&v);
            noted(note_of(m, &m.params[0].name, "done", Expr::Str(format!("\t{text}")), line), v)
        }
        // SDL's `nextstate -`: the state the machine is in
        Expr::Var(n, line) if n == "stay" => noted(next_note(m, "-", state, line), again(m, var(state, line), line)),
        Expr::Block(mut stmts) => match stmts.pop() {
            Some(Stmt::Expr(last)) => {
                stmts.push(Stmt::Expr(next(m, last, state)?));
                Expr::Block(stmts)
            }
            Some(other) => {
                let line = match &other {
                    Stmt::Bind { line, .. } | Stmt::PatBind { line, .. } => *line,
                    Stmt::Fn(f) => f.line,
                    Stmt::Expr(_) => m.line,
                };
                return Err(SyntaxError::new(line, 0, "a state's arm ends in the next state, or `stop`".to_string()));
            }
            None => return Err(SyntaxError::new(m.line, 0, "a state's arm ends in the next state, or `stop`".to_string())),
        },
        Expr::If { cond, then, els, newline } => Expr::If {
            cond,
            then: Box::new(next(m, *then, state)?),
            els: Box::new(next(m, *els, state)?),
            newline,
        },
        Expr::Match { scrutinee, arms, line } => {
            let mut out = Vec::with_capacity(arms.len());
            for a in arms {
                out.push(Arm { pattern: a.pattern, guard: a.guard, body: next(m, a.body, state)?, line: a.line });
            }
            Expr::Match { scrutinee, arms: out, line }
        }
        other => {
            let line = crate::fmt::expr_line(&other);
            let line = if line == 0 { m.line } else { line };
            let text = crate::fmt::expr_text(&other);
            // the next state made first, so the trace can say its value
            let next = "machine.next".to_string();
            Expr::Block(vec![
                Stmt::Bind { name: next.clone(), value: other, line },
                Stmt::Expr(next_note(m, &text, &next, line)),
                Stmt::Expr(again(m, var(&next, line), line)),
            ])
        }
    })
}

/// The `next` event: the next state as written, and its value — shown
/// only when the trace is on, since showing has a cost.
fn next_note(m: &MachineDef, text: &str, value: &str, line: usize) -> Expr {
    let rest = concat(vec![Expr::Str(format!("\t{text}\t")), call("machine_show", vec![var(value, line)], line)], line);
    Expr::If {
        cond: Box::new(Expr::Binary { op: BinOp::Eq, lhs: Box::new(call("tracing", vec![], line)), rhs: Box::new(Expr::Int(1)), line, newline: false }),
        then: Box::new(note_of(m, &m.params[0].name, "next", rest, line)),
        els: Box::new(Expr::Unit),
        newline: false,
    }
}

/// The machine called again, with its parameters, what it carries, and
/// this state.
fn again(m: &MachineDef, state: Expr, line: usize) -> Expr {
    let plan = plan_of(m);
    let mut args: Vec<Expr> = m.params.iter().map(|p| var(&p.name, line)).collect();
    for h in plan.hidden() {
        args.push(var(h, line));
    }
    args.push(state);
    if plan.timers || plan.saves {
        call(&plan.loop_name(&m.name), args, line)
    } else {
        call(&m.name, args, line)
    }
}

fn plan_of(m: &MachineDef) -> Plan {
    let timers = m.states.iter().any(|st| st.arms.iter().any(|a| uses_timers(&a.body)) || st.after.as_ref().is_some_and(|(_, b, _)| uses_timers(b)) || st.whens.iter().any(|(_, b, _)| uses_timers(b)));
    let saves = m.states.iter().any(|st| !st.saves.is_empty() || !st.whens.is_empty() || !st.priorities.is_empty());
    Plan { timers, saves }
}

fn var(n: &str, line: usize) -> Expr {
    Expr::Var(n.to_string(), line)
}

fn call(name: &str, args: Vec<Expr>, line: usize) -> Expr {
    Expr::Call { name: name.to_string(), args, line, piped: false, stacked: false }
}

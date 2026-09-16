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
use crate::ast::*;
use crate::SyntaxError;

pub fn lower(m: &MachineDef) -> Result<Vec<FnDef>, SyntaxError> {
    let line = m.line;
    let state = "machine.state".to_string();
    let mailbox = m.params[0].name.clone();
    let plan = plan_of(m);
    let mut arms = Vec::new();
    for st in &m.states {
        arms.push(Arm { pattern: st.pattern.clone(), guard: None, body: state_body(m, st, &mailbox, &state, &plan)?, line: st.line });
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
    }
}

/// What this machine carries besides its state.
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

/// The timer calls in a body rewritten onto the machine's table.
fn with_timers(e: Expr) -> Expr {
    let e = match e {
        Expr::Call { name, args, line, piped, stacked } => match timer_call(&name, &args) {
            Some(t) => {
                let mut a = vec![var("machine.timers", line), Expr::Str(t)];
                let mut rest = args.into_iter();
                let signal = rest.next().expect("checked");
                let fname = match name.as_str() {
                    "set" => {
                        a.push(rest.next().expect("checked"));
                        a.push(signal);
                        "machine_set"
                    }
                    "reset" => "machine_reset",
                    _ => "machine_active",
                };
                Expr::Call { name: fname.to_string(), args: a, line, piped, stacked }
            }
            None => Expr::Call { name, args, line, piped, stacked },
        },
        other => other,
    };
    crate::lift::map_children(e, &mut with_timers)
}

/// What a state does: take a saved signal it can answer, or receive, or
/// wait with a deadline; then answer the signal, or keep it.
fn state_body(m: &MachineDef, st: &StateDef, mailbox: &str, state: &str, plan: &Plan) -> Result<Expr, SyntaxError> {
    let line = st.line;
    let sig = "machine.sig".to_string();
    let got = "machine.got".to_string();
    let mut sig_arms = Vec::new();
    let mut catches_all = false;
    for a in &st.arms {
        if a.guard.is_none() && matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)) {
            catches_all = true;
        }
        let body = if plan.timers { with_timers(a.body.clone()) } else { a.body.clone() };
        sig_arms.push(Arm { pattern: a.pattern.clone(), guard: a.guard.clone(), body: next(m, body, state)?, line: a.line });
    }
    if !catches_all {
        // a signal no arm names is consumed, and the state kept
        sig_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: again(m, var(state, line), line), line });
    }
    let answer = Expr::Match { scrutinee: Box::new(var(&sig, line)), arms: sig_arms, line };
    let after_body = match &st.after {
        Some((_, body, _)) => {
            let body = if plan.timers { with_timers(body.clone()) } else { body.clone() };
            next(m, body, state)?
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
                Arm { pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(sig.clone())], line }, guard: None, body: answer, line },
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
    let pred = Expr::Lambda { params: vec![x.clone()], body: Box::new(Expr::Match { scrutinee: Box::new(var(&x, line)), arms: save_arms, line }), line };
    let pending = "machine.pending".to_string();
    let rest = "machine.rest".to_string();
    // (the signal if any, the pending list without it)
    let source = Expr::Match {
        scrutinee: Box::new(call("machine_take", vec![var(&pending, line), var(&saved, line)], line)),
        arms: vec![
            Arm {
                pattern: Pattern::Ctor { name: "Some".into(), args: vec![tuple_pat(vec![Pattern::Var(s.clone()), Pattern::Var(rest.clone())], line)], line },
                guard: None,
                body: tuple(vec![call("Some", vec![var(&s, line)], line), var(&rest, line)], line),
                line,
            },
            Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line }, guard: None, body: tuple(vec![wait, var(&pending, line)], line), line },
        ],
        line,
    };
    let keep = Expr::Block(vec![
        Stmt::Bind { name: pending.clone(), value: call("append", vec![var(&pending, line), call("Cons", vec![var(&sig, line), var("Nil", line)], line)], line), line },
        Stmt::Expr(again(m, var(state, line), line)),
    ]);
    let judged = Expr::If {
        cond: Box::new(call(&saved, vec![var(&sig, line)], line)),
        then: Box::new(keep),
        els: Box::new(answer),
        newline: false,
    };
    let outcome = Expr::Match {
        scrutinee: Box::new(var(&got, line)),
        arms: vec![
            Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line: aline }, guard: None, body: after_body, line: aline },
            Arm { pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(sig.clone())], line }, guard: None, body: judged, line },
        ],
        line,
    };
    Ok(Expr::Block(vec![
        Stmt::Bind { name: saved, value: pred, line },
        Stmt::PatBind { pattern: tuple_pat(vec![Pattern::Var(got), Pattern::Var(pending)], line), value: source, line },
        Stmt::Expr(outcome),
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
        Expr::Var(n, _) if n == "stop" => Expr::Int(0),
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
            again(m, other, if line == 0 { m.line } else { line })
        }
    })
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
    let timers = m.states.iter().any(|st| st.arms.iter().any(|a| uses_timers(&a.body)) || st.after.as_ref().is_some_and(|(_, b, _)| uses_timers(b)));
    let saves = m.states.iter().any(|st| !st.saves.is_empty());
    Plan { timers, saves }
}

fn var(n: &str, line: usize) -> Expr {
    Expr::Var(n.to_string(), line)
}

fn call(name: &str, args: Vec<Expr>, line: usize) -> Expr {
    Expr::Call { name: name.to_string(), args, line, piped: false, stacked: false }
}

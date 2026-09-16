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

use crate::ast::*;
use crate::SyntaxError;

pub fn lower(m: &MachineDef) -> Result<FnDef, SyntaxError> {
    let line = m.line;
    let state = "machine.state".to_string();
    let mailbox = m.params[0].name.clone();
    let mut arms = Vec::new();
    for st in &m.states {
        arms.push(Arm { pattern: st.pattern.clone(), guard: None, body: state_body(m, st, &mailbox, &state)?, line: st.line });
    }
    let body = Expr::Match { scrutinee: Box::new(var(&state, line)), arms, line };
    let mut params = m.params.clone();
    params.push(Param { name: state, ann: None });
    Ok(FnDef {
        name: m.name.clone(),
        exported: false,
        params,
        ret_ann: None,
        body,
        line,
        lifted_from: None,
        wheres: Vec::new(),
        private: false,
    })
}

/// What a state does: receive, or wait with a deadline, then answer the
/// signal.
fn state_body(m: &MachineDef, st: &StateDef, mailbox: &str, state: &str) -> Result<Expr, SyntaxError> {
    let line = st.line;
    let sig = "machine.sig".to_string();
    let mut sig_arms = Vec::new();
    let mut catches_all = false;
    for a in &st.arms {
        if a.guard.is_none() && matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)) {
            catches_all = true;
        }
        sig_arms.push(Arm { pattern: a.pattern.clone(), guard: a.guard.clone(), body: next(m, a.body.clone(), state)?, line: a.line });
    }
    if !catches_all {
        // a signal no arm names is consumed, and the state kept
        sig_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: again(m, var(state, line), line), line });
    }
    let answer = Expr::Match { scrutinee: Box::new(var(&sig, line)), arms: sig_arms, line };
    match &st.after {
        None => Ok(Expr::Block(vec![
            Stmt::Bind { name: sig, value: call("recv", vec![var(mailbox, line)], line), line },
            Stmt::Expr(answer),
        ])),
        Some((ms, body, aline)) => {
            let got = "machine.got".to_string();
            let s = "machine.s".to_string();
            let select = Expr::Select {
                arms: vec![
                    SelectArm {
                        op: SelectOp::Recv { chan: var(mailbox, line), bind: Some(s.clone()) },
                        body: call("Some", vec![var(&s, line)], line),
                        line,
                    },
                    SelectArm { op: SelectOp::Timeout { ms: ms.clone() }, body: var("None", *aline), line: *aline },
                ],
                default: None,
                line,
            };
            let outcome = Expr::Match {
                scrutinee: Box::new(var(&got, line)),
                arms: vec![
                    Arm { pattern: Pattern::Ctor { name: "None".into(), args: Vec::new(), line: *aline }, guard: None, body: next(m, body.clone(), state)?, line: *aline },
                    Arm {
                        pattern: Pattern::Ctor { name: "Some".into(), args: vec![Pattern::Var(sig.clone())], line },
                        guard: None,
                        body: answer,
                        line,
                    },
                ],
                line,
            };
            Ok(Expr::Block(vec![Stmt::Bind { name: got, value: select, line }, Stmt::Expr(outcome)]))
        }
    }
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

/// The machine called again, with its parameters and this state.
fn again(m: &MachineDef, state: Expr, line: usize) -> Expr {
    let mut args: Vec<Expr> = m.params.iter().map(|p| var(&p.name, line)).collect();
    args.push(state);
    call(&m.name, args, line)
}

fn var(n: &str, line: usize) -> Expr {
    Expr::Var(n.to_string(), line)
}

fn call(name: &str, args: Vec<Expr>, line: usize) -> Expr {
    Expr::Call { name: name.to_string(), args, line, piped: false, stacked: false }
}

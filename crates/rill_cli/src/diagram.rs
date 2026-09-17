//! `rill diagram`: a `machine` drawn.
//!
//! Two forms. Mermaid's `stateDiagram-v2` is the summary: one arrow per
//! transition, labelled with the signal, its guard, and what goes out on
//! the way. SVG is the diagram SDL drew for a process: a state at the
//! top, and under it one column per signal — the input symbol, then the
//! outputs and tasks in order, a decision where the arm branches, and at
//! the bottom the next state, or the stop mark. That shape is a tree, so
//! it is laid out top-down with no arrows to route: SDL's own reason for
//! drawing it that way.

use rill_syntax::ast::*;
use rill_syntax::fmt::{expr_line, expr_text, pattern_text};
use crate::json::Json;

/// One thing that happens on the way from an input to the next state.
enum Step {
    /// `send(to, sig)`: a signal out, to whom.
    Output(String, String),
    /// `set(T, ms)` or `reset(T)`: a timer started or stopped.
    Timer(String),
    /// A call of one of the file's own functions — SDL's procedure call —
    /// as written, and the function's name, whose diagram is drawn too.
    Call(String, String),
    /// `spawn m(...)`: another strand started — SDL's create request.
    Create(String),
    /// Any other statement, as written.
    Task(String),
}

/// The names of the file's own functions: what a call is a procedure
/// call of. Everything is `machine`s, `system`s and functions; a call of
/// anything else is a task.
pub type Procs = [String];

/// A step with the line it was written on.
struct Placed {
    step: Step,
    line: usize,
}

/// The line a step was written on, for the editor to point at.
fn step_line(s: &Stmt) -> usize {
    match s {
        Stmt::Expr(e) => expr_line(e),
        Stmt::Bind { line, .. } | Stmt::PatBind { line, .. } => *line,
        Stmt::Fn(f) => f.line,
    }
}

/// How an arm ends.
enum End {
    Next(String),
    Stop,
    /// `if` or `match` at the tail: the question, and each branch with
    /// its label, its steps and its end.
    Decision(String, Vec<Branch>),
}

struct Branch {
    label: String,
    steps: Vec<Placed>,
    end: End,
}

struct Transition {
    /// The input: the signal pattern with its guard, or the timer.
    input: String,
    timer: bool,
    /// `input`, `after` or `when`.
    kind: &'static str,
    line: usize,
    steps: Vec<Placed>,
    end: End,
}

fn transitions(st: &StateDef, procs: &Procs) -> Vec<Transition> {
    let mut out = Vec::new();
    let mut items: Vec<(usize, Transition)> = Vec::new();
    for a in &st.arms {
        let input = match &a.guard {
            Some(g) => format!("{} if {}", pattern_text(&a.pattern), expr_text(g)),
            None => pattern_text(&a.pattern),
        };
        let (steps, end) = body_of(&a.body, procs);
        items.push((a.line, Transition { input, timer: false, kind: "input", line: a.line, steps, end }));
    }
    if let Some((ms, body, line)) = &st.after {
        let (steps, end) = body_of(body, procs);
        items.push((*line, Transition { input: format!("after {} ms", expr_text(ms)), timer: true, kind: "after", line: *line, steps, end }));
    }
    for (cond, body, line) in &st.whens {
        let (steps, end) = body_of(body, procs);
        items.push((*line, Transition { input: format!("when {}", expr_text(cond)), timer: false, kind: "when", line: *line, steps, end }));
    }
    items.sort_by_key(|(l, _)| *l);
    for (_, t) in items {
        out.push(t);
    }
    out
}

/// The steps a body takes and how it ends.
fn body_of(e: &Expr, procs: &Procs) -> (Vec<Placed>, End) {
    match e {
        Expr::Block(stmts) => {
            let mut steps = Vec::new();
            let n = stmts.len();
            for (i, s) in stmts.iter().enumerate() {
                if i + 1 == n {
                    if let Stmt::Expr(last) = s {
                        let (more, end) = body_of(last, procs);
                        steps.extend(more);
                        return (steps, end);
                    }
                }
                steps.push(Placed { step: step_of(s, procs), line: step_line(s) });
            }
            (steps, End::Next("?".into()))
        }
        Expr::Var(n, _) if n == "stop" => (Vec::new(), End::Stop),
        Expr::Call { name, args, .. } if name == "done" && args.len() == 1 => (Vec::new(), End::Next(format!("done({})", expr_text(&args[0])))),
        Expr::If { cond, then, els, .. } => {
            let (ts, te) = body_of(then, procs);
            let (es, ee) = body_of(els, procs);
            (
                Vec::new(),
                End::Decision(
                    expr_text(cond),
                    vec![
                        Branch { label: "yes".into(), steps: ts, end: te },
                        Branch { label: "no".into(), steps: es, end: ee },
                    ],
                ),
            )
        }
        Expr::Match { scrutinee, arms, .. } => {
            let branches = arms
                .iter()
                .map(|a| {
                    let (steps, end) = body_of(&a.body, procs);
                    let label = match &a.guard {
                        Some(g) => format!("{} if {}", pattern_text(&a.pattern), expr_text(g)),
                        None => pattern_text(&a.pattern),
                    };
                    Branch { label, steps, end }
                })
                .collect();
            (Vec::new(), End::Decision(expr_text(scrutinee), branches))
        }
        other => (Vec::new(), End::Next(expr_text(other))),
    }
}

/// What a state answers first: `priority Sig`, with its guard.
fn priorities_of(st: &StateDef) -> Vec<String> {
    st.priorities
        .iter()
        .map(|(p, g, _)| match g {
            Some(g) => format!("{} if {}", pattern_text(p), expr_text(g)),
            None => pattern_text(p),
        })
        .collect()
}

/// What a state keeps for later: `save Sig`, with its guard.
fn saves_of(st: &StateDef) -> Vec<String> {
    st.saves
        .iter()
        .map(|(p, g, _)| match g {
            Some(g) => format!("{} if {}", pattern_text(p), expr_text(g)),
            None => pattern_text(p),
        })
        .collect()
}

fn step_of(s: &Stmt, procs: &Procs) -> Step {
    let called = |e: &Expr| match e {
        Expr::Call { name, .. } if procs.iter().any(|p| p == name) => Some(name.clone()),
        _ => None,
    };
    match s {
        Stmt::Expr(e @ Expr::Spawn { .. }) => Step::Create(expr_text(e)),
        Stmt::Bind { name, value: v @ Expr::Spawn { .. }, .. } => Step::Create(format!("{name} = {}", expr_text(v))),
        Stmt::Expr(Expr::Call { name, args, .. }) if (name == "send" || name == "output") && args.len() == 2 => {
            Step::Output(expr_text(&args[0]), expr_text(&args[1]))
        }
        Stmt::Expr(Expr::Call { name, args, .. }) if name == "reply" && args.len() == 1 => {
            Step::Output("sender".to_string(), expr_text(&args[0]))
        }
        Stmt::Expr(Expr::Call { name, args, .. }) if name == "set" && args.len() == 2 => {
            Step::Timer(format!("set {} {} ms", expr_text(&args[0]), expr_text(&args[1])))
        }
        Stmt::Expr(Expr::Call { name, args, .. }) if name == "reset" && args.len() == 1 => {
            Step::Timer(format!("reset {}", expr_text(&args[0])))
        }
        Stmt::Expr(e) => match called(e) {
            Some(f) => Step::Call(expr_text(e), f),
            None => Step::Task(expr_text(e)),
        },
        Stmt::Bind { name, value, .. } => match called(value) {
            Some(f) => Step::Call(format!("{name} = {}", expr_text(value)), f),
            None => Step::Task(format!("{name} = {}", expr_text(value))),
        },
        Stmt::PatBind { pattern, value, .. } => {
            Step::Task(format!("{} = {}", pattern_text(pattern), expr_text(value)))
        }
        Stmt::Fn(f) => Step::Task(format!("fn {}", f.name)),
    }
}

/// The constructor a state pattern names.
fn state_name(p: &Pattern) -> String {
    match p {
        Pattern::Ctor { name, .. } => name.clone(),
        Pattern::Wildcard => "any_state".to_string(),
        other => pattern_text(other),
    }
}

/// The constructor a next-state expression makes, for the arrow's end.
fn ctor_of(next: &str) -> String {
    next.split('(').next().unwrap_or(next).trim().to_string()
}

// ---- Mermaid ----------------------------------------------------------------

pub fn mermaid(m: &MachineDef, procs: &Procs) -> String {
    let mut out = String::from("stateDiagram-v2\n");
    out.push_str(&format!("  %% machine {}\n", m.name));
    let states = m.all_states();
    if states.iter().any(|st| matches!(st.pattern, Pattern::Wildcard)) {
        out.push_str("  state \"* (every state)\" as any_state\n");
    }
    for st in &states {
        let from = state_name(&st.pattern);
        for t in transitions(st, procs) {
            let mut label = t.input.clone();
            let outs = outputs_text(&t.steps);
            if !outs.is_empty() {
                label.push_str(" / ");
                label.push_str(&outs);
            }
            mermaid_end(&mut out, &from, &label, &t.end);
        }
        for s in saves_of(st) {
            out.push_str(&format!("  {from} --> {from}: save {}\n", clean(&s)));
        }
        for s in priorities_of(st) {
            out.push_str(&format!("  note right of {from}: priority {}\n", clean(&s)));
        }
    }
    out
}

fn outputs_text(steps: &[Placed]) -> String {
    let parts: Vec<String> = steps
        .iter()
        .map(|p| match &p.step {
            Step::Output(to, sig) => format!("{sig} to {to}"),
            Step::Timer(t) | Step::Task(t) | Step::Call(t, _) | Step::Create(t) => t.clone(),
        })
        .collect();
    parts.join(", ")
}

fn mermaid_end(out: &mut String, from: &str, label: &str, end: &End) {
    match end {
        End::Next(n) if n == "stay" => out.push_str(&format!("  {from} --> {from}: {}\n", clean(label))),
        End::Next(n) if n.starts_with("done(") => out.push_str(&format!("  {from} --> [*]: {} / {}\n", clean(label), clean(n))),
        End::Next(n) => out.push_str(&format!("  {from} --> {}: {}\n", ctor_of(n), clean(label))),
        End::Stop => out.push_str(&format!("  {from} --> [*]: {}\n", clean(label))),
        End::Decision(q, branches) => {
            for b in branches {
                let mut l = format!("{label} [{q} = {}]", b.label);
                let outs = outputs_text(&b.steps);
                if !outs.is_empty() {
                    l.push_str(" / ");
                    l.push_str(&outs);
                }
                mermaid_end(out, from, &l, &b.end);
            }
        }
    }
}

/// A label on one line, without the characters Mermaid reads as its own.
fn clean(s: &str) -> String {
    s.replace('\n', " ").replace(':', "∶").replace(';', ",")
}

// ---- SVG, as SDL drew it ----------------------------------------------------

const W: f64 = 160.0;
const W_MOST: f64 = 340.0;
const CHAR: f64 = 7.3;
const H: f64 = 40.0;
const GAP_X: f64 = 24.0;
const GAP_Y: f64 = 22.0;
const PAD: f64 = 30.0;
const STATE_GAP: f64 = 60.0;
/// Room under a decision's bus for the branches' labels.
const LABEL: f64 = 18.0;

/// Something to draw in a column: its symbol and its text.
enum Sym {
    State(String),
    Input(String),
    Timer(String),
    Output(String),
    Task(String),
    /// A timer set or reset: the task symbol with the hourglass.
    TimerTask(String),
    /// A signal kept for a later state.
    Save(String),
    /// A procedure called: the task symbol with a bar down each side.
    Call(String),
    /// A strand started: the task symbol with a bar along its top and bottom.
    Create(String),
    Decision(String),
    Stop,
}

/// A column of symbols top-down, which may fork into columns under a
/// decision.
struct Column {
    syms: Vec<Sym>,
    forks: Vec<(String, Column)>,
}

/// A symbol as wide as its text asks, within reason; longer text is cut
/// and kept whole as the tooltip.
fn sym_width(s: &Sym) -> f64 {
    let t = match s {
        Sym::State(t) | Sym::Input(t) | Sym::Timer(t) | Sym::Output(t) | Sym::Task(t) | Sym::TimerTask(t) | Sym::Save(t) | Sym::Call(t) | Sym::Create(t) | Sym::Decision(t) => t.as_str(),
        Sym::Stop => "",
    };
    (t.chars().count() as f64 * CHAR + 40.0).clamp(W, W_MOST)
}

impl Column {
    /// The width of the symbols in this column alone.
    fn own(&self) -> f64 {
        self.syms.iter().map(sym_width).fold(W, f64::max)
    }

    fn width(&self) -> f64 {
        if self.forks.is_empty() {
            self.own()
        } else {
            let sum: f64 = self.forks.iter().map(|(_, c)| c.width()).sum();
            (sum + GAP_X * (self.forks.len() as f64 - 1.0)).max(self.own())
        }
    }

    fn height(&self) -> f64 {
        let own = self.syms.len() as f64 * (H + GAP_Y);
        let below = self.forks.iter().map(|(_, c)| c.height()).fold(0.0, f64::max);
        own + if self.forks.is_empty() { 0.0 } else { below + LABEL }
    }
}

fn column_of(steps: &[Placed], end: &End) -> Column {
    let mut syms: Vec<Sym> = steps
        .iter()
        .map(|p| match &p.step {
            Step::Output(to, sig) => Sym::Output(format!("{sig} → {to}")),
            Step::Timer(t) => Sym::TimerTask(t.clone()),
            Step::Call(t, _) => Sym::Call(t.clone()),
            Step::Create(t) => Sym::Create(t.clone()),
            Step::Task(t) => Sym::Task(t.clone()),
        })
        .collect();
    let mut forks = Vec::new();
    match end {
        End::Next(n) if n == "stay" => syms.push(Sym::State("-".to_string())),
        End::Next(n) => syms.push(Sym::State(n.clone())),
        End::Stop => syms.push(Sym::Stop),
        End::Decision(q, branches) => {
            syms.push(Sym::Decision(q.clone()));
            for b in branches {
                forks.push((b.label.clone(), column_of(&b.steps, &b.end)));
            }
        }
    }
    Column { syms, forks }
}

pub fn svg(m: &MachineDef, procs: &Procs) -> String {
    // one block per state: the state symbol over its transitions' columns
    let mut blocks: Vec<(String, Vec<Column>)> = Vec::new();
    for st in m.all_states() {
        let mut cols: Vec<Column> = transitions(st, procs)
            .into_iter()
            .map(|t| {
                let mut c = column_of(&t.steps, &t.end);
                c.syms.insert(0, if t.timer { Sym::Timer(t.input) } else { Sym::Input(t.input) });
                c
            })
            .collect();
        for s in saves_of(st) {
            cols.push(Column { syms: vec![Sym::Save(s)], forks: Vec::new() });
        }
        let title = if matches!(st.pattern, Pattern::Wildcard) { "* (every state)".to_string() } else { state_name(&st.pattern) };
        blocks.push((title, cols));
    }
    let mut body = String::new();
    let mut y = PAD;
    let mut width: f64 = W + 2.0 * PAD;
    for (name, cols) in &blocks {
        let cols_w: f64 = cols.iter().map(Column::width).sum::<f64>() + GAP_X * (cols.len().saturating_sub(1) as f64);
        let block_w = cols_w.max(W);
        width = width.max(block_w + 2.0 * PAD);
        // the state symbol, centred over its columns
        let sx = PAD + (block_w - W) / 2.0;
        draw(&mut body, &Sym::State(name.clone()), sx, y, W);
        let bus_y = y + H + GAP_Y / 2.0;
        line(&mut body, sx + W / 2.0, y + H, sx + W / 2.0, bus_y);
        let mut x = PAD + (block_w - cols_w) / 2.0;
        let first_x = x + cols[0].width() / 2.0;
        let last_x = x + cols_w - cols.last().map(Column::width).unwrap_or(W) / 2.0;
        if cols.len() > 1 {
            line(&mut body, first_x, bus_y, last_x, bus_y);
        }
        let mut tallest: f64 = 0.0;
        for c in cols {
            let cx = x + c.width() / 2.0;
            line(&mut body, cx, bus_y, cx, y + H + GAP_Y);
            draw_column(&mut body, c, x, y + H + GAP_Y);
            tallest = tallest.max(c.height());
            x += c.width() + GAP_X;
        }
        y += H + GAP_Y + tallest + STATE_GAP;
    }
    let height = y - STATE_GAP + PAD;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" font-family=\"ui-monospace, Menlo, monospace\" font-size=\"12\">\n\
         <title>machine {}</title>\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n{body}</svg>\n",
        esc(&m.name)
    )
}

/// The symbols of a column at `x`, top-down from `y`, joined by lines; a
/// decision's forks side by side under it.
fn draw_column(out: &mut String, c: &Column, x: f64, y: f64) {
    let cx = x + c.width() / 2.0;
    let w = c.own();
    let sx = cx - w / 2.0;
    let mut yy = y;
    for (i, s) in c.syms.iter().enumerate() {
        if i > 0 {
            line(out, cx, yy - GAP_Y, cx, yy);
        }
        draw(out, s, sx, yy, w);
        yy += H + GAP_Y;
    }
    if !c.forks.is_empty() {
        let total: f64 = c.forks.iter().map(|(_, f)| f.width()).sum::<f64>() + GAP_X * (c.forks.len() as f64 - 1.0);
        // the branches' labels sit in a gap of their own under the bus
        let mut fx = cx - total / 2.0;
        let bus_y = yy - GAP_Y / 2.0;
        let top = yy + LABEL;
        let first = fx + c.forks[0].1.width() / 2.0;
        let last = fx + total - c.forks.last().unwrap().1.width() / 2.0;
        line(out, cx, yy - GAP_Y, cx, bus_y);
        line(out, first, bus_y, last, bus_y);
        for (label, f) in &c.forks {
            let fcx = fx + f.width() / 2.0;
            line(out, fcx, bus_y, fcx, top);
            text(out, fcx + 5.0, bus_y + 15.0, label, "start");
            draw_column(out, f, fx, top);
            fx += f.width() + GAP_X;
        }
    }
}

fn draw(out: &mut String, s: &Sym, x: f64, y: f64, w: f64) {
    let (x2, y2, ym) = (x + w, y + H, y + H / 2.0);
    let style = "fill=\"white\" stroke=\"black\" stroke-width=\"1.2\"";
    match s {
        Sym::State(t) => {
            out.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{H}\" rx=\"14\" {style}/>\n"));
            text(out, x + w / 2.0, ym + 4.0, t, "middle");
        }
        Sym::Input(t) => {
            out.push_str(&format!("<polygon points=\"{x},{y} {x2},{y} {x2},{y2} {x},{y2} {},{ym}\" {style}/>\n", x + 12.0));
            text(out, x + w / 2.0 + 6.0, ym + 4.0, t, "middle");
        }
        Sym::Timer(t) => {
            // SDL's timer input: the input symbol with the hourglass at its notch
            out.push_str(&format!("<polygon points=\"{x},{y} {x2},{y} {x2},{y2} {x},{y2} {},{ym}\" {style}/>\n", x + 12.0));
            out.push_str(&format!("<polygon points=\"{},{} {},{} {},{} {},{}\" fill=\"black\"/>\n", x + 16.0, y + 8.0, x + 26.0, y + 8.0, x + 16.0, y2 - 8.0, x + 26.0, y2 - 8.0));
            text(out, x + w / 2.0 + 12.0, ym + 4.0, t, "middle");
        }
        Sym::Output(t) => {
            out.push_str(&format!("<polygon points=\"{x},{y} {},{y} {x2},{ym} {},{y2} {x},{y2}\" {style}/>\n", x2 - 12.0, x2 - 12.0));
            text(out, x + w / 2.0 - 6.0, ym + 4.0, t, "middle");
        }
        Sym::Task(t) => {
            out.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{H}\" {style}/>\n"));
            text(out, x + w / 2.0, ym + 4.0, t, "middle");
        }
        Sym::TimerTask(t) => {
            // a task with the hourglass: a timer set or reset
            out.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{H}\" {style}/>\n"));
            out.push_str(&format!("<polygon points=\"{},{} {},{} {},{} {},{}\" fill=\"black\"/>\n", x + 8.0, y + 8.0, x + 18.0, y + 8.0, x + 8.0, y2 - 8.0, x + 18.0, y2 - 8.0));
            text(out, x + w / 2.0 + 6.0, ym + 4.0, t, "middle");
        }
        Sym::Save(t) => {
            // SDL's save: the rectangle with its right side notched inward
            out.push_str(&format!("<polygon points=\"{x},{y} {x2},{y} {},{ym} {x2},{y2} {x},{y2}\" {style}/>\n", x2 - 12.0));
            text(out, x + w / 2.0 - 6.0, ym + 4.0, &format!("save {t}"), "middle");
        }
        Sym::Call(t) => {
            // SDL's procedure call: the task with a bar inside each side
            out.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{H}\" {style}/>\n"));
            line(out, x + 6.0, y, x + 6.0, y2);
            line(out, x2 - 6.0, y, x2 - 6.0, y2);
            text(out, x + w / 2.0, ym + 4.0, t, "middle");
        }
        Sym::Create(t) => {
            // SDL's create request: the task with a bar inside its top and bottom
            out.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{H}\" {style}/>\n"));
            line(out, x, y + 5.0, x2, y + 5.0);
            line(out, x, y2 - 5.0, x2, y2 - 5.0);
            text(out, x + w / 2.0, ym + 4.0, t, "middle");
        }
        Sym::Decision(t) => {
            let cx = x + w / 2.0;
            out.push_str(&format!("<polygon points=\"{cx},{y} {x2},{ym} {cx},{y2} {x},{ym}\" {style}/>\n"));
            text(out, cx, ym + 4.0, t, "middle");
        }
        Sym::Stop => {
            let cx = x + w / 2.0;
            out.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"black\" stroke-width=\"2\"/>\n", cx - 12.0, y + 6.0, cx + 12.0, y2 - 6.0));
            out.push_str(&format!("<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"black\" stroke-width=\"2\"/>\n", cx + 12.0, y + 6.0, cx - 12.0, y2 - 6.0));
        }
    }
}

fn line(out: &mut String, x1: f64, y1: f64, x2: f64, y2: f64) {
    out.push_str(&format!("<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"black\" stroke-width=\"1.2\"/>\n"));
}

/// Text that fits its symbol: what does not is cut, with an ellipsis, and
/// the whole kept as the tooltip.
fn text(out: &mut String, x: f64, y: f64, t: &str, anchor: &str) {
    let most = ((W_MOST - 40.0) / CHAR) as usize;
    let shown: String = if t.chars().count() > most { t.chars().take(most - 1).collect::<String>() + "…" } else { t.to_string() };
    out.push_str(&format!("<text x=\"{x}\" y=\"{y}\" text-anchor=\"{anchor}\"><title>{}</title>{}</text>\n", esc(t), esc(&shown)));
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- a trace, as a message sequence chart -----------------------------------
//
// `RILL_TRACE=1` makes every machine say what it did, one event a line on
// standard error, the fields tab-separated and the line numbered by the
// runtime in the order events happened. `rill msc` reads those lines and
// draws the MSC the SDL tools drew from a simulation: a lifeline per
// machine and one per mailbox of the environment, an arrow per signal,
// and a note where a state changed, a timer was set, or a signal was
// dropped.

/// One event of a trace.
struct Event {
    kind: String,
    /// `line#2`, or for `post` the mailbox it came from.
    who: String,
    fields: Vec<String>,
}

fn parse_trace(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix("rill trace ") else { continue };
        let Some((_, body)) = rest.split_once(' ') else { continue };
        let mut parts = body.split('\t').map(|s| s.to_string());
        let Some(kind) = parts.next() else { continue };
        let Some(who) = parts.next() else { continue };
        out.push(Event { kind, who, fields: parts.collect() });
    }
    out
}

/// A mailbox number as the lifeline it stands for: a machine's name with
/// its number where a machine has that mailbox, `env#n` otherwise.
fn lifeline(id: &str, machines: &std::collections::HashMap<String, String>) -> String {
    machines.get(id).cloned().unwrap_or_else(|| format!("env#{id}"))
}

fn mermaid_id(name: &str) -> String {
    name.replace('#', "_")
}

/// One thing drawn on the chart.
enum Item {
    /// A signal from one lifeline to another; the same for a timer's.
    Arrow(String, String, String),
    /// A note over a lifeline.
    Note(String, String),
}

/// The lifelines in the order they first appear, and the items.
fn chart(text: &str) -> (Vec<String>, Vec<Item>) {
    let events = parse_trace(text);
    // which mailbox numbers are machines, from the events they logged
    let mut machines: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in &events {
        if e.kind != "post" {
            if let Some((_, id)) = e.who.rsplit_once('#') {
                machines.insert(id.to_string(), e.who.clone());
            }
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut seen = |name: String, order: &mut Vec<String>| {
        if !order.contains(&name) {
            order.push(name);
        }
    };
    for e in &events {
        match e.kind.as_str() {
            "post" => {
                seen(lifeline(&e.who, &machines), &mut order);
                if let Some(to) = e.fields.first() {
                    seen(lifeline(to, &machines), &mut order);
                }
            }
            "recv" | "drop" => {
                if let Some(from) = e.fields.last() {
                    seen(lifeline(from, &machines), &mut order);
                }
                seen(e.who.clone(), &mut order);
            }
            "send" => {
                seen(e.who.clone(), &mut order);
                if let Some(to) = e.fields.get(1) {
                    seen(lifeline(to, &machines), &mut order);
                }
            }
            _ => seen(e.who.clone(), &mut order),
        }
    }
    // A signal is drawn once: from the sender's `send` between machines,
    // and from the receiver's `recv` when it came from the environment,
    // which logs nothing of its own; a timer's signal is a self-arrow.
    let mut items = Vec::new();
    let mut last_state: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in &events {
        let me = e.who.clone();
        match e.kind.as_str() {
            "recv" | "drop" => {
                let (state, sig, from) = match e.kind.as_str() {
                    "recv" => (e.fields.first(), e.fields.get(1), e.fields.get(2)),
                    _ => (e.fields.first(), None, e.fields.get(1)),
                };
                let from_id = from.cloned().unwrap_or_default();
                let from_name = lifeline(&from_id, &machines);
                let label = sig.cloned().unwrap_or_else(|| "a signal".to_string());
                if from_name == me {
                    items.push(Item::Arrow(me.clone(), me.clone(), format!("{label} (timer)")));
                } else if !machines.contains_key(&from_id) {
                    items.push(Item::Arrow(from_name, me.clone(), label));
                }
                if e.kind == "drop" {
                    items.push(Item::Note(me.clone(), format!("dropped in {}", state.map(String::as_str).unwrap_or("?"))));
                }
                if let Some(s) = state {
                    last_state.insert(me, s.clone());
                }
            }
            "send" => {
                let sig = e.fields.first().cloned().unwrap_or_default();
                let to = lifeline(e.fields.get(1).map(String::as_str).unwrap_or(""), &machines);
                items.push(Item::Arrow(me, to, sig));
            }
            "next" => {
                let next = e.fields.first().cloned().unwrap_or_default();
                let from = last_state.get(&me).cloned().unwrap_or_default();
                if next != "-" {
                    items.push(Item::Note(me, format!("{from} → {next}")));
                }
            }
            "stop" => items.push(Item::Note(me, "stop".to_string())),
            "done" => items.push(Item::Note(me, format!("done {}", e.fields.first().cloned().unwrap_or_default()))),
            "after" => {
                let state = e.fields.first().cloned().unwrap_or_default();
                let ms = e.fields.get(1).cloned().unwrap_or_default();
                items.push(Item::Note(me.clone(), format!("after {ms} ms in {state}")));
                last_state.insert(me, state);
            }
            "when" => {
                let state = e.fields.first().cloned().unwrap_or_default();
                let cond = e.fields.get(1).cloned().unwrap_or_default();
                items.push(Item::Note(me.clone(), format!("when {cond}")));
                last_state.insert(me, state);
            }
            "set" => {
                let t = e.fields.first().cloned().unwrap_or_default();
                let ms = e.fields.get(1).cloned().unwrap_or_default();
                items.push(Item::Note(me, format!("set {t} {ms} ms")));
            }
            "reset" => items.push(Item::Note(me, format!("reset {}", e.fields.first().cloned().unwrap_or_default()))),
            "save" => items.push(Item::Note(me, format!("saved in {}", e.fields.first().cloned().unwrap_or_default()))),
            _ => {}
        }
    }
    (order, items)
}

/// The trace as Mermaid's `sequenceDiagram`.
pub fn msc(text: &str) -> String {
    let (order, items) = chart(text);
    let mut out = String::from("sequenceDiagram\n");
    for name in &order {
        out.push_str(&format!("  participant {} as {}\n", mermaid_id(name), name));
    }
    for item in &items {
        match item {
            Item::Arrow(from, to, label) => out.push_str(&format!("  {}->>{}: {}\n", mermaid_id(from), mermaid_id(to), clean(label))),
            Item::Note(who, text) => out.push_str(&format!("  Note over {}: {}\n", mermaid_id(who), clean(text))),
        }
    }
    out
}

/// The trace as SVG: a lifeline per participant, a row per item.
pub fn msc_svg(trace: &str) -> String {
    let (order, items) = chart(trace);
    const COL: f64 = 200.0;
    const ROW: f64 = 34.0;
    const HEAD: f64 = 40.0;
    const TOP: f64 = 30.0;
    const SIDE: f64 = 40.0;
    let x_of = |name: &str| -> f64 {
        let i = order.iter().position(|n| n == name).unwrap_or(0) as f64;
        SIDE + COL / 2.0 + i * COL
    };
    let width = SIDE * 2.0 + COL * order.len().max(1) as f64;
    let height = TOP + HEAD + ROW * (items.len() as f64 + 1.0) + 20.0;
    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" font-family=\"Menlo, Consolas, monospace\" font-size=\"12\">\n<title>message sequence chart</title>\n<defs><marker id=\"arrow\" markerWidth=\"10\" markerHeight=\"8\" refX=\"9\" refY=\"4\" orient=\"auto\"><path d=\"M0,0 L10,4 L0,8 z\" fill=\"black\"/></marker></defs>\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n"
    ));
    let bottom = TOP + HEAD + ROW * (items.len() as f64 + 1.0);
    for name in &order {
        let x = x_of(name);
        out.push_str(&format!("<rect x=\"{}\" y=\"{TOP}\" width=\"{}\" height=\"{HEAD}\" fill=\"white\" stroke=\"black\" stroke-width=\"1.2\"/>\n", x - COL / 2.0 + 10.0, COL - 20.0));
        text(&mut out, x, TOP + HEAD / 2.0 + 4.0, name, "middle");
        line(&mut out, x, TOP + HEAD, x, bottom);
    }
    let mut y = TOP + HEAD + ROW;
    for item in &items {
        match item {
            Item::Arrow(from, to, label) => {
                let (x1, x2) = (x_of(from), x_of(to));
                if from == to {
                    // a timer: out and back on the lifeline's right
                    out.push_str(&format!("<path d=\"M{x1},{} h 30 v 14 h -30\" fill=\"none\" stroke=\"black\" stroke-width=\"1.2\" marker-end=\"url(#arrow)\"/>\n", y - 7.0));
                    text(&mut out, x1 + 36.0, y + 4.0, label, "start");
                } else {
                    out.push_str(&format!("<line x1=\"{x1}\" y1=\"{y}\" x2=\"{x2}\" y2=\"{y}\" stroke=\"black\" stroke-width=\"1.2\" marker-end=\"url(#arrow)\"/>\n"));
                    text(&mut out, (x1 + x2) / 2.0, y - 5.0, label, "middle");
                }
            }
            Item::Note(who, note) => {
                let x = x_of(who);
                // as wide as the text the helper will show, which it cuts
                let shown = note.chars().count().min(((W_MOST - 40.0) / CHAR) as usize);
                let w = (shown as f64 * 7.3 + 16.0).max(60.0);
                out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{w}\" height=\"22\" fill=\"#fff8dc\" stroke=\"black\" stroke-width=\"1\"/>\n", x - w / 2.0, y - 11.0));
                text(&mut out, x, y + 4.0, note, "middle");
            }
        }
        y += ROW;
    }
    out.push_str("</svg>\n");
    out
}

// ---- a system, as SDL's block diagram --------------------------------------

/// The system as Mermaid's `flowchart`: a box per machine and one for the
/// environment, an arrow per route with its signals.
pub fn system_mermaid(sd: &SystemDef) -> String {
    let routes = sd.all_routes();
    let mut out = String::from("flowchart LR\n");
    out.push_str(&format!("  %% system {}\n", sd.name));
    let mut ends: Vec<String> = Vec::new();
    for r in &routes {
        for e in [&r.from, &r.to] {
            if !ends.contains(e) {
                ends.push(e.clone());
            }
        }
    }
    for e in &ends {
        if e == "env" {
            out.push_str("  env([environment])\n");
        } else {
            out.push_str(&format!("  {e}[{e}]\n"));
        }
    }
    for r in &routes {
        // a channel with delay is drawn dashed
        let arrow = match (r.both, r.delay) {
            (true, true) => "<-.->",
            (true, false) => "<-->",
            (false, true) => "-.->",
            (false, false) => "-->",
        };
        out.push_str(&format!("  {} {arrow}|{}| {}\n", r.from, clean(&r.signals.join(", ")), r.to));
    }
    out
}

/// The system as SVG: the environment on the left, the machines in a row,
/// each route an arc between its ends with the signals along it, a route
/// from a machine to itself a loop above it.
pub fn system_svg(sd: &SystemDef) -> String {
    let routes = sd.all_routes();
    let mut ends: Vec<String> = Vec::new();
    for r in &routes {
        for e in [&r.from, &r.to] {
            if !ends.contains(e) {
                ends.push(e.clone());
            }
        }
    }
    ends.sort_by_key(|e| if e == "env" { 0 } else { 1 });
    const COL: f64 = 260.0;
    const BOX_W: f64 = 150.0;
    const BOX_H: f64 = 50.0;
    const SIDE: f64 = 50.0;
    let lanes = ((routes.len() + 1) / 2) as f64;
    let top = 40.0 + 34.0 * lanes;
    let width = SIDE * 2.0 + COL * ends.len().max(1) as f64;
    let height = top + BOX_H + 60.0 + 34.0 * lanes;
    let x_of = |name: &str| -> f64 {
        let i = ends.iter().position(|n| n == name).unwrap_or(0) as f64;
        SIDE + COL / 2.0 + i * COL
    };
    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" font-family=\"Menlo, Consolas, monospace\" font-size=\"12\">\n<title>system {}</title>\n<defs><marker id=\"arrow\" markerWidth=\"10\" markerHeight=\"8\" refX=\"9\" refY=\"4\" orient=\"auto\"><path d=\"M0,0 L10,4 L0,8 z\" fill=\"black\"/></marker><marker id=\"arrow_back\" markerWidth=\"10\" markerHeight=\"8\" refX=\"1\" refY=\"4\" orient=\"auto\"><path d=\"M10,0 L0,4 L10,8 z\" fill=\"black\"/></marker></defs>\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n",
        esc(&sd.name)
    ));
    for e in &ends {
        let x = x_of(e);
        if e == "env" {
            out.push_str(&format!("<rect x=\"{}\" y=\"{top}\" width=\"{BOX_W}\" height=\"{BOX_H}\" rx=\"25\" fill=\"white\" stroke=\"black\" stroke-width=\"1.2\" stroke-dasharray=\"6,4\"/>\n", x - BOX_W / 2.0));
            text(&mut out, x, top + BOX_H / 2.0 + 4.0, "environment", "middle");
        } else {
            out.push_str(&format!("<rect x=\"{}\" y=\"{top}\" width=\"{BOX_W}\" height=\"{BOX_H}\" fill=\"white\" stroke=\"black\" stroke-width=\"1.2\"/>\n", x - BOX_W / 2.0));
            text(&mut out, x, top + BOX_H / 2.0 + 4.0, e, "middle");
        }
    }
    // Routes as SDL drew channels: up from one box, across in a lane of
    // their own, down into the other; the first above, the next below,
    // and so on outward, so no two labels share a line. Several routes at
    // one box spread along its edge.
    let mut slots: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, r) in routes.iter().enumerate() {
        let label = r.signals.join(", ");
        let (x1, x2) = (x_of(&r.from), x_of(&r.to));
        let above = i % 2 == 0;
        let lift = 30.0 + 34.0 * ((i / 2) as f64);
        let mut slot = |end: &str| -> f64 {
            let k = format!("{end}{}", if above { "^" } else { "v" });
            let n = slots.entry(k).or_insert(0);
            *n += 1;
            *n as f64
        };
        let dx1 = (slot(&r.from) - 1.0) * 14.0 - 21.0;
        let dx2 = (slot(&r.to) - 1.0) * 14.0 + 21.0 - if x2 > x1 { 42.0 } else { 0.0 };
        let markers = if r.both { "marker-start=\"url(#arrow_back)\" marker-end=\"url(#arrow)\"" } else { "marker-end=\"url(#arrow)\"" };
        let y = if above { top } else { top + BOX_H };
        let ly = if above { y - lift } else { y + lift };
        let label_y = if above { ly - 5.0 } else { ly + 14.0 };
        if r.from == r.to {
            out.push_str(&format!("<path d=\"M{},{y} V{ly} H{} V{y}\" fill=\"none\" stroke=\"black\" stroke-width=\"1.2\" {markers}/>\n", x1 - 30.0, x1 + 30.0));
            text(&mut out, x1, label_y, &label, "middle");
        } else {
            out.push_str(&format!("<path d=\"M{},{y} V{ly} H{} V{y}\" fill=\"none\" stroke=\"black\" stroke-width=\"1.2\" {markers}/>\n", x1 + dx1, x2 + dx2));
            text(&mut out, (x1 + x2) / 2.0, label_y, &label, "middle");
            // a channel with delay: the arrowhead midway too, as SDL told a
            // channel from a signal route
            if r.delay {
                let mid = (x1 + dx1 + x2 + dx2) / 2.0;
                let dir = if x2 > x1 { 1.0 } else { -1.0 };
                out.push_str(&format!("<path d=\"M{},{} L{},{ly} L{},{} z\" fill=\"black\"/>\n", mid - 6.0 * dir, ly - 4.0, mid + 4.0 * dir, mid - 6.0 * dir, ly + 4.0));
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

// ---- the model, as data ---------------------------------------------------
//
// `rill diagram --json`: systems and machines as the editor wants them —
// every symbol with the line it was written on, so a diagram drawn from
// this can point back at the code, and an edit made on it knows where in
// the file it belongs.

fn step_json(p: &Placed) -> Json {
    let line = ("line", Json::Num(p.line as f64));
    match &p.step {
        Step::Output(to, sig) => Json::obj(vec![("kind", Json::str("output")), ("to", Json::str(to.clone())), ("signal", Json::str(sig.clone())), line]),
        Step::Timer(t) => Json::obj(vec![("kind", Json::str("timer")), ("text", Json::str(t.clone())), line]),
        Step::Call(t, f) => Json::obj(vec![("kind", Json::str("call")), ("text", Json::str(t.clone())), ("name", Json::str(f.clone())), line]),
        Step::Create(t) => Json::obj(vec![("kind", Json::str("create")), ("text", Json::str(t.clone())), line]),
        Step::Task(t) => Json::obj(vec![("kind", Json::str("task")), ("text", Json::str(t.clone())), line]),
    }
}

fn end_json(e: &End) -> Json {
    match e {
        End::Next(n) if n == "stay" => Json::obj(vec![("kind", Json::str("stay"))]),
        End::Next(n) if n.starts_with("done(") => Json::obj(vec![("kind", Json::str("done")), ("text", Json::str(n.clone()))]),
        End::Next(n) => Json::obj(vec![("kind", Json::str("next")), ("text", Json::str(n.clone())), ("state", Json::str(ctor_of(n)))]),
        End::Stop => Json::obj(vec![("kind", Json::str("stop"))]),
        End::Decision(q, branches) => Json::obj(vec![
            ("kind", Json::str("decision")),
            ("question", Json::str(q.clone())),
            (
                "branches",
                Json::Arr(
                    branches
                        .iter()
                        .map(|b| {
                            Json::obj(vec![
                                ("label", Json::str(b.label.clone())),
                                ("steps", Json::Arr(b.steps.iter().map(step_json).collect())),
                                ("end", end_json(&b.end)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
    }
}

/// The functions a body calls, by name, through the decisions.
fn calls_in(steps: &[Placed], end: &End, out: &mut Vec<String>) {
    for p in steps {
        if let Step::Call(_, f) = &p.step {
            if !out.contains(f) {
                out.push(f.clone());
            }
        }
    }
    if let End::Decision(_, branches) = end {
        for b in branches {
            calls_in(&b.steps, &b.end, out);
        }
    }
}

/// A route, with the block it is written in (`""` at the top of the system).
fn route_json(r: &Route, block: &str) -> Json {
    Json::obj(vec![
        ("from", Json::str(r.from.clone())),
        ("to", Json::str(r.to.clone())),
        ("both", Json::Bool(r.both)),
        ("delay", Json::Bool(r.delay)),
        ("signals", Json::Arr(r.signals.iter().map(|s| Json::str(s.clone())).collect())),
        ("line", Json::Num(r.line as f64)),
        ("block", Json::str(block.to_string())),
    ])
}

/// A block: its name, the machines that belong to it — named by its own
/// routes and by no block within it — and the blocks within it.
fn block_json(sd: &SystemDef, b: &BlockDef) -> Json {
    let mut own: Vec<&Route> = Vec::new();
    b.all_routes(&mut own);
    let mut machines: Vec<String> = Vec::new();
    for r in &own {
        for end in [&r.from, &r.to] {
            if end != "env" && !machines.contains(end) && sd.block_of(end).map(|found| std::ptr::eq(found, b)).unwrap_or(false) {
                machines.push(end.clone());
            }
        }
    }
    Json::obj(vec![
        ("name", Json::str(b.name.clone())),
        ("line", Json::Num(b.line as f64)),
        ("machines", Json::Arr(machines.into_iter().map(Json::str).collect())),
        ("routes", Json::Arr(b.routes.iter().map(|r| route_json(r, &b.name)).collect())),
        ("blocks", Json::Arr(b.blocks.iter().map(|inner| block_json(sd, inner)).collect())),
    ])
}

/// A system: every route, the blocks' included, each saying which block
/// it is written in; and the blocks as a tree.
fn system_json(sd: &SystemDef) -> Json {
    let mut routes: Vec<Json> = sd.routes.iter().map(|r| route_json(r, "")).collect();
    fn walk(b: &BlockDef, out: &mut Vec<Json>) {
        for r in &b.routes {
            out.push(route_json(r, &b.name));
        }
        for inner in &b.blocks {
            walk(inner, out);
        }
    }
    for b in &sd.blocks {
        walk(b, &mut routes);
    }
    let lists: Vec<Json> = sd
        .lists
        .iter()
        .map(|l| {
            Json::obj(vec![
                ("name", Json::str(l.name.clone())),
                ("line", Json::Num(l.line as f64)),
                ("signals", Json::Arr(l.signals.iter().map(|s| Json::str(s.clone())).collect())),
            ])
        })
        .collect();
    Json::obj(vec![
        ("name", Json::str(sd.name.clone())),
        ("line", Json::Num(sd.line as f64)),
        ("lists", Json::Arr(lists)),
        ("routes", Json::Arr(routes)),
        ("blocks", Json::Arr(sd.blocks.iter().map(|b| block_json(sd, b)).collect())),
    ])
}

/// A state: its transitions, saves and priorities — and, for a composite
/// state, its substates, each the same.
fn state_json(m: &MachineDef, st: &StateDef, procs: &Procs) -> Json {
    let keyword = |word: &str, list: &[(Pattern, Option<Expr>, usize)]| -> Json {
        Json::Arr(
            list.iter()
                .map(|(p, g, l)| {
                    let text = match g {
                        Some(g) => format!("{} if {}", pattern_text(p), expr_text(g)),
                        None => pattern_text(p),
                    };
                    Json::obj(vec![("kind", Json::str(word)), ("text", Json::str(text)), ("line", Json::Num(*l as f64))])
                })
                .collect(),
        )
    };
    Json::obj(vec![
        ("name", Json::str(if matches!(st.pattern, Pattern::Wildcard) { "*".to_string() } else { state_name(&st.pattern) })),
        ("pattern", Json::str(if matches!(st.pattern, Pattern::Wildcard) { "*".to_string() } else { pattern_text(&st.pattern) })),
        ("line", Json::Num(st.line as f64)),
        (
            "transitions",
            Json::Arr(
                transitions(st, procs)
                    .iter()
                    .map(|t| {
                        Json::obj(vec![
                            ("kind", Json::str(t.kind)),
                            ("input", Json::str(t.input.clone())),
                            ("line", Json::Num(t.line as f64)),
                            ("steps", Json::Arr(t.steps.iter().map(step_json).collect())),
                            ("end", end_json(&t.end)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("saves", keyword("save", &st.saves)),
        ("priorities", keyword("priority", &st.priorities)),
        ("composite", Json::Bool(st.is_composite())),
        ("substates", Json::Arr(st.substates.iter().map(|s| state_json(m, s, procs)).collect())),
    ])
}

/// Everything drawable in a file: its systems, its machines, and the
/// procedures — the file's functions the machines call, and those they
/// call in turn — each drawn as SDL drew a procedure: its body's steps
/// and decisions, ending in what it returns.
pub fn model_json(systems: &[SystemDef], machines: &[MachineDef], functions: &[FnDef]) -> Json {
    let procs: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();
    let procs = procs.as_slice();
    let systems: Vec<Json> = systems.iter().map(system_json).collect();
    let drawn_machines: Vec<Json> = machines
        .iter()
        .map(|m| {
            let states: Vec<Json> = m.states.iter().map(|st| state_json(m, st, procs)).collect();
            Json::obj(vec![
                ("name", Json::str(m.name.clone())),
                ("line", Json::Num(m.line as f64)),
                ("params", Json::Arr(m.params.iter().map(|p| Json::str(p.name.clone())).collect())),
                ("states", Json::Arr(states)),
            ])
        })
        .collect();
    // the procedures: what the machines call, and what those call, until nothing new is called
    let mut wanted: Vec<String> = Vec::new();
    for m in machines {
        for st in m.all_states() {
            for t in transitions(st, procs) {
                calls_in(&t.steps, &t.end, &mut wanted);
            }
        }
    }
    let mut i = 0;
    while i < wanted.len() {
        if let Some(f) = functions.iter().find(|f| f.name == wanted[i]) {
            let (steps, end) = body_of(&f.body, procs);
            calls_in(&steps, &end, &mut wanted);
        }
        i += 1;
    }
    let procedures: Vec<Json> = functions
        .iter()
        .filter(|f| wanted.contains(&f.name))
        .map(|f| {
            let (steps, end) = body_of(&f.body, procs);
            Json::obj(vec![
                ("name", Json::str(f.name.clone())),
                ("line", Json::Num(f.line as f64)),
                ("params", Json::Arr(f.params.iter().map(|p| Json::str(p.name.clone())).collect())),
                ("steps", Json::Arr(steps.iter().map(step_json).collect())),
                ("end", end_json(&end)),
            ])
        })
        .collect();
    Json::obj(vec![("systems", Json::Arr(systems)), ("machines", Json::Arr(drawn_machines)), ("procedures", Json::Arr(procedures))])
}

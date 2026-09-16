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
use rill_syntax::fmt::{expr_text, pattern_text};

/// One thing that happens on the way from an input to the next state.
enum Step {
    /// `send(to, sig)`: a signal out, to whom.
    Output(String, String),
    /// Any other statement, as written.
    Task(String),
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
    steps: Vec<Step>,
    end: End,
}

struct Transition {
    /// The input: the signal pattern with its guard, or the timer.
    input: String,
    timer: bool,
    steps: Vec<Step>,
    end: End,
}

fn transitions(st: &StateDef) -> Vec<Transition> {
    let mut out = Vec::new();
    let mut items: Vec<(usize, Transition)> = Vec::new();
    for a in &st.arms {
        let input = match &a.guard {
            Some(g) => format!("{} if {}", pattern_text(&a.pattern), expr_text(g)),
            None => pattern_text(&a.pattern),
        };
        let (steps, end) = body_of(&a.body);
        items.push((a.line, Transition { input, timer: false, steps, end }));
    }
    if let Some((ms, body, line)) = &st.after {
        let (steps, end) = body_of(body);
        items.push((*line, Transition { input: format!("after {} ms", expr_text(ms)), timer: true, steps, end }));
    }
    items.sort_by_key(|(l, _)| *l);
    for (_, t) in items {
        out.push(t);
    }
    out
}

/// The steps a body takes and how it ends.
fn body_of(e: &Expr) -> (Vec<Step>, End) {
    match e {
        Expr::Block(stmts) => {
            let mut steps = Vec::new();
            let n = stmts.len();
            for (i, s) in stmts.iter().enumerate() {
                if i + 1 == n {
                    if let Stmt::Expr(last) = s {
                        let (more, end) = body_of(last);
                        steps.extend(more);
                        return (steps, end);
                    }
                }
                steps.push(step_of(s));
            }
            (steps, End::Next("?".into()))
        }
        Expr::Var(n, _) if n == "stop" => (Vec::new(), End::Stop),
        Expr::If { cond, then, els, .. } => {
            let (ts, te) = body_of(then);
            let (es, ee) = body_of(els);
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
                    let (steps, end) = body_of(&a.body);
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

fn step_of(s: &Stmt) -> Step {
    match s {
        Stmt::Expr(Expr::Call { name, args, .. }) if name == "send" && args.len() == 2 => {
            Step::Output(expr_text(&args[0]), expr_text(&args[1]))
        }
        Stmt::Expr(e) => Step::Task(expr_text(e)),
        Stmt::Bind { name, value, .. } => Step::Task(format!("{name} = {}", expr_text(value))),
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
        other => pattern_text(other),
    }
}

/// The constructor a next-state expression makes, for the arrow's end.
fn ctor_of(next: &str) -> String {
    next.split('(').next().unwrap_or(next).trim().to_string()
}

// ---- Mermaid ----------------------------------------------------------------

pub fn mermaid(m: &MachineDef) -> String {
    let mut out = String::from("stateDiagram-v2\n");
    out.push_str(&format!("  %% machine {}\n", m.name));
    for st in &m.states {
        let from = state_name(&st.pattern);
        for t in transitions(st) {
            let mut label = t.input.clone();
            let outs = outputs_text(&t.steps);
            if !outs.is_empty() {
                label.push_str(" / ");
                label.push_str(&outs);
            }
            mermaid_end(&mut out, &from, &label, &t.end);
        }
    }
    out
}

fn outputs_text(steps: &[Step]) -> String {
    let parts: Vec<String> = steps
        .iter()
        .map(|s| match s {
            Step::Output(to, sig) => format!("{sig} to {to}"),
            Step::Task(t) => t.clone(),
        })
        .collect();
    parts.join(", ")
}

fn mermaid_end(out: &mut String, from: &str, label: &str, end: &End) {
    match end {
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
        Sym::State(t) | Sym::Input(t) | Sym::Timer(t) | Sym::Output(t) | Sym::Task(t) | Sym::Decision(t) => t.as_str(),
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

fn column_of(steps: &[Step], end: &End) -> Column {
    let mut syms: Vec<Sym> = steps
        .iter()
        .map(|s| match s {
            Step::Output(to, sig) => Sym::Output(format!("{sig} → {to}")),
            Step::Task(t) => Sym::Task(t.clone()),
        })
        .collect();
    let mut forks = Vec::new();
    match end {
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

pub fn svg(m: &MachineDef) -> String {
    // one block per state: the state symbol over its transitions' columns
    let mut blocks: Vec<(String, Vec<Column>)> = Vec::new();
    for st in &m.states {
        let cols = transitions(st)
            .into_iter()
            .map(|t| {
                let mut c = column_of(&t.steps, &t.end);
                c.syms.insert(0, if t.timer { Sym::Timer(t.input) } else { Sym::Input(t.input) });
                c
            })
            .collect();
        blocks.push((state_name(&st.pattern), cols));
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

//! `rill doc`: a module's documentation, read off the file.
//!
//! There is no doc-comment syntax: the comment a declaration already has
//! above it is its documentation, as every module in `lib/` has been
//! written. A `#` block on the lines just before a `type`, `trait`, `fn`,
//! `machine` or `system` belongs to it; the block after a `module` line
//! is the module's own. What comes out is Markdown — the module's head,
//! then its types with their constructors, its traits with their methods,
//! its machines with their states and the diagram, its systems with
//! their routes, and its functions with the signatures the checker
//! inferred, private ones left out. Nothing is written twice: what the
//! page says is what the file says.
//!
//! A comment line indented four spaces past the `#` is an example: code
//! as the reader would write it, the module by its name — `json.parse`
//! — and, after `# =>`, what `show` prints of its value (a string bare,
//! as `show` prints it). The page shows
//! the examples as code; `rill doctest` runs every one and holds it to
//! its `=>`, so an example on the page is one that was true when the
//! page was made. A block whose first example line is `# not run` is
//! shown and left alone — for a server that would listen forever.

use std::collections::BTreeMap;
use std::path::Path;

use rill_syntax::ast::*;
use rill_syntax::fmt::type_ref;

/// The own-line comments of a file, by line, without their `#`.
fn comments_of(src: &str) -> BTreeMap<usize, String> {
    let mut out = BTreeMap::new();
    if let Ok((_, comments)) = rill_syntax::lexer::lex_with_comments(src) {
        for c in comments {
            if c.own_line {
                let text = c.text.trim_start_matches('#');
                out.insert(c.line, text.strip_prefix(' ').unwrap_or(text).to_string());
            }
        }
    }
    out
}

/// The comment block ending on the line before `line` — unless it is the
/// file's own head, which belongs to the file and not to whatever comes
/// first in it.
fn doc_above(comments: &BTreeMap<usize, String>, line: usize, head_end: usize) -> String {
    let mut lines = Vec::new();
    let mut l = line;
    while l > 1 && comments.contains_key(&(l - 1)) {
        l -= 1;
        lines.push(comments[&l].clone());
    }
    if l <= head_end {
        return String::new();
    }
    lines.reverse();
    // blank lines at the ends go; the first line's own indent stays, since
    // an example line is one that starts with four spaces
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    lines.join("\n").trim_end().to_string()
}

/// The comment block after `line` — a blank line between allowed — up to
/// a gap, and the line it ends on.
fn doc_below(comments: &BTreeMap<usize, String>, line: usize) -> (String, usize) {
    let mut lines = Vec::new();
    let mut l = line + 1;
    if !comments.contains_key(&l) && comments.contains_key(&(l + 1)) {
        l += 1;
    }
    let mut end = line;
    while let Some(t) = comments.get(&l) {
        lines.push(t.clone());
        end = l;
        l += 1;
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    (lines.join("\n").trim_end().to_string(), end)
}

/// A doc block as Markdown: the prose as written, the example lines
/// fenced as code.
fn render_doc(doc: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in doc.lines() {
        let example = line.starts_with("    ");
        if example && !in_code {
            out.push_str("```\n");
            in_code = true;
        } else if !example && in_code && !line.trim().is_empty() {
            out.push_str("```\n");
            in_code = false;
        }
        if in_code {
            if !line.trim().is_empty() {
                out.push_str(&line[4..]);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_code {
        out.push_str("```\n");
    }
    out.trim_end().to_string()
}

/// One example line from a doc block: the code, and what `show` of its
/// value should print if the line says (`# => …`).
#[derive(Debug, Clone)]
pub struct Example {
    pub line: usize,
    pub code: String,
    pub expect: Option<String>,
}

/// The example blocks of every doc comment in a file, each block the
/// consecutive example lines of one comment — a block is one program's
/// worth, its bindings its own — with `not run` blocks left out.
pub fn examples_of(src: &str) -> Vec<Vec<Example>> {
    let comments = comments_of(src);
    let mut blocks: Vec<Vec<Example>> = Vec::new();
    let mut current: Vec<Example> = Vec::new();
    let mut skip = false;
    let mut last_line = 0;
    for (&line, text) in &comments {
        let contiguous = line == last_line + 1;
        last_line = line;
        let is_example = text.starts_with("    ");
        if !contiguous || !is_example {
            if !current.is_empty() && !skip {
                blocks.push(std::mem::take(&mut current));
            }
            current.clear();
            skip = false;
            if !is_example {
                continue;
            }
        }
        let code = text[4..].to_string();
        if current.is_empty() && code.trim() == "# not run" {
            skip = true;
            continue;
        }
        if code.trim().is_empty() || code.trim_start().starts_with('#') {
            continue;
        }
        let (code, expect) = match code.find("# =>") {
            Some(i) => (code[..i].trim_end().to_string(), Some(code[i + 4..].trim().to_string())),
            None => (code.trim_end().to_string(), None),
        };
        current.push(Example { line, code, expect });
    }
    if !current.is_empty() && !skip {
        blocks.push(current);
    }
    blocks
}

fn ctor_text(c: &CtorDef) -> String {
    if c.fields.is_empty() {
        return c.name.clone();
    }
    let fields: Vec<String> = c
        .fields
        .iter()
        .map(|(n, t)| match n {
            Some(n) => format!("{n}: {}", type_ref(t)),
            None => type_ref(t),
        })
        .collect();
    format!("{}({})", c.name, fields.join(", "))
}

fn method_text(m: &MethodSig) -> String {
    let params: Vec<String> = m
        .params
        .iter()
        .zip(m.names.iter().chain(std::iter::repeat(&None)))
        .map(|(t, n)| match n {
            Some(n) => format!("{n}: {}", type_ref(t)),
            None => type_ref(t),
        })
        .collect();
    format!("{}({}) -> {}", m.name, params.join(", "), type_ref(&m.ret))
}

/// The written signature of a function, for when the checker had none.
fn written_signature(f: &FnDef) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.ann {
            Some(t) => format!("{}: {}", p.name, type_ref(t)),
            None => p.name.clone(),
        })
        .collect();
    let ret = f.ret_ann.as_ref().map(|t| format!(" -> {}", type_ref(t))).unwrap_or_default();
    format!("fn {}({}){ret}", f.name, params.join(", "))
}

/// The Markdown for one file.
pub fn markdown(path: &Path, src: &str, own: &Module, checked: Option<&rill_types::TModule>) -> String {
    let comments = comments_of(src);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut out = String::new();
    let name = own.name.clone().unwrap_or(stem);
    out.push_str(&format!("# `{name}`\n\n"));
    let (head, head_end) = if own.name.is_some() { doc_below(&comments, own.name_line) } else { doc_below(&comments, 0) };
    // in a module, a block with a declaration right under it is that
    // declaration's, not the file's; a program's head is its first comment
    let lines: Vec<&str> = src.lines().collect();
    let (head, head_end) = match lines.get(head_end) {
        Some(next) if own.name.is_some() && !next.trim().is_empty() && !next.trim_start().starts_with('#') => (String::new(), own.name_line),
        _ => (head, head_end),
    };
    if !head.is_empty() {
        out.push_str(&render_doc(&head));
        out.push_str("\n\n");
    }
    let section = |out: &mut String, title: &str| out.push_str(&format!("## {title}\n\n"));

    if !own.types.is_empty() {
        section(&mut out, "Types");
        for t in &own.types {
            let params = if t.params.is_empty() { String::new() } else { format!("({})", t.params.join(", ")) };
            out.push_str(&format!("### `{}{params}`\n\n", t.name));
            let doc = render_doc(&doc_above(&comments, t.line, head_end));
            if !doc.is_empty() {
                out.push_str(&doc);
                out.push_str("\n\n");
            }
            if let Some(a) = &t.alias {
                out.push_str(&format!("An alias for `{}`.\n\n", type_ref(a)));
            }
            for c in &t.ctors {
                out.push_str(&format!("- `{}`\n", ctor_text(c)));
            }
            if !t.ctors.is_empty() {
                out.push('\n');
            }
        }
    }

    if !own.traits.is_empty() {
        section(&mut out, "Traits");
        for tr in &own.traits {
            let supers = if tr.supers.is_empty() { String::new() } else { format!(": {}", tr.supers.join(", ")) };
            out.push_str(&format!("### `trait {}({}){supers}`\n\n", tr.name, tr.param));
            let doc = render_doc(&doc_above(&comments, tr.line, head_end));
            if !doc.is_empty() {
                out.push_str(&doc);
                out.push_str("\n\n");
            }
            for m in &tr.methods {
                let dflt = if m.default.is_some() { " — with a default" } else { "" };
                out.push_str(&format!("- `{}`{dflt}\n", method_text(m)));
            }
            out.push('\n');
        }
    }

    if !own.impls.is_empty() {
        section(&mut out, "Implementations");
        for im in &own.impls {
            out.push_str(&format!("- `impl {}({})`\n", im.trait_name, type_ref(&im.target)));
        }
        out.push('\n');
    }

    if !own.systems.is_empty() {
        section(&mut out, "Systems");
        for sd in &own.systems {
            out.push_str(&format!("### `system {}`\n\n", sd.name));
            let doc = render_doc(&doc_above(&comments, sd.line, head_end));
            if !doc.is_empty() {
                out.push_str(&doc);
                out.push_str("\n\n");
            }
            for r in &sd.routes {
                let arrow = if r.both { "<->" } else { "->" };
                out.push_str(&format!("- `{} {arrow} {}`: {}\n", r.from, r.to, r.signals.join(", ")));
            }
            out.push_str("\n```mermaid\n");
            out.push_str(&crate::diagram::system_mermaid(sd));
            out.push_str("```\n\n");
        }
    }

    if !own.machines.is_empty() {
        section(&mut out, "Machines");
        for m in &own.machines {
            let params: Vec<String> = m
                .params
                .iter()
                .map(|p| match &p.ann {
                    Some(t) => format!("{}: {}", p.name, type_ref(t)),
                    None => p.name.clone(),
                })
                .collect();
            out.push_str(&format!("### `machine {}({})`\n\n", m.name, params.join(", ")));
            let doc = render_doc(&doc_above(&comments, m.line, head_end));
            if !doc.is_empty() {
                out.push_str(&doc);
                out.push_str("\n\n");
            }
            out.push_str("States:\n\n");
            for st in &m.states {
                let inputs: Vec<String> = st
                    .arms
                    .iter()
                    .map(|a| rill_syntax::fmt::pattern_text(&a.pattern))
                    .chain(st.after.iter().map(|_| "after".to_string()))
                    .chain(st.whens.iter().map(|_| "when".to_string()))
                    .collect();
                let shown = if matches!(st.pattern, Pattern::Wildcard) { "*".to_string() } else { rill_syntax::fmt::pattern_text(&st.pattern) };
                out.push_str(&format!("- `{shown}` answers {}\n", if inputs.is_empty() { "nothing".to_string() } else { inputs.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ") }));
            }
            out.push_str("\n```mermaid\n");
            out.push_str(&crate::diagram::mermaid(m));
            out.push_str("```\n\n");
        }
    }

    let machine_fns: Vec<String> = own.machines.iter().flat_map(|m| vec![m.name.clone(), format!("{}.run", m.name)]).collect();
    let fns: Vec<&FnDef> = own
        .functions
        .iter()
        .filter(|f| !f.private && !f.name.contains('.') && !f.name.starts_with("test_") && f.name != "main" && !machine_fns.contains(&f.name))
        .collect();
    if !fns.is_empty() {
        section(&mut out, "Functions");
        for f in fns {
            let sig = checked
                .and_then(|t| t.fn_index.get(&f.name).map(|&i| rill_types::show_signature(&t.fns[i], &t.adts, &t.traits)))
                .unwrap_or_else(|| written_signature(f));
            out.push_str(&format!("### `{sig}`\n\n"));
            let doc = render_doc(&doc_above(&comments, f.line, head_end));
            if !doc.is_empty() {
                out.push_str(&doc);
                out.push_str("\n\n");
            }
        }
    }
    let tests: Vec<&FnDef> = own.functions.iter().filter(|f| f.name.starts_with("test_")).collect();
    if !tests.is_empty() {
        section(&mut out, "Tests");
        for f in tests {
            let doc = doc_above(&comments, f.line, head_end);
            out.push_str(&format!("- `{}`{}\n", f.name, if doc.is_empty() { String::new() } else { format!(" — {}", doc.replace('\n', " ")) }));
        }
        out.push('\n');
    }
    out
}

//! `rill lsp` — the compiler, kept open, answering an editor.
//!
//! A language where annotations are optional is a language whose types live
//! somewhere the program does not say them. `rill types` says them for a file;
//! this says them for the name under the cursor, and underlines what does not
//! check while it is being typed.
//!
//! What it answers is worked out from names rather than from positions: the
//! AST carries the line a thing is on and not the column, so a question about
//! (line, character) is turned into a question about the identifier written
//! there. That is exact for a top-level name and right for a local wherever
//! one name in a function means one thing, which is nearly always. The honest
//! limit is a name bound twice in one body: hover shows the first.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use rill_types::{Ty, TK};

use crate::json::{parse as parse_json, Json};

pub fn serve() -> Result<ExitCode, String> {
    let mut server = Server { docs: HashMap::new(), shutting_down: false };
    let mut stdin = std::io::stdin().lock();
    loop {
        let Some(msg) = read_message(&mut stdin)? else {
            return Ok(ExitCode::SUCCESS);
        };
        let Some(msg) = parse_json(&msg) else {
            continue;
        };
        if server.handle(&msg)? {
            return Ok(ExitCode::SUCCESS);
        }
    }
}

/// One `Content-Length` framed message, or `None` at end of input.
fn read_message(input: &mut impl Read) -> Result<Option<String>, String> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        loop {
            let mut b = [0u8; 1];
            match input.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                Err(e) => return Err(format!("lsp: cannot read: {e}")),
            }
            if b[0] == b'\n' {
                break;
            }
            if b[0] != b'\r' {
                line.push(b[0]);
            }
        }
        let line = String::from_utf8_lossy(&line).to_string();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            len = rest.trim().parse().ok();
        }
    }
    let len = len.ok_or("lsp: a message arrived without a Content-Length")?;
    let mut body = vec![0u8; len];
    input.read_exact(&mut body).map_err(|e| format!("lsp: short message: {e}"))?;
    Ok(Some(String::from_utf8_lossy(&body).to_string()))
}

fn send(value: Json) {
    let body = value.render();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn respond(id: &Json, result: Json) {
    send(Json::obj(vec![("jsonrpc", Json::str("2.0")), ("id", id.clone()), ("result", result)]));
}

struct Server {
    docs: HashMap<String, String>,
    shutting_down: bool,
}

impl Server {
    /// Answers one message; `true` means the editor said to stop.
    fn handle(&mut self, msg: &Json) -> Result<bool, String> {
        let method = msg.get("method").as_str().unwrap_or("").to_string();
        let id = msg.get("id").clone();
        let params = msg.get("params");
        // Between `shutdown` and `exit` the protocol says to do nothing but
        // answer, so that an editor closing down is not kept waiting.
        if self.shutting_down && method != "exit" {
            if !id.is_null() {
                respond(&id, Json::Null);
            }
            return Ok(false);
        }
        match method.as_str() {
            "initialize" => {
                respond(
                    &id,
                    Json::obj(vec![
                        (
                            "capabilities",
                            Json::obj(vec![
                                // 1 = the whole document arrives on every
                                // change, which for files this size is less
                                // work than stitching ranges together.
                                ("textDocumentSync", Json::num(1)),
                                ("hoverProvider", Json::Bool(true)),
                                ("definitionProvider", Json::Bool(true)),
                                ("documentSymbolProvider", Json::Bool(true)),
                                ("documentFormattingProvider", Json::Bool(true)),
                            ]),
                        ),
                        (
                            "serverInfo",
                            Json::obj(vec![
                                ("name", Json::str("rill")),
                                ("version", Json::str(env!("CARGO_PKG_VERSION"))),
                            ]),
                        ),
                    ]),
                );
            }
            "initialized" => {}
            "shutdown" => {
                self.shutting_down = true;
                respond(&id, Json::Null);
            }
            "exit" => return Ok(true),
            "textDocument/didOpen" => {
                let doc = params.get("textDocument");
                if let (Some(uri), Some(text)) =
                    (doc.get("uri").as_str(), doc.get("text").as_str())
                {
                    self.docs.insert(uri.to_string(), text.to_string());
                    self.publish(uri);
                }
            }
            "textDocument/didChange" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("").to_string();
                if let Some(text) = params.get("contentChanges").at(0).get("text").as_str() {
                    self.docs.insert(uri.clone(), text.to_string());
                    self.publish(&uri);
                }
            }
            "textDocument/didSave" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("").to_string();
                if !uri.is_empty() {
                    self.publish(&uri);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.get("textDocument").get("uri").as_str() {
                    self.docs.remove(uri);
                }
            }
            "textDocument/hover" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("");
                let line = params.get("position").get("line").as_usize().unwrap_or(0);
                let ch = params.get("position").get("character").as_usize().unwrap_or(0);
                respond(&id, self.hover(uri, line, ch));
            }
            "textDocument/definition" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("");
                let line = params.get("position").get("line").as_usize().unwrap_or(0);
                let ch = params.get("position").get("character").as_usize().unwrap_or(0);
                respond(&id, self.definition(uri, line, ch));
            }
            "textDocument/documentSymbol" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("");
                respond(&id, self.symbols(uri));
            }
            "textDocument/formatting" => {
                let uri = params.get("textDocument").get("uri").as_str().unwrap_or("");
                respond(&id, self.format(uri));
            }
            _ => {
                // A request nobody answers still needs an answer, or the
                // editor waits for one for ever.
                if !id.is_null() {
                    respond(&id, Json::Null);
                }
            }
        }
        Ok(false)
    }

    fn text(&self, uri: &str) -> Option<&String> {
        self.docs.get(uri)
    }

    fn publish(&mut self, uri: &str) {
        let Some(text) = self.text(uri) else { return };
        let a = analyze(uri, text);
        let items: Vec<Json> = a
            .diagnostics
            .iter()
            .map(|(line, col, msg)| {
                Json::obj(vec![
                    ("range", span(*line, *col, text)),
                    ("severity", Json::num(1)),
                    ("source", Json::str("rill")),
                    ("message", Json::str(msg.clone())),
                ])
            })
            .collect();
        send(Json::obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("method", Json::str("textDocument/publishDiagnostics")),
            (
                "params",
                Json::obj(vec![("uri", Json::str(uri)), ("diagnostics", Json::Arr(items))]),
            ),
        ]));
    }

    fn hover(&self, uri: &str, line: usize, ch: usize) -> Json {
        let Some(text) = self.text(uri) else { return Json::Null };
        let Some(word) = word_at(text, line, ch) else { return Json::Null };
        let a = analyze(uri, text);
        let Some(found) = a.lookup(&word) else { return Json::Null };
        Json::obj(vec![(
            "contents",
            Json::obj(vec![
                ("kind", Json::str("markdown")),
                ("value", Json::str(format!("```rill\n{found}\n```"))),
            ]),
        )])
    }

    fn definition(&self, uri: &str, line: usize, ch: usize) -> Json {
        let Some(text) = self.text(uri) else { return Json::Null };
        let Some(word) = word_at(text, line, ch) else { return Json::Null };
        let a = analyze(uri, text);
        // The last segment as well, so `geo.area` finds `area` where the
        // module it came from is the file being edited.
        let at = a
            .defs
            .get(&word)
            .or_else(|| word.rsplit('.').next().and_then(|w| a.defs.get(w)))
            .copied();
        match at {
            Some(l) => Json::obj(vec![
                ("uri", Json::str(uri)),
                ("range", whole_line(l, text)),
            ]),
            None => Json::Null,
        }
    }

    fn symbols(&self, uri: &str) -> Json {
        let Some(text) = self.text(uri) else { return Json::Arr(Vec::new()) };
        let a = analyze(uri, text);
        Json::Arr(
            a.symbols
                .iter()
                .map(|(name, line, kind)| {
                    Json::obj(vec![
                        ("name", Json::str(name.clone())),
                        ("kind", Json::num(*kind)),
                        ("range", whole_line(*line, text)),
                        ("selectionRange", whole_line(*line, text)),
                    ])
                })
                .collect(),
        )
    }

    fn format(&self, uri: &str) -> Json {
        let Some(text) = self.text(uri) else { return Json::Null };
        let Ok(formatted) = rill_syntax::format_source(text) else {
            // Nothing to do with a file that does not parse; the diagnostic
            // already says why.
            return Json::Arr(Vec::new());
        };
        let last = text.lines().count();
        Json::Arr(vec![Json::obj(vec![
            (
                "range",
                Json::obj(vec![
                    ("start", position(0, 0)),
                    ("end", position(last + 1, 0)),
                ]),
            ),
            ("newText", Json::str(formatted)),
        ])])
    }
}

fn position(line: usize, ch: usize) -> Json {
    Json::obj(vec![("line", Json::num(line)), ("character", Json::num(ch))])
}

/// The compiler counts lines from one and the protocol from zero. A line of
/// zero means the message is about the file rather than a line in it.
fn whole_line(line: usize, text: &str) -> Json {
    span(line, None, text)
}

/// A line, or the rest of a line from the column something went wrong at.
/// The type checker knows the line; the parser knows both.
fn span(line: usize, col: Option<usize>, text: &str) -> Json {
    let l = line.saturating_sub(1);
    let len = text.lines().nth(l).map(|s| s.chars().count()).unwrap_or(0);
    let from = col.map(|c| c.saturating_sub(1)).unwrap_or(0).min(len);
    Json::obj(vec![("start", position(l, from)), ("end", position(l, len))])
}

/// The identifier written at (line, character), dots and all — `geo.area` and
/// `p.x` are one name to the lexer, and the lookup below tries both halves.
fn word_at(text: &str, line: usize, ch: usize) -> Option<String> {
    let src = text.lines().nth(line)?;
    let chars: Vec<char> = src.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    if chars.is_empty() {
        return None;
    }
    let mut i = ch.min(chars.len().saturating_sub(1));
    if !is_word(chars[i]) {
        i = i.checked_sub(1)?;
        if !is_word(chars[i]) {
            return None;
        }
    }
    let mut start = i;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    let word: String = chars[start..=end].iter().collect();
    let word = word.trim_matches('.').to_string();
    if word.is_empty() || word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(word)
}

fn uri_to_path(uri: &str) -> PathBuf {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    // Percent-encoding is the only escaping an editor puts in a file URI.
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&path[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).to_string())
}

#[derive(Default)]
struct Analysis {
    /// Line, the column when there is one, and what is wrong. The editor
    /// draws the position; the message it shows is the message, since it
    /// knows where it put it.
    diagnostics: Vec<(usize, Option<usize>, String)>,
    /// Everything a name can be, in the order a lookup tries them.
    sigs: HashMap<String, String>,
    locals: HashMap<String, String>,
    defs: HashMap<String, usize>,
    symbols: Vec<(String, usize, usize)>,
}

impl Analysis {
    fn lookup(&self, word: &str) -> Option<String> {
        let tail = word.rsplit('.').next().unwrap_or(word);
        for key in [word, tail] {
            if let Some(s) = self.sigs.get(key) {
                return Some(s.clone());
            }
            if let Some(t) = self.locals.get(key) {
                return Some(format!("{key}: {t}"));
            }
        }
        None
    }
}

/// Everything the editor can be told about one file, worked out from scratch.
/// A file this size type-checks in single-digit milliseconds, so there is no
/// cache here to be wrong.
fn analyze(uri: &str, text: &str) -> Analysis {
    let mut a = Analysis::default();
    let path = uri_to_path(uri);

    // The file's own declarations: what the outline shows, what "go to
    // definition" can reach, and which of the program's functions belong to
    // this file rather than to the prelude or an import.
    let own = match rill_syntax::parse(text) {
        Ok(m) => m,
        Err(e) => {
            a.diagnostics.push((e.line, Some(e.col), e.msg));
            return a;
        }
    };
    let mut mine: Vec<String> = Vec::new();
    for f in &own.functions {
        mine.push(f.name.clone());
        a.defs.insert(f.name.clone(), f.line);
        a.symbols.push((f.name.clone(), f.line, 12)); // Function
    }
    for t in &own.types {
        a.defs.insert(t.name.clone(), t.line);
        a.symbols.push((t.name.clone(), t.line, 10)); // Enum
        for c in &t.ctors {
            a.defs.insert(c.name.clone(), c.line);
        }
    }
    for t in &own.traits {
        a.defs.insert(t.name.clone(), t.line);
        a.symbols.push((t.name.clone(), t.line, 11)); // Interface
    }

    let module = match rill_syntax::load_program_text(&path, text) {
        Ok(m) => m,
        Err(e) => {
            // An import that does not resolve or does not parse. The file
            // being edited is fine — it parsed above — so this belongs at the
            // top of it rather than on any line in particular.
            a.diagnostics.push((0, None, e));
            return a;
        }
    };

    let tmod = match rill_types::check_all(&module) {
        Ok(t) => t,
        Err(errors) => {
            for e in errors {
                // A file that is a library and not a program has no `main`,
                // and being told so on every keystroke helps nobody.
                if e.msg.contains("no `main` function defined") {
                    continue;
                }
                a.diagnostics.push((e.line, None, e.msg));
            }
            return a;
        }
    };

    for (i, f) in tmod.fns.iter().enumerate() {
        let _ = i;
        a.sigs.insert(f.name.clone(), rill_types::show_signature(f, &tmod.adts));
    }
    for (name, &(adt, ci)) in &tmod.ctors {
        let sig = &tmod.adts[adt].ctors[ci];
        let fields: Vec<String> =
            sig.fields.iter().map(|t| rill_types::show_type(t, &tmod.adts)).collect();
        let targs: Vec<Ty> =
            (0..tmod.adts[adt].nparams).map(|g| Ty::Gen(g as u32)).collect();
        let owner = rill_types::show_type(&Ty::Adt(adt, targs), &tmod.adts);
        a.sigs.insert(
            name.clone(),
            if fields.is_empty() {
                format!("{name} -> {owner}")
            } else {
                format!("{name}({}) -> {owner}", fields.join(", "))
            },
        );
    }
    // The names inside this file's own functions, with what they turned out
    // to be. A parameter is one of these too, which is the answer to the
    // question an unannotated signature raises.
    for name in &mine {
        if let Some(&i) = tmod.fn_index.get(name) {
            let f = &tmod.fns[i];
            for (p, ty) in &f.params {
                a.locals
                    .entry(p.clone())
                    .or_insert_with(|| rill_types::show_type(ty, &tmod.adts));
            }
            collect_locals(&f.body, &tmod, &mut a.locals);
        }
    }
    a
}

/// Every name a body binds, with its type. First one wins: a name bound twice
/// in one function is the honest limit of answering by name.
fn collect_locals(
    e: &rill_types::TExpr,
    tmod: &rill_types::TModule,
    out: &mut HashMap<String, String>,
) {
    if let TK::Block(stmts) = &e.kind {
        for s in stmts {
            if let rill_types::TStmt::Bind { name, value } = s {
                out.entry(name.clone())
                    .or_insert_with(|| rill_types::show_type(&value.ty, &tmod.adts));
            }
        }
    }
    if let TK::Local(name) = &e.kind {
        out.entry(name.clone()).or_insert_with(|| rill_types::show_type(&e.ty, &tmod.adts));
    }
    if let TK::Lambda { params, .. } = &e.kind {
        for (p, ty) in params {
            out.entry(p.clone()).or_insert_with(|| rill_types::show_type(ty, &tmod.adts));
        }
    }
    rill_types::walk_texpr(e, &mut |c| collect_locals(c, tmod, out));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_word_under_the_cursor() {
        let text = "fn area(r) = 3.14 * r * r\n  x = geo.area(2.0)\n";
        assert_eq!(word_at(text, 0, 3).as_deref(), Some("area"));
        assert_eq!(word_at(text, 0, 8).as_deref(), Some("r"));
        // A dotted name comes back whole; the lookup tries both halves.
        assert_eq!(word_at(text, 1, 8).as_deref(), Some("geo.area"));
        // On a space between names, the one just before it.
        assert_eq!(word_at(text, 0, 2).as_deref(), Some("fn"));
        assert_eq!(word_at(text, 0, 13).as_deref(), None);
    }

    #[test]
    fn turns_a_file_uri_into_a_path() {
        assert_eq!(uri_to_path("file:///tmp/a%20b/x.rill"), PathBuf::from("/tmp/a b/x.rill"));
        assert_eq!(uri_to_path("/plain/path.rill"), PathBuf::from("/plain/path.rill"));
    }
}

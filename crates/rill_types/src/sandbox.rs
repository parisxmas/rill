//! What a program may reach of the machine, checked before it is built.
//!
//! A program that will run for someone else — a model uploaded to an
//! editor open to the world, a submission, a plugin — should not get to
//! read the files of the machine it runs on, open its sockets, call into
//! its C libraries, or write to whatever address it likes. Those are the
//! four things a Rill program can do that the language cannot account
//! for, and each is a named capability here: `files`, `net`, `ffi`, `os`.
//! A sandbox says which are allowed, and everything else the runtime
//! offers — the console, the clocks, strands and channels, the random
//! numbers, sleeping — stays open, since none of it reaches past the
//! process.
//!
//! The question is answered over the checked program, before code is
//! made: every function reachable from an entry — `main`, the tests, an
//! `export` — is searched for a runtime call or a foreign call that needs
//! a capability the sandbox withholds. A library function that would
//! need one is fine to import and fine to leave uncalled; it is the call
//! that is refused, and the error names the author's own function the
//! call was reached from, since a line in a library is not one the author
//! can go and fix.

use std::collections::{HashMap, VecDeque};

use crate::{TExpr, TK, TModule, TypeError, walk_texpr};

/// What a sandboxed program is allowed to reach.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sandbox {
    /// Files and directories: reading, writing, listing, renaming.
    pub files: bool,
    /// Sockets: listening, connecting, reading and writing them.
    pub net: bool,
    /// Foreign code: `extern fn`, `asm fn`, and raw memory through `Ptr`.
    pub ffi: bool,
    /// The process's standing in the system: waiting on its signals.
    pub os: bool,
}

impl Sandbox {
    /// The sandbox that allows none of the four.
    pub fn none() -> Sandbox {
        Sandbox::default()
    }

    /// `"files,net"`: the capabilities named, comma-separated; `""` for
    /// none of them.
    pub fn parse(spec: &str) -> Result<Sandbox, String> {
        let mut sb = Sandbox::none();
        for cap in spec.split(',').map(str::trim).filter(|c| !c.is_empty()) {
            match cap {
                "files" => sb.files = true,
                "net" => sb.net = true,
                "ffi" => sb.ffi = true,
                "os" => sb.os = true,
                other => {
                    return Err(format!(
                        "unknown capability `{other}` — a sandbox allows some of `files`, `net`, `ffi`, `os`"
                    ))
                }
            }
        }
        Ok(sb)
    }

    fn allows(&self, cap: &str) -> bool {
        match cap {
            "files" => self.files,
            "net" => self.net,
            "ffi" => self.ffi,
            "os" => self.os,
            _ => true,
        }
    }

    /// The capabilities allowed, as `parse` would read them back.
    pub fn describe(&self) -> String {
        let mut caps = Vec::new();
        if self.files {
            caps.push("files");
        }
        if self.net {
            caps.push("net");
        }
        if self.ffi {
            caps.push("ffi");
        }
        if self.os {
            caps.push("os");
        }
        if caps.is_empty() { "nothing beyond the process".to_string() } else { caps.join(", ") }
    }
}

/// The capability a runtime function needs, by its symbol, or `None` for
/// one that stays inside the process.
pub fn capability_of(rt: &str) -> Option<&'static str> {
    let name = rt.strip_prefix("rill_").unwrap_or(rt);
    if name.starts_with("file_")
        || name.starts_with("dir_")
        || name == "read_file"
        || name == "write_file"
    {
        return Some("files");
    }
    if name.starts_with("tcp_") || name.starts_with("sock_") {
        return Some("net");
    }
    if name.starts_with("peek_")
        || name.starts_with("poke_")
        || matches!(
            name,
            "to_cstr"
                | "from_cstr"
                | "str_from_ptr"
                | "free_ptr"
                | "alloc_bytes"
                | "ptr_offset"
                // `buf_get_unchecked`, `buf_set_unchecked`: no bounds test, as
                // able to reach past the buffer as a pointer is
                | "unchecked"
        )
    {
        return Some("ffi");
    }
    if name == "wait_signal" {
        return Some("os");
    }
    None
}

/// Where a run starts, and so where the search does: `main` and the
/// exports for a program, the `test_` functions for `rill test`, all of
/// them for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entries {
    Program,
    Tests,
    All,
}

impl Entries {
    fn is_entry(self, name: &str, exported: bool) -> bool {
        let program = name == "main" || exported;
        let test = name.starts_with("test_");
        match self {
            Entries::Program => program,
            Entries::Tests => test,
            Entries::All => program || test,
        }
    }
}

/// A sandboxed program checked: every runtime or foreign call reachable
/// from an entry that needs a capability the sandbox withholds is an
/// error, one per call site, naming the author's function it is reached
/// from when the call is in a library or the prelude.
pub fn check_sandbox(tmod: &TModule, sb: &Sandbox, entries: Entries) -> Result<(), Vec<TypeError>> {
    let n = tmod.fns.len();
    // The author's functions: after the prelude, and not from an import.
    let own = |i: usize| i >= tmod.prelude_fns && !tmod.imported_fns.contains(&tmod.fns[i].name);
    // Where each function is called from, found by walking out from the
    // entries: the first caller recorded is the one the report follows back.
    let mut from: Vec<Option<(usize, usize)>> = vec![None; n];   // (caller, line of the call)
    let mut seen = vec![false; n];
    let mut queue = VecDeque::new();
    for (i, f) in tmod.fns.iter().enumerate() {
        if entries.is_entry(&f.name, f.exported) && own(i) {
            seen[i] = true;
            queue.push_back(i);
        }
    }
    // Every implementation a method call could pick: the receiver type
    // is settled later, so all of them are reached.
    let mut impls: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (&(t, m, _), &f) in &tmod.impl_methods {
        impls.entry((t, m)).or_default().push(f);
    }
    let mut errors = Vec::new();
    while let Some(i) = queue.pop_front() {
        let mut calls: Vec<(usize, usize)> = Vec::new();   // (callee, line)
        let mut needs: Vec<(String, &'static str, usize)> = Vec::new();   // (what, capability, line)
        each_expr(&tmod.fns[i].body, &mut |e| match &e.kind {
            TK::CallFn { fn_idx, .. } | TK::FnRef { fn_idx, .. } => calls.push((*fn_idx, e.line)),
            TK::MethodCall { trait_id, method, .. } => {
                for &f in impls.get(&(*trait_id, *method)).map(|v| v.as_slice()).unwrap_or(&[]) {
                    calls.push((f, e.line));
                }
            }
            TK::Builtin { rt, .. } => {
                if let Some(cap) = capability_of(rt) {
                    needs.push((format!("`{}`", rt.strip_prefix("rill_").unwrap_or(rt)), cap, e.line));
                }
            }
            TK::CallExtern { name, .. } => {
                let what = if tmod.asms.contains_key(name) || tmod.naked_asms.contains_key(name) {
                    format!("`asm fn {name}`")
                } else {
                    format!("`extern fn {name}`")
                };
                needs.push((what, "ffi", e.line));
            }
            _ => {}
        });
        for (what, cap, line) in needs {
            if sb.allows(cap) {
                continue;
            }
            // The report lands on the author's code: the call itself when
            // it is theirs, else the call into the chain that reaches it.
            let (at, through) = if own(i) {
                (line, String::new())
            } else {
                let mut j = i;
                let mut chain = vec![tmod.fns[j].name.clone()];
                let mut at = tmod.fns[i].line;
                while let Some((caller, call_line)) = from[j] {
                    at = call_line;
                    j = caller;
                    if own(j) {
                        break;
                    }
                    chain.push(tmod.fns[j].name.clone());
                }
                chain.reverse();
                (at, format!(", reached through `{}`", chain.join("` > `")))
            };
            errors.push(TypeError::new(
                at,
                format!(
                    "{what} is not allowed here: it needs `{cap}`, and this sandbox allows {}{through}",
                    sb.describe()
                ),
            ));
        }
        for (callee, line) in calls {
            if callee < n && !seen[callee] {
                seen[callee] = true;
                from[callee] = Some((i, line));
                queue.push_back(callee);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        // One report a line: the first thing found there says enough.
        errors.sort_by_key(|e| e.line);
        errors.dedup_by(|a, b| a.line == b.line);
        Err(errors)
    }
}

/// `f` on an expression and everything under it.
fn each_expr<'a>(e: &'a TExpr, f: &mut impl FnMut(&'a TExpr)) {
    f(e);
    walk_texpr(e, &mut |sub| each_expr(sub, f));
}

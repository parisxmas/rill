//! `rill` — the Rill compiler CLI.
//!
//! Usage:
//!   rill build <file.rill> [-o <out>] [--emit-ir]
//!   rill run <file.rill>

mod json;
mod lsp;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:\n  rill build <file.rill> [-o <out>] [--debug] [--parallel] [--unchecked] [--shared] [--emit-ir] [--target <triple>]\n  rill run <file.rill> [--debug] [--parallel] [--unchecked]\n  rill fmt <file.rill>... [--check | --stdout]\n  rill check <file.rill>\n  rill types <file.rill>\n  rill lsp\n  rill repl\n\n  --debug   no optimization passes, DWARF line info and symbols (for lldb)\n  --parallel  allow RILL_THREADS>1 (reference counts become atomic)\n  --unchecked  no bounds test on buffer accesses (run it checked first)\n  --shared  a shared library (.dylib/.so) whose `export fn`s a C or Python\n            program calls; `main` runs once as `rill_init`\n  --target  compile for another machine; arm64, x86-64 and wasm32 triples\n            only. Windows and wasm targets are linked, anything else stops\n            at the object file\n  --check   exit 1 if any file is not already formatted\n  --stdout  print the formatted source instead of rewriting the file";

fn run(args: &[String]) -> Result<ExitCode, String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    match cmd.as_str() {
        "build" => {
            let opts = BuildOpts::parse(rest)?;
            build(&opts)?;
            Ok(ExitCode::SUCCESS)
        }
        "run" => {
            let opts = BuildOpts::parse(rest)?;
            let exe = build(&opts)?;
            let status = Command::new(&exe)
                .status()
                .map_err(|e| format!("failed to run `{}`: {e}", exe.display()))?;
            Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
        }
        "check" => {
            let src = rest.first().ok_or(USAGE)?;
            let path = PathBuf::from(src);
            let module = rill_syntax::load_program(&path)?;
            // Everything one pass can find, rather than the first thing: a
            // file with three mistakes in it should take one run to learn
            // about three mistakes.
            match rill_types::check_all(&module) {
                Ok(_) => Ok(ExitCode::SUCCESS),
                Err(errors) => {
                    for e in &errors {
                        eprintln!("{}: {e}", path.display());
                    }
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        "types" => types_cmd(rest),
        "lsp" => lsp::serve(),
        "repl" => {
            repl()?;
            Ok(ExitCode::SUCCESS)
        }
        "fmt" => fmt_cmd(rest),
        _ => Err(USAGE.to_string()),
    }
}

// ---- types -----------------------------------------------------------------

/// Every signature the checker worked out for the file's own functions. A
/// language where annotations are optional is a language where the types are
/// somewhere the program does not say, and this is where they are said.
fn types_cmd(args: &[String]) -> Result<ExitCode, String> {
    let src = args.first().ok_or(USAGE)?;
    let path = PathBuf::from(src);
    // The file's own names, so the prelude and the imports it pulled in do
    // not answer a question about this file.
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let own = rill_syntax::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mine: Vec<String> = own.functions.iter().map(|f| f.name.clone()).collect();

    let module = rill_syntax::load_program(&path)?;
    let tmod = match rill_types::check_all(&module) {
        Ok(t) => t,
        Err(errors) => {
            for e in &errors {
                eprintln!("{}: {e}", path.display());
            }
            return Ok(ExitCode::FAILURE);
        }
    };
    for name in mine {
        if let Some(&i) = tmod.fn_index.get(&name) {
            println!("{}", rill_types::show_signature(&tmod.fns[i], &tmod.adts));
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- fmt -------------------------------------------------------------------

fn fmt_cmd(args: &[String]) -> Result<ExitCode, String> {
    let mut check = false;
    let mut to_stdout = false;
    let mut files: Vec<PathBuf> = Vec::new();
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--stdout" => to_stdout = true,
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`\n{USAGE}")),
            _ => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() {
        return Err(format!("no source files given\n{USAGE}"));
    }

    let mut unformatted = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        let formatted = rill_syntax::format_source(&src)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if to_stdout {
            print!("{formatted}");
        } else if formatted != src {
            if check {
                unformatted.push(path.clone());
            } else {
                std::fs::write(path, &formatted)
                    .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
                println!("formatted {}", path.display());
            }
        }
    }
    if !unformatted.is_empty() {
        for p in &unformatted {
            eprintln!("not formatted: {}", p.display());
        }
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- repl ------------------------------------------------------------------

/// Compile-and-run REPL: definitions accumulate for the session, expressions
/// are wrapped into a `main`, compiled to a native binary and executed
/// immediately. Prompts go to stderr so stdout stays pipeable.
fn repl() -> Result<(), String> {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let tmp_src = PathBuf::from(".rill_repl.rill");
    let tmp_exe = std::env::temp_dir().join(format!("rill-repl-{}", std::process::id()));
    let mut defs = String::new();

    eprintln!("rill repl — definitions persist; expressions run immediately.");
    eprintln!("commands: :quit  :clear  :defs   (end multi-line definitions with a blank line)");

    let mut next_line = |prompt: &str| -> Option<String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        input.next().and_then(|l| l.ok())
    };

    loop {
        let Some(line) = next_line("rill> ") else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed {
            ":quit" | ":q" => break,
            ":clear" => {
                defs.clear();
                continue;
            }
            ":defs" => {
                eprintln!("{defs}");
                continue;
            }
            _ => {}
        }

        let is_def = ["fn ", "type ", "trait ", "impl ", "import "]
            .iter()
            .any(|k| trimmed.starts_with(k));

        if is_def {
            let mut block = format!("{line}\n");
            loop {
                let Some(cont) = next_line("....> ") else { break };
                if cont.trim().is_empty() {
                    break;
                }
                block.push_str(&cont);
                block.push('\n');
            }
            if trimmed.starts_with("fn main") {
                // A full main: run it directly, don't store it.
                let program = format!("{defs}{block}");
                match repl_compile(&program, &tmp_src, &tmp_exe) {
                    Ok(()) => repl_run(&tmp_exe),
                    Err(e) => eprintln!("{e}"),
                }
                continue;
            }
            // Validate against the session before committing.
            let probe = format!("{defs}{block}\nfn main() = 0\n");
            match repl_compile(&probe, &tmp_src, &tmp_exe) {
                Ok(()) => {
                    defs.push_str(&block);
                    defs.push('\n');
                }
                Err(e) => eprintln!("{e}"),
            }
            continue;
        }

        // Expression: print its value if printable, otherwise just run it.
        let printed = format!("{defs}fn main() =\n  println({trimmed})\n");
        let bare = format!("{defs}fn main() =\n  {trimmed}\n");
        let compiled = repl_compile(&printed, &tmp_src, &tmp_exe)
            .or_else(|_| repl_compile(&bare, &tmp_src, &tmp_exe));
        match compiled {
            Ok(()) => {
                repl_run(&tmp_exe);
                if looks_like_binding(trimmed) {
                    eprintln!("note: bindings don't persist between inputs — use `fn` definitions");
                }
            }
            Err(e) => eprintln!("{e}"),
        }
    }
    let _ = std::fs::remove_file(&tmp_src);
    let _ = std::fs::remove_file(&tmp_exe);
    Ok(())
}

fn looks_like_binding(s: &str) -> bool {
    if let Some((lhs, rest)) = s.split_once('=') {
        let lhs = lhs.trim();
        !rest.starts_with('=')
            && !lhs.is_empty()
            && lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    } else {
        false
    }
}

fn repl_compile(program: &str, tmp_src: &Path, tmp_exe: &Path) -> Result<(), String> {
    std::fs::write(tmp_src, program).map_err(|e| format!("cannot write repl scratch: {e}"))?;
    let result = build(&BuildOpts {
        target: None,
        source: tmp_src.to_path_buf(),
        output: tmp_exe.to_path_buf(),
        emit_ir: false,
        unchecked: false,
        shared: false,
        debug: true, // skip O3 for snappy iteration
        parallel: false,
    });
    let _ = std::fs::remove_file(tmp_src);
    // The scratch filename means nothing to the user; strip it from errors.
    result.map(|_| ()).map_err(|e| {
        e.replace(&format!("{}: ", tmp_src.display()), "")
            .replace(&tmp_src.display().to_string(), "input")
    })
}

fn repl_run(exe: &Path) {
    match Command::new(exe).status() {
        Ok(_) => {}
        Err(e) => eprintln!("failed to run: {e}"),
    }
}

struct BuildOpts {
    source: PathBuf,
    output: PathBuf,
    emit_ir: bool,
    debug: bool,
    parallel: bool,
    /// No bounds test on buffer accesses: the trade Zig's ReleaseFast and
    /// Swift's -Ounchecked make. An index out of range is then what it is in
    /// C, so this is for a program that has been run checked first.
    unchecked: bool,
    /// A shared library rather than a program: the `export fn`s are its
    /// entry points, `main` runs once as `rill_init`, and every counted type
    /// at the boundary gets a `rill_drop_<Type>` for the host to let go by.
    shared: bool,
    /// An LLVM triple to compile for instead of this machine. The object is
    /// written and the link is left alone: the linker and the runtime for
    /// that machine are not this machine's.
    target: Option<String>,
}

impl BuildOpts {
    fn parse(args: &[String]) -> Result<BuildOpts, String> {
        let mut source: Option<PathBuf> = None;
        let mut output: Option<PathBuf> = None;
        let mut emit_ir = false;
        let mut unchecked = false;
        let mut debug = false;
        let mut parallel = false;
        let mut shared = false;
        let mut target: Option<String> = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-o" => {
                    output = Some(PathBuf::from(
                        it.next().ok_or("`-o` requires an output path")?,
                    ));
                }
                "--emit-ir" => emit_ir = true,
                "--unchecked" => unchecked = true,
                "--debug" => debug = true,
                "--parallel" => parallel = true,
                "--shared" => shared = true,
                "--target" => {
                    target = Some(it.next().ok_or("`--target` requires an LLVM triple")?.clone());
                }
                _ if a.starts_with('-') => return Err(format!("unknown option `{a}`\n{USAGE}")),
                _ => {
                    if source.replace(PathBuf::from(a)).is_some() {
                        return Err(format!("more than one source file given\n{USAGE}"));
                    }
                }
            }
        }
        let source = source.ok_or_else(|| format!("no source file given\n{USAGE}"))?;
        let output = output.unwrap_or_else(|| {
            source
                .file_stem()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("a.out"))
        });
        let output = if shared && output.extension().is_none() {
            output.with_extension(if cfg!(target_os = "macos") { "dylib" } else { "so" })
        } else {
            output
        };
        Ok(BuildOpts { source, output, emit_ir, debug, parallel, unchecked, shared, target })
    }
}

fn build(opts: &BuildOpts) -> Result<PathBuf, String> {
    let module = rill_syntax::load_program(&opts.source)?;

    let tmod = rill_types::check(&module)
        .map_err(|e| format!("{}: {e}", opts.source.display()))?;

    // wasm has no instructions to spell out: a module is verified before it
    // runs and there is no place in it for a machine's own encoding. Say so
    // here rather than letting LLVM fail on the way to an object file.
    if let Some(t) = opts.target.as_deref() {
        if t.contains("wasm") {
            if let Some(name) = tmod.asms.keys().chain(tmod.naked_asms.keys()).min() {
                return Err(format!(
                    "{}: `asm fn {name}` cannot be built for {t} — wasm has no inline \
                     assembly. A module with an `export fn` in it may import the operation \
                     from its host instead.",
                    opts.source.display()
                ));
            }
        }
    }

    if opts.emit_ir {
        let ir = rill_codegen::compile_to_ir(&tmod, opts.parallel, opts.unchecked)
            .map_err(|e| format!("{}: {e}", opts.source.display()))?;
        println!("{ir}");
    }

    let obj_path = opts.output.with_extension("o");
    rill_codegen::compile_to_object(
        &tmod,
        &obj_path,
        opts.debug,
        opts.parallel,
        &opts.source,
        opts.target.as_deref(),
        opts.unchecked,
        opts.shared,
    )
    .map_err(|e| format!("{}: {e}", opts.source.display()))?;

    // Cross-compiling to Windows or to wasm: `lld` will do it from here, given
    // that machine's runtime. Everything else stops at the object — linking
    // needs the other machine's linker and its libraries, and pretending
    // otherwise produces something that runs neither here nor there.
    if let Some(triple) = opts.target.as_deref() {
        if triple.contains("windows") {
            return link_windows(opts, &obj_path, triple);
        }
        if triple.starts_with("wasm") {
            return link_wasm(opts, &obj_path, triple, &tmod);
        }
        println!("{} written for {triple}", obj_path.display());
        return Ok(obj_path);
    }

    let runtime = find_runtime()?;
    let mut link_cmd = Command::new("cc");
    link_cmd.arg(&obj_path).arg(&runtime).arg("-o").arg(&opts.output);
    // A library rather than a program: no entry point of its own, and the
    // exports stay visible for the host to find by name.
    if opts.shared {
        link_cmd.arg(if cfg!(target_os = "macos") { "-dynamiclib" } else { "-shared" });
    }
    // Libraries named by `extern "lib" fn ...`. Three forms: a bare name is a
    // library to search for, `framework:Name` is a macOS framework, and a
    // string starting with `-` is handed to the linker as written — which is
    // how a program says where its libraries live. Search paths go first, so
    // they apply no matter which declaration carried them.
    for lib in tmod.libs.iter().filter(|l| l.starts_with('-')) {
        link_cmd.arg(lib);
    }
    for lib in tmod.libs.iter().filter(|l| !l.starts_with('-')) {
        match lib.strip_prefix("framework:") {
            Some(f) => link_cmd.arg("-framework").arg(f),
            None => link_cmd.arg(format!("-l{lib}")),
        };
    }
    // Dropping what nothing reaches, under the name the platform's linker
    // knows it by. `-dead_strip` is Apple's; GNU ld does not have it and says
    // so by failing the link rather than ignoring the flag.
    if !opts.debug && !opts.shared {
        link_cmd.arg(if cfg!(target_os = "macos") { "-Wl,-dead_strip" } else { "-Wl,--gc-sections" });
    }
    let link = link_cmd
        .output()
        .map_err(|e| format!("failed to invoke the system linker (`cc`): {e}"))?;
    if !opts.debug {
        let _ = std::fs::remove_file(&obj_path);
    }
    if !link.status.success() {
        return Err(format!(
            "linking failed:\n{}",
            String::from_utf8_lossy(&link.stderr)
        ));
    }
    if opts.debug {
        // On macOS the linker leaves DWARF in the object file and records a
        // debug map, so the object has to stay put for lldb to find lines.
        // `dsymutil` folds it into a .dSYM bundle when it is available.
        let _ = Command::new("dsymutil").arg(&opts.output).output();
    } else if !opts.shared {
        // Strip symbols for C-like binary sizes; best-effort.
        let _ = Command::new("strip").arg(&opts.output).output();
    }

    // `rill run foo.rill` on a bare output name needs `./foo`, not `foo`.
    if opts.output.components().count() == 1 {
        Ok(Path::new(".").join(&opts.output))
    } else {
        Ok(opts.output.clone())
    }
}

/// Links a Windows executable out of the object just written.
///
/// `lld-link` is the linker because it is the one that runs anywhere: the
/// alternative is Microsoft's, which is only on Windows, and the point of a
/// cross-compiler is not having to be there. The runtime is the one built for
/// that target — `cargo build --target <triple> -p rill_runtime --release` —
/// and the libraries a program names are `.lib` files rather than `-l` flags.
fn link_windows(opts: &BuildOpts, obj: &Path, triple: &str) -> Result<PathBuf, String> {
    let runtime = target_runtime(triple, "rill_runtime.lib")?;
    let exe = opts.output.with_extension("exe");
    let mut cmd = Command::new("lld-link");
    for path in windows_libpaths(triple) {
        cmd.arg(format!("/libpath:{}", path.display()));
    }
    cmd.arg(obj)
        .arg(&runtime)
        .arg(format!("/out:{}", exe.display()))
        .arg("/subsystem:console")
        // What the Rust runtime it was built against needs from the system.
        .arg("kernel32.lib")
        .arg("ntdll.lib")
        .arg("advapi32.lib")
        .arg("userenv.lib")
        .arg("ws2_32.lib")
        .arg("bcrypt.lib")
        // The C library itself: `ucrt` is where the standard functions live on
        // a modern Windows, and `vcruntime` is the compiler's own support.
        .arg("ucrt.lib")
        // `snprintf` and its family are inline definitions in the modern
        // headers rather than symbols in the library; this is the shim that
        // gives them a body for anything that calls them by name.
        .arg("legacy_stdio_definitions.lib")
        .arg("vcruntime.lib")
        .arg("msvcrt.lib");
    let out = cmd
        .output()
        .map_err(|e| format!("failed to invoke `lld-link`: {e}\ninstall it with `brew install lld`"))?;
    if !out.status.success() {
        return Err(format!(
            "linking for {triple} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let _ = std::fs::remove_file(obj);
    println!("{} written for {triple}", exe.display());
    Ok(exe)
}

/// Links a WebAssembly module out of the object just written.
///
/// `wasm-ld` is the linker, and the C library is wasi-libc — which rustup
/// already ships for this target, so there is nothing to fetch. The startup
/// object has to come first: it defines `_start`, which is the only thing a
/// WASI host will call.
///
/// Libraries named by `extern "lib" fn ...` are not passed on. A wasm module
/// imports from its host rather than linking against a shared object, so there
/// is nothing here that `-lfoo` could mean; a program that names one gets an
/// undefined symbol from the linker, naming the function it could not find.

/// The browser side of a reactor, generated next to the module: a WASI shim
/// (browsers ship none), instantiation, and the init calls, so the page that
/// embeds a Rill module writes only its own code. Regenerated on every build;
/// not meant to be edited.
const WASM_LOADER: &str = r#"// Generated by `rill build` — the loader for the Rill wasm module beside it.
// Usage:
//   import { load } from "./NAME.js";
//   const rill = await load("./NAME.wasm", { imports... });
//   rill.some_export(...);
// The second argument supplies the module's `env` imports (extern fns no
// library satisfied). `rill.memory` is the module's linear memory; `println`
// output arrives on the console prefixed with [rill].

function wasiShim(getMemory, print) {
  const text = new TextDecoder();
  let carry = "";
  return {
    fd_write(fd, iovs, iovcnt, nwritten) {
      const view = new DataView(getMemory().buffer);
      let written = 0, out = "";
      for (let i = 0; i < iovcnt; i++) {
        const ptr = view.getUint32(iovs + i * 8, true);
        const len = view.getUint32(iovs + i * 8 + 4, true);
        out += text.decode(new Uint8Array(getMemory().buffer, ptr, len));
        written += len;
      }
      carry += out;
      const lines = carry.split("\n");
      carry = lines.pop();
      for (const line of lines) print(line);
      view.setUint32(nwritten, written, true);
      return 0;
    },
    fd_close: () => 0, fd_seek: () => 0,
    fd_prestat_get: () => 8, fd_prestat_dir_name: () => 8,
    args_sizes_get(a, b) { const v = new DataView(getMemory().buffer); v.setUint32(a, 0, true); v.setUint32(b, 0, true); return 0; },
    args_get: () => 0,
    environ_sizes_get(a, b) { const v = new DataView(getMemory().buffer); v.setUint32(a, 0, true); v.setUint32(b, 0, true); return 0; },
    environ_get: () => 0,
    proc_exit(code) { throw new Error(`rill module exited with code ${code}`); },
  };
}

export async function load(src, env = {}, opts = {}) {
  const print = opts.print ?? (line => console.log(`[rill] ${line}`));
  // A URL to fetch, or the module's bytes if the caller already has them —
  // a page that ships its wasm inside a bundle of its own has nothing left
  // to fetch, and making it invent a blob URL to hand back to `fetch` is a
  // round trip through the network stack to reach memory it is holding.
  const bytes = (src instanceof ArrayBuffer || ArrayBuffer.isView(src))
    ? src
    : await (await fetch(src, { cache: "no-store" })).arrayBuffer();
  const module = await WebAssembly.compile(bytes);
  let instance;
  instance = await WebAssembly.instantiate(module, {
    env,
    wasi_snapshot_preview1: wasiShim(() => instance.exports.memory, print),
  });
  instance.exports._initialize();   // wasi-libc setup
  instance.exports.rill_init();     // the module's `main`, once
  return instance.exports;
}
"#;

fn link_wasm(
    opts: &BuildOpts,
    obj: &Path,
    triple: &str,
    tmod: &rill_types::TModule,
) -> Result<PathBuf, String> {
    let runtime = target_runtime(triple, "librill_runtime.a")?;
    let sysroot = wasi_sysroot(triple)?;
    let out = opts.output.with_extension("wasm");
    let exports: Vec<&str> =
        tmod.fns.iter().filter(|f| f.exported).map(|f| f.name.as_str()).collect();
    let mut cmd = Command::new("wasm-ld");
    // A module with exports is a reactor — it has no `_start` and stays
    // instantiated for the host to call into — so it gets the reactor's
    // startup object and no entry point. `rill_init` is the program's `main`,
    // renamed to what it now is: something the host calls once.
    if exports.is_empty() {
        cmd.arg(sysroot.join("crt1-command.o"));
    } else {
        cmd.arg(sysroot.join("crt1-reactor.o"));
        cmd.arg("--no-entry");
        cmd.arg("--export=_initialize");
        cmd.arg("--export=rill_init");
        for name in &exports {
            cmd.arg(format!("--export={name}"));
        }
        // A reactor's imports are the host's to supply, so a C name that
        // resolves against nothing becomes an `env` import rather than an
        // error. A command keeps the strict behaviour: it runs with nobody
        // there to supply anything.
        cmd.arg("--allow-undefined");
    }
    cmd.arg(obj)
        .arg(&runtime)
        .arg(sysroot.join("libc.a"))
        .arg("-o")
        .arg(&out)
        // A declaration that disagrees with the C function it names is only a
        // warning here, and what the linker emits for it is a stub that traps
        // when called. On a target where the C `size_t` is 32 bits and Rill's
        // `Int` is 64, that is easy to write by accident — so it is an error,
        // reported now rather than half way through a run.
        .arg("--fatal-warnings");
    if !opts.debug {
        cmd.arg("--gc-sections").arg("--strip-all");
    }
    let link = cmd
        .output()
        .map_err(|e| format!("failed to invoke `wasm-ld`: {e}\ninstall it with `brew install lld`"))?;
    if !link.status.success() {
        let said = String::from_utf8_lossy(&link.stderr);
        // Two of these read as toolchain trouble and are not. A symbol wasi-libc
        // does not have is usually one wasi has no notion of — processes,
        // signals, users — and no flag will conjure it; the answer is either to
        // drop it or to let the host supply it, which is what a module with an
        // `export fn` does. A signature mismatch is the other: a declaration
        // that says 64 bits where the C is a word wide, which `--fatal-warnings`
        // catches here rather than letting it trap at run time.
        let hint = if said.contains("undefined symbol") {
            "\nwasi-libc has no processes, signals or users, so a C name for one of those is \
             not there to link. A module with an `export fn` in it is a reactor and may import \
             the name from its host instead."
        } else if said.contains("signature mismatch") {
            "\nthe declared width disagrees with the C symbol's. On wasm32 a `size_t` or a \
             pointer is 32 bits: write `USize`/`ISize` for those rather than `Int`, and they \
             stay 64-bit on every native target."
        } else {
            ""
        };
        return Err(format!("linking for {triple} failed:\n{said}{hint}"));
    }
    let _ = std::fs::remove_file(obj);
    // A reactor is made to be embedded, so the embedding half comes with it:
    // one loader module, regenerated beside the wasm on every build.
    if !exports.is_empty() {
        let loader = opts.output.with_extension("js");
        std::fs::write(&loader, WASM_LOADER)
            .map_err(|e| format!("cannot write {}: {e}", loader.display()))?;
        println!("{} and {} written for {triple}", out.display(), loader.display());
    } else {
        println!("{} written for {triple}", out.display());
    }
    Ok(out)
}

/// The toolchain's own bundle: `lib/rill/<triple>/` next to the executable's
/// directory. A packaged `rill` carries its runtimes and the wasi sysroot
/// there, so building for any target — including the browser — needs no Rust
/// toolchain on the user's machine. The development tree has no bundle and
/// falls back to cargo's and rustup's paths.
fn bundled_dir(triple: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?.join("lib").join("rill").join(triple);
    if dir.exists() { Some(dir) } else { None }
}

/// wasi-libc: the bundle first, then `$WASI_SYSROOT`, then the copy rustup
/// installs along with the target's standard library.
fn wasi_sysroot(triple: &str) -> Result<PathBuf, String> {
    if let Some(dir) = bundled_dir(triple) {
        if dir.join("libc.a").exists() {
            return Ok(dir);
        }
    }
    if let Ok(p) = std::env::var("WASI_SYSROOT") {
        return Ok(PathBuf::from(p));
    }
    let sysroot = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .map_err(|e| format!("failed to ask rustc where its sysroot is: {e}"))?;
    let root = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim().to_string());
    let lib = root.join("lib/rustlib").join(triple).join("lib/self-contained");
    if lib.join("libc.a").exists() {
        return Ok(lib);
    }
    Err(format!(
        "no wasi-libc for {triple} at {}\ninstall it with: rustup target add {triple}",
        lib.display()
    ))
}

/// Where Microsoft's own import libraries are. They cannot be shipped with
/// this compiler and they are not on a machine that is not Windows, so they
/// are fetched once — `cargo install xwin && xwin --accept-license splat
/// --output ~/.xwin` — and looked for where that leaves them. If the
/// directory is not there the linker says which library it wanted, which is
/// the more useful error than one made up here.
fn windows_libpaths(triple: &str) -> Vec<PathBuf> {
    let arch = if triple.starts_with("aarch64") { "aarch64" } else { "x86_64" };
    let root = std::env::var("XWIN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".xwin")
        });
    [
        root.join("crt").join("lib").join(arch),
        root.join("sdk").join("lib").join("um").join(arch),
        root.join("sdk").join("lib").join("ucrt").join(arch),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect()
}

/// The runtime built for that target, which cargo leaves under its own
/// directory. `$RILL_RUNTIME` is not consulted here: that one is this
/// machine's. The file name differs by target — `.lib` where the linker is
/// Microsoft's, `.a` everywhere else — so the caller says which it wants.
fn target_runtime(triple: &str, name: &str) -> Result<PathBuf, String> {
    if let Some(dir) = bundled_dir(triple) {
        let lib = dir.join(name);
        if lib.exists() {
            return Ok(lib);
        }
    }
    let here = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|d| d.parent()).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("target"));
    let lib = here.join(triple).join("release").join(name);
    if lib.exists() {
        return Ok(lib);
    }
    Err(format!(
        "no runtime for {triple} at {}\nbuild one with: cargo build --target {triple} -p rill_runtime --release",
        lib.display()
    ))
}

/// Locates `librill_runtime.a`: `$RILL_RUNTIME` if set, otherwise next to the
/// `rill` executable itself (which is where cargo puts both artifacts).
fn find_runtime() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("RILL_RUNTIME") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(format!("RILL_RUNTIME points to `{}`, which does not exist", p.display()));
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate rill executable: {e}"))?;
    // `current_exe` keeps the path we were launched through on some platforms
    // (macOS among them), so a `rill` symlinked onto $PATH would look next to
    // the link rather than the build directory. Try the real path too.
    let real = exe.canonicalize().ok();
    for dir in [Some(exe.as_path()), real.as_deref()].into_iter().flatten() {
        if let Some(c) = dir.parent().map(|d| d.join("librill_runtime.a")) {
            if c.exists() {
                return Ok(c);
            }
        }
    }
    Err("cannot find librill_runtime.a; build the workspace with `cargo build` \
         or set RILL_RUNTIME to its path"
        .to_string())
}

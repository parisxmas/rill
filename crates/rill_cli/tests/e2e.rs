//! End-to-end tests: compile each example with the real `rill` binary, run
//! the produced native executable, and assert on its stdout.
//!
//! Requires `librill_runtime.a` to exist next to the `rill` binary — running
//! `cargo test` from the workspace root builds all members, which guarantees
//! that.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

/// The runtime is a staticlib with no tests and no dependents, so `cargo test`
/// alone will not rebuild it — and linking a stale one produces failures that
/// look like miscompiles. Fail with the actual instruction instead.
fn require_fresh_runtime() {
    let root = workspace_root();
    let lib = Path::new(env!("CARGO_BIN_EXE_rill")).with_file_name("librill_runtime.a");
    let src = root.join("crates/rill_runtime/src/lib.rs");
    let (Ok(lib_m), Ok(src_m)) = (
        std::fs::metadata(&lib).and_then(|m| m.modified()),
        std::fs::metadata(&src).and_then(|m| m.modified()),
    ) else {
        return;
    };
    assert!(
        lib_m >= src_m,
        "`{}` is older than the runtime source — run `cargo build` before `cargo test`",
        lib.display()
    );
}

fn compile_and_run(example: &str) -> String {
    require_fresh_runtime();
    let root = workspace_root();
    let source = root.join("examples").join(example);
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join(Path::new(example).file_stem().unwrap());

    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(
        build.status.success(),
        "rill build {example} failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&exe).output().expect("failed to run produced executable");
    assert!(run.status.success(), "{example} exited with {:?}", run.status.code());
    String::from_utf8(run.stdout).expect("program output was not UTF-8")
}

/// Builds an example without running it, for the ones that open a window.
fn compile_only(example: &str) {
    require_fresh_runtime();
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(root.join("examples").join(example))
        .arg("-o")
        .arg(out_dir.join(Path::new(example).file_stem().unwrap()))
        .output()
        .expect("failed to spawn rill");
    assert!(
        build.status.success(),
        "rill build {example} failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
}

/// Compiles a program written here rather than kept in `examples/`, runs it,
/// and gives back its stdout.
fn run_source(name: &str, src: &str) -> String {
    require_fresh_runtime();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-src-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join(format!("{name}.rill"));
    std::fs::write(&path, src).unwrap();
    let exe = out_dir.join(name);
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(
        build.status.success(),
        "rill build {name} failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&exe).output().expect("failed to run produced executable");
    assert!(run.status.success(), "{name} exited with {:?}", run.status.code());
    String::from_utf8(run.stdout).expect("program output was not UTF-8")
}

/// Compiles a source that is expected to be rejected, and gives back what the
/// compiler said.
fn compile_error(src: &str) -> String {
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-err-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("bad.rill");
    std::fs::write(&path, src).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(out_dir.join("bad"))
        .output()
        .expect("failed to spawn rill");
    assert!(!build.status.success(), "expected this to be rejected:\n{src}");
    String::from_utf8_lossy(&build.stderr).into_owned()
}

#[test]
fn hello() {
    assert_eq!(compile_and_run("hello.rill"), "hello, world\n");
}

#[test]
fn fib() {
    assert_eq!(compile_and_run("fib.rill"), "832040\n");
}

#[test]
fn mixed() {
    assert_eq!(compile_and_run("mixed.rill"), "42\n6.28318\ntrue\ndone\n");
}

#[test]
fn shapes() {
    assert_eq!(compile_and_run("shapes.rill"), "3.14159\n6\n");
}

#[test]
fn list() {
    assert_eq!(compile_and_run("list.rill"), "5050\n");
}

#[test]
fn colors() {
    assert_eq!(compile_and_run("colors.rill"), "green\ntrue\n");
}

#[test]
fn map_fold_lambdas() {
    assert_eq!(compile_and_run("map.rill"), "385\n");
}

#[test]
fn option_pipeline() {
    assert_eq!(compile_and_run("option.rill"), "40\n42\n");
}

#[test]
fn tail_calls_run_in_constant_stack() {
    assert_eq!(compile_and_run("loop.rill"), "500000500000\n");
}

/// A function written inside another sees the names around it and may call
/// itself; lifting turns it into a top-level one, so the loop stays a loop
/// and a captured buffer keeps the type it was captured with.
#[test]
fn local_functions_capture_and_loop() {
    let src = "fn main() =\n  \
                 v = buf_int(4)\n  \
                 n = 5000000\n\n  \
                 fn fill(i) =\n    \
                   if i >= 4 then 0\n    \
                   else\n      \
                     v[i] = i * 2\n      \
                     fill(i + 1)\n\n  \
                 fn even(i) = if i == 0 then true else odd(i - 1)\n\n  \
                 fn odd(i) = if i == 0 then false else even(i - 1)\n\n  \
                 fn count(i, acc) = if i > n then acc else count(i + 1, acc + v[i % 4])\n\n  \
                 fill(0)\n  \
                 println(even(n))\n  \
                 println(count(1, 0))\n";
    assert_eq!(run_source("local_fns", src), "true\n15000000\n");
}

/// A loop that carries something keeps it in a parameter, which is to say a
/// register — the alternative being a one-element buffer reached through a
/// pointer on every touch.
#[test]
fn loops_can_carry_a_value() {
    let src = "fn main() =\n  \
                 v = buf_int(6)\n\n  \
                 for i in 0..6\n    \
                   v[i] = i * i\n\n  \
                 total = for i in 0..6 with acc = 0\n    \
                   acc + v[i]\n\n  \
                 println(total)\n  \
                 println(for i in 1..6 with p = 1 p * i)\n  \
                 println(for i in 0..3 with s = \"\" s + int_to_str(i))\n";
    assert_eq!(run_source("for_with", src), "55\n120\n012\n");
    // Nothing counted, and the carried value never reaches memory.
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-fw-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("carry.rill");
    std::fs::write(
        &path,
        "fn main() = println(for i in 0..1000 with acc = 0 acc + i)\n",
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("--emit-ir")
        .arg("-o")
        .arg(out_dir.join("carry"))
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&build.stdout);
    let body: String = ir
        .lines()
        .skip_while(|l| !l.starts_with("define i64 @main.for"))
        .take_while(|l| *l != "}")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!body.is_empty(), "the loop was not emitted as its own function");
    assert!(!body.contains("store"), "the carried value went to memory:\n{body}");
}

/// A builder is storage for bytes on their way to a string: adding to one is
/// a copy of what is added, not of what is already there.
#[test]
fn string_builders_accumulate() {
    let src = "fn main() =\n  \
                 b = strbuf()\n  \
                 b += \"mer\"\n  \
                 b += \"haba\"\n  \
                 b += 33\n  \
                 println(strbuf_str(b))\n  \
                 println(#b)\n  \
                 big = strbuf()\n\n  \
                 for i in 0..100000\n    \
                   big += \"0123456789\"\n\n  \
                 println(#big)\n  \
                 println(#strbuf_str(big))\n";
    assert_eq!(run_source("strbuf", src), "merhaba!\n8\n1000000\n1000000\n");
}

/// `scan_int` scans a number straight out of the input buffer: no line, no
/// split, no list of strings, nothing allocated. It stops on the delimiter it
/// finds without swallowing it, so the rest of the line is still there.
#[test]
fn scan_int_scans_numbers() {
    let src = "fn main() =\n  \
                 a = scan_int()\n  \
                 b = scan_int()\n  \
                 c = scan_int()\n  \
                 println(a + b + c)\n  \
                 println(read_line())\n  \
                 d = scan_int()\n  \
                 println(d)\n  \
                 println(io_ok())\n  \
                 e = scan_int()\n  \
                 println(io_ok())\n";
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-ri-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("scan.rill");
    std::fs::write(&path, src).unwrap();
    let exe = out_dir.join("scan");
    assert!(Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap()
        .success());
    let mut child = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    write!(child.stdin.as_mut().unwrap(), "  10 -3\n7 tail\n42\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "14\n tail\n42\ntrue\nfalse\n");
}

/// Standard input is read in blocks. It used to be a syscall a byte, which on
/// a large input was the whole of what a program did.
#[test]
fn standard_input_is_read_in_blocks() {
    let src = "fn main() =\n  \
                 a = read_line()\n  \
                 b = read_line()\n  \
                 println(#a)\n  \
                 println(b)\n  \
                 println(io_ok())\n";
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-in-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("input.rill");
    std::fs::write(&path, src).unwrap();
    let exe = out_dir.join("input");
    assert!(Command::new(env!("CARGO_BIN_EXE_rill"))
        .args(["build"])
        .arg(&path)
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap()
        .success());
    let mut child = std::process::Command::new(&exe)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    let long = "x".repeat(100_000);
    write!(child.stdin.as_mut().unwrap(), "{long}\nsecond\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "100000\nsecond\ntrue\n");
}

/// A loop over buffers must not count references. The buffers a loop carries
/// are borrowed where they are read and moved where they are handed on, so
/// the body of one has no retain and no release left in it — which is the
/// difference between this running at C's speed and at a fraction of it.
#[test]
fn loops_over_buffers_do_not_count_references() {
    let src = "fn main() =\n  \
                 v = buf_int(8)\n  \
                 acc = buf_int(1)\n\n  \
                 for i in 0..8\n    \
                   acc[0] += v[i]\n\n  \
                 println(acc[0])\n";
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-ir-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("loop.rill");
    std::fs::write(&path, src).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("--emit-ir")
        .arg("-o")
        .arg(out_dir.join("loop"))
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let ir = String::from_utf8_lossy(&build.stdout);
    let body: String = ir
        .lines()
        .skip_while(|l| !l.starts_with("define i64 @main.for"))
        .take_while(|l| *l != "}")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!body.is_empty(), "the loop was not emitted as its own function");
    // The exit branch releases what the frame owns, once; nothing else may.
    assert_eq!(body.matches("rc.retain").count(), 0, "retains in a loop body:\n{body}");
    assert!(
        body.matches("rill_buf_release").count() <= 2,
        "more than the frame's own releases:\n{body}"
    );
}

/// Slicing runs the runtime's `str_sub` with an end instead of a length,
/// each part evaluated once, and clamps rather than failing.
#[test]
fn slices_a_string() {
    let src = "fn main() =\n  \
                 s = \"merhaba\"\n  \
                 println(s[0:3])\n  \
                 println(s[3:])\n  \
                 println(s[:3])\n  \
                 println(s[2:2])\n  \
                 println(s[5:100])\n  \
                 println(s[1:#s - 1])\n";
    assert_eq!(run_source("slices", src), "mer\nhaba\nmer\n\nba\nerhab\n");
}

/// Tearing down a long list must not cost a stack frame per link. The
/// generated drop frees the box first and releases its last heap field by a
/// tail call, so this list — which is only released when `main` ends, long
/// after it is done being read — unwinds in constant stack.
#[test]
fn long_lists_release_in_constant_stack() {
    let src = "fn build(i, acc) = if i == 0 then acc else build(i - 1, Cons(int_to_str(i), acc))\n\
               \n\
               fn count(l, n) = match l\n  \
                 Nil -> n\n  \
                 Cons(_, t) -> count(t, n + 1)\n\
               \n\
               fn main() =\n  \
                 l = build(2000000, Nil)\n  \
                 println(count(l, 0))\n";
    assert_eq!(run_source("deep_drop", src), "2000000\n");
}

/// Every `.rill` file in the repository is kept in canonical form, so
/// `rill fmt` stays honest about its own output.
#[test]
fn repository_sources_are_formatted() {
    let root = workspace_root();
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [
        "examples",
        "examples/multi",
        "examples/http",
        "examples/json",
        "examples/yaml",
        "examples/gl",
        "benchmarks",
        "benchmarks/http",
        "editors/vscode/test",
    ] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "rill") {
                files.push(p);
            }
        }
    }
    files.push(root.join("crates/rill_syntax/src/prelude.rill"));
    let out = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("fmt")
        .arg("--check")
        .args(&files)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "run `rill fmt` on these:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fmt_normalizes_and_preserves_behavior() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-fmt-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("messy.rill");
    std::fs::write(
        &src,
        "# a list length\ntype   T\n     A\n     B(Int)\nfn  size(x)=match x\n       A->0\n       B(n)->n\nfn main()=println(size(B(41))+size(A)+1)\n",
    )
    .unwrap();

    let fmt = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rill")).arg("fmt").args(args).output().unwrap()
    };
    let first = fmt(&["--stdout", src.to_str().unwrap()]);
    assert!(first.status.success());
    let once = String::from_utf8(first.stdout).unwrap();
    assert!(once.contains("fn size(x) = match x\n  A -> 0\n"), "{once}");
    assert!(once.starts_with("# a list length\n"), "{once}");

    // Idempotent, and the program still behaves the same.
    std::fs::write(&src, &once).unwrap();
    let second = fmt(&["--stdout", src.to_str().unwrap()]);
    assert_eq!(String::from_utf8(second.stdout).unwrap(), once);

    let exe = out_dir.join("messy");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
    let _ = root;
}

#[test]
fn repl_session() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn repl");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fn double(x) = x * 2\n\ndouble(21)\nSome(double(5))\n\"a\" + \"b\"\n:quit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\nSome(10)\nab\n");
}

/// A `--debug` build carries DWARF that names the Rill source, which is what
/// lets a debugger stop on a source line rather than a mangled symbol.
#[test]
fn debug_builds_carry_source_lines() {
    require_fresh_runtime();
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-dwarf-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("fib");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg("--debug")
        .arg(root.join("examples/fib.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    assert_eq!(String::from_utf8_lossy(&Command::new(&exe).output().unwrap().stdout), "832040\n");

    // The object file stays next to the binary in debug builds; macOS keeps
    // the DWARF there and reaches it through the linker's debug map.
    let obj = exe.with_extension("o");
    let bytes = std::fs::read(&obj).expect("debug builds keep the object file");
    let needle = b"fib.rill";
    assert!(
        bytes.windows(needle.len()).any(|w| w == needle),
        "no reference to the source file in the debug object"
    );
}

/// YAML for configuration files, reusing the JSON module's value type so a
/// parsed document prints as JSON with no conversion.
#[test]
fn yaml_module() {
    assert_eq!(
        compile_and_run("yaml/demo.rill"),
        "{\"name\":\"gateway\",\"port\":8080,\"debug\":false,\
         \"limits\":{\"memory\":512,\"cpu\":1.5},\"tags\":[\"fast\",\"small\"],\
         \"servers\":[{\"name\":\"alpha\",\"port\":1000},{\"name\":\"beta\",\"port\":2000}]}\n\
         gateway\n1.5\nfast+small\n2000\n"
    );
}

/// A JSON parser and printer written in Rill: nested lookup, escapes, and a
/// round trip that comes back identical.
#[test]
fn json_module() {
    assert_eq!(
        compile_and_run("json/demo.rill"),
        "ada\n36\ntrue\nx+y\n\
         {\"user\":{\"name\":\"ada\",\"age\":36},\"tags\":[\"x\",\"y\"],\"ok\":true,\"pi\":3.5}\n\
         [1,\"a\\nb\",null,false,[2]]\n"
    );
}

/// A WebSocket server and client in one program: the handshake needs SHA-1
/// and base64, both of which are written in Rill too.
#[test]
fn websocket_server() {
    assert_eq!(
        compile_and_run("http/ws_server.rill"),
        "HTTP/1.1 101 Switching Protocols\necho: merhaba\necho: dunya\n"
    );
}

/// SHA-1 and base64 in Rill, against the values the RFCs publish.
#[test]
fn sha1_and_base64() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-sha-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::copy(root.join("examples/http/sha1.rill"), out_dir.join("sha1.rill")).unwrap();
    let src = out_dir.join("check.rill");
    std::fs::write(
        &src,
        "import \"sha1\"\n\nfn main() =\n  println(base64(sha1(\"abc\")))\n  println(base64(\"Man\"))\n  println(base64(sha1(\"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11\")))\n",
    )
    .unwrap();
    let exe = out_dir.join("check");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        // The last is the example key from RFC 6455.
        "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=\nTWFu\ns3pPLMBiTxaQ9kYGzzhZRbK+xOo=\n"
    );
}

/// An HTTP server written in Rill, answering its own requests. Six requests
/// arrive over five connections: the fourth and fifth share one, which is the
/// keep-alive path.
#[test]
fn http_server() {
    assert_eq!(
        compile_and_run("http/server.rill"),
        "200 hello from rill\n200 GET /echo\n200 42\n\
         200 hello from rill\n200 2\n404 no route for /nope\n"
    );
}

/// A TCP server and its client in one program: strands waiting on sockets
/// park, so this works even on a single worker.
#[test]
fn tcp_echo_server() {
    assert_eq!(compile_and_run("echo_server.rill"), "echo: merhaba\necho: dunya\n");
}

/// Calls into libm and libc, including the string bridge and a null check.
#[test]
fn foreign_functions() {
    assert_eq!(
        compile_and_run("ffi.rill"),
        "1.41421\n1024\n7\ntrue\n7\n<unset>\n"
    );
}

/// The typed buffer: a width in the type, a length it checks against, and a
/// count that frees it — with C reading the same bytes through `buf_data`.
#[test]
fn typed_buffers() {
    assert_eq!(
        compile_and_run("buffers.rill"),
        "7.5\n6\n200 -56\n4\ndone\n"
    );
}

/// The maze carver, with no window in sight: the grid and the search stack
/// are raw buffers, which is the language's only mutable memory, and the
/// generator is seeded so the same maze comes out every run.
#[test]
fn maze_is_carved_deterministically() {
    let expected = concat!(
        "##########################################\n",
        "##  ##                                  ##\n",
        "##  ##  ##########  ######  ##########  ##\n",
        "##  ##  ##      ##  ##      ##      ##  ##\n",
        "##  ######  ##  ##  ##  ##  ##  ##  ######\n",
        "##      ##  ##      ##  ##      ##      ##\n",
        "######  ##  ##############  ##########  ##\n",
        "##  ##  ##                          ##  ##\n",
        "##  ##  ##  ##########################  ##\n",
        "##      ##          ##      ##      ##  ##\n",
        "##  ##############  ##  ######  ##  ##  ##\n",
        "##  ##          ##  ##  ##      ##  ##  ##\n",
        "##  ##  ######  ##  ##  ##  ######  ##  ##\n",
        "##  ##  ##      ##      ##      ##      ##\n",
        "##  ##  ##  ##################  ######  ##\n",
        "##  ##                      ##  ##      ##\n",
        "##  ##  ##################  ##  ##  ##  ##\n",
        "##      ##              ##      ##  ##  ##\n",
        "##########  ######  ##############  ##  ##\n",
        "##              ##                      ##\n",
        "##########################################\n",
    );
    assert_eq!(compile_and_run("gl/mazemap.rill"), expected);
}

/// C calling back into Rill: a top-level function crosses as a C function
/// pointer, wrapped in a trampoline that widens C's 32-bit argument. Two of
/// the callbacks are invoked by C for their effect, one of them after `main`
/// has already returned.
///
/// The two sorted lines carry the rest. `qsort` only orders the array if the
/// comparator's answer travels back out, so a trampoline that dropped the
/// result would leave the input order alone; and both orderings reach C
/// through one helper that takes the comparator as a parameter, so they can
/// only differ if that helper was specialized per comparator.
#[test]
fn callbacks_into_rill() {
    assert_eq!(
        compile_and_run("callbacks.rill"),
        "raising\nhandler ran, signal 30\nup   1 2 3 4 5\ndown 5 4 3 2 1\nmain done\n\
         atexit ran, after main returned\n"
    );
}

/// The AppKit examples open a window, so they cannot be run from here — but
/// they are the only users of several parts of the FFI (a callback that
/// answers, callbacks carried as parameters, five-argument methods), and
/// building them is enough to catch a binding that has drifted.
#[cfg(target_os = "macos")]
#[test]
fn cocoa_examples_compile() {
    compile_only("cocoa/demo.rill");
    compile_only("cocoa/taskman.rill");
}

/// A callback still has to be settled while compiling, since what crosses is
/// a bare code pointer. The diagnostic names the parameter it travelled under
/// rather than pointing at the C signature.
#[test]
fn a_callback_chosen_at_run_time_is_rejected() {
    let err = compile_error(
        "extern fn qsort(base: Ptr, n: U64, size: U64, cmp: Fn(Ptr, Ptr) -> I32) -> Unit\n\
         \n\
         fn order(b: Buf(I32), cmp) =\n\
        \x20 qsort(buf_data(b), buf_len(b), 4, cmp)\n\
        \x20 b\n\
         \n\
         fn main() =\n\
        \x20 b = buf_i32(1)\n\
        \x20 order(b, \\x, y -> 0)\n\
        \x20 println(1)\n",
    );
    assert!(err.contains("`order` hands its parameter `cmp` to C"), "{err}");
}

/// Narrow C types and raw buffers. The first line is the load-bearing one:
/// `sqrtf` takes a 32-bit float, so a double handed over unconverted would
/// leave it reading an all-zero half of the register and answering 0.
#[test]
fn foreign_widths_and_buffers() {
    assert_eq!(
        compile_and_run("ffi_buffers.rill"),
        "1.41421\n1024\n7\n7\n109\n200\n-56\n7.5\n1\n"
    );
}

/// OpenGL, driven straight from Rill: a context with no window behind it, a
/// compiled shader pair, vertex data in a raw buffer, and the pixels read
/// back. The shape is stable because no pixel centre lands on a triangle
/// edge, so the rasteriser has nothing to decide.
#[test]
fn opengl_offscreen_triangle() {
    let art = concat!(
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        ".......................##.......................\n",
        "......................####......................\n",
        ".....................######.....................\n",
        "....................########....................\n",
        "..................############..................\n",
        ".................##############.................\n",
        "................################................\n",
        "...............##################...............\n",
        "..............####################..............\n",
        ".............######################.............\n",
        "............########################............\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
        "................................................\n",
    );
    assert_eq!(compile_and_run("gl/offscreen.rill"), art);
}

#[test]
fn select_over_channels() {
    assert_eq!(compile_and_run("select.rill"), "-1\n55\nsent\n7\n");
}

#[test]
fn buffered_channels() {
    assert_eq!(
        compile_and_run("buffered.rill"),
        "30\n5050\nhanded over directly\n"
    );
}

#[test]
fn loops_are_tail_recursion() {
    assert_eq!(
        compile_and_run("loops.rill"),
        "500000500000\n\
         21\n\
         5\n\
         Cons(4, Cons(16, Cons(36, Cons(64, Cons(100, Nil)))))\n\
         720\n\
         3\n2\n1\n\
         42\n\
         0\n"
    );
}

#[test]
fn custom_show_and_ord() {
    assert_eq!(
        compile_and_run("ordering.rill"),
        "$12.50\n\
         Cons($0.99, Cons($12.50, Cons($300.00, Nil)))\n\
         true\n\
         Cons(apple, Cons(fig, Cons(pear, Nil)))\n\
         Cons(1, Cons(2, Cons(3, Nil)))\n"
    );
}

#[test]
fn nested_patterns() {
    assert_eq!(
        compile_and_run("patterns.rill"),
        "empty\n\
         one hole\n\
         starts with zero\n\
         two values: 7\n\
         one value: 7\n\
         starts with a hole\n\
         origin\n\
         on the y axis at 5\n\
         at 2,3\n\
         single 42\n\
         many\n\
         empty list\n\
         nothing\n"
    );
}

#[test]
fn user_defined_traits() {
    assert_eq!(
        compile_and_run("traits.rill"),
        "shape Circle(1) [p2]\nint 42 [p1]\nstr 'merhaba' [p7]\n4\n"
    );
}

/// Aliased imports keep each module's names to itself, so two files can both
/// define `Unit`, `label` and `scale`.
#[test]
fn qualified_module_names() {
    assert_eq!(compile_and_run("multi/units.rill"), "1.5m\n2ft\n20\n30\n");
}

#[test]
fn multi_file_program_with_imports() {
    assert_eq!(
        compile_and_run("multi/main.rill"),
        "Circle(1) -> 3.14159\n\
         Rect(2, 3) -> 6\n\
         Circle(0.5) -> 0.785397\n\
         --------------------\n\
         total: 9.92699\n"
    );
}

#[test]
fn file_io_and_text_processing() {
    assert_eq!(
        compile_and_run("io.rill"),
        "args >= 1: true\n\
         written: true\n\
         cannot read /no/such/rill/file\n\
         lines: 3\n\
         alpha\n\
         Cons(beta, Cons(gamma, Cons(delta, Nil)))\n\
         84\n\
         true\n"
    );
}

#[test]
fn logical_operators_short_circuit() {
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-ops-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("ops.rill");
    std::fs::write(
        &src,
        "fn loud(s: Str, v) =\n  println(s)\n  v\n\nfn main() =\n  println(loud(\"a\", true) && loud(\"b\", false))\n  println(loud(\"c\", false) && loud(\"d\", true))\n  println(loud(\"e\", false) || loud(\"f\", true))\n  println(!true)\n",
    )
    .unwrap();
    let exe = out_dir.join("ops");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    // `d` must never print: && short-circuits.
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "a\nb\nfalse\nc\nfalse\ne\nf\ntrue\nfalse\n"
    );
}

#[test]
fn showcase_program() {
    assert_eq!(
        compile_and_run("showcase.rill"),
        "Mul(Add(Num(2), Num(3)), Sub(Num(10), Num(4))) = 30\n\
         error: division by zero\n\
         Cons(1, Cons(2, Cons(3, Cons(5, Nil))))\n\
         iki\n\
         ?\n\
         Cons(1, Cons(2, Cons(3, Cons(7, Cons(8, Cons(9, Nil))))))\n\
         true\n\
         workers: 2870 formula: 2870 match: true\n\
         workers stopped, acks: 0\n\
         w1, w2, w3, w4, w5\n\
         14\n\
         1\n"
    );
}

#[test]
fn derived_show_and_eq() {
    assert_eq!(
        compile_and_run("show.rill"),
        "Some(42)\nCons(1, Cons(2, Cons(3, Nil)))\ntrue\ntrue\nfalse\nOk(yes)!\nfalse\n"
    );
}

#[test]
fn stdlib_prelude_and_strings() {
    assert_eq!(
        compile_and_run("stdlib.rill"),
        "30\n10\n25\nrill is fast\nn = 55\nababab\n7\ntrue\n27\n"
    );
}

#[test]
fn strands_and_channels() {
    assert_eq!(compile_and_run("pingpong.rill"), "5050\n");
}

/// The same concurrent programs must give the same answers on several
/// workers, and still free everything.
#[test]
fn multi_core_scheduler() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-mt-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();

    // `leak_free` is false where the program finishes with strands still
    // queued: like Go, exiting `main` abandons them, envs and all. Even a
    // program that drains cleanly on one worker only does so deterministically
    // there, so the check is limited to that case.
    for (src, expected, leak_free) in [
        (root.join("examples/pingpong.rill"), "5050\n", true),
        (root.join("benchmarks/parallel.rill"), "17426472\n", false),
        // Several workers allocating at once. This is the case that a
        // single global pool got wrong: it corrupted its own free list and
        // the program died with a segmentation fault rather than a wrong
        // number, so the assertion that matters here is that it finishes.
        (root.join("benchmarks/parallel_alloc.rill"), "80800000\n", false),
    ] {
        let exe = out_dir.join(src.file_stem().unwrap());
        let build = Command::new(env!("CARGO_BIN_EXE_rill"))
            .arg("build")
            .arg("--parallel")
            .arg(&src)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
        for threads in ["1", "4"] {
            let run = Command::new(&exe)
                .env("RILL_THREADS", threads)
                .env("RILL_DEBUG_ALLOC", "1")
                .output()
                .unwrap();
            assert!(run.status.success(), "{} at {threads} threads crashed", src.display());
            assert_eq!(
                String::from_utf8_lossy(&run.stdout),
                expected,
                "{} at {threads} threads",
                src.display()
            );
            if leak_free && threads == "1" {
                assert!(
                    String::from_utf8_lossy(&run.stderr).contains("live allocations: 0"),
                    "{} leaked at {threads} threads",
                    src.display()
                );
            }
        }
    }
}

/// Sockets across several workers: a strand that parks on a file
/// descriptor can only be resumed by the worker whose stack holds its frames,
/// so the readiness event has to reach that worker and no other. When it does
/// not, the program does not fail — it waits, which is why this runs the
/// server example rather than asserting on a stopwatch.
#[test]
fn socket_server_on_several_workers() {
    require_fresh_runtime();
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-mtsock-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("server");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg("--parallel")
        .arg(root.join("examples/http/server.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).env("RILL_THREADS", "4").output().unwrap();
    assert!(run.status.success(), "server exited with {:?}", run.status.code());
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "200 hello from rill\n200 GET /echo\n200 42\n\
         200 hello from rill\n200 2\n404 no route for /nope\n"
    );
}

/// A binary built without `--parallel` counts references without
/// synchronizing, so the runtime must refuse to spread it over workers.
#[test]
fn parallelism_is_opt_in() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-seq-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("pingpong");
    Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(root.join("examples/pingpong.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    let run = Command::new(&exe).env("RILL_THREADS", "4").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "5050\n");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("rebuild with `rill build --parallel`"),
        "expected a note about the missing flag"
    );
}

#[test]
fn ring_of_ten_thousand_strands() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-ring-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("ring");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(root.join("examples/ring.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).env("RILL_DEBUG_ALLOC", "1").output().unwrap();
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "10000\n");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("live allocations: 0"),
        "channels/strand envs leaked"
    );
}

#[test]
fn reference_counting_frees_everything() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-rc-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("churn");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(root.join("examples/churn.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&exe).env("RILL_DEBUG_ALLOC", "1").output().unwrap();
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "33835000\n");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("live allocations: 0"),
        "expected zero live allocations, got: {stderr}"
    );
}

#[test]
fn binaries_are_small_like_c() {
    require_fresh_runtime();
    // Its own directory: the shared one is rewritten by other tests as they
    // run, and this test reads the file rather than just executing it.
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-size-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("hello");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(workspace_root().join("examples/hello.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let size = std::fs::metadata(&exe).unwrap().len();
    assert!(size < 100_000, "hello binary is {size} bytes; expected C-like (<100 KB)");

    // A program with no strands, channels or sockets in it links no
    // scheduler, which is most of the runtime. Compared against a program
    // that does, rather than against a constant, so the test measures the
    // difference rather than the absolute size of the runtime.
    let with_chans = out_dir.join("pingpong");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(workspace_root().join("examples/pingpong.rill"))
        .arg("-o")
        .arg(&with_chans)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let concurrent = std::fs::metadata(&with_chans).unwrap().len();
    assert!(
        size < concurrent,
        "hello ({size}) should be smaller than a program using channels \
         ({concurrent}) — did the scheduler get linked in anyway?"
    );
    // File size is the wrong ruler for the difference: a Mach-O segment is
    // padded to a 16 KB page, so seven kilobytes of scheduler can disappear
    // into slack the file was already carrying and show up as a few hundred
    // bytes on disk. What is being asserted is that the code is not there, so
    // measure the code.
    assert!(
        text_bytes(&exe) + 4_000 < text_bytes(&with_chans),
        "hello's __text ({}) should be far smaller than that of a program \
         using channels ({}) — did the scheduler get linked in anyway?",
        text_bytes(&exe),
        text_bytes(&with_chans)
    );
}

/// The size of the executable code in a Mach-O binary, as `size -m` reports
/// it. Only used by the test above, which is macOS-only for the same reason
/// the linker flags are.
#[cfg(target_os = "macos")]
fn text_bytes(path: &std::path::Path) -> u64 {
    let out = Command::new("size").arg("-m").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("Section __text:"))
        .and_then(|n| n.trim().parse().ok())
        .expect("no __text section in size -m output")
}

#[cfg(not(target_os = "macos"))]
fn text_bytes(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

#[test]
fn type_errors_fail_the_build() {
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-err-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("bad.rill");
    std::fs::write(&src, "fn main() = println(1 + 2.0)\n").unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(out_dir.join("bad"))
        .output()
        .expect("failed to spawn rill");
    assert!(!build.status.success());
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(stderr.contains("type error"), "stderr was: {stderr}");
}

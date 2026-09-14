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

/// A program expected to die: its exit status and what it said on standard
/// error, given the arguments.
fn compile_and_crash(example: &str, args: &[&str]) -> (Option<i32>, String) {
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
    let run = Command::new(&exe).args(args).output().expect("failed to run produced executable");
    assert!(!run.status.success(), "{example} {args:?} was expected to die");
    (run.status.code(), String::from_utf8_lossy(&run.stderr).into_owned())
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

/// The same, for what only a wasm build refuses. The refusal happens in
/// codegen, so this stops before anything has to be linked and needs no wasm
/// toolchain to be installed.
fn compile_error_for_wasm(src: &str) -> String {
    let out_dir = std::env::temp_dir().join(bad_name("wasm"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("bad.rill");
    std::fs::write(&path, src).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(out_dir.join("bad.wasm"))
        .arg("--target")
        .arg("wasm32-wasip1")
        .output()
        .expect("failed to spawn rill");
    assert!(!build.status.success(), "expected wasm to reject this:\n{src}");
    String::from_utf8_lossy(&build.stderr).into_owned()
}

/// A number no other test in this process will use. The rejected-source
/// helpers all wrote to one `bad.rill` per process, which is a race the moment
/// two of them run at once — and they do, because the harness runs tests in
/// parallel. One would read back the other's source and be told the compiler
/// had accepted it.
static NEXT_BAD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn bad_name(kind: &str) -> String {
    let n = NEXT_BAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("bad-{kind}-{}-{n}", std::process::id())
}

/// Compiles a source that is expected to be rejected, and gives back what the
/// compiler said.
fn compile_error(src: &str) -> String {
    let out_dir = std::env::temp_dir().join(bad_name("err"));
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

/// A program written here that is expected to die: what it said on
/// standard error.
fn run_source_dies(name: &str, src: &str) -> String {
    require_fresh_runtime();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-dies-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join(format!("{name}.rill"));
    std::fs::write(&path, src).unwrap();
    let exe = out_dir.join(name);
    let build = Command::new(env!("CARGO_BIN_EXE_rill")).arg("build").arg(&path).arg("-o").arg(&exe).output().unwrap();
    assert!(build.status.success(), "rill build {name} failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().expect("failed to run produced executable");
    assert!(!run.status.success(), "{name} was expected to die");
    String::from_utf8_lossy(&run.stderr).into_owned()
}

/// What `rill check --parallel` says about a program written here: empty
/// when it passes.
fn check_parallel(src: &str) -> String {
    let out_dir = std::env::temp_dir().join(bad_name("par"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("par.rill");
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill")).arg("check").arg("--parallel").arg(&path).output().unwrap();
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// What `rill fmt` writes a program written here back as.
fn fmt_source(src: &str) -> String {
    let out_dir = std::env::temp_dir().join(bad_name("fmt"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("in.rill");
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill")).arg("fmt").arg("--stdout").arg(&path).output().unwrap();
    assert!(out.status.success(), "rill fmt refused:\n{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
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
        .skip_while(|l| !(l.starts_with("define") && l.contains("@main.for")))
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

/// `add(2, _)` is the function you get by leaving an argument to be given
/// later. Written out as the lambda it means, so everything after `lift` sees
/// an ordinary closure.
#[test]
fn a_placeholder_leaves_an_argument_for_later() {
    let src = "type Sekil\n  \
                 Daire(r: Int)\n\n\
               fn add(a, b) = a + b\n\n\
               fn bol(a, b) = a / b\n\n\
               fn uc(a, b, c) = a * 100 + b * 10 + c\n\n\
               fn uygula(x, f) = f(x)\n\n\
               fn main() =\n  \
                 iki = add(2, _)\n  \
                 println(int_to_str(iki(5)))\n  \
                 yari = bol(_, 2)\n  \
                 println(int_to_str(yari(10)))\n  \
                 ikisi = uc(_, 5, _)\n  \
                 println(int_to_str(ikisi(9, 4)))\n  \
                 l = Cons(1, Cons(2, Nil))\n  \
                 println(join(map(map(l, add(10, _)), int_to_str(_)), \",\"))\n  \
                 ds = map(Cons(4, Nil), Daire(_))\n  \
                 println(match ds\n    \
                   Cons(Daire(r), _) -> int_to_str(r)\n    \
                   Nil -> \"yok\")\n  \
                 g = uygula(_, add(1, _))\n  \
                 println(int_to_str(g(5)))\n";
    // Bound first and called after, because calling what a call returned —
    // `add(2, _)(5)` — is not something the parser does, placeholder or not.
    // Any position, not only the last: `bol(_, 2)` halves what it is given.
    // `int_to_str(_)` is a builtin, which could not be passed as a value at
    // all before. `Daire(_)` is a constructor. The last is a placeholder
    // inside a placeholder.
    assert_eq!(run_source("placeholder", src), "7\n5\n954\n11,12\n4\n6\n");
}

/// Leaving an argument out altogether is still the mistake it usually is. This
/// is the reason for the underscore: under-application that means it says so,
/// and under-application that does not is still caught where it happened.
#[test]
fn too_few_arguments_is_still_an_error() {
    let src = "fn add(a, b) = a + b\n\n\
               fn main() =\n  \
                 f = add(2)\n  \
                 println(int_to_str(f(3)))\n";
    let err = compile_error(src);
    assert!(err.contains("takes 2 argument(s), but 1 given"), "{err}");

    let bare = "fn main() =\n  \
                  x = _\n  \
                  println(int_to_str(1))\n";
    let err = compile_error(bare);
    assert!(err.contains("only means something where an argument would go"), "{err}");
}

/// Every program that ships with the repository still compiles.
///
/// The tests around this one are all sources written inside them, which says
/// nothing about the corpus in `book/code` and `examples` — and that corpus
/// rots quietly. `examples/gl/hud.rill` returned a `Batch(vao, vbo)` of a type
/// that was never declared anywhere, and called a `clampf` that only exists in
/// a different example; the gl README tells you to run it. Nothing had
/// compiled it in a long time.
///
/// What each file is, is read off the file rather than listed here, so an
/// example added tomorrow is covered without anybody remembering to say so: a
/// file with `export fn` is built for wasm, one with `fn main(` is built for
/// the host, and anything else is a module and is reached through whoever
/// imports it.
#[test]
fn every_example_in_the_repository_compiles() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Two programs in the book are meant not to compile: they are what the
    // chapter is showing. If either ever builds, the chapter is wrong.
    let must_fail = ["book/code/ch03/mixing.rill", "book/code/ch05/missing.rill"];

    let mut files = Vec::new();
    for dir in ["book/code", "examples"] {
        collect_rill(&root.join(dir), &mut files);
    }
    files.sort();
    assert!(files.len() > 50, "found only {} sources — is the walk right?", files.len());

    let out_dir = std::env::temp_dir().join(bad_name("corpus"));
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut broken = Vec::new();
    let mut compiled = 0;
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let src = std::fs::read_to_string(path).unwrap();
        let wasm = src.lines().any(|l| l.starts_with("export fn"));
        let host = src.lines().any(|l| l.starts_with("fn main("));
        if !wasm && !host {
            continue; // a module; its importers cover it
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rill"));
        cmd.arg("build").arg(path).arg("-o").arg(out_dir.join("out"));
        if wasm {
            cmd.arg("--target").arg("wasm32-wasip1");
        }
        let build = cmd.output().expect("failed to spawn rill");
        let expected_to_fail = must_fail.contains(&rel.as_str());
        if build.status.success() == expected_to_fail {
            let said = String::from_utf8_lossy(&build.stderr);
            broken.push(if expected_to_fail {
                format!("{rel}: compiled, and the book says it does not")
            } else {
                format!("{rel}: {}", said.lines().next().unwrap_or("(nothing said)"))
            });
        }
        compiled += 1;
    }
    assert!(broken.is_empty(), "{} of {compiled} did not do as expected:\n{}", broken.len(), broken.join("\n"));
}

fn collect_rill(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rill(&p, out);
        } else if p.extension().is_some_and(|x| x == "rill") {
            out.push(p);
        }
    }
}

/// `.x` used to be answered where it was written, which made it depend on the
/// order the statements were in: reading a field before some later line pinned
/// the type was an error, and the same two lines the other way round compiled.
/// It is deferred now and settled once the whole dependency group is inferred.
#[test]
fn a_field_can_be_read_before_its_type_is_known() {
    let head = "type Nokta\n  \
                  Nokta(x: Int, y: Int)\n\n\
                fn uzak(p: Nokta) = p.x + p.y\n\n";
    // The line that says what `p` is comes after the line that reads a field.
    let after = format!(
        "{head}fn f(p) =\n  \
           a = p.x\n  \
           a + uzak(p)\n\n\
         fn main() = println(int_to_str(f(Nokta(1, 2))))\n"
    );
    assert_eq!(run_source("fieldafter", &after), "4\n");

    // And before, which always worked.
    let before = format!(
        "{head}fn f(p) =\n  \
           a = uzak(p)\n  \
           a + p.x\n\n\
         fn main() = println(int_to_str(f(Nokta(1, 2))))\n"
    );
    assert_eq!(run_source("fieldbefore", &before), "4\n");
}

/// A field no type has is said to be that, now that inference has looked for
/// the type among the fields and found none to name.
#[test]
fn a_field_no_type_has_is_said_so() {
    let src = "fn f(p) = p.x\n\n\
               fn main() = println(\"x\")\n";
    let err = compile_error(src);
    assert!(
        err.contains("nothing says what `p` is") && err.contains("no type has a field `x`"),
        "{err}"
    );
}

/// When nothing in the function says the type of a value, the fields read
/// from it do: it is the one type with one constructor that has them all.
/// An update's fields say it the same way, and a field of a field chains.
#[test]
fn the_fields_read_name_the_type() {
    let src = "type Nokta\n  \
                 Nokta(x: Int, y: Int)\n\n\
               type Oyuncu\n  \
                 Oyuncu(ad: Str, yer: Nokta, can: Int)\n\n\
               fn uzak(p) = p.x + p.y\n\n\
               fn yara(o, n) = { o | can = o.can - n }\n\n\
               fn nerede(o) = o.yer.x\n\n\
               fn main() =\n  \
                 o = Oyuncu(\"ada\", Nokta(3, 4), 100)\n  \
                 println(int_to_str(uzak(o.yer)))\n  \
                 println(int_to_str(yara(o, 35).can))\n  \
                 println(int_to_str(nerede(o)))\n";
    assert_eq!(run_source("fieldnames", src), "7\n65\n3\n");
}

/// Two types with the same field is the one case an annotation is still for,
/// and the error names them. Reading a second field only one of them has
/// settles it; a field no type has says which types have the others.
#[test]
fn two_types_with_the_field_still_ask() {
    let head = "type Nokta\n  \
                  Nokta(x: Int, y: Int)\n\n\
                type Uzay\n  \
                  Uzay(x: Int, y: Int, z: Int)\n\n";
    let err = compile_error(&format!("{head}fn f(p) = p.x\n\nfn main() = println(\"x\")\n"));
    assert!(
        err.contains("`Nokta` and `Uzay` alike have a field `x`") && err.contains("`p: Nokta`"),
        "{err}"
    );

    let src = format!("{head}fn f(p) = p.x + p.z\n\nfn main() = println(int_to_str(f(Uzay(1, 2, 3))))\n");
    assert_eq!(run_source("fieldpair", &src), "4\n");

    let err = compile_error(&format!("{head}fn f(p) = p.x + p.q\n\nfn main() = println(\"x\")\n"));
    assert!(
        err.contains("no type has fields `x`, `q` together") && err.contains("`Nokta` has x, y"),
        "{err}"
    );
}

/// An update is deferred the way a read is: `{ p | x = 9 }` before the line
/// that says what `p` is compiles, and its complaints still land.
#[test]
fn an_update_can_be_written_before_its_type_is_known() {
    let head = "type Nokta\n  \
                  Nokta(x: Int, y: Int)\n\n\
                fn uzak(p: Nokta) = p.x + p.y\n\n";
    let src = format!(
        "{head}fn f(p) =\n  \
           q = {{ p | x = 9 }}\n  \
           uzak(q) + uzak(p)\n\n\
         fn main() = println(int_to_str(f(Nokta(1, 2))))\n"
    );
    assert_eq!(run_source("updafter", &src), "14\n");

    let err = compile_error(&format!(
        "{head}fn f(p) =\n  q = {{ p | z = 9 }}\n  uzak(q)\n\nfn main() = println(\"x\")\n"
    ));
    assert!(err.contains("no field `z`") && err.contains("it has x, y"), "{err}");

    let err = compile_error(&format!(
        "{head}fn f(p) =\n  q = {{ p | x = \"no\" }}\n  uzak(q)\n\nfn main() = println(\"x\")\n"
    ));
    assert!(
        err.contains("the new value for a field") && err.contains("expected Int, found Str"),
        "{err}"
    );
}

/// Deferring must not lose the ordinary complaints.
#[test]
fn field_errors_still_land() {
    let two = "type Sekil\n  \
                 Daire(r: Int)\n  \
                 Kare(k: Int)\n\n\
               fn f(s: Sekil) = s.r\n\n\
               fn main() = println(int_to_str(f(Daire(1))))\n";
    assert!(compile_error(two).contains("more than one constructor"));

    let missing = "type Nokta\n  \
                     Nokta(x: Int, y: Int)\n\n\
                   fn f(p: Nokta) = p.z\n\n\
                   fn main() = println(int_to_str(f(Nokta(1, 2))))\n";
    let err = compile_error(missing);
    assert!(err.contains("no field `z`") && err.contains("it has x, y"), "{err}");

    let nofields = "fn f(n: Int) = n.x\n\n\
                    fn main() = println(int_to_str(f(1)))\n";
    assert!(compile_error(nofields).contains("`Int` has no fields"));
}

/// `{ p | x = 9 }` — everything `p` had, apart from these. It becomes a call
/// to the constructor: the named fields take the new values, and every other
/// one is read off the base. `p` itself does not change; nothing in Rill does.
#[test]
fn record_update_replaces_named_fields() {
    let src = "type Nokta\n  \
                 Nokta(x: Int, y: Int, ad: Str)\n\n\
               fn goster(p: Nokta) = \
                 int_to_str(p.x) + \",\" + int_to_str(p.y) + \",\" + p.ad\n\n\
               fn main() =\n  \
                 a = Nokta(1, 2, \"a\")\n  \
                 println(goster({ a | x = 9 }))\n  \
                 println(goster({ a | x = 9, ad = \"yeni\" }))\n  \
                 println(goster(a))\n  \
                 println(goster({ { a | x = 7 } | y = 8 }))\n";
    assert_eq!(run_source("update", src), "9,2,a\n9,2,yeni\n1,2,a\n7,8,a\n");
}

/// The base is read once per field it did not mention, so it is bound first:
/// `{ f(a) | x = 9 }` calls `f` once, not once for every field carried over.
#[test]
fn record_update_evaluates_its_base_once() {
    let src = "type Nokta\n  \
                 Nokta(x: Int, y: Int, z: Int)\n\n\
               fn once(n) =\n  \
                 println(\"read\")\n  \
                 Nokta(n, n, n)\n\n\
               fn main() =\n  \
                 p = { once(5) | y = 0 }\n  \
                 println(int_to_str(p.x) + int_to_str(p.y) + int_to_str(p.z))\n";
    assert_eq!(run_source("updonce", src), "read\n505\n");
}

/// A field name says which field only when there is one constructor for it to
/// belong to — the rule `.x` has, for the reason `.x` has it.
#[test]
fn record_update_needs_one_constructor_and_real_fields() {
    let two = "type Sekil\n  \
                 Daire(r: Int)\n  \
                 Kare(k: Int)\n\n\
               fn main() =\n  \
                 a = Daire(1)\n  \
                 println(int_to_str({ a | r = 9 }.r))\n";
    let err = compile_error(two);
    assert!(err.contains("more than one constructor"), "{err}");

    let missing = "type Nokta\n  \
                     Nokta(x: Int, y: Int)\n\n\
                   fn main() =\n  \
                     a = Nokta(1, 2)\n  \
                     println(int_to_str({ a | z = 9 }.x))\n";
    let err = compile_error(missing);
    assert!(err.contains("no field `z`") && err.contains("it has x, y"), "{err}");

    let twice = "type Nokta\n  \
                   Nokta(x: Int, y: Int)\n\n\
                 fn main() =\n  \
                   a = Nokta(1, 2)\n  \
                   println(int_to_str({ a | x = 9, x = 8 }.x))\n";
    let err = compile_error(twice);
    assert!(err.contains("set twice"), "{err}");
}

/// `whole @ Circle(r)` matches what follows the `@` and names all of it too.
/// Nests, unlike `|`, because it asks nothing of the arm.
#[test]
fn as_patterns_name_the_whole() {
    let src = "type Sekil\n  \
                 Daire(r: Float)\n  \
                 Kare(k: Float)\n\n\
               fn buyut(s) = match s\n  \
                 hepsi @ Daire(r) if r > 10.0 -> hepsi\n  \
                 Daire(r) -> Daire(r * 2.0)\n  \
                 hepsi @ Kare(_) -> hepsi\n\n\
               fn yan(s) = match s\n  \
                 Daire(r) -> r\n  \
                 Kare(k) -> k\n\n\
               fn inner(l) = match l\n  \
                 Cons(bir @ 1, _) -> int_to_str(bir)\n  \
                 Cons(n, _) -> \"baska \" + int_to_str(n)\n  \
                 Nil -> \"bos\"\n\n\
               fn main() =\n  \
                 println(float_to_str(yan(buyut(Daire(2.0)))))\n  \
                 println(float_to_str(yan(buyut(Daire(20.0)))))\n  \
                 println(float_to_str(yan(buyut(Kare(3.0)))))\n  \
                 println(inner(Cons(1, Nil)) + \" \" + inner(Cons(5, Nil)))\n";
    assert_eq!(run_source("aspat", src), "4\n20\n3\n1 baska 5\n");
}

/// A name is a binding and not a test, and everything that asks what a pattern
/// tests for has to read through the `@` to find out. Miss one and the arm
/// looks like a wildcard — which is what `head_of`'s `_ => None` says — and a
/// `match` that is not exhaustive compiles.
#[test]
fn as_patterns_are_transparent_to_exhaustiveness() {
    let src = "type Sekil\n  \
                 Daire(r: Float)\n  \
                 Kare(k: Float)\n\n\
               fn f(s) = match s\n  \
                 hepsi @ Daire(r) -> 1\n\n\
               fn main() = println(int_to_str(f(Kare(1.0))))\n";
    let err = compile_error(src);
    assert!(err.contains("not exhaustive") && err.contains("Kare"), "{err}");
}

/// The box an arm took apart can be built over — unless the arm is still
/// holding it, which is exactly what an `@` name does.
#[test]
fn as_pattern_keeps_the_box_alive() {
    let src = "fn topla(l, acc) = match l\n  \
                 Nil -> acc\n  \
                 hepsi @ Cons(h, t) -> topla(t, acc + h + len(hepsi))\n\n\
               fn main() = println(int_to_str(topla(Cons(1, Cons(2, Cons(3, Nil))), 0)))\n";
    assert_eq!(run_source("asreuse", src), "12\n");
}

/// Several patterns, one arm. `lift` turns it into one arm per alternative
/// before anything else sees it, so guards, exhaustiveness and reachability all
/// go on working on ordinary arms.
#[test]
fn or_patterns_take_any_alternative() {
    let src = "type Yon\n  \
                 Kuzey\n  \
                 Guney\n  \
                 Dogu\n  \
                 Bati\n\n\
               fn eksen(y) = match y\n  \
                 Kuzey | Guney -> \"dikey\"\n  \
                 Dogu | Bati -> \"yatay\"\n\n\
               fn az(n) = match n\n  \
                 0 | 1 | 2 -> \"az\"\n  \
                 _ -> \"cok\"\n\n\
               fn inner(o) = match o\n  \
                 Some(0) | Some(1) -> \"kucuk\"\n  \
                 Some(n) -> int_to_str(n)\n  \
                 None -> \"yok\"\n\n\
               fn guarded(n) = match n\n  \
                 1 | 2 if n > 1 -> \"iki\"\n  \
                 1 | 2 -> \"bir\"\n  \
                 _ -> \"baska\"\n\n\
               fn main() =\n  \
                 println(eksen(Kuzey) + \" \" + eksen(Bati))\n  \
                 println(az(1) + \" \" + az(9))\n  \
                 println(inner(Some(1)) + \" \" + inner(Some(7)) + \" \" + inner(None))\n  \
                 println(guarded(1) + \" \" + guarded(2) + \" \" + guarded(5))\n";
    assert_eq!(
        run_source("orpat", src),
        "dikey yatay\naz cok\nkucuk 7 yok\nbir iki baska\n"
    );
}

/// The body is one body and it is written once, so a name it uses has to mean
/// something whichever alternative was taken.
#[test]
fn or_pattern_alternatives_must_bind_alike() {
    let src = "fn f(o) = match o\n  \
                 Some(x) | None -> \"?\"\n  \
                 _ -> \"!\"\n\n\
               fn main() = println(f(None))\n";
    let err = compile_error(src);
    assert!(
        err.contains("must bind the same names") && err.contains("one binds x"),
        "{err}"
    );
}

/// Exhaustiveness counts the alternatives, not the arms: three directions and
/// an arm naming two of them still leaves one uncovered.
#[test]
fn or_patterns_still_have_to_be_exhaustive() {
    let src = "type Yon\n  \
                 Kuzey\n  \
                 Guney\n  \
                 Dogu\n\n\
               fn f(y) = match y\n  \
                 Kuzey | Guney -> \"a\"\n\n\
               fn main() = println(f(Kuzey))\n";
    let err = compile_error(src);
    assert!(err.contains("not exhaustive") && err.contains("Dogu"), "{err}");
}

/// A byte is not a character. `#s` counts bytes and stays counting bytes —
/// a lexer wants that, and so does the compiler that compiles the prelude —
/// so the characters are their own layer on top, and the point of it is that
/// cutting a string no longer lands in the middle of one.
#[test]
fn characters_are_not_bytes() {
    let src = "fn main() =\n  \
                 s = \"günaydın\"\n  \
                 println(#s)\n  \
                 println(char_count(s))\n  \
                 println(char_take(s, 2))\n  \
                 println(char_sub(s, 3, 2))\n  \
                 println(char_drop(s, 6))\n  \
                 println(from_chars(chars(s)) == s)\n";
    assert_eq!(run_source("chars", src), "10\n8\ngü\nay\nın\ntrue\n");
}

/// All four widths UTF-8 has, decoded and encoded again — and `from_char` is
/// the one that spells a codepoint properly, which `chr` does not: `chr` writes
/// a low byte, which is right for bytes and wrong for every character past 127.
#[test]
fn every_utf8_width_round_trips() {
    let src = "fn main() =\n  \
                 s = \"aβ✓🚀\"\n  \
                 println(#s)\n  \
                 println(char_count(s))\n  \
                 println(join(map(chars(s), \\c -> int_to_str(c)), \" \"))\n  \
                 println(from_char(97) + from_char(946) + from_char(10003) + from_char(128640))\n";
    assert_eq!(run_source("widths", src), "10\n4\n97 946 10003 128640\naβ✓🚀\n");
}

/// Malformed input is what arrives over a socket, and none of it may hang: a
/// string cut in the middle of a character — which is what byte slicing makes
/// — still counts, still decodes, and never reads past its own end.
#[test]
fn broken_utf8_terminates() {
    let src = "fn main() =\n  \
                 s = \"günaydın\"\n  \
                 broken = s[0:2]\n  \
                 println(char_count(broken))\n  \
                 println(char_at(broken, 1))\n  \
                 println(char_count(\"\"))\n  \
                 println(char_take(s, 99))\n  \
                 println(\"[\" + char_drop(s, 99) + \"]\")\n  \
                 println(\"[\" + char_take(s, 0 - 2) + \"]\")\n";
    assert_eq!(run_source("broken", src), "2\n195\n0\ngünaydın\n[]\n[]\n");
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
        .skip_while(|l| !(l.starts_with("define") && l.contains("@main.for")))
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

/// `str_find_from` starts partway along and clamps a start outside the
/// string; `split` is written on top of it, so it walks a line once instead
/// of copying what is left of it at every separator.
#[test]
fn finds_from_a_starting_point() {
    let src = "fn main() =\n  \
                 s = \"a,bb,,c\"\n  \
                 println(str_find_from(s, \",\", 0))\n  \
                 println(str_find_from(s, \",\", 2))\n  \
                 println(str_find_from(s, \",\", -7))\n  \
                 println(str_find_from(s, \",\", 100))\n  \
                 println(str_find_from(s, \"\", 3))\n  \
                 println(str_find_from(s, \"\", 100))\n  \
                 println(join(split(s, \",\"), \"|\"))\n  \
                 println(join(split(\",x,\", \",\"), \"|\"))\n  \
                 println(join(split(\"abc\", \"q\"), \"|\"))\n";
    assert_eq!(
        run_source("findfrom", src),
        "1\n4\n1\n-1\n3\n7\na|bb||c\n|x|\nabc\n"
    );
}

/// A line with many fields must cost time in proportion to its length, not to
/// its length times its field count: `split` used to copy the whole remainder
/// at every separator, which made a long line quadratic and overflowed the
/// stack besides.
#[test]
fn splitting_a_long_line_is_linear() {
    let src = "fn count(l, n) = match l\n  \
                 Nil -> n\n  \
                 Cons(_, t) -> count(t, n + 1)\n\
               \n\
               fn wide(i, acc) = if i == 0 then acc else wide(i - 1, acc + \" ab\")\n\
               \n\
               fn main() = println(count(split(wide(200000, \"\"), \" \"), 0))\n";
    assert_eq!(run_source("splitwide", src), "200001\n");
}

/// The prelude's list functions must cost constant stack, whatever the list.
/// Written the natural way they do not: `Cons(f(x), map(rest, f))` leaves
/// work after the call, so half a million elements used to be a segmentation
/// fault. They build backwards and turn the result round instead.
#[test]
fn list_functions_take_constant_stack() {
    let src = "fn count(l, n) = match l\n  \
                 Nil -> n\n  \
                 Cons(_, t) -> count(t, n + 1)\n\
               \n\
               fn main() =\n  \
                 n = 500000\n  \
                 println(len(range(1, n)))\n  \
                 println(count(map(range(1, n), \\x -> x + 1), 0))\n  \
                 println(count(filter(range(1, n), \\x -> x > 0), 0))\n  \
                 println(count(take(range(1, n), n), 0))\n  \
                 println(count(append(range(1, n), Nil), 0))\n  \
                 println(#join(map(range(1, n), \\x -> \"ab\"), \",\"))\n  \
                 println(#repeat(\"ab\", n))\n";
    assert_eq!(
        run_source("liststack", src),
        "500000\n500000\n500000\n500000\n500000\n1499999\n1000000\n"
    );
}

/// A local function that closes over nothing is an ordinary function once it
/// has been lifted, so it may be used at more than one type. One that closes
/// over something is checked with the function it was written in — that is
/// what lets a captured buffer keep its width without an annotation — and so
/// has one type. Both halves have to keep working.
#[test]
fn local_functions_generalize_unless_they_capture() {
    let src = "fn main() =\n                   fn id(x) = x\n\n                   v = buf_int(4)\n                   w = buf_f32(3)\n\n                   fn fill(i) =\n                       if i < 4 then\n                           v[i] = i * i\n                           fill(i + 1)\n                       else\n                           0\n\n                   fn fillf(i) =\n                       if i < 3 then\n                           w[i] = to_float(i) / 2.0\n                           fillf(i + 1)\n                       else\n                           0\n\n                   fill(0)\n                   fillf(0)\n                   println(id(1))\n                   println(id(\"a\"))\n                   println(v[0] + v[1] + v[2] + v[3])\n                   println(w[2])\n";
    assert_eq!(run_source("localpoly", src), "1\na\n14\n1\n");
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
        "lib",
        "examples",
        "examples/multi",
        "examples/http",
        "examples/json",
        "examples/yaml",
        "examples/gl",
        "examples/oxidb",
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
///
/// The last line is the two shapes the parser used to get wrong: a `-` alone
/// on its line became a mapping key named `-`, because `is_item` looked for
/// `"- "` in a line it had just trimmed the space off; and a key with no
/// value took the next line as its block whatever that line's indentation.
#[test]
fn yaml_module() {
    assert_eq!(
        compile_and_run("yaml/demo.rill"),
        "{\"name\":\"gateway\",\"port\":8080,\"debug\":false,\
         \"limits\":{\"memory\":512,\"cpu\":1.5},\"tags\":[\"fast\",\"small\"],\
         \"servers\":[{\"name\":\"alpha\",\"port\":1000},{\"name\":\"beta\",\"port\":2000}]}\n\
         gateway\n1.5\nfast+small\n2000\n\
         {\"rules\":[{\"name\":\"first\",\"on\":true},\
         {\"name\":\"second\",\"on\":false}],\"spare\":null,\"after\":1}\n"
    );
}

/// A JSON parser and printer written in Rill: nested lookup, escapes, and a
/// round trip that comes back identical — and then five documents that are
/// not documents, each of which used to come back as a value. `nonsense` in
/// particular parsed as `null`, because the literal was taken on its first
/// byte alone.
#[test]
fn json_module() {
    assert_eq!(
        compile_and_run("json/demo.rill"),
        "ada\n36\ntrue\nx+y\n\
         {\"user\":{\"name\":\"ada\",\"age\":36},\"tags\":[\"x\",\"y\"],\"ok\":true,\"pi\":3.5}\n\
         [1,\"a\\nb\",null,false,[2]]\n\
         [\"café\",\"😀\",\"\u{FFFD}x\"]\n\
         error: `null` expected, at byte 0\n\
         error: a value expected and the text ended, at byte 6\n\
         error: the text ended inside an array, at byte 4\n\
         error: a key expected, at byte 7\n\
         error: trailing text, at byte 6\n"
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
///
/// The program is written outside the project, so nothing above it holds a
/// `lib/` and `RILL_PATH` is the only way `sha1` can be found — which makes
/// this the test that the variable is read at all.
#[test]
fn sha1_and_base64() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-sha-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
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
        .env("RILL_PATH", root.join("lib"))
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

/// The OxiDB document client against a stand-in server on a strand. This is
/// the test that was missing when the JSON module's `parse` grew a `Result`:
/// nothing compiled `examples/oxidb/` at all, so a caller left behind went
/// unnoticed.
#[test]
fn oxidb_document_client() {
    assert_eq!(
        compile_and_run("oxidb/demo.rill"),
        "ping: pong\ninsert: 1\ninsert_many: 2,3\ncount: 3\n\
         find: ada,grace\nupdate: 1\ndelete: 1\n"
    );
}

/// The OxiWire codec — OxiDB's binary wire — encoded and decoded by Rill
/// against itself. Every value goes out as JSON, round-trips through the
/// binary form, and comes back the same text; the last two lines are reply
/// frames as the server writes them, status byte and all.
#[test]
fn oxidb_tsdb_codec() {
    assert_eq!(
        compile_and_run("oxidb/tsdemo.rill"),
        "null out: null\nnull back: null\n\
         bools out: [true,false]\nbools back: [true,false]\n\
         ints out: [0,-1,1755000000000]\nints back: [0,-1,1755000000000]\n\
         floats out: [0.93,-273.15]\nfloats back: [0.93,-273.15]\n\
         empties out: [[],{},\"\"]\nempties back: [[],{},\"\"]\n\
         a point out: {POINT}\na point back: {POINT}\n\
         a request out: {REQUEST}\na request back: {REQUEST}\n\
         an answer: ok {\"written\":3}\n\
         a complaint: err unknown tsdb op: \"nope\"\n"
            .replace(
                "{POINT}",
                "{\"measurement\":\"cpu\",\"tags\":{\"host\":\"a\",\"region\":\"eu\"},\
                 \"fields\":{\"usage\":0.93,\"cores\":8},\"ts\":1755000000000}"
            )
            .replace(
                "{REQUEST}",
                "{\"engine\":\"tsdb\",\"cmd\":\"tsdb\",\"op\":\"write\",\"points\":[\
                 {\"measurement\":\"cpu\",\"tags\":{\"host\":\"a\",\"region\":\"eu\"},\
                 \"fields\":{\"usage\":0.93,\"cores\":8},\"ts\":1755000000000}]}"
            )
    );
}

/// The OxiDB SQL client against a stand-in on a strand. An answer is a list
/// of results, one per statement sent, and each is one of three shapes — DDL,
/// a count of rows affected, or columns and rows. `both` sends two statements
/// in one request and gets two results back, which is why the list is a list
/// even for callers who only ever send one.
#[test]
fn oxidb_sql_client() {
    assert_eq!(
        compile_and_run("oxidb/sqldemo.rill"),
        "ddl: ddl\n\
         insert: affected 1\n\
         select: id,name,score (INT,TEXT,DOUBLE) [1 \"ada\" 9.5 <- ada]\n\
         both: affected 1 | n (INT) [1]\n\
         no such table: error: sql error: no such table: nope\n"
    );
}

/// The OxiDB recommendation client against a stand-in on a strand. The
/// stand-in's answers are the ones a real engine gave for this basket data,
/// compared against it byte for byte — including that a query as of a moment
/// long before the baskets were tracked answers with an empty list rather
/// than an error, scores being decayed by age.
#[test]
fn oxidb_rec_client() {
    assert_eq!(
        compile_and_run("oxidb/recdemo.rill"),
        "track: {\"counted\":true}\n\
         track: {\"counted\":true}\n\
         track: {\"counted\":true}\n\
         track: {\"counted\":true}\n\
         track: {\"counted\":true}\n\
         related: ekmek 2.23144, sut 2.23144, poset 0.505343, peynir 0.505343\n\
         for basket: ekmek 2.23144, sut 2.23144, peynir 0.505343\n\
         as of long ago: nothing\n\
         stats: 5 baskets, 14 pairs\n\
         checkpoint: {\"ok\":true}\n\
         nonsense: error: unknown rec op: \"nope\" \
         (track, related, for_basket, stats, checkpoint)\n"
    );
}

/// RESP — the protocol Redis speaks — encoded and decoded by Rill, with no
/// server in reach. Three halves worth the test are ones a length-prefixed
/// wire never needs: a reply that has only half arrived must read as "not
/// yet" rather than as malformed, a bulk string must carry any byte including
/// a NUL, and two pushed messages landing in one read must both survive —
/// the second is not going to be sent again.
#[test]
fn oxidb_resp_protocol() {
    assert_eq!(
        compile_and_run("oxidb/kvdemo.rill"),
        "PING out: *1\\r\\n$4\\r\\nPING\\r\\n\n\
         SET out: *3\\r\\n$3\\r\\nSET\\r\\n$6\\r\\nuser:1\\r\\n$3\\r\\nada\\r\\n\n\
         a value with a NUL in it out: \
         *3\\r\\n$3\\r\\nSET\\r\\n$3\\r\\nraw\\r\\n$3\\r\\na\\0b\\r\\n\n\
         simple: OK (5 bytes)\n\
         error: error: ERR no such key (18 bytes)\n\
         integer: 42 (5 bytes)\n\
         bulk: \"ada\" (9 bytes)\n\
         empty bulk: \"\" (6 bytes)\n\
         a NUL inside a bulk: \"a\\0b\" (9 bytes)\n\
         no value: nil (5 bytes)\n\
         no array: nil-array (5 bytes)\n\
         array: [\"role\", 7, nil] (23 bytes)\n\
         nested: [[a, b], 1] (20 bytes)\n\
         a bulk arriving in pieces (first half): not yet\n\
         a bulk arriving in pieces (both): \"ada\" (9 bytes)\n\
         an array arriving in pieces (first half): not yet\n\
         an array arriving in pieces (both): [\"ada\", \"grace\"] (24 bytes)\n\
         not RESP at all: bad: no RESP value begins with `h`, at byte 0\n\
         a subscribe note: subscribe news, now holding 1\n\
         a message: message on news: hello\n\
         a pattern message: message matching news.* on news.home: hi\n\
         two in one read: message on news: one\n\
         \x20 and then: message on news: two\n"
    );
}

/// A hundred thousand documents in OxiDB behind an HTTP server. It needs a
/// running `oxidb-server`, which a test cannot assume, so this checks that it
/// builds — the two libraries it joins are exercised against stand-ins by the
/// tests above and against a real server by hand.
#[test]
fn catalogue_example_builds() {
    compile_only("oxidb/catalogue.rill");
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

/// `sleep_ms` parks the strand and not the worker, so a thousand of them can
/// be waiting at once and the whole program still takes as long as the longest
/// one. A worker held by a sleep would make this the sum instead, which at
/// these numbers is half a minute.
///
/// The upper bound is loose on purpose: this asserts that the waits overlap,
/// not what the operating system's timer slack is.
#[test]
fn sleeping_strands_do_not_hold_a_worker() {
    let out = run_source(
        "sleepers",
        "fn sleeper(ch, ms) =\n  \
           sleep_ms(ms)\n  \
           send(ch, 1)\n\
         \n\
         fn start_all(ch, i, n) = if i == n then 0 else start_one(ch, i, n)\n\
         \n\
         fn start_one(ch, i, n) =\n  \
           spawn sleeper(ch, 20 + i % 30)\n  \
           start_all(ch, i + 1, n)\n\
         \n\
         fn gather(ch, left, acc) =\n  \
           n = recv(ch)\n  \
           if left == 1 then acc + n else gather(ch, left - 1, acc + n)\n\
         \n\
         fn main() =\n  \
           ch = channel()\n  \
           t0 = now_nanos()\n  \
           start_all(ch, 0, 1000)\n  \
           woke = gather(ch, 1000, 0)\n  \
           ms = (now_nanos() - t0) / 1000000\n  \
           println(int_to_str(woke) + \" woke\")\n  \
           println(if ms < 2000 then \"overlapped\" else \"serialised: \" + int_to_str(ms) + \" ms\")\n",
    );
    assert_eq!(out, "1000 woke\noverlapped\n");
}

/// A program can say how much memory it is holding — the live resident set,
/// not the peak, so a server can report its own weight while it runs. The
/// test takes eighty megabytes and insists the number follows, which a peak
/// or a constant would not do.
#[test]
fn a_program_can_say_what_it_weighs() {
    let out = run_source(
        "own_weight",
        "fn hold(sb: StrBuf, left) =\n  \
           if left <= 0 then 0\n  \
           else\n    \
             strbuf_add(sb, \"0123456789\")\n    \
             hold(sb, left - 1)\n\
         \n\
         fn mb() = 1048576\n\
         \n\
         fn main() =\n  \
           at_rest = rss_bytes()\n  \
           sb = strbuf()\n  \
           hold(sb, 8000000)\n  \
           held = rss_bytes()\n  \
           println(if at_rest > 0 then \"has a weight\" else \"nothing at rest\")\n  \
           println(if held - at_rest > 60 * mb() then \"and it grew\" else \"it did not grow: \" + int_to_str((held - at_rest) / mb()) + \" MB\")\n  \
           println(int_to_str(str_len(strbuf_str(sb)) / mb()) + \" MB of it asked for\")\n",
    );
    assert_eq!(out, "has a weight\nand it grew\n76 MB of it asked for\n");
}

/// And the wait is the length that was asked for, within the slack any
/// operating system takes on a timer.
#[test]
fn sleep_ms_waits_about_that_long() {
    let out = run_source(
        "sleep_length",
        "fn main() =\n  \
           t0 = now_nanos()\n  \
           sleep_ms(200)\n  \
           ms = (now_nanos() - t0) / 1000000\n  \
           println(if ms >= 195 && ms < 320 then \"about 200 ms\" else int_to_str(ms) + \" ms\")\n",
    );
    assert_eq!(out, "about 200 ms\n");
}

/// wasm has no scheduler to park in, so a sleep there would be a program
/// silently running at the wrong speed. It is refused where the sockets and
/// the strands are.
#[test]
fn wasm_refuses_sleep_ms() {
    let err = compile_error_for_wasm("fn main() = sleep_ms(10)\n");
    assert!(err.contains("`sleep_ms` needs the scheduler"), "{err}");
}

/// Nagle can be turned off, and the answer says whether the kernel took it.
///
/// The delay itself cannot be shown here — over loopback the acknowledgement
/// comes back before the next write is even built, which is exactly why a
/// server that looks fine on one machine can be a tenth of a second slow
/// across the sea. What is pinned is that the option reaches the socket on
/// both platforms and that a descriptor which is not a socket is refused
/// rather than reported as success.
#[test]
fn a_socket_can_have_nagle_turned_off() {
    let out = run_source(
        "sock_nodelay",
        "fn serve(conn) =\n  \
           println(\"server took it: \" + int_to_str(sock_nodelay(conn)))\n  \
           sock_write(conn, sock_read(conn))\n  \
           sock_close(conn)\n\
         \n\
         fn accept_loop(server) =\n  \
           conn = tcp_accept(server)\n  \
           spawn serve(conn)\n  \
           sock_close(server)\n\
         \n\
         fn talker(port) = match connect_to(\"127.0.0.1\", port)\n  \
           Err(e) -> println(e)\n  \
           Ok(fd) -> say(fd)\n\
         \n\
         fn say(fd) =\n  \
           println(\"client took it: \" + int_to_str(sock_nodelay(fd)))\n  \
           sock_write(fd, \"ping\")\n  \
           println(\"echo [\" + sock_read(fd) + \"]\")\n  \
           sock_close(fd)\n\
         \n\
         fn main() = match listen_on(8098)\n  \
           Err(e) -> println(e)\n  \
           Ok(server) -> run(server)\n\
         \n\
         fn run(server) =\n  \
           spawn accept_loop(server)\n  \
           sleep_ms(100)\n  \
           println(\"not a socket: \" + int_to_str(sock_nodelay(0 - 1)))\n  \
           talker(8098)\n  \
           sleep_ms(200)\n",
    );
    assert_eq!(
        out,
        "not a socket: 0\nclient took it: 1\nserver took it: 1\necho [ping]\n"
    );
}

/// A read can be given a deadline, so a connection that says nothing is not a
/// connection held for ever.
#[test]
fn a_socket_read_can_be_given_a_deadline() {
    let out = run_source(
        "sock_wait",
        "fn serve(conn) =\n  \
           if sock_wait(conn, 300) then answer(conn) else refuse(conn)\n\
         \n\
         fn answer(conn) =\n  \
           msg = sock_read(conn)\n  \
           sock_write(conn, \"heard: \" + msg)\n  \
           sock_close(conn)\n\
         \n\
         fn refuse(conn) =\n  \
           println(\"nothing said, closing\")\n  \
           sock_close(conn)\n\
         \n\
         fn accept_loop(server, n) =\n  \
           conn = tcp_accept(server)\n  \
           spawn serve(conn)\n  \
           if n == 1 then sock_close(server) else accept_loop(server, n - 1)\n\
         \n\
         fn talker(port) = match connect_to(\"127.0.0.1\", port)\n  \
           Err(e) -> println(e)\n  \
           Ok(fd) -> say(fd)\n\
         \n\
         fn say(fd) =\n  \
           sock_write(fd, \"hello\")\n  \
           println(sock_read(fd))\n  \
           sock_close(fd)\n\
         \n\
         fn silent(port) = match connect_to(\"127.0.0.1\", port)\n  \
           Err(e) -> println(e)\n  \
           Ok(fd) -> hush(fd)\n\
         \n\
         fn hush(fd) =\n  \
           sleep_ms(600)\n  \
           println(\"the silent one read [\" + sock_read(fd) + \"]\")\n  \
           sock_close(fd)\n\
         \n\
         fn main() = match listen_on(8096)\n  \
           Err(e) -> println(e)\n  \
           Ok(server) -> run(server)\n\
         \n\
         fn run(server) =\n  \
           spawn accept_loop(server, 2)\n  \
           spawn silent(8096)\n  \
           sleep_ms(100)\n  \
           talker(8096)\n  \
           sleep_ms(700)\n",
    );
    assert_eq!(out, "heard: hello\nnothing said, closing\nthe silent one read []\n");
}

/// A `select` in a loop must leave the loop a loop.
///
/// Its case table and its result slot are `alloca`s, and an `alloca` outside
/// the entry block is a real adjustment of the stack pointer every time it is
/// reached — so a tail call that should have been a jump grew the stack by a
/// hundred and sixty bytes a turn, and a server built on `select` ran off the
/// bottom of its stack after a couple of hundred thousand messages. It died
/// with a bus error somewhere else entirely, wherever the code happened to be.
///
/// A million and a half turns is seven times the old ceiling and takes a third
/// of a second.
#[test]
fn a_select_loop_runs_in_constant_stack() {
    let out = run_source(
        "select_loop",
        "fn spam(ch) =\n  \
           send(ch, 1)\n  \
           spam(ch)\n\
         \n\
         fn drain(a, n) =\n  \
           x = select\n    \
             v = recv(a) -> v\n  \
           if n == 0 then done(n) else drain(a, n - 1)\n\
         \n\
         fn done(n) = println(\"the loop held\")\n\
         \n\
         fn main() =\n  \
           a = channel()\n  \
           spawn spam(a)\n  \
           drain(a, 1500000)\n",
    );
    assert_eq!(out, "the loop held\n");
}

// The valley used to be checked from here — `webgpu/world/check.rill` played a
// round of it and `server.rill` was compiled for a Linux box. It lives in its
// own repository now, and runs both against whichever compiler it is pointed
// at; there is nothing left in this tree for these tests to open.

/// Calls into libm and libc, including the string bridge and a null check.
#[test]
fn foreign_functions() {
    assert_eq!(
        compile_and_run("ffi.rill"),
        "1.41421\n1024\n7\ntrue\n7\n<unset>\n"
    );
}

/// Reading a field by name: a single-constructor type, a field of a field, a
/// field of what a call answered with — and the value the first one was, left
/// as it was, since building a changed record is what changing one is.
#[test]
fn record_fields() {
    assert_eq!(
        compile_and_run("records.rill"),
        "ada at 0,0 hp 100\n65\n100\nada at 3,4 hp 65\n5\n9\ntrue\ntile\ncoin\n"
    );
}

/// A field every constructor has, of one type, is certainly there whichever
/// the value is, so it reads with the dot and updates with `{ | }` — each
/// becoming the `match` a program would have written. The positions may
/// differ between constructors, and the type may be generic.
#[test]
fn a_field_every_constructor_has_reads_without_a_match() {
    let src = "type Sekil\n  \
                 Daire(r: Float, ad: Str)\n  \
                 Kare(k: Float, ad: Str)\n\n\
               type Kutu(a)\n  \
                 Sol(v: a, n: Int)\n  \
                 Sag(n: Int, v: a)\n\n\
               fn ad(s) = s.ad\n\n\
               fn adla(s, a) = { s | ad = a }\n\n\
               fn yap(a) =\n  \
                 println(\"built\")\n  \
                 Kare(2.0, a)\n\n\
               fn main() =\n  \
                 println(ad(Daire(1.0, \"daire\")))\n  \
                 println(ad(adla(Kare(2.0, \"kare\"), \"kutu\")))\n  \
                 println({ yap(\"a\") | ad = \"b\" }.ad)\n  \
                 println(Sol(\"s\", 1).v)\n  \
                 println(show({ Sag(2, 3.5) | n = 9 }))\n";
    assert_eq!(
        run_source("common", src),
        "daire\nkutu\nbuilt\nb\ns\nSag(9, 3.5)\n"
    );
}

/// A whole signature with type variables in it declares the scheme up front,
/// so a use inside the function's own dependency group can take it at any
/// type: a group-mate used at two types, and a function calling itself at
/// another. The body is held to the declaration.
#[test]
fn a_declared_signature_is_taken_at_any_type_in_its_own_group() {
    let mates = "fn f(n) = if n == 0 then 0 else g(1, n) + #g(\"s\", n)\n\n\
                 fn g(x: a, n: Int) -> a =\n  \
                   h = f(n - 1)\n  \
                   x\n\n\
                 fn main() = println(int_to_str(f(3)))\n";
    assert_eq!(run_source("polyrec", mates), "2\n");

    let itself = "fn twice(x: a, n: Int) -> a = if n == 0 then x else twice(x, n - 1)\n\n\
                  fn main() =\n  \
                    println(int_to_str(twice(3, 2)))\n  \
                    println(twice(\"s\", 2))\n";
    assert_eq!(run_source("polyself", itself), "3\ns\n");

    // Without the signature the group shares one type, as it always did.
    let bare = "fn f(n) = if n == 0 then 0 else g(1, n) + #g(\"s\", n)\n\n\
                fn g(x, n) =\n  \
                  h = f(n - 1)\n  \
                  x\n\n\
                fn main() = println(int_to_str(f(3)))\n";
    assert!(compile_error(bare).contains("expected Int, found Str"));

    let pinned = "fn f(x: a) -> a = x + 1\n\nfn main() = println(int_to_str(f(1)))\n";
    let err = compile_error(pinned);
    assert!(
        err.contains("`f` is declared `('a) -> 'a`, but its body only works as `(Int) -> Int`"),
        "{err}"
    );

    // A type variable in a partial annotation: the same name is the same type.
    let same = "fn same(x: a, y: a) = x\n\nfn main() = println(int_to_str(same(1, \"s\")))\n";
    assert!(compile_error(same).contains("expected Int, found Str"));
}

/// One copy is made per type, so types that grow without end are stopped:
/// a self-call at a bigger type each time in codegen, and a type holding
/// itself at a bigger type where it is written.
#[test]
fn types_that_grow_without_end_are_refused() {
    let growing = "fn f(x: a, n: Int) -> Int = if n == 0 then 0 else f(Cons(x, Nil), n - 1)\n\n\
                   fn main() = println(int_to_str(f(1, 3)))\n";
    let err = compile_error(growing);
    assert!(err.contains("`f` is being specialized at ever bigger types"), "{err}");

    let nested = "type Nest(a)\n  \
                    Leaf(a)\n  \
                    Deep(Nest((a, a)))\n\n\
                  fn depth(n: Nest(a)) -> Int = match n\n  \
                    Leaf(_) -> 0\n  \
                    Deep(inner) -> 1 + depth(inner)\n\n\
                  fn main() = println(int_to_str(depth(Leaf(1))))\n";
    let err = compile_error(nested);
    assert!(err.contains("`Nest` holds a `Nest(('a, 'a))` in its own definition"), "{err}");
}

/// A name may be bound again — `x = x + 1` reads the old one to bind a new
/// one — and what a block binds is the block's: a name bound in one branch
/// of an `if` is not there after it, and a rebinding inside a branch, or an
/// arm, shadows only inside it, whatever type it gives the name.
#[test]
fn a_branch_binding_stays_in_its_branch() {
    let src = "fn main() =\n  \
                 x = 1\n  \
                 x = x + 1\n  \
                 f = \\y -> x + y\n  \
                 x = 100\n  \
                 y = if x > 50 then\n    \
                   x = \"big\"\n    \
                   #x\n  \
                 else 0\n  \
                 z = match x\n    \
                   100 ->\n      \
                     x = 7\n      \
                     x\n    \
                   _ -> 0\n  \
                 println(int_to_str(x))\n  \
                 println(int_to_str(y))\n  \
                 println(int_to_str(z))\n  \
                 println(int_to_str(f(1)))\n";
    assert_eq!(run_source("rebind", src), "100\n3\n7\n3\n");

    let leaked = "fn main() =\n  \
                    y = if true then\n    \
                      z = 5\n    \
                      z\n  \
                    else 0\n  \
                    println(int_to_str(z))\n";
    assert!(compile_error(leaked).contains("unknown variable `z`"));
}

/// A lambda's body may be an indented block after the `->`, and the
/// formatter writes it back the same way.
#[test]
fn a_lambda_may_have_a_block_body() {
    let src = "fn main() =\n  \
                 x = 1\n  \
                 f = \\y ->\n    \
                   x = x + y\n    \
                   x * 2\n  \
                 println(int_to_str(f(5)))\n  \
                 println(int_to_str(x))\n";
    assert_eq!(run_source("lamblock", src), "12\n1\n");
    assert_eq!(fmt_source(src), src);
}

/// Where the author broke a line, `rill fmt` keeps the break: before an
/// operator, a `|>`, a `then` or `else`, and between a call's arguments —
/// nested ones a level in again — with a trailing comment on the first line.
#[test]
fn fmt_keeps_the_breaks_the_author_wrote() {
    let src = "fn main() =\n  \
                 xs = Cons(1, Cons(2, Nil))\n  \
                 n = xs  # tens\n    \
                   |> map(\\x -> x * 10)\n    \
                   |> sum\n  \
                 ok = n > 10\n    \
                   && n < 100\n    \
                   || n == 0\n  \
                 msg = \"a\"\n    \
                   + \"b\"\n  \
                 p = pair(n,\n    \
                   describe(msg,\n      \
                     ok))\n  \
                 q = if ok\n    \
                   then \"yes\"\n    \
                   else \"no\"\n  \
                 println(p + \" \" + q)\n\n\
               fn pair(a, b) = int_to_str(a) + b\n\n\
               fn describe(m, o) = m + (if o then \"!\" else \"?\")\n";
    assert_eq!(fmt_source(src), src);
    assert_eq!(run_source("breaks", src), "30ab! yes\n");

    // And still folds what was written on one line.
    let flat = "fn main() = println(int_to_str(1 + 2))\n";
    assert_eq!(fmt_source(flat), flat);
}

/// `?` on an `Option`, in a function that answers one: the value inside the
/// `Some`, or else `None`. The crossings are written: `ok_or` gives an
/// absence its error, `map_err` rewrites one before the `?` carries it, and
/// an `Option` in a function that answers a `Result` is told to say which.
#[test]
fn question_mark_takes_the_value_out_of_an_option() {
    let src = "fn find_pos(xs, n) = match xs\n  \
                 Nil -> None\n  \
                 Cons(x, rest) -> if x == n then Some(0) else opt_map(find_pos(rest, n), \\i -> i + 1)\n\n\
               fn both(xs) =\n  \
                 a = find_pos(xs, 2)?\n  \
                 b = find_pos(xs, 3)?\n  \
                 Some(a + b)\n\n\
               fn check(n) = if n > 1 then Ok(n * 10) else Err(\"small\")\n\n\
               fn named(xs) =\n  \
                 p = ok_or(find_pos(xs, 3), \"no 3\")?\n  \
                 q = map_err(check(p), \\e -> \"check: \" + e)?\n  \
                 Ok(q)\n\n\
               fn main() =\n  \
                 xs = Cons(1, Cons(2, Cons(3, Nil)))\n  \
                 println(show(both(xs)))\n  \
                 println(show(both(Cons(2, Nil))))\n  \
                 println(show(named(xs)))\n  \
                 println(show(named(Cons(3, Nil))))\n  \
                 println(show(named(Nil)))\n";
    assert_eq!(
        run_source("optry", src),
        "Some(3)\nNone\nOk(20)\nErr(check: small)\nErr(no 3)\n"
    );

    let crossing = "fn f(o: Option(Int)) =\n  \
                      x = o?\n  \
                      Ok(x + 1)\n\n\
                    fn main() = println(show(f(Some(1))))\n";
    let err = compile_error(crossing);
    assert!(err.contains("`?` on an `Option` leaves the function with `None`") && err.contains("ok_or(o, e)?"), "{err}");

    let neither = "fn f(n: Int) =\n  \
                     x = n?\n  \
                     Some(x)\n\n\
                   fn main() = println(show(f(1)))\n";
    assert!(compile_error(neither).contains("`Result` or an `Option`, and this is `Int`"));
}

/// An error of another layer's kind crosses at the `?`: where the function
/// answers `Result(_, AppErr)`, an `IoErr` goes out as `Err(Io(e))`, through
/// the one constructor that takes it. Two that take it, or none, is an error
/// that asks for `map_err`; a function whose return type is not written gets
/// the error type its block settled on, and is told so.
#[test]
fn question_mark_wraps_an_error_in_the_constructor_that_takes_it() {
    let head = "type IoErr\n  \
                  NotFound(Str)\n\n\
                type ParseErr\n  \
                  Bad(Int)\n\n\
                type AppErr\n  \
                  Io(IoErr)\n  \
                  Parse(ParseErr)\n  \
                  Other(Str)\n\n\
                fn read(p) = if p == \"ok\" then Ok(\"42\") else Err(NotFound(p))\n\n\
                fn parse(s) = if s == \"42\" then Ok(42) else Err(Bad(#s))\n\n";
    let layered = format!(
        "{head}fn load(p) -> Result(Int, AppErr) =\n  \
           s = read(p)?\n  \
           n = parse(s)?\n  \
           Ok(n + 1)\n\n\
         fn settled(p) =\n  \
           s = read(p)?\n  \
           if #s > 0 then Ok(s) else Err(Other(\"empty\"))\n\n\
         fn main() =\n  \
           println(show(load(\"ok\")))\n  \
           println(show(load(\"nope\")))\n  \
           println(show(settled(\"zz\")))\n"
    );
    assert_eq!(run_source("wraperr", &layered), "Ok(43)\nErr(Io(NotFound(nope)))\nErr(Io(NotFound(zz)))\n");

    let none = format!(
        "{head}fn load(p) -> Result(Str, ParseErr) =\n  s = read(p)?\n  Ok(s)\n\nfn main() = println(1)\n"
    );
    let err = compile_error(&none);
    assert!(err.contains("no constructor of `ParseErr` takes one") && err.contains("map_err"), "{err}");

    let two = format!(
        "{head}type Two\n  A(IoErr)\n  B(IoErr)\n\n\
         fn load(p) -> Result(Str, Two) =\n  s = read(p)?\n  Ok(s)\n\nfn main() = println(1)\n"
    );
    assert!(compile_error(&two).contains("`A`, `B` all take one"));

    let unwritten = format!(
        "{head}fn load(p) =\n  s = read(p)?\n  n = parse(s)?\n  Ok(n)\n\nfn main() = println(1)\n"
    );
    assert!(compile_error(&unwritten).contains("the function's return type is not written"));
}

/// Integer arithmetic at the edges is decided, not left to the machine:
/// `+ - *` wrap, the smallest integer over `-1` wraps to itself with no
/// remainder, `to_int` saturates and gives zero for a NaN, a shift by 64 or
/// more or by a negative count is zero — and a division by zero ends the
/// program, saying where.
#[test]
fn integer_arithmetic_is_decided_at_the_edges() {
    let src = "fn least() = -9223372036854775807 - 1\n\n\
               fn over(a, b) = a / b\n\n\
               fn rem(a, b) = a % b\n\n\
               fn main() =\n  \
                 println(int_to_str(9223372036854775807 + 1))\n  \
                 println(int_to_str(least() * 2))\n  \
                 println(int_to_str(over(least(), -1)))\n  \
                 println(int_to_str(rem(least(), -1)))\n  \
                 println(int_to_str(over(-7, 2)))\n  \
                 println(int_to_str(rem(-7, 2)))\n  \
                 println(int_to_str(to_int(1.0e30)))\n  \
                 println(int_to_str(to_int(-1.0e30)))\n  \
                 println(int_to_str(to_int(0.0 / 0.0)))\n  \
                 println(int_to_str(to_int(-2.7)))\n  \
                 println(int_to_str(shl(1, 64)))\n  \
                 println(int_to_str(shr(1, -1)))\n  \
                 println(int_to_str(shl(3, 62)))\n  \
                 println(int_to_str(shr(-1, 60)))\n";
    assert_eq!(
        run_source("intedges", src),
        "-9223372036854775808\n0\n-9223372036854775808\n0\n-3\n-1\n9223372036854775807\n\
         -9223372036854775808\n0\n-2\n0\n0\n-4611686018427387904\n15\n"
    );

    let dies = "fn over(a, b) = a / b\n\nfn main() = println(int_to_str(over(7, 0)))\n";
    let err = run_source_dies("divzero", dies);
    assert!(err.contains("rill: division by zero"), "{err}");
    let dies = "fn rem(a, b) = a % b\n\nfn main() = println(int_to_str(rem(7, 0)))\n";
    assert!(run_source_dies("remzero", dies).contains("rill: division by zero"));
}

/// An object of a trait: `Speak(x)` is a record of the trait's methods as
/// closures over `x`, typed by the trait's name, so a list may hold a dog
/// and a robot and `s.speak()` calls whichever it is. Underneath: a
/// function's type in an annotation, a field that holds a function called
/// by name, and `\ -> e`. A trait whose method takes two of the type
/// cannot be one, and says so.
#[test]
fn an_object_of_a_trait_is_a_record_of_closures() {
    let src = "trait Speak(a)\n  \
                 speak(a) -> Str\n  \
                 greet(a, Str) -> Str\n\n\
               type Dog\n  \
                 Dog(n: Str)\n\n\
               type Robot\n  \
                 Robot(id: Int)\n\n\
               impl Speak(Dog)\n  \
                 fn speak(d) = \"woof from \" + d.n\n  \
                 fn greet(d, who) = d.n + \" licks \" + who\n\n\
               impl Speak(Robot)\n  \
                 fn speak(r) = \"beep \" + int_to_str(r.id)\n  \
                 fn greet(r, who) = \"unit \" + int_to_str(r.id) + \" greets \" + who\n\n\
               type Handler\n  \
                 Handler(name: Str, run: () -> Str, times: (Int) -> Str)\n\n\
               fn say(xs: List(Speak)) = match xs\n  \
                 Nil -> 0\n  \
                 Cons(s, rest) ->\n    \
                   println(s.speak() + \"; \" + s.greet(\"ada\"))\n    \
                   say(rest)\n\n\
               fn main() =\n  \
                 say(Cons(Speak(Dog(\"rex\")), Cons(Speak(Robot(7)), Nil)))\n  \
                 h = Handler(\"h\", \\ -> \"ran\", \\n -> repeat(\"ho\", n))\n  \
                 println(h.name + \" \" + h.run() + \" \" + h.times(2))\n  \
                 f = h.run\n  \
                 println(f())\n";
    assert_eq!(
        run_source("objects", src),
        "woof from rex; rex licks ada\nbeep 7; unit 7 greets ada\nh ran hoho\nran\n"
    );
    assert_eq!(fmt_source(src), src);

    let two = "trait Same(a)\n  \
                 same(a, a) -> Bool\n\n\
               type T\n  \
                 T(n: Int)\n\n\
               impl Same(T)\n  \
                 fn same(a, b) = a.n == b.n\n\n\
               fn main() = println(int_to_str(1 + Same(T(1)).n))\n";
    let err = compile_error(two);
    assert!(err.contains("`Same` cannot be the type of an object") && err.contains("`same` takes a second one of the type"), "{err}");

    let c = "extern \"c\" fn qsort(base: Ptr, n: I64, w: I64, cmp: (Ptr, Ptr) -> I32)\n\nfn main() = println(\"x\")\n";
    assert!(compile_error(c).contains("a callback C calls is written `Fn(I32) -> I32`"));
}

/// A type alias is a name for a type, read as what it names everywhere an
/// annotation can be — a field, a parameter, a result, a loop's carried
/// name — with parameters put in; one that names itself is refused.
#[test]
fn a_type_alias_is_a_name_for_a_type() {
    let src = "type Meters = Float\n\n\
               type Pair(a) = (a, a)\n\n\
               type Span = Pair(Meters)\n\n\
               type Leg\n  \
                 Leg(from: Meters, to: Meters)\n\n\
               fn length(l: Leg) -> Meters = l.to - l.from\n\n\
               fn ends(s: Span) -> Meters = match s\n  \
                 (a, b) -> a + b\n\n\
               fn main() =\n  \
                 println(show(length(Leg(1.5, 4.0))))\n  \
                 println(show(ends((1.0, 2.0))))\n  \
                 n = while n < 3.0 with n: Meters = 0.0\n    \
                   n + 1.0\n  \
                 println(show(n))\n";
    assert_eq!(run_source("alias", src), "2.5\n3\n3\n");
    assert_eq!(fmt_source(src), src);

    let loops = "type A = B\n\ntype B = A\n\nfn f(x: A) = x\n\nfn main() = println(1)\n";
    assert!(compile_error(loops).contains("names itself"));
    let arity = "type Pair(a) = (a, a)\n\nfn f(x: Pair) = x\n\nfn main() = println(1)\n";
    assert!(compile_error(arity).contains("takes 1 type argument(s), but 0 given"));
}

/// Two traits may have a method of one name. A call by the bare name is
/// settled by the type it is called on; where both traits are implemented
/// for it, or the type is not known yet, the call says which trait.
#[test]
fn a_method_name_belongs_to_its_trait() {
    let head = "trait Describe(a)\n  \
                  name(a) -> Str\n\n\
                trait Label(a)\n  \
                  name(a) -> Str\n  \
                  short(a) -> Str\n\n\
                type Dog\n  \
                  Dog(n: Str)\n\n\
                type Tag\n  \
                  Tag(t: Str)\n\n\
                impl Describe(Dog)\n  \
                  fn name(d) = \"dog \" + d.n\n\n\
                impl Label(Dog)\n  \
                  fn name(d) = \"label \" + d.n\n  \
                  fn short(d) = d.n\n\n\
                impl Label(Tag)\n  \
                  fn name(t) = \"tag \" + t.t\n  \
                  fn short(t) = t.t\n\n";
    let src = format!(
        "{head}fn main() =\n  \
           println(name(Tag(\"x\")))\n  \
           println(Describe.name(Dog(\"rex\")))\n  \
           println(Label.name(Dog(\"rex\")))\n  \
           println(short(Dog(\"rex\")))\n"
    );
    assert_eq!(run_source("methods", &src), "tag x\ndog rex\nlabel rex\nrex\n");

    let both = format!("{head}fn main() = println(name(Dog(\"rex\")))\n");
    let err = compile_error(&both);
    assert!(err.contains("both implemented for `Dog`") && err.contains("`Describe.name(..)`"), "{err}");

    let unknown = format!("{head}fn call(x) = name(x)\n\nfn main() = println(call(Tag(\"x\")))\n");
    let err = compile_error(&unknown);
    assert!(err.contains("the type it is called on is not known here"), "{err}");
}

/// A `where` clause says what a signature's type variables must implement.
/// It is checked at every use, printed by `rill types`, and refused at the
/// definition where it names nothing.
#[test]
fn a_where_clause_says_what_a_type_variable_must_implement() {
    let head = "trait Describe(a)\n  \
                  describe(a) -> Str\n\n\
                type Dog\n  \
                  Dog(n: Str)\n\n\
                impl Describe(Dog)\n  \
                  fn describe(d) = \"dog \" + d.n\n\n\
                fn shout(x: a) -> Str where Describe(a) = describe(x) + \"!\"\n\n\
                fn biggest(a: t, b: t) -> t where Ord(t) = if a > b then a else b\n\n";
    let src = format!("{head}fn main() =\n  println(shout(Dog(\"rex\")))\n  println(int_to_str(biggest(3, 9)))\n");
    assert_eq!(run_source("wheres", &src), "dog rex!\n9\n");
    assert_eq!(fmt_source(&src), src);

    let wrong = format!("{head}fn main() = println(shout(1))\n");
    let err = compile_error(&wrong);
    assert!(err.contains("`shout` cannot be used at Int") && err.contains("implement `Describe`"), "{err}");

    let stray = "fn f(x: Int) -> Int where Show(b) = x\n\nfn main() = println(1)\n";
    assert!(compile_error(stray).contains("`b` is not a type variable of this signature"));
    let none = "fn f(x: a) -> Int where Nope(a) = 1\n\nfn main() = println(1)\n";
    assert!(compile_error(none).contains("unknown trait `Nope`"));
}

/// A trait method with a default is what an impl gets where it leaves the
/// method out, checked at that impl's type; a trait may require others; and
/// a loop or a `?` inside an impl method is spelled out like anywhere else.
#[test]
fn a_default_method_and_a_required_trait() {
    let src = "trait Describe(a)\n  \
                 describe(a) -> Str\n  \
                 priority(x: a) -> Int = 1\n  \
                 banner(x: a) -> Str =\n    \
                   p = priority(x)\n    \
                   describe(x) + \" (\" + int_to_str(p) + \")\"\n\n\
               trait Pretty(a): Describe\n  \
                 pretty(a) -> Str\n\n\
               type Dog\n  \
                 Dog(n: Str)\n\n\
               type Cat\n  \
                 Cat(n: Str)\n\n\
               impl Describe(Dog)\n  \
                 fn describe(d) = \"dog \" + d.n\n\n\
               impl Describe(Cat)\n  \
                 fn describe(c) = \"cat \" + c.n\n  \
                 fn priority(c) =\n    \
                   k = while k < 3 with k = 0\n      \
                     k + 1\n    \
                   k * 3\n\n\
               impl Pretty(Dog)\n  \
                 fn pretty(d) = \"*\" + banner(d) + \"*\"\n\n\
               fn main() =\n  \
                 println(banner(Dog(\"rex\")))\n  \
                 println(banner(Cat(\"tom\")))\n  \
                 println(pretty(Dog(\"rex\")))\n";
    assert_eq!(run_source("defaults", src), "dog rex (1)\ncat tom (9)\n*dog rex (1)*\n");
    assert_eq!(fmt_source(src), src);

    let missing = "trait Describe(a)\n  \
                     describe(a) -> Str\n\n\
                   trait Pretty(a): Describe\n  \
                     pretty(a) -> Str\n\n\
                   type Dog\n  \
                     Dog(n: Str)\n\n\
                   impl Pretty(Dog)\n  \
                     fn pretty(d) = d.n\n\n\
                   fn main() = println(pretty(Dog(\"rex\")))\n";
    let err = compile_error(missing);
    assert!(err.contains("needs `Describe` implemented for `Dog` as well"), "{err}");

    let unnamed = "trait T(a)\n  f(a) -> Int = 1\n\nfn main() = println(1)\n";
    assert!(compile_error(unnamed).contains("a default body needs its parameters named"));
}

/// `rill explain` is the generator's account of its memory decisions for
/// the file's own functions: borrowed and owned parameters, constructor
/// calls as reuse or allocation or a flat value, bindings handed over or
/// retained, pipelines that became loops, closures and what they hold.
#[test]
fn explain_reports_the_memory_decisions() {
    let src = "type P\n  \
                 P(x: Float, y: Float)\n\n\
               fn total(xs, acc) = match xs\n  \
                 Nil -> acc\n  \
                 Cons(v, rest) -> total(rest, acc + v)\n\n\
               fn bump(xs) = match xs\n  \
                 Nil -> Nil\n  \
                 Cons(v, rest) -> Cons(v + 1, bump(rest))\n\n\
               fn main() =\n  \
                 xs = Cons(1, Cons(2, Nil))\n  \
                 println(int_to_str(total(xs, 0)))\n  \
                 println(int_to_str(total(bump(xs), 0)))\n  \
                 println(show(P(1.0, 2.0)))\n  \
                 println(int_to_str(range(1, 10) |> filter(\\v -> v > 5) |> sum))\n";
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-explain-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("explain.rill");
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill")).arg("explain").arg(&path).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report = String::from_utf8(out.stdout).unwrap();
    for want in [
        "fn total(xs, acc)",
        "`xs` is borrowed",
        "fn bump(xs)",
        "`Cons(..)` is built in the box a match took apart: no allocation",
        "`Cons(..)` allocates a box with 2 field(s)",
        "`xs` is retained for the call",
        "`P(..)` is a flat value",
        "this pipeline is one loop: no list is built",
    ] {
        assert!(report.contains(want), "missing {want:?} in:\n{report}");
    }
}

/// A `--parallel` build refuses storage that two strands could disagree
/// about: written by a strand and still used by the side that gave it, or
/// sent and kept. Reading is shared freely, a hand-over is the strand's
/// own, a closure is opaque, and `apart(x)` is the program's claim.
#[test]
fn a_parallel_build_checks_what_strands_share() {
    let fill = "fn fill(b: Buf(Int), v) = for i in 0..#b\n  \
                  b[i] = v\n\n";
    let two = format!("{fill}fn main() =\n  b = buf_int(8)\n  spawn fill(b, 1)\n  spawn fill(b, 2)\n  println(int_to_str(b[0]))\n");
    let err = check_parallel(&two);
    assert!(err.contains("`b` is written by this strand and still used here after the `spawn`"), "{err}");

    let handed = format!("{fill}fn main() =\n  b = buf_int(8)\n  b[0] = 5\n  spawn fill(b, 1)\n  println(\"handed over\")\n");
    assert_eq!(check_parallel(&handed), "");

    let readers = "fn total(b: Buf(Int), ch) =\n  \
                     s = for i in 0..#b with acc = 0\n    \
                       acc + b[i]\n  \
                     send(ch, s)\n\n\
                   fn main() =\n  \
                     b = buf_int(8)\n  \
                     ch = channel()\n  \
                     spawn total(b, ch)\n  \
                     spawn total(b, ch)\n  \
                     println(int_to_str(recv(ch) + recv(ch) + b[0]))\n";
    assert_eq!(check_parallel(readers), "");

    let record = "type Grid\n  Grid(n: Int, data: Buf(Float))\n\n\
                  fn scale(g: Grid, k) = for i in 0..g.n\n  g.data[i] = g.data[i] * k\n\n\
                  fn main() =\n  g = Grid(4, buf_float(4))\n  spawn scale(g, 2.0)\n  println(show(g.data[0]))\n";
    assert!(check_parallel(record).contains("`g` is written by this strand"));

    let worker = "fn work(f, lo, hi, done) =\n  \
                    f(lo, hi)\n  \
                    send(done, 1)\n\n\
                  fn main() =\n  \
                    done = channel()\n  \
                    b = buf_int(8)\n  \
                    f = \\lo, hi -> for i in lo..hi\n    \
                      b[i] = i\n  \
                    for t in 0..2\n    \
                      spawn work(FF, t * 4, t * 4 + 4, done)\n  \
                    recv(done)\n  \
                    recv(done)\n  \
                    println(int_to_str(b[5]))\n";
    let bare = worker.replace("FF", "f");
    assert!(check_parallel(&bare).contains("`f` is a function value"), "{}", check_parallel(&bare));
    let claimed = worker.replace("FF", "apart(f)");
    assert_eq!(check_parallel(&claimed), "");

    let sent = "fn store(b: Buf(Int)) =\n  b[0] = 9\n  0\n\n\
                fn sink(ch) =\n  store(recv(ch))\n  sink(ch)\n\n\
                fn main() =\n  ch = channel()\n  b = buf_int(4)\n  spawn sink(ch)\n  send(ch, b)\n  b[1] = 2\n  println(int_to_str(b[0]))\n";
    assert!(check_parallel(sent).contains("`b` is sent down a channel and still used here after"));
}

/// `rill test`: every `test_...` function of a file, each in its own
/// process; a failing assertion says the line and both values, a crash is
/// one test failing, output is shown, and one test can be picked.
#[test]
fn rill_test_runs_each_test_in_its_own_process() {
    let root = workspace_root();
    let out = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("test")
        .arg(root.join("examples/testing.rill"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ok    test_add\nok    test_rev\nok    test_strings\n3 tests: 3 passed, 0 failed\n"
    );

    let out_dir = std::env::temp_dir().join(format!("rill-e2e-test-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("failing.rill");
    std::fs::write(
        &path,
        "fn half(n) = n / 2\n\nfn test_half() = assert_eq(half(7), 4)\n\nfn test_ok() = assert(half(8) == 4)\n\n\
         fn test_crash() =\n  b = buf_int(1)\n  b[3] = 1\n\nfn test_loud() =\n  println(\"some output\")\n  assert(false)\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill")).arg("test").arg(&path).output().unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for want in [
        "FAIL  test_half",
        "rill: assertion failed at line 3",
        "expected: 4",
        "got: 3",
        "ok    test_ok",
        "FAIL  test_crash",
        "buffer index 3 out of range",
        "FAIL  test_loud",
        "it printed:",
        "some output",
        "4 tests: 1 passed, 3 failed",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    let one = Command::new(env!("CARGO_BIN_EXE_rill")).arg("test").arg(&path).arg("test_ok").output().unwrap();
    assert!(one.status.success());
    assert_eq!(String::from_utf8_lossy(&one.stdout), "ok    test_ok\n1 test: 1 passed, 0 failed\n");

    let bad = "fn test_x() = assert(1)\n\nfn main() = println(1)\n";
    assert!(compile_error(bad).contains("`assert`: expected Bool, found Int"));
}

/// A closure crosses into C where the signature says `Env`: boxed, handed
/// out by the `Env` parameter, unpacked by a trampoline in the callback's
/// `Env` slot. A callback without one still needs a named function, and
/// an `Env` without its callback, or a callback with two, is refused.
#[test]
fn a_closure_crosses_into_c_by_an_env() {
    assert_eq!(compile_and_run("callback_env.rill"), "88\nweights\n14\n606\n");

    let bare = "extern fn qsort(base: Ptr, n: U64, size: U64, cmp: Fn(Ptr, Ptr) -> I32)\n\n\
                fn main() =\n  b = buf_i32(3)\n  k = 1\n  qsort(b, 3, 4, \\p, q -> k)\n  println(\"x\")\n";
    let err = compile_error(bare);
    assert!(err.contains("C has nowhere to put it — unless the signature says where"), "{err}");

    let lone = "extern fn oops(n: I64, arg: Env) -> I64\n\nfn main() = println(\"x\")\n";
    assert!(compile_error(lone).contains("has exactly one callback whose `Fn(...)` has an `Env` slot"));
    let two = "extern fn oops(n: I64, f: Fn(Env, Env) -> I64, arg: Env) -> I64\n\nfn main() = println(\"x\")\n";
    assert!(compile_error(two).contains("in one `Env` slot, not two"));
    let extra = "extern fn rill_sum_by(n: I64, f: Fn(I64, Env) -> I64, arg: Env) -> I64\n\n\
                 fn main() = println(int_to_str(rill_sum_by(4, \\i -> i, 0)))\n";
    assert!(compile_error(extra).contains("takes 2 argument(s), but 3 given"));
}

/// `private fn` is the file's own: an importer cannot call it, is told which
/// file keeps it, and may have a function of the same name.
#[test]
fn a_private_function_is_the_files_own() {
    let dir = std::env::temp_dir().join(format!("rill-e2e-private-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helper.rill"),
        "private fn walk(xs, n) = match xs\n  \
           Nil -> n\n  \
           Cons(x, rest) -> walk(rest, n + x)\n\n\
         fn total(xs) = walk(xs, 0)\n\n\
         trait Sized(a)\n  \
           size(x: a) -> Int = walk(Cons(1, Nil), 0)\n",
    )
    .unwrap();
    let run = |name: &str, src: &str| {
        let path = dir.join(name);
        std::fs::write(&path, src).unwrap();
        Command::new(env!("CARGO_BIN_EXE_rill")).arg("run").arg(&path).output().unwrap()
    };
    // The importer's own `walk` and the helper's never meet.
    let ok = run(
        "main.rill",
        "import \"helper\"\n\nfn walk(s) = s + \"!\"\n\nfn main() =\n  \
           println(int_to_str(total(Cons(1, Cons(2, Nil)))))\n  \
           println(walk(\"own\"))\n",
    );
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    assert_eq!(String::from_utf8_lossy(&ok.stdout), "3\nown!\n");

    let no = run("no.rill", "import \"helper\"\n\nfn main() = println(int_to_str(walk(Nil, 0)))\n");
    assert!(!no.status.success());
    let err = String::from_utf8_lossy(&no.stderr);
    assert!(err.contains("`walk` is private to `helper.rill`"), "{err}");

    let aliased = run(
        "al.rill",
        "import \"helper\" as h\n\nfn main() = println(int_to_str(h.total(Cons(5, Nil))))\n",
    );
    assert!(aliased.status.success(), "{}", String::from_utf8_lossy(&aliased.stderr));
    assert_eq!(String::from_utf8_lossy(&aliased.stdout), "5\n");

    let types = Command::new(env!("CARGO_BIN_EXE_rill")).arg("types").arg(dir.join("helper.rill")).output().unwrap();
    let shown = String::from_utf8_lossy(&types.stdout);
    assert!(shown.contains("private fn walk(") && shown.contains("fn total("), "{shown}");
    let src = std::fs::read_to_string(dir.join("helper.rill")).unwrap();
    assert_eq!(fmt_source(&src), src);
}

/// A `while` that calls nothing is checked for preemption — another strand
/// on the worker runs while it spins — and a long `for` gives the turn up
/// with `yield()`.
#[test]
fn a_spinning_while_lets_the_other_strand_run() {
    let spin = "fn spin(n, acc) = while i < n with (i, acc) = (0, acc)\n  \
                  (i + 1, acc + i % 7)\n\n\
                fn talker(ch) =\n  \
                  println(\"talker ran\")\n  \
                  send(ch, 1)\n\n\
                fn main() =\n  \
                  ch = channel()\n  \
                  spawn talker(ch)\n  \
                  spin(600000000, 0)\n  \
                  println(\"spin done\")\n  \
                  recv(ch)\n";
    assert_eq!(run_source("spinwhile", spin), "talker ran\nspin done\n");

    let yielding = "fn spin(n) = for c in 0..4 with acc = 0\n  \
                      yield()\n  \
                      for i in 0..n with acc2 = acc\n    \
                        acc2 + i % 7\n\n\
                    fn talker(ch) =\n  \
                      println(\"talker ran\")\n  \
                      send(ch, 1)\n\n\
                    fn main() =\n  \
                      ch = channel()\n  \
                      spawn talker(ch)\n  \
                      spin(50000000)\n  \
                      println(\"spin done\")\n  \
                      recv(ch)\n";
    assert_eq!(run_source("spinyield", yielding), "talker ran\nspin done\n");
}

/// `extern blocking fn` makes the call on a helper thread while the strand
/// is parked: another strand runs during a sleep that would otherwise hold
/// the worker. A blocking call cannot take a callback.
#[test]
fn a_blocking_extern_lets_the_worker_go_on() {
    let body = "fn talker(ch) =\n  \
                  println(\"talker ran\")\n  \
                  send(ch, 1)\n\n\
                fn main() =\n  \
                  ch = channel()\n  \
                  spawn talker(ch)\n  \
                  r = usleep(200000)\n  \
                  println(\"slept \" + int_to_str(r))\n  \
                  recv(ch)\n";
    let blocking = format!("extern blocking fn usleep(us: U32) -> I32\n\n{body}");
    assert_eq!(run_source("blocking", &blocking), "talker ran\nslept 0\n");
    assert_eq!(fmt_source(&blocking), blocking);
    let plain = format!("extern fn usleep(us: U32) -> I32\n\n{body}");
    assert_eq!(run_source("holding", &plain), "slept 0\ntalker ran\n");

    let cb = "extern blocking fn qsort(base: Ptr, n: U64, size: U64, cmp: Fn(Ptr, Ptr) -> I32)\n\nfn main() = println(\"x\")\n";
    assert!(compile_error(cb).contains("a call that calls back is not `blocking`"));
}

/// A field only some constructors have is not certainly there, and one of two
/// types across them has no one type; both say so.
#[test]
fn a_field_not_in_every_constructor_still_needs_a_match() {
    let head = "type Yarim\n  \
                  C(n: Int)\n  \
                  D(m: Int)\n\n\
                type Kotu\n  \
                  A(n: Int)\n  \
                  B(m: Int, n: Str)\n\n";
    let err = compile_error(&format!("{head}fn f(h: Yarim) = h.n\n\nfn main() = println(\"x\")\n"));
    assert!(
        err.contains("more than one constructor") && err.contains("`n` is a field of `C` but not of `D`"),
        "{err}"
    );
    let err = compile_error(&format!("{head}fn f(h: Yarim) = {{ h | n = 1 }}\n\nfn main() = println(\"x\")\n"));
    assert!(err.contains("`n` is a field of `C` but not of `D`"), "{err}");

    let err = compile_error(&format!("{head}fn f(b: Kotu) = b.n\n\nfn main() = println(\"x\")\n"));
    assert!(err.contains("`n` is `Int` in `A` and `Str` in `B`"), "{err}");

    let err = compile_error(&format!("{head}fn f(h: Yarim) = h.zz\n\nfn main() = println(\"x\")\n"));
    assert!(err.contains("no field `zz`") && err.contains("`C` has n; `D` has m"), "{err}");
}

/// `?` — a chain of things that can fail, and the first failure out of the
/// middle of it. Every line here would be a `match` without it.
#[test]
fn error_propagation() {
    assert_eq!(
        compile_and_run("results.rill"),
        concat!(
            "example.com:8080 x3\n",
            "localhost:8080 x1\n",
            "error: no host\n",
            "error: no port\n",
            "error: not a number: zero\n",
            "has a host\n",
            "error: cannot read no-such-file-in-this-directory\n"
        )
    );
}

/// The hash table: strings, a data type and a tuple as keys, an `Option` out
/// of every read, and a walk that is summed rather than printed — the order
/// entries come back in is the hash's, and no program should depend on it.
#[test]
fn hash_tables() {
    assert_eq!(
        compile_and_run("maps.rill"),
        concat!(
            "the=3 cat=2 dog=0 \n5\n1\ntrue\nfalse\n100\n5\ntrue\nfalse\n4\n",
            "105\n4\n4\ncorner\n?\n10\ntwo\ndone\n"
        )
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
        \x20 qsort(b, buf_len(b), 4, cmp)\n\
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

/// Bytes back from C by length rather than by a terminator. The zero in the
/// middle is the point: `from_cstr` would stop there and hand back two bytes,
/// which is right for a C string and wrong for anything a library filled a
/// buffer with. A null pointer and a length that is not positive are empty
/// rather than a crash, because a C call that failed returns exactly those.
#[test]
fn bytes_come_back_from_c_by_length() {
    assert_eq!(
        run_source("bytes_from_c", 
            "fn main() =\n\
            \x20 s = \"ab\" + chr(0) + \"cd\"\n\
            \x20 p = to_cstr(s)\n\
            \x20 back = str_from_ptr(p, 5)\n\
            \x20 free_ptr(p)\n\
            \x20 println(int_to_str(str_len(back)))\n\
            \x20 println(int_to_str(str_get(back, 2)))\n\
            \x20 println(back[3:])\n\
            \x20 println(int_to_str(str_len(str_from_ptr(null_ptr(), 4))))\n\
            \x20 println(int_to_str(str_len(str_from_ptr(to_cstr(s), 0))))\n"
        ),
        "5\n0\ncd\n0\n0\n"
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
        "500000500000\n21\n5\nCons(4, Cons(16, Cons(36, Cons(64, Cons(100, Nil)))))\n720\n3\n2\n1\n\
         10\n15\n21\n5\n1\n2\n3\n42\n0\n"
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

/// A loop written as several functions calling each other in tail position,
/// where every path returns the same thing — which is what a loop written for
/// its effects looks like, and what used to grow the stack by a frame a turn.
/// Two hundred thousand turns is far past what eight megabytes holds.
#[test]
fn a_tail_call_cycle_runs_in_constant_stack() {
    let src = "fn take(acc) = (\"i 1 2\", \"rest\")\n\n\
               fn reader(n, acc) =\n  \
                 (msg, rest) = take(acc)\n  \
                 if n == 0 then 0 else gate(n, msg, rest)\n\n\
               fn gate(n, msg, rest) = handle(n, msg, rest)\n\n\
               fn handle(n, msg, rest) =\n  \
                 parts = split(msg, \" \")\n  \
                 route(unwrap_or(nth(parts, 0), \"\"), parts)\n  \
                 reader(n - 1, rest)\n\n\
               fn route(kind, parts) = if kind == \"i\" then 1 else 0\n\n\
               fn main() = println(reader(200000, \"\"))\n";
    assert_eq!(run_source("tailcycle", src), "0\n");
}

/// The language server, driven the way an editor drives it: a buffer that
/// was never saved is what gets checked, the type of a name that nothing
/// annotated is what comes back, and the answers arrive framed.
#[test]
fn language_server_answers_an_editor() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("rill-lsp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("buffer.rill");
    // On disk: nothing. What the editor holds is the whole story.
    std::fs::write(&file, "").unwrap();
    let uri = format!("file://{}", file.display());

    let good = "fn twice(n) = n * 2\n\nfn main() = println(twice(21))\n";
    let bad = "fn main() = println(nope)\n";
    let mut input = Vec::new();
    let mut frame = |body: String| {
        input.extend_from_slice(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    };
    frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string());
    frame(format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","text":"{good}"}}}}}}"#
    ));
    // Hover over `twice`, whose parameter and result nothing wrote down.
    frame(format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":4}}}}}}"#
    ));
    frame(format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{uri}"}},"contentChanges":[{{"text":"{bad}"}}]}}}}"#
    ));
    frame(r#"{"jsonrpc":"2.0","method":"exit","params":{}}"#.to_string());

    let mut child = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("lsp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn the server");
    child.stdin.as_mut().unwrap().write_all(&input).unwrap();
    let out = child.wait_with_output().expect("server did not finish");
    let text = String::from_utf8(out.stdout).expect("the protocol is UTF-8");

    assert!(text.contains("\"hoverProvider\":true"), "{text}");
    // Every message is framed with the length of its body.
    assert!(text.starts_with("Content-Length: "), "{text}");
    assert!(text.contains("fn twice(n: Int) -> Int"), "hover said: {text}");
    // The first buffer checks; the second does not, and says why.
    assert!(text.contains("unknown variable `nope`"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Nested patterns, the decision tree they compile to, and the guards that
/// cover nothing: the arms below a guarded one are still reachable, and still
/// have to answer for the case its condition turns down.
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
         nothing\n\
         starts big: 500\n\
         starts small: 5\n\
         starts at or below zero: 0\n\
         empty\n\
         3 under, then 9 over, then end\n\
         positive\n\
         not positive\n\
         a hole\n"
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
    assert_eq!(compile_and_run("multi/units.rill"), "1.5m\n2ft\n20\n30\n3\n");
}

/// A crash says which functions were on the stack — from the table the
/// compiler leaves in the binary, so a stripped release build names them
/// too — and what kind of crash it was.
#[test]
fn crash_reports_name_the_frames() {
    let (_, said) = compile_and_crash("crashes.rill", &["copy"]);
    assert!(said.contains("rill: buffer copy of 4096 at 0 from 0 out of range, lengths 16 and 4096\n"), "{said}");
    assert!(said.contains("  in bad_copy +"), "{said}");
    let (_, said) = compile_and_crash("crashes.rill", &["cmp"]);
    assert!(said.contains("rill: buffer compare of 16 at 8 and 0 out of range, lengths 16 and 16\n"), "{said}");
    let (_, said) = compile_and_crash("crashes.rill", &["oob"]);
    assert!(said.contains("rill: buffer index 20 out of range, length 16\n"), "{said}");
    assert!(said.contains("  in bad_index +"), "{said}");
    let (_, said) = compile_and_crash("crashes.rill", &["ptr"]);
    assert!(said.contains("rill: segmentation fault at 0x"), "{said}");
    assert!(said.contains("  in the main strand\n"), "{said}");
    assert!(said.contains("  in bad_ptr +"), "{said}");
    let (_, said) = compile_and_crash("crashes.rill", &["deep"]);
    assert!(said.contains("a stack overflow: the strand ran off the end of its stack\n"), "{said}");
    assert!(said.contains("  in dive +"), "{said}");
    assert!(said.contains(" more times\n"), "{said}");
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

/// A connection that is refused leaves the descriptor registered in the poller
/// unless something takes the registration back down, and the number then goes
/// back to the operating system to hand out again. The next socket given that
/// number, waited on by the same strand, was taken for one already registered
/// and so was never registered at all: the wait never ended. Two refused
/// connections in a row were enough.
#[test]
fn a_refused_connection_leaves_nothing_behind() {
    let out = run_source(
        "connect_refused_twice",
        "fn try(port, n) =\n  \
           if n <= 0 then 0\n  \
           else\n    \
             r = match connect_to(\"127.0.0.1\", port)\n      \
               Err(e) -> 0\n      \
               Ok(fd) ->\n        \
                 sock_close(fd)\n        \
                 1\n    \
             try(port, n - 1)\n\
         \n\
         fn main() =\n  \
           try(1, 4)\n  \
           println(\"four refusals, four answers\")\n",
    );
    assert_eq!(out, "four refusals, four answers\n");
}

/// A descriptor is registered for reading and for writing separately but is
/// named in the poller by the descriptor alone, so an edge belonging to the
/// other direction wakes a wait that was not for it. A socket written to a
/// moment ago is writable, and that edge used to end a `sock_wait` for reading
/// in under a millisecond with a deadline that had not remotely passed.
#[test]
fn a_socket_wait_is_not_ended_by_the_write_before_it() {
    let out = run_source(
        "sock_wait_after_write",
        "fn quiet(server) =\n  \
           conn = tcp_accept(server)\n  \
           sleep_ms(3000)\n  \
           sock_close(conn)\n\
         \n\
         fn main() = match listen_on(0)\n  \
           Err(e) -> println(e)\n  \
           Ok(server) ->\n    \
             port = sock_port(server)\n    \
             spawn quiet(server)\n    \
             match connect_to(\"127.0.0.1\", port)\n      \
               Err(e) -> println(e)\n      \
               Ok(fd) -> ask(fd)\n\
         \n\
         fn ask(fd) =\n  \
           sock_write(fd, \"anything\")\n  \
           start = now_nanos()\n  \
           ready = sock_wait(fd, 250)\n  \
           took = (now_nanos() - start) / 1000000\n  \
           println(bool_to_str(!ready && took >= 240))\n  \
           sock_close(fd)\n",
    );
    assert_eq!(out, "true\n");
}

/// A value received by a `select` arm comes out of the channel owned, and an
/// arm that only reads it hands that ownership to nobody. It used to be
/// dropped on the floor: a loop receiving a million strings and asking each
/// one its length ended with a million of them still allocated.
#[test]
fn a_select_arm_releases_what_it_bound() {
    let out = run_source(
        "select_arm_release",
        "fn taker(ch, done, n) =\n  \
           if n <= 0 then 0\n  \
           else\n    \
             got = select\n      \
               v = recv(ch) -> if str_len(v) > 0 then 1 else 1\n      \
               recv(done) -> 0\n    \
             if got == 0 then 0 else taker(ch, done, n - 1)\n\
         \n\
         fn giver(ch, i, n) =\n  \
           if i >= n then 0\n  \
           else\n    \
             send(ch, \"a body of some length \" + int_to_str(i))\n    \
             giver(ch, i + 1, n)\n\
         \n\
         fn main() =\n  \
           ch = channel(64)\n  \
           done = channel()\n  \
           spawn giver(ch, 0, 20000)\n  \
           taker(ch, done, 20000)\n  \
           println(\"drained\")\n",
    );
    assert_eq!(out, "drained\n");
}

#[test]
fn a_select_can_carry_a_deadline() {
    assert_eq!(
        compile_and_run("timeouts.rill"),
        "gave up after 80 ms\n\
         answered 42\n\
         gave up after 0 ms\n\
         still nothing after 60 ms\n\
         waited on nothing in particular\n"
    );
}

/// The dangerous case: a strand in a `select` with a deadline is registered in
/// two places that can both come ready, and queueing a strand twice corrupts
/// the run queue. Four thousand rounds with the deadline swept across the
/// arrival, so that neither outcome is the only one taken.
#[test]
fn a_deadline_racing_a_value_leaves_nothing_behind() {
    let out = run_source(
        "select_deadline_race",
        "fn racer(ch) =\n  \
           sleep_ms(2)\n  \
           send(ch, 1)\n\
         \n\
         fn round(n, got, late) =\n  \
           if n <= 0 then (got, late)\n  \
           else\n    \
             ch = channel()\n    \
             spawn racer(ch)\n    \
             ms = 1 + n - (n / 4) * 4\n    \
             r = select\n      \
               v = recv(ch) -> 1\n      \
               timeout(ms) -> 0\n    \
             if r == 1 then round(n - 1, got + 1, late)\n    \
             else\n      \
               recv(ch)\n      \
               round(n - 1, got, late + 1)\n\
         \n\
         fn main() =\n  \
           (got, late) = round(2000, 0, 0)\n  \
           println(got > 0 && late > 0 && got + late == 2000)\n  \
           sleep_ms(50)\n",
    );
    assert_eq!(out, "true\n");
}

/// `default` means do not wait, so a deadline beside one could never be
/// reached. Written either way round, that is refused rather than ignored.
#[test]
fn a_default_beside_a_timeout_is_refused() {
    for src in [
        "fn main() =\n  \
           ch = channel()\n  \
           r = select\n    \
             v = recv(ch) -> v\n    \
             timeout(50) -> 0\n    \
             default -> -1\n  \
           println(int_to_str(r))\n",
        "fn main() =\n  \
           ch = channel()\n  \
           r = select\n    \
             v = recv(ch) -> v\n    \
             default -> -1\n    \
             timeout(50) -> 0\n  \
           println(int_to_str(r))\n",
    ] {
        let err = compile_error(src);
        assert!(
            err.contains("`select` with `default` never waits"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn a_signal_reaches_a_strand_and_it_stops_the_others() {
    assert_eq!(
        compile_and_run("signals.rill"),
        "told: SIGTERM\n\
         worker 3 stopped\n\
         worker 2 stopped\n\
         worker 1 stopped\n\
         everything stopped, and main is still the one exiting\n"
    );
}

/// Waiting for a signal parks the strand in the poller rather than holding a
/// worker, so a computation beside it runs at the speed it runs at alone. And
/// a program whose only strand is waiting is not a deadlock: it is waiting on
/// something that answers.
#[test]
fn waiting_for_a_signal_holds_no_worker() {
    let out = run_source(
        "signal_parks",
        "extern fn raise(sig: I32) -> I32\n\
         \n\
         fn spin(n, acc) = if n <= 0 then acc else spin(n - 1, acc + n)\n\
         \n\
         fn watcher(said) = send(said, wait_signal())\n\
         \n\
         fn main() =\n  \
           said = channel()\n  \
           spawn watcher(said)\n  \
           sleep_ms(50)\n  \
           alone = timed(300000000)\n  \
           raise(sig_term())\n  \
           println(int_to_str(recv(said)))\n  \
           beside = timed(300000000)\n  \
           println(beside < alone * 3)\n\
         \n\
         fn timed(work) =\n  \
           start = now_nanos()\n  \
           spin(work, 0)\n  \
           (now_nanos() - start) / 1000000\n",
    );
    assert_eq!(out, "15\ntrue\n");
}

#[test]
fn closing_a_channel_wakes_everything_parked_on_it() {
    assert_eq!(
        compile_and_run("closing.rill"),
        "sum of everything sent: 15\n\
         gave up after 0 sends nobody took\n\
         woken by one close: 4\n"
    );
}

/// Buffered values are not lost by closing early, and a `recv` that was
/// already parked when the close happened comes back rather than staying.
#[test]
fn a_close_keeps_what_is_buffered_and_frees_what_is_waiting() {
    let out = run_source(
        "chan_close",
        "fn waiter(ch, done) =\n  \
           r = match recv_opt(ch)\n    \
             None -> \"woke on close\"\n    \
             Some(v) -> \"got \" + int_to_str(v)\n  \
           send(done, r)\n\
         \n\
         fn main() =\n  \
           b = channel(3)\n  \
           send(b, 1)\n  \
           send(b, 2)\n  \
           close(b)\n  \
           println(show(recv_opt(b)) + \" \" + show(recv_opt(b)) + \" \" + show(recv_opt(b)))\n  \
           println(bool_to_str(closed(b)))\n  \
           idle = channel()\n  \
           done = channel()\n  \
           spawn waiter(idle, done)\n  \
           sleep_ms(20)\n  \
           close(idle)\n  \
           println(recv(done))\n  \
           close(idle)\n  \
           println(\"closing twice is not an error\")\n",
    );
    assert_eq!(
        out,
        "Some(1) Some(2) None\n\
         true\n\
         woke on close\n\
         closing twice is not an error\n"
    );
}

/// A strand parked on a read is parked in the poller, so closing its socket
/// from another strand does not reach it. Shutting the connection down does.
#[test]
fn shutdown_gets_another_strand_out_of_a_read() {
    let out = run_source(
        "sock_shutdown",
        "fn reader(conn, report) =\n  \
           msg = sock_read(conn)\n  \
           send(report, if str_len(msg) == 0 then \"reader woke, peer gone\" else \"read: \" + msg)\n  \
           sock_close(conn)\n\
         \n\
         fn serve(server, report) =\n  \
           conn = tcp_accept(server)\n  \
           spawn reader(conn, report)\n  \
           sleep_ms(30)\n  \
           println(\"shutdown said: \" + int_to_str(sock_shutdown(conn)))\n\
         \n\
         fn quiet_client(port) = match connect_to(\"127.0.0.1\", port)\n  \
           Err(e) -> println(e)\n  \
           Ok(fd) ->\n    \
             sleep_ms(400)\n    \
             sock_close(fd)\n\
         \n\
         fn main() = match listen_on(0)\n  \
           Err(e) -> println(e)\n  \
           Ok(server) ->\n    \
             port = sock_port(server)\n    \
             report = channel()\n    \
             spawn serve(server, report)\n    \
             spawn quiet_client(port)\n    \
             println(recv(report))\n    \
             sock_close(server)\n",
    );
    assert_eq!(out, "shutdown said: 0\nreader woke, peer gone\n");
}

#[test]
fn a_file_is_a_descriptor_read_written_and_made_durable() {
    assert_eq!(
        compile_and_run("files.rill"),
        "record offsets: Cons(0, Cons(6, Cons(11, Nil)))\n\
         synced: true\n\
         second record: beta\n\
         first record:  alpha\n\
         whole file is 17 bytes\n\
         in /tmp/rill_files_example: Cons(journal, Nil)\n\
         manifest: generation 1\n\
         \n"
    );
}

/// The point of `file_sync` is not that it is fast, it is that the worker is
/// free while it happens. A strand computing beside a strand syncing has to
/// take about as long as it takes on its own; if the sync held the worker it
/// would have to wait for every one of them.
#[test]
fn a_sync_does_not_hold_the_worker() {
    let out = run_source(
        "file_sync_parks",
        "fn spin(n, acc) = if n <= 0 then acc else spin(n - 1, acc + n)\n\
         \n\
         fn syncer(path, n, done) =\n  \
           fd = file_open(path, \"w\")\n  \
           syncs(fd, n)\n  \
           file_close(fd)\n  \
           file_remove(path)\n  \
           send(done, 1)\n\
         \n\
         fn syncs(fd, n) =\n  \
           if n <= 0 then 0\n  \
           else\n    \
             file_write(fd, \"x\")\n    \
             file_sync(fd)\n    \
             syncs(fd, n - 1)\n\
         \n\
         fn timed(work) =\n  \
           start = now_nanos()\n  \
           spin(work, 0)\n  \
           (now_nanos() - start) / 1000000\n\
         \n\
         fn main() =\n  \
           path = \"/tmp/rill_e2e_sync_parks\"\n  \
           spin(50000000, 0)\n  \
           alone = timed(400000000)\n  \
           done = channel()\n  \
           spawn syncer(path, 200, done)\n  \
           beside = timed(400000000)\n  \
           println(beside < alone * 2)\n  \
           recv(done)\n",
    );
    assert_eq!(out, "true\n");
}

#[test]
fn file_io_and_text_processing() {
    assert_eq!(
        compile_and_run("io.rill"),
        "args >= 1: true\n\
         made a new one: true\n\
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

// ---- lanes, holders, grids, and a library a host loads -----------------------

#[test]
fn float2_lanes_and_eight_way_tuples() {
    let out = compile_and_run("lanes.rill");
    assert_eq!(
        out,
        "8024\n-32\n(6.25, -7.75)\n-0.5\ntrue\n1.5\n-2\n(6, 12, 18, 24, 30, 36, 42, 48)\n"
    );
}

#[test]
fn unique_sees_the_sole_holder_and_last_uses_move() {
    let out = compile_and_run("holders.rill");
    assert_eq!(out, "3\n6\n12\n10\nfalse\ntrue\n");
}

#[test]
fn grid_tour_agrees_with_numpy() {
    let out = compile_and_run("grid_tour.rill");
    assert_eq!(
        out,
        "[[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10, 11]]\n\
         [[1, 3, 5, 7], [9, 11, 13, 15], [17, 19, 21, 23]]\n\
         450.095\n\
         [6, 22, 38]\n\
         [12, 15, 18, 21]\n\
         [[14, 38, 62], [38, 126, 214], [62, 214, 366]]\n\
         506\n\
         5\n\
         [0, 10, 20, 30, 40, 50, 6, 7, 8, 9, 10, 11]\n\
         11.9167\n\
         [0, 1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 66]\n\
         [6, 22, 38]\n\
         [false, false, false, false, false, false, true, true, true, true, true, true]\n\
         6\n\
         [false, false, false, false, false, false, true, true, true, false, false, false]\n\
         556\n\
         4870\n"
    );
}

/// `--shared` makes a library whose `export fn`s a C host calls: counted
/// values cross as handles the host drops, `main` runs as `rill_init`.
#[test]
fn shared_library_serves_a_c_host() {
    require_fresh_runtime();
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-shared-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let lib = out_dir.join("gridlib");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(root.join("bindings/python/gridlib.rill"))
        .arg("--shared")
        .arg("-o")
        .arg(&lib)
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "rill build --shared failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib = lib.with_extension(ext);
    let host_src = out_dir.join("host.c");
    std::fs::write(
        &host_src,
        "#include <stdio.h>\n\
         void rill_init(void);\n\
         void *grid_arange(long n);\n\
         void *grid_scale(void *a, double v);\n\
         void *grid_matmul(void *a, void *b);\n\
         void *grid_reshape2(void *a, long r, long c);\n\
         void *grid_transpose(void *a);\n\
         double grid_sum(void *a);\n\
         char *grid_show(void *a);\n\
         void rill_free_ptr(void *p);\n\
         void rill_drop_g_Grid(void *a);\n\
         int main(void) {\n\
           rill_init();\n\
           void *a = grid_arange(6);\n\
           void *b = grid_scale(a, 3.0);\n\
           void *m = grid_reshape2(b, 2, 3);\n\
           void *t = grid_transpose(m);\n\
           void *p = grid_matmul(m, t);\n\
           char *s = grid_show(p);\n\
           printf(\"%s %g\\n\", s, grid_sum(a));\n\
           rill_free_ptr(s);\n\
           rill_drop_g_Grid(p); rill_drop_g_Grid(t); rill_drop_g_Grid(m);\n\
           rill_drop_g_Grid(b); rill_drop_g_Grid(a);\n\
           return 0;\n\
         }\n",
    )
    .unwrap();
    let host = out_dir.join("host");
    let cc = Command::new("cc").arg(&host_src).arg(&lib).arg("-o").arg(&host).output().expect("cc");
    assert!(cc.status.success(), "cc failed:\n{}", String::from_utf8_lossy(&cc.stderr));
    let run = Command::new(&host).output().expect("failed to run the host");
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "[[45, 126], [126, 450]] 15\n");
}

/// The Python package over the same library, when there is a Python to run
/// it: `build.py` makes the library and `test_grid.py` checks it against
/// NumPy or known answers.
#[test]
fn python_package_passes_its_tests() {
    require_fresh_runtime();
    let Some(python) = ["python3", "python"].iter().find(|p| {
        Command::new(p).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }) else {
        return;
    };
    let root = workspace_root();
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_rill")).parent().unwrap().to_path_buf();
    let build = Command::new(python)
        .arg(root.join("bindings/python/build.py"))
        .env("RILL", env!("CARGO_BIN_EXE_rill"))
        .env("RILL_PATH", root.join("lib"))
        .current_dir(&bin_dir)
        .output()
        .expect("python");
    assert!(build.status.success(), "build.py failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let test = Command::new(python).arg(root.join("bindings/python/test_grid.py")).output().expect("python");
    assert!(
        test.status.success(),
        "test_grid.py failed:\n{}\n{}",
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr)
    );
}

/// The template generator: a template goes in, Rill comes out, and what that
/// Rill prints is the page. The two benchmark templates are the ones checked,
/// because their pages are kept next to them byte for byte — they are what a
/// well-known Rust suite renders, and the whole point of the generator is that
/// the page is the same one.
#[test]
fn templates_render_the_page_they_are_given() {
    require_fresh_runtime();
    let root = workspace_root();
    let here = root.join("benchmarks/template");
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-tmpl-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = root.join("lib").display().to_string() + ":" + &here.display().to_string();

    let build = |src: std::path::PathBuf, exe: &std::path::Path| {
        let out = Command::new(env!("CARGO_BIN_EXE_rill"))
            .arg("build")
            .arg(&src)
            .arg("-o")
            .arg(exe)
            .env("RILL_PATH", &path)
            .output()
            .expect("failed to spawn rill");
        assert!(
            out.status.success(),
            "rill build {} failed:\n{}",
            src.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // The generator, run over every template in the repository, writing the
    // Rill that is checked in beside it.
    let gen = out_dir.join("tmplgen");
    build(root.join("tools/tmplgen.rill"), &gen);
    for (tmpl, rill) in [
        ("benchmarks/template/big-table.tmpl", "benchmarks/template/big_table.rill"),
        ("benchmarks/template/teams.tmpl", "benchmarks/template/teams_tmpl.rill"),
        ("examples/oxidb/catalogue.tmpl", "examples/oxidb/catalogue_page.rill"),
        ("examples/oxidb/catalogue_form.tmpl", "examples/oxidb/catalogue_form_tmpl.rill"),
        ("examples/oxidb/catalogue_table.tmpl", "examples/oxidb/catalogue_table_tmpl.rill"),
        ("examples/oxidb/catalogue_nav.tmpl", "examples/oxidb/catalogue_nav_tmpl.rill"),
        ("examples/template/banner.tmpl", "examples/template/banner_tmpl.rill"),
        ("examples/template/row.tmpl", "examples/template/row_tmpl.rill"),
        ("examples/template/page.tmpl", "examples/template/page_tmpl.rill"),
    ] {
        let made = out_dir.join(Path::new(rill).file_name().unwrap());
        // From the root with a relative path, because the generator writes
        // the name it is given into the first line of what it makes.
        let out = Command::new(&gen)
            .arg(tmpl)
            .arg(&made)
            .current_dir(&root)
            .output()
            .expect("tmplgen");
        assert!(out.status.success(), "tmplgen {tmpl} failed");
        assert_eq!(
            std::fs::read_to_string(&made).unwrap(),
            std::fs::read_to_string(root.join(rill)).unwrap(),
            "the Rill generated from {tmpl} is not what is checked in"
        );
    }

    // And the pages themselves.
    let bench = out_dir.join("tmplbench");
    build(here.join("bench.rill"), &bench);
    for case in ["big-table", "teams"] {
        let run = Command::new(&bench).arg(case).output().expect("failed to run the benchmark");
        assert!(run.status.success(), "{case} exited with {:?}", run.status.code());
        assert_eq!(
            String::from_utf8(run.stdout).expect("the page was not UTF-8"),
            std::fs::read_to_string(here.join(format!("{case}.expected"))).unwrap(),
            "the {case} page is not the one expected"
        );
    }
}

/// A handler with layers on it. `wrap` puts them on outermost first, so the
/// one that announces what it saw sees the answer the rate limiter gave
/// without the handler underneath ever being called — which is the point of a
/// middleware being able to answer for itself. The fourth write comes from
/// another client in the same second and is let through, which is the limit
/// being per client rather than per route.
#[test]
fn middleware_wraps_a_handler_and_may_answer_for_it() {
    assert_eq!(
        compile_and_run("http/middleware.rill"),
        "saw POST /write -> 200\n200 wrote one\n\
         saw POST /write -> 200\n200 wrote two\n\
         saw POST /write -> 429\n429 {\"error\":\"too many writes — the limit is 2 a second\"}\n\
         saw POST /write -> 200\n200 wrote four\n\
         saw GET /read -> 200\n200 wrote \n"
    );
}

/// `p.x` reaches the type checker as one dotted name, because the lexer cannot
/// tell a field from a module. A lambda that read a field of what it closed
/// over therefore captured nothing — the name it looked for was `r.body`,
/// which is in no scope — and the compiler walked off the end of a map while
/// generating code for it. It captures `r`.
#[test]
fn a_lambda_captures_what_it_reads_a_field_of() {
    let out = run_source(
        "capture_field",
        r#"type Req
  Req(path: Str, body: Str)

fn main() =
  r = Req("/a", "b")
  println(apply(\x -> x + r.body))
  fn near(x) = x + r.path
  println(near("local: "))
  ch = channel(1)
  spawn send(ch, r.path + r.body)
  println(recv(ch))

fn apply(f) = f("lambda: ")
"#,
    );
    assert_eq!(out, "lambda: b\nlocal: /a\n/ab\n");
}

/// The catalogue hands out the files its page pulls in by name, and nothing
/// else — a URL cannot ask for a path. So the list in the source and what is
/// on disk have to be the same list, and this is what says so.
#[test]
fn the_catalogue_serves_every_file_its_page_asks_for() {
    let root = workspace_root();
    let source = std::fs::read_to_string(root.join("examples/oxidb/catalogue.rill")).unwrap();
    // Every file the source names, wherever the whitelist is spelled out —
    // a quoted name, not a bare extension like the one the content type is
    // chosen by.
    let named: std::collections::BTreeSet<String> = source
        .split('"')
        .filter(|piece| piece.ends_with(".js") || piece.ends_with(".css"))
        .filter(|piece| !piece.starts_with('.'))
        .map(|piece| piece.to_string())
        .collect();

    let dir = root.join("examples/oxidb/static");
    let mut found = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            for inner in std::fs::read_dir(entry.path()).unwrap() {
                let inner = inner.unwrap();
                found.insert(format!(
                    "{}/{}",
                    entry.file_name().to_string_lossy(),
                    inner.file_name().to_string_lossy()
                ));
            }
        } else {
            found.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    assert_eq!(named, found, "the files named in catalogue.rill are not the files in static/");
}

/// A page built out of four templates: `page.tmpl` includes `banner.tmpl`
/// twice with different words and `row.tmpl` once per item, handing each one
/// the item and where it sits in the list. An include is a call, so what one
/// template passes another is whatever it has in scope.
#[test]
fn a_page_made_of_several_templates() {
    assert_eq!(
        compile_and_run("template/shop.rill"),
        r#"<!doctype html>
<html lang="en">
  <body>
<header>
  <h1>Ada &amp; co</h1>
  <p>everything we have, today</p>
</header>
    <table>
      <tr><th>#</th><th>what</th><th>price</th><th>note</th><th>stock</th></tr>
<tr>
  <td>1</td>
  <td>tea &amp; jam</td>
  <td>40</td>
  <td><b>two</b> jars</td>
  <td>12 in stock</td>
</tr>
<tr>
  <td>2</td>
  <td>&lt;hat&gt;</td>
  <td>1250</td>
  <td>—</td>
  <td>none left</td>
</tr>
<tr>
  <td>3</td>
  <td>rope</td>
  <td>99</td>
  <td>by the metre</td>
  <td>only 3</td>
</tr>
    </table>
<header>
  <h1>that is all</h1>
  <p>Ada &amp; co</p>
</header>
  </body>
</html>
"#
    );
}

/// An exponent on a float literal, which the lexer used to refuse: a digit
/// run may be followed by nothing else that means anything, so `1e6` is a
/// float without needing a dot to say so.
#[test]
fn a_float_literal_may_carry_an_exponent() {
    // Compared against the digits rather than printed: `println` renders a
    // float to six significant figures, which would hide whether the literal
    // was read correctly in the first place.
    let out = run_source(
        "exponents",
        "fn main() =\n             println(bool_to_str(1.0e3 == 1000.0))\n             println(bool_to_str(2.5e-3 == 0.0025))\n             println(bool_to_str(1e6 == 1000000.0))\n             println(bool_to_str(1E6 == 1000000.0))\n             println(bool_to_str(1.0e+9 == 1000000000.0))\n             println(bool_to_str(1.0e9 + 1.0 > 1.0e9))\n             println(2.5e-3)\n",
    );
    assert_eq!(out, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n0.0025\n");
}

/// The canonical spelling has no exponent, so the formatter writes one back
/// as the digits it stands for. That is what keeps one program one spelling.
#[test]
fn an_exponent_formats_back_to_plain_digits() {
    let out_dir = std::env::temp_dir().join(bad_name("fmt-exp"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("e.rill");
    std::fs::write(&path, "fn main() = println(1.0e9)\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("fmt")
        .arg(&path)
        .arg("--stdout")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "fn main() = println(1000000000.0)\n");
}

/// A loop meant to carry something, with the `with` left off. This compiled
/// and answered `0`: the binding in the body was thrown away every iteration
/// and nothing said so. The error names the form that was meant.
#[test]
fn an_accumulator_written_in_the_body_names_the_with_form() {
    let err = compile_error(
        "fn main() =\n  v = buf_float(4)\n  for i in 0..4\n    v[i] = 1.0\n  acc = 0.0\n  for i in 0..4\n    acc = acc + v[i]\n  println(acc)\n",
    );
    assert!(err.contains("`acc`"), "{err}");
    assert!(err.contains("with acc"), "{err}");
}

/// The same rule without the loop: a block's value is the expression it ends
/// with, so a binding there binds a name nothing can read.
#[test]
fn a_binding_at_the_end_of_a_block_is_refused() {
    let err = compile_error("fn main() =\n  x = 1\n  y = x + 1\n");
    assert!(err.contains("`y`"), "{err}");
    assert!(err.contains("nothing can read it"), "{err}");
}

/// `fmin2` and `fmax2`, lane by lane. A NaN loses to a number, the way it
/// does under `<`, which is what lets a reduction keep its answer in lanes
/// and still agree with the scalar loop.
#[test]
fn fmin2_and_fmax2_are_lane_wise_and_ignore_a_nan() {
    let out = run_source(
        "lanes_minmax",
        "fn main() =\n  a = float2(1.0, 8.0)\n  b = float2(5.0, 2.0)\n  lo = fmin2(a, b)\n  hi = fmax2(a, b)\n  println(lane0(lo))\n  println(lane1(lo))\n  println(lane0(hi))\n  println(lane1(hi))\n  nan = 0.0 / 0.0\n  n = fmin2(float2(nan, 3.0), float2(4.0, nan))\n  println(lane0(n))\n  println(lane1(n))\n",
    );
    assert_eq!(out, "1\n2\n5\n8\n4\n3\n");
}

/// `lib/col.rill` against the plain loops it replaces. The kernels keep their
/// accumulators in lanes, so they add in a different order — over these
/// values the answer is the same to the bit, and the point of the test is
/// that the ranges, the tail past the last block of eight, and the empty
/// range all land where the loop lands.
#[test]
fn the_column_aggregates_agree_with_the_loops_they_replace() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(bad_name("col"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("col_check.rill");
    std::fs::write(
        &src,
        "import \"col\" as c\n\n\
         fn l_sum(v: Buf(Float), lo, hi) = for i in lo..hi with a = 0.0\n  a + v[i]\n\n\
         fn l_min(v: Buf(Float), lo, hi) = for i in lo..hi with a = v[lo]\n  if v[i] < a then v[i] else a\n\n\
         fn l_max(v: Buf(Float), lo, hi) = for i in lo..hi with a = v[lo]\n  if v[i] > a then v[i] else a\n\n\
         fn main() =\n  \
           n = 37\n  \
           v = buf_float(n)\n  \
           for i in 0..n\n    v[i] = to_float(i) + 0.5\n  \
           println(bool_to_str(c.sum(v, 0, n) == l_sum(v, 0, n)))\n  \
           println(bool_to_str(c.sum(v, 3, 29) == l_sum(v, 3, 29)))\n  \
           println(bool_to_str(c.sum(v, 5, 5) == 0.0))\n  \
           println(bool_to_str(c.min_of(v, 0, n) == Some(l_min(v, 0, n))))\n  \
           println(bool_to_str(c.max_of(v, 0, n) == Some(l_max(v, 0, n))))\n  \
           println(bool_to_str(c.min_of(v, 11, 12) == Some(11.5)))\n  \
           println(show(c.min_of(v, 5, 5)))\n  \
           println(show(c.mean(v, 0, n)))\n  \
           println(bool_to_str(c.dot(v, v, 0, n) == c.sum_sq(v, 0, n)))\n",
    )
    .unwrap();
    let exe = out_dir.join("col_check");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .env("RILL_PATH", root.join("lib"))
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\nNone\nSome(18.5)\ntrue\n"
    );
}


// ---------------------------------------------------------------- assembly

/// Adding two 64-bit registers, in the instruction the machine has for it.
///
/// The declaration is what differs between machines, not what the program does
/// with it — which is the point of the tests below reaching for this constant
/// rather than spelling an instruction out each time.
#[cfg(target_arch = "aarch64")]
const ASM_ADD: &str = "asm fn asm_add(a: I64, b: I64) -> I64 = \"add $0, $1, $2\" : \"=r,r,r\"\n";
#[cfg(target_arch = "x86_64")]
const ASM_ADD: &str = "asm fn asm_add(a: I64, b: I64) -> I64 = \"addq $2, $0\" : \"=r,0,r\"\n";

/// An instruction takes its operands from the language and answers into it.
#[test]
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn assembly_computes_with_its_operands() {
    let src = format!("{ASM_ADD}fn main() =\n  println(asm_add(20, 22))\n");
    assert_eq!(run_source("asm_add", &src), "42\n");
}

/// Assembly is emitted with LLVM's `sideeffect`, so two calls that look alike
/// stay two calls. A kernel reading a device register twice means it, and a
/// pass that folded the second read into the first would be reading a value
/// from before the write it was checking for.
#[test]
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn assembly_is_not_merged_with_the_call_before_it() {
    let out_dir = std::env::temp_dir().join(bad_name("asmir"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("twice.rill");
    std::fs::write(
        &path,
        format!("{ASM_ADD}fn main() =\n  println(asm_add(1, 2) + asm_add(1, 2))\n"),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(out_dir.join("twice"))
        .arg("--emit-ir")
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let ir = String::from_utf8_lossy(&build.stdout);
    assert_eq!(
        ir.matches("asm sideeffect").count(),
        2,
        "both calls should survive the optimizer:\n{ir}"
    );
}

/// A constraint string that does not describe the signature is caught here,
/// with a line number. LLVM's own answer to one is an assertion that takes the
/// compiler down with it.
#[test]
fn a_constraint_that_misses_a_parameter_is_refused() {
    let err = compile_error(
        "asm fn one(a: I64) -> I64 = \"nop\" : \"=r\"\nfn main() = println(one(1))\n",
    );
    assert!(err.contains("0 input(s)") && err.contains("1 parameter(s)"), "{err}");
}

#[test]
fn a_constraint_with_no_output_for_a_result_is_refused() {
    let err = compile_error(
        "asm fn get() -> I64 = \"nop\" : \"\"\nfn main() = println(get())\n",
    );
    assert!(err.contains("output"), "{err}");
}

/// An instruction has nowhere to put a function, and says so rather than
/// letting the trampoline that a C call would have built go somewhere odd.
#[test]
fn an_instruction_may_not_take_a_callback() {
    let err = compile_error(
        "asm fn go(f: Fn(Ptr) -> I32) = \"nop\" : \"r\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("callback"), "{err}");
}

#[test]
fn an_instruction_may_not_take_a_built_in_name() {
    let err = compile_error(
        "asm fn println(a: I64) = \"nop\" : \"r\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("already a built-in"), "{err}");
}

/// The formatter has to know the declaration, or `rill fmt` would drop it.
#[test]
fn fmt_keeps_an_assembly_declaration() {
    let out_dir = std::env::temp_dir().join(bad_name("asmfmt"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("messy.rill");
    std::fs::write(
        &src,
        "asm  fn  nap( )   =  \"nop\"\nasm fn pin(v: U64) = \"mov $0, $0\" : \"{x1}\"\nfn main()=println(1)\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("fmt")
        .arg("--stdout")
        .arg(&src)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("asm fn nap() = \"nop\"\n"), "{text}");
    assert!(text.contains("asm fn pin(v: U64) = \"mov $0, $0\" : \"{x1}\"\n"), "{text}");
}

/// wasm verifies a module before it runs it and has no place for a machine's
/// own encoding, so the refusal comes before anything is emitted.
#[test]
fn wasm_refuses_assembly() {
    let err = compile_error_for_wasm(
        "asm fn nap() = \"nop\"\nfn main() =\n  nap()\n  println(1)\n",
    );
    assert!(err.contains("no inline") && err.contains("asm fn nap"), "{err}");
}

/// A naked function is the instructions and nothing else: no prologue saving
/// registers a caller expected saved, no epilogue returning somewhere. What
/// enters it is an interrupt or a vector, and what leaves is whatever the
/// instructions say.
/// How each machine returns from an interrupt. Only the test that reaches
/// codegen needs it — the rest are refused before any assembler sees them.
#[cfg(target_arch = "aarch64")]
const NAKED_RET: &str = "eret";
#[cfg(target_arch = "x86_64")]
const NAKED_RET: &str = "iretq";

#[test]
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn a_naked_function_is_emitted_with_no_prologue() {
    let out_dir = std::env::temp_dir().join(bad_name("naked"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("k.rill");
    std::fs::write(
        &path,
        format!("naked asm fn isr_timer() = \"{NAKED_RET}\"\nfn main() = println(1)\n"),
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&path)
        .arg("-o")
        .arg(out_dir.join("k"))
        .arg("--emit-ir")
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let ir = String::from_utf8_lossy(&build.stdout);
    assert!(ir.contains("define void @isr_timer()"), "{ir}");
    assert!(ir.contains(NAKED_RET), "{ir}");
    // The attribute group the definition points at has to carry `naked`, or
    // LLVM writes the prologue back in.
    let attrs = ir
        .lines()
        .find(|l| l.starts_with("define void @isr_timer()"))
        .and_then(|l| l.split('#').nth(1))
        .and_then(|n| n.split_whitespace().next())
        .map(|n| format!("attributes #{n} ="))
        .expect("the definition names an attribute group");
    let group = ir.lines().find(|l| l.starts_with(&attrs)).unwrap_or_default();
    assert!(group.contains("naked"), "{group}");
}

/// `$` is an operand reference in LLVM's template syntax and an immediate in
/// the assembler's, and a naked function has no operands for it to be the
/// first of. Writing the instruction the way the manual has it works.
#[test]
fn a_naked_function_may_write_an_immediate() {
    let err = compile_error(
        "naked asm fn boot() = \"movq $0x1000, %rsp\"\nfn main() = boot()\n",
    );
    // It compiled far enough to reject the *call*, which is the point: the
    // template itself was accepted rather than read as operand zero.
    assert!(err.contains("naked function and cannot be called"), "{err}");
}

#[test]
fn a_naked_function_takes_no_parameters() {
    let err = compile_error(
        "naked asm fn boot(a: I64) = \"hlt\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("takes no parameters"), "{err}");
}

#[test]
fn a_naked_function_answers_with_nothing() {
    let err = compile_error(
        "naked asm fn boot() -> I64 = \"hlt\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("answers with nothing"), "{err}");
}

#[test]
fn a_naked_function_has_no_operands_to_constrain() {
    let err = compile_error(
        "naked asm fn boot() = \"hlt\" : \"=r\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("no operands to constrain"), "{err}");
}

#[test]
fn fmt_keeps_a_naked_declaration() {
    let out_dir = std::env::temp_dir().join(bad_name("nakedfmt"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let src = out_dir.join("messy.rill");
    std::fs::write(&src, "naked   asm fn  boot( )  =  \"hlt\"\nfn main()=println(1)\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("fmt")
        .arg("--stdout")
        .arg(&src)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("naked asm fn boot() = \"hlt\"\n"), "{text}");
}

#[test]
fn wasm_refuses_a_naked_function() {
    let err = compile_error_for_wasm(
        "naked asm fn boot() = \"hlt\"\nfn main() = println(1)\n",
    );
    assert!(err.contains("no inline") && err.contains("boot"), "{err}");
}

/// `lib/str.rill`: every function once, on values checked by hand — the
/// encodings against what Python's codecs, base64 and urllib say — and then
/// the strand functions against the plain scans they stand in for, at
/// several cuts, including more ranges than there are bytes of lookahead and
/// a pattern that overlaps itself.
const STRING_LIBRARY_TOUR: &str = r#"  HELLO, WORLD! HELLO  
  hello, world! hello  
AbC1
Hello
cba
nıdyanüg
Hello, World! hello  |
  Hello, World! hello|
a/b
|
a+b+c
ba
abc
a+b-c
000042
ab...|
**ab***
abc
1
3
-1
1
3
-1
-1
2
5
true false
true false
2 3 0
3
Cons(0, Cons(3, Cons(6, Nil)))
Some(body)
None
Some(file)
Some()
Cons(the, Cons(quick, Cons(brown, Cons(fox, Nil))))
Nil
Some((key, value: more))
None
(ab, cdef)
abc
0 1 2 2
-1 1 0
true false
5
-3750763034362895579
-5808556873153909620
3
3
2
Some(42) Some(-17) Some(5)
None None None Some(7)
true false
true false
true false false
true false
3 104 hey
true false 2 -1
false false false true
a��b��
Some(Cons(97, Cons(231, Nil))) None
Some(aç😀) None
true x true false
006100e7d83dde00 6100e7003dd800de 00000067000000fc
Some(aç😀) Some(günaydın İstanbul) Some(ok)
None None None
Some(a😀) None
Some(636166e920b1) None café
€“x”… Some(8093789485) None
ĞİŞğış Some(d0dddef0fdfe) None
ÇAĞRI ÉÀ ÖÜ Ĳ ß ΑΒΓΣ ЖЯ Ѐ çağri éà öü ĳ αβγ жя ѐ
İSTANBUL IŞIK ıstanbul ışık izmir ISTANBUL izmir
Cagri Ozturk Sisli GUSIOC strasse OEuvre ae n y y k n s o r t u w z l d
|Zg== Zm8= Zm9v Zm9vYmFy +/8= -_8
Some(foobar) Some(f) Some(fo) Some(fo) Some(fbff) None None Some()
52696c6c0a Some(Rill
) None None ff -1000 0
Some(255) Some(26) Some(-16) None None
a%20b%2F%C3%A7%3Fx%3D1%26y%3D~_ a+b%2Bc Some(a b/ç) Some(a b+c) None None
&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&#39;s&lt;/a&gt; <b> &amp; "q" 's' ç ç 😀 &unknown; & x &
tab\there \"q\" back\\ nl\n\x01\x7f Some(tab	here "q" back\ nl
ç😀 ) None Some(plain)
177129
true true true true true
13127 7175 17114 1315
true
true true
true
"#;

#[test]
fn string_library() {
    assert_eq!(compile_and_run("strings.rill"), STRING_LIBRARY_TOUR);
}

/// The same tour built `--parallel` and run on four workers, where
/// `par_count` and the rest really do cut the text across cores: the answers
/// must not move.
#[test]
fn string_library_on_several_workers() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-strlib-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("strings");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg("--parallel")
        .arg(root.join("examples/strings.rill"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    for threads in ["1", "4"] {
        let run = Command::new(&exe).env("RILL_THREADS", threads).output().unwrap();
        assert!(run.status.success(), "strings.rill at {threads} threads crashed");
        assert_eq!(String::from_utf8_lossy(&run.stdout), STRING_LIBRARY_TOUR, "at {threads} threads");
    }
}

/// `while cond with state = init`: a single name, a tuple of numbers, a
/// tuple carrying a list, a body that ends in an `if` with a tuple in each
/// arm, a body that is a call (the tuple travels whole), no `with` at all,
/// one loop inside another inside a nested function, a state that is a
/// name rather than a tuple written in place, and an annotated buffer in
/// the state that the code after the loop indexes. `loops.rill`, with its
/// `while` section, is `loops_are_tail_recursion`.
#[test]
fn while_loops() {
    let out = run_source(
        "while_shapes",
        "fn is_space(c) = c == 32 || c == 9\n\
         \n\
         fn main() =\n  \
           s = \"   hello world  \"\n  \
           a = while a < #s && is_space(s[a]) with a = 0\n    a + 1\n  \
           println(a)\n  \
           (i, n) = while i >= 0 with (i, n) = (str_find(s, \"o\"), 0)\n    (str_find_from(s, \"o\", i + 1), n + 1)\n  \
           println(int_to_str(i) + \" \" + int_to_str(n))\n  \
           (_, acc) = while j < #s with (j, acc) = (0, Nil)\n    if is_space(s[j]) then (j + 1, acc) else (j + 1, Cons(s[j], acc))\n  \
           println(len(acc))\n  \
           fn step(p) = match p\n    (x, y) -> (x + 1, y + x)\n  \
           (x, y) = while x < 5 with (x, y) = (0, 0)\n    step((x, y))\n  \
           println(int_to_str(x) + \" \" + int_to_str(y))\n  \
           b = buf_int(1)\n  \
           while b[0] < 10\n    b[0] += 3\n  \
           println(b[0])\n  \
           fn count_words(t) =\n    \
             (_, k) = while p < #t with (p, k) = (0, 0)\n      \
               q = while q < #t && is_space(t[q]) with q = p\n        q + 1\n      \
               if q >= #t then (q, k)\n      \
               else\n        \
                 r = while r < #t && !is_space(t[r]) with r = q\n          r + 1\n        \
                 (r, k + 1)\n    \
             k\n  \
           println(count_words(s))\n  \
           init = (3, 4)\n  \
           (u, v) = while u > 0 with (u, v) = init\n    (u - 1, v * 2)\n  \
           println(v)\n  \
           w0 = buf_int(4)\n  \
           (_, w) = while i < 4 with (i, w: Buf(Int)) = (0, w0)\n    w[i] = i * 2\n    (i + 1, w)\n  \
           println(w[3])\n",
    );
    assert_eq!(out, "3\n-1 2\n10\n5 10\n12\n2\n32\n6\n");
}

/// A function written inside another and handing back a buffer — which is
/// what a `while` with an annotated state is — used to fail to check: the
/// call to `main.go` named `main` as something `go` depends on, so the two
/// were one inference group, the parent first, and the parent's `w[3]`
/// took the not-yet-known result for a string.
#[test]
fn a_nested_function_can_hand_back_a_buffer() {
    let out = run_source(
        "nested_buf",
        "fn main() =\n  v = buf_int(4)\n  fn go(i, w: Buf(Int)) =\n    if i < 4 then\n      w[i] = i * 2\n      go(i + 1, w)\n    else w\n  w = go(0, v)\n  println(w[3])\n",
    );
    assert_eq!(out, "6\n");
}

/// A `for` in one arm of a function that also calls itself in other arms,
/// not in tail position: `eval` over an expression tree, with `Pow` a loop.
/// The loop is a `musttail` call, and LLVM's tail-recursion elimination,
/// finding `eval(a) + eval(b)`, turned that recursion into an accumulator
/// loop and rewrote every return as `acc + value` — the `musttail` one
/// included, which is not a module LLVM accepts. The function now asks
/// that pass to leave it alone.
#[test]
fn a_loop_inside_a_function_that_recurses_elsewhere() {
    let out = run_source(
        "loop_in_recursion",
        "type Expr\n  Num(Int)\n  X\n  Add(a: Expr, b: Expr)\n  Pow(a: Expr, n: Int)\n\n\
         fn eval(e, x) = match e\n  Num(n) -> n\n  X -> x\n  Add(a, b) -> eval(a, x) + eval(b, x)\n  \
         Pow(a, n) -> for _ in 0..n with p = 1 p * eval(a, x)\n\n\
         fn main() = println(eval(Pow(Add(X, Num(1)), 3), 2))\n",
    );
    assert_eq!(out, "27\n");
}

/// A `match` arm that returns takes its box apart by emptying it: when the
/// frame's reference is the only one, the fields become the frame's own
/// with no count touched and the box is a shell for the arm's answer to be
/// built in; when the box is shared, the fields are retained and the box
/// let go. `match (simp(a), simp(b))` never builds the pair at all, and its
/// parts are emptied the same way. The tree `main` keeps across two calls
/// takes the shared path and must come out whole; `@` names the box, so
/// that arm borrows as before; and the whole run frees everything.
#[test]
fn emptying_a_box_leaves_shared_ones_whole() {
    let src = r#"type E
  Num(Int)
  X
  Add(a: E, b: E)
  Mul(a: E, b: E)

# Every arm returns, so each takes its box apart by emptying it.
fn simp(e) = match e
  Add(a, b) -> match (simp(a), simp(b))
    (Num(x), Num(y)) -> Num(x + y)
    (Num(0), y) -> y
    (x, Num(0)) -> x
    (x, y) -> Add(x, y)
  Mul(a, b) -> match (simp(a), simp(b))
    (Num(x), Mul(Num(y), z)) | (Mul(Num(x), z), Num(y)) -> Mul(Num(x * y), z)
    (Num(1), y) -> y
    (x, y) -> Mul(x, y)
  other -> other

# Names the box whole, so it cannot be emptied; the fields stay borrowed.
fn keep(e) = match e
  m @ Mul(a, _) -> Add(m, a)
  Add(a, b) if a == b -> a
  other -> other

fn depth(e) = match e
  Add(a, b) | Mul(a, b) -> 1 + max(depth(a), depth(b))
  _ -> 1

fn main() =
  # a tree held by main across two calls: shared on the first, still whole after
  t = Mul(Num(2), Mul(Num(3), Add(X, Num(0))))
  s1 = simp(t)
  s2 = simp(t)
  println(show(s1))
  println(show(s2))
  println(show(t))
  # unique trees, built and dropped in place
  println(show(simp(Add(Mul(Num(1), Add(Num(2), Num(3))), Mul(Num(4), Mul(Num(5), X))))))
  println(show(keep(Mul(X, Num(7)))))
  println(show(keep(Add(Num(1), Num(1)))))
  println(show(keep(Add(Num(1), Num(2)))))
  # a shared subtree inside a unique box
  sub = Add(X, X)
  println(show(simp(Mul(Num(2), Mul(Num(3), sub)))))
  println(depth(sub))
"#;
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-empty-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let path = out_dir.join("empty.rill");
    std::fs::write(&path, src).unwrap();
    let exe = out_dir.join("empty");
    let build = Command::new(env!("CARGO_BIN_EXE_rill")).arg("build").arg(&path).arg("-o").arg(&exe).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).env("RILL_DEBUG_ALLOC", "1").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "Mul(Num(6), X)\nMul(Num(6), X)\nMul(Num(2), Mul(Num(3), Add(X, Num(0))))\n\
         Add(Num(5), Mul(Num(20), X))\nAdd(Mul(X, Num(7)), X)\nNum(1)\nAdd(Num(1), Num(2))\n\
         Mul(Num(6), Add(X, X))\n2\n"
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("live allocations: 0"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// `lib/kv.rill` — the store as a program uses it, then under stress. The
/// example puts, commits, rolls one back, deletes, scans, is refused a
/// second open of the same file, and reads everything back after a close.
#[test]
fn key_value_store() {
    assert_eq!(
        compile_and_run("kv_store.rill"),
        "Some(1906)\nNone\nSome(1815)\nCons((ada, 1815), Cons((barbara, 1939), Nil))\n3\n\
         /tmp/rill_kv_example.db is open elsewhere\n\
         Cons((ada, 1815), Cons((barbara, 1939), Cons((grace, 1906), Nil)))\n"
    );
}

/// Twenty thousand random keys against a map, through splits at every
/// level: puts, overwrites, deletes, a range scan that must come back
/// sorted, a rollback, a close and a reopen — and then a commit with no
/// close, which leaves the pages in the log and not the tree, so the second
/// program's open has to replay them, and free everything at its close.
#[test]
fn key_value_store_under_stress_and_recovery() {
    let root = workspace_root();
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-kv-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut outs = Vec::new();
    for (name, src) in [("stress", STRESS), ("after", AFTER)] {
        let path = out_dir.join(format!("{name}.rill"));
        std::fs::write(&path, src).unwrap();
        let exe = out_dir.join(name);
        let build = Command::new(env!("CARGO_BIN_EXE_rill"))
            .arg("build")
            .arg(&path)
            .arg("-o")
            .arg(&exe)
            .env("RILL_PATH", root.join("lib"))
            .output()
            .unwrap();
        assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
        let run = Command::new(&exe).env("RILL_DEBUG_ALLOC", "1").output().unwrap();
        assert!(run.status.success(), "{name} failed: {}", String::from_utf8_lossy(&run.stderr));
        // The first program leaves through `exit`, which is the crash being
        // staged and prints no ledger; the second returns from `main`.
        if name == "after" {
            assert!(
                String::from_utf8_lossy(&run.stderr).contains("live allocations: 0"),
                "{name}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
        outs.push(String::from_utf8_lossy(&run.stdout).to_string());
    }
    assert_eq!(
        outs[0],
        "inserted, count 18219 oracle 18219\nafter updates, count 20658 oracle 20658\nmismatches 0\n\
         absent wrong 0\nrange k5..k6: 2311 sorted 2311\nafter rollback: true\n\
         reopened, count 20658 mismatches 0\ncommitted without close\n"
    );
    assert_eq!(outs[1], "recovered: Some(survives)\ncount 20659\n16\n");
}

const STRESS: &str = r#"import "kv"

fn key_of(x) = "k" + int_to_str(x % 100003)

fn val_of(x) = "v" + int_to_str(x) + repeat("x", x % 37)

fn nxt(x) = bit_and(x * 1103515245 + 12345, 2147483647)

fn check(db: Db, oracle, keys, bad) = match keys
  Nil -> bad
  Cons(k, rest) ->
    want = map_get(oracle, k)
    got = kv_get(db, k)
    check(db, oracle, rest, if want == got then bad else bad + 1)

fn sorted_ok(l, prev, n) = match l
  Nil -> n
  Cons((k, _), rest) -> sorted_ok(rest, k, if str_cmp(prev, k) < 0 then n + 1 else n)

fn main() =
  file_remove("/tmp/rill-e2e-kv.db")
  file_remove("/tmp/rill-e2e-kv.db.wal")
  n = 20000
  oracle = map_new()
  match kv_open("/tmp/rill-e2e-kv.db")
    Err(e) -> println(e)
    Ok(db) ->
      # insert
      for i in 0..n with x = 42
        y = nxt(x)
        kv_put(db, key_of(y), val_of(y))
        oracle[key_of(y)] = val_of(y)
        y
      kv_commit(db)
      println("inserted, count " + int_to_str(kv_count(db)) + " oracle " + int_to_str(map_len(oracle)))
      # overwrite and delete some
      for i in 0..n / 4 with x = 7
        y = nxt(x)
        if y % 3 == 0 then
          kv_delete(db, key_of(y))
          map_del(oracle, key_of(y))
          0
        else
          kv_put(db, key_of(y), "new" + int_to_str(y))
          oracle[key_of(y)] = "new" + int_to_str(y)
          0
        y
      kv_commit(db)
      println("after updates, count " + int_to_str(kv_count(db)) + " oracle " + int_to_str(map_len(oracle)))
      println("mismatches " + int_to_str(check(db, oracle, map_keys(oracle), 0)))
      # keys that must be absent
      absent = for i in 0..1000 with bad = 0
        if kv_get(db, "zz" + int_to_str(i)) == None then bad else bad + 1
      println("absent wrong " + int_to_str(absent))
      r = kv_range(db, "k5", "k6")
      println("range k5..k6: " + int_to_str(len(r)) + " sorted " + int_to_str(sorted_ok(r, "", 0)))
      # uncommitted change, then rollback
      kv_put(db, "k1", "rolled")
      kv_rollback(db)
      println("after rollback: " + show(kv_get(db, "k1") == map_get(oracle, "k1")))
      kv_close(db)
  # reopen: everything must still be there
  match kv_open("/tmp/rill-e2e-kv.db")
    Err(e) -> println(e)
    Ok(db) ->
      println("reopened, count " + int_to_str(kv_count(db)) + " mismatches " + int_to_str(check(db, oracle, map_keys(oracle), 0)))
      kv_put(db, "crash", "survives")
      kv_commit(db)
      # no close: the tree file has not got the page, the log has
      println("committed without close")
      exit(0)
"#;

const AFTER: &str = r#"import "kv"
fn main() =
  match kv_open("/tmp/rill-e2e-kv.db")
    Err(e) -> println(e)
    Ok(db) ->
      println("recovered: " + show(kv_get(db, "crash")))
      println("count " + int_to_str(kv_count(db)))
      kv_close(db)
  println(file_size(file_open("/tmp/rill-e2e-kv.db.wal", "r")))
"#;

/// Built with `--parallel` and run on eight workers with the allocator's
/// checks on, several times over: a program whose strands share a list
/// and keep copying it. The counts on the shared cells are taken up and
/// down from several workers at once; one taken down with a plain store
/// — as the in-place-reuse path of a `match` once did — loses another
/// worker's retain, and the list is freed under its holders.
#[test]
fn shared_list_counts_stay_right_across_workers() {
    require_fresh_runtime();
    let root = workspace_root();
    let source = root.join("examples").join("shared_list.rill");
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("shared_list");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&source)
        .arg("--parallel")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "rill build shared_list.rill failed:\n{}", String::from_utf8_lossy(&build.stderr));
    for _ in 0..5 {
        let run = Command::new(&exe)
            .env("RILL_THREADS", "8")
            .env("RILL_ALLOC_CHECK", "1")
            .output()
            .expect("failed to run produced executable");
        assert!(run.status.success(), "shared_list died:\n{}", String::from_utf8_lossy(&run.stderr));
        assert_eq!(String::from_utf8_lossy(&run.stdout), "332800000\n");
    }
}

/// A builder that grew past the pool's largest block, holding a string
/// short enough for the pool: taking the block over as the string would
/// free it, by the string's length, onto a pool free list it never came
/// from — a server's every 200-byte answer kept for good. The runtime's
/// checks report exactly that free, so a program that makes and drops
/// such strings by the thousand under them must come through clean. The
/// same program adds up `str_len(answer(i))` over strings beyond the
/// pool, whose header `free` writes over: the length has to be read
/// before the release, which `invariant.load` on it once let LLVM undo.
#[test]
fn short_string_in_a_big_builder_is_copied_not_taken() {
    require_fresh_runtime();
    let root = workspace_root();
    let source = root.join("examples").join("short_of_long.rill");
    let out_dir = std::env::temp_dir().join(format!("rill-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("short_of_long");
    let build = Command::new(env!("CARGO_BIN_EXE_rill"))
        .arg("build")
        .arg(&source)
        .arg("--parallel")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to spawn rill");
    assert!(build.status.success(), "rill build short_of_long.rill failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).env("RILL_ALLOC_CHECK", "1").output().expect("failed to run produced executable");
    assert!(run.status.success(), "short_of_long died:\n{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "200000\n300000\n");
}

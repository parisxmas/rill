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

/// The annotation is still needed where inference genuinely never learns the
/// type, and that is what the error says now — at the end, having looked.
#[test]
fn a_field_on_a_type_never_learned_still_asks() {
    let src = "fn f(p) = p.x\n\n\
               fn main() = println(\"x\")\n";
    let err = compile_error(src);
    assert!(
        err.contains("the type of `p` is never known") && err.contains("`p: SomeType`"),
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

/// The valley with several people in it. `valley.rill` is the file the browser
/// compiles to wasm and the server compiles for a Linux box, and this runs the
/// same arithmetic with neither: three players join one world, one of them
/// walks, one shoots another down, and one leaves.
#[test]
fn shared_valley() {
    assert_eq!(
        compile_and_run("webgpu/world/check.rill"),
        concat!(
            "300 crystals and 400 pines, nobody in it yet: 0\n",
            "three arrive: 3, with ids 0 1 2\n",
            "  the first came in at 17, -40, on ground 69 decimetres up\n",
            "  the second came in at 28, 36, on ground 29 decimetres up\n",
            "  the third came in at -41, -32, on ground 89 decimetres up\n",
            "the first is at -40, -40 on 1\n",
            "the third is at -29, -41 on 1\n",
            "the first shoots the second in the leg, the body and the head:\n",
            "  they are on 0.9, the shooter still on 1\n",
            "  they are on 0.6, the shooter still on 1\n",
            "  they are on 0, the shooter still on 1\n",
            "  the first has landed 3 and taken 0\n",
            "  the second has landed 0 and taken 3\n",
            "after a respawn they are on 1\n",
            "  with the shooter looking away, they are still on 1\n",
            "  and after one of those the server does not want them gone yet: 0\n",
            "  after 12 of them, drop this connection: 1\n",
            "  one honest shot and they are welcome again: 0\n",
            "  they are on 0.7, the shooter still on 1\n",
            "the camera was level, then thrown, and level again\n",
            "with nobody drawn there, the round is still flying ten metres on\n",
            "with somebody drawn there, it is stopped at the same point\n",
            "  they are on 0, the shooter still on 1\n",
            "shot down: down, camera thrown, eye at 0.980103\n",
            "six seconds later: down, camera thrown, eye at 0.35\n",
            "eleven seconds later: on their feet, camera level, eye at 1.75\n",
            "three metres apart on the same ground: yes, and the other way yes\n",
            "forty metres away: yes; a hundred and thirty: no\n",
            "across the valley, 11 of 16 places are in sight of the middle\n",
            "one leaves: 2 remain, ids 0 2\n",
            "the valley is put away\n",
        )
    );
}

/// And the server in front of it — compiled rather than run, because it holds
/// a port and a world and does not stop.
#[test]
fn valley_server_compiles() {
    compile_only("webgpu/world/server.rill");
}

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
        "ada at 0,0 hp 100\n65\n100\nada at 3,4 hp 65\n5\n9\ntrue\n"
    );
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
         10\n\
         15\n\
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

//! rill_syntax: lexer, layout handling, parser and AST for the Rill language.

pub mod ast;
pub mod fmt;
pub mod lexer;
pub mod lift;
pub mod loader;
pub mod parser;
pub mod prelude;

pub use ast::*;
pub use fmt::format_source;
pub use loader::load_program;
pub use parser::parse;
pub use prelude::{parse_program, PRELUDE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl SyntaxError {
    pub fn new(line: usize, col: usize, msg: String) -> Self {
        SyntaxError { line, col, msg }
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "syntax error at line {}, column {}: {}", self.line, self.col + 1, self.msg)
    }
}

impl std::error::Error for SyntaxError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_fn() {
        let m = parse("fn add(a, b) = a + b\n").unwrap();
        assert_eq!(m.functions.len(), 1);
        assert_eq!(m.functions[0].name, "add");
        let names: Vec<&str> = m.functions[0].params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn parses_annotations() {
        let m = parse("fn mul(a: Float, b: Float) -> Float = a * b\n").unwrap();
        let f = &m.functions[0];
        assert_eq!(f.params[0].ann.as_ref().unwrap().name, "Float");
        assert_eq!(f.ret_ann.as_ref().unwrap().name, "Float");
    }

    #[test]
    fn parses_type_and_match() {
        let src = "type Shape\n  Circle(r: Float)\n  Rect(Float, Float)\n\nfn area(s: Shape) -> Float = match s\n  Circle(r) -> 3.0 * r * r\n  Rect(w, h) -> w * h\n";
        let m = parse(src).unwrap();
        assert_eq!(m.types.len(), 1);
        assert_eq!(m.types[0].ctors.len(), 2);
        assert_eq!(m.types[0].ctors[0].fields.len(), 1);
        match &m.functions[0].body {
            Expr::Match { arms, .. } => assert_eq!(arms.len(), 2),
            other => panic!("expected match, got {other:?}"),
        }
    }

    #[test]
    fn parses_match_with_wildcard_and_literals() {
        let src = "fn f(n) = match n\n  0 -> 1\n  -1 -> 2\n  _ -> n\n";
        let m = parse(src).unwrap();
        match &m.functions[0].body {
            Expr::Match { arms, .. } => {
                assert_eq!(arms[0].pattern, Pattern::Int(0));
                assert_eq!(arms[1].pattern, Pattern::Int(-1));
                assert_eq!(arms[2].pattern, Pattern::Wildcard);
            }
            other => panic!("expected match, got {other:?}"),
        }
    }

    #[test]
    fn if_may_span_lines() {
        let src = "fn f(x) =\n  if x > 0 then \"pos\"\n  else \"neg\"\n";
        parse(src).unwrap();
        let src2 = "fn f(x) =\n  if x > 0\n  then \"pos\"\n  else \"neg\"\n";
        parse(src2).unwrap();
    }

    #[test]
    fn line_break_still_ends_a_statement() {
        // `else` continues an `if`; an ordinary expression on the next line
        // must still be a separate statement.
        let src = "fn f(x) =\n  println(x)\n  x + 1\n";
        let m = parse(src).unwrap();
        match &m.functions[0].body {
            Expr::Block(stmts) => assert_eq!(stmts.len(), 2),
            other => panic!("expected block, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_patterns() {
        let src = "fn f(x) = match x\n  Some(Cons(0, _)) -> 1\n  _ -> 0\n";
        let m = parse(src).unwrap();
        match &m.functions[0].body {
            Expr::Match { arms, .. } => match &arms[0].pattern {
                Pattern::Ctor { name, args, .. } => {
                    assert_eq!(name, "Some");
                    match &args[0] {
                        Pattern::Ctor { name, args, .. } => {
                            assert_eq!(name, "Cons");
                            assert_eq!(args[0], Pattern::Int(0));
                            assert_eq!(args[1], Pattern::Wildcard);
                        }
                        other => panic!("expected nested ctor, got {other:?}"),
                    }
                }
                other => panic!("expected ctor pattern, got {other:?}"),
            },
            other => panic!("expected match, got {other:?}"),
        }
    }

    #[test]
    fn parses_generic_type_def() {
        let src = "type List(a)\n  Nil\n  Cons(head: a, tail: List(a))\n\nfn main() = println(1)\n";
        let m = parse(src).unwrap();
        assert_eq!(m.types[0].params, vec!["a"]);
        let tail = &m.types[0].ctors[1].fields[1].1;
        assert_eq!(tail.name, "List");
        assert_eq!(tail.args[0].name, "a");
    }

    #[test]
    fn parses_lambda() {
        let m = parse("fn main() = apply(\\x, y -> x + y)\n").unwrap();
        match &m.functions[0].body {
            Expr::Call { args, .. } => match &args[0] {
                Expr::Lambda { params, .. } => assert_eq!(params, &vec!["x", "y"]),
                other => panic!("expected lambda, got {other:?}"),
            },
            other => panic!("expected call, got {other:?}"),
        }
    }

    #[test]
    fn parses_block_fn_with_bindings() {
        let src = "fn main() =\n  x = 1 + 2\n  println(x)\n";
        let m = parse(src).unwrap();
        let body = &m.functions[0].body;
        match body {
            Expr::Block(stmts) => assert_eq!(stmts.len(), 2),
            other => panic!("expected block, got {other:?}"),
        }
    }

    #[test]
    fn parses_two_fns_and_if() {
        let src = "fn fib(n) = if n < 2 then n else fib(n - 1) + fib(n - 2)\n\nfn main() =\n  println(fib(10))\n";
        let m = parse(src).unwrap();
        assert_eq!(m.functions.len(), 2);
    }

    #[test]
    fn pipe_desugars_to_call() {
        let m = parse("fn main() = 5 |> println\n").unwrap();
        match &m.functions[0].body {
            Expr::Call { name, args, .. } => {
                assert_eq!(name, "println");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected call, got {other:?}"),
        }
    }

    #[test]
    fn pipe_prepends_argument() {
        let m = parse("fn main() = 5 |> add(1) |> println\n").unwrap();
        match &m.functions[0].body {
            Expr::Call { name, args, .. } => {
                assert_eq!(name, "println");
                match &args[0] {
                    Expr::Call { name, args, .. } => {
                        assert_eq!(name, "add");
                        assert_eq!(args.len(), 2);
                        assert_eq!(args[0], Expr::Int(5));
                    }
                    other => panic!("expected inner call, got {other:?}"),
                }
            }
            other => panic!("expected call, got {other:?}"),
        }
    }

    #[test]
    fn multiline_expr_inside_parens() {
        let src = "fn main() = println(1 +\n    2 +\n    3)\n";
        parse(src).unwrap();
    }

    #[test]
    fn rejects_bad_dedent() {
        let src = "fn main() =\n    x = 1\n  x\n";
        assert!(parse(src).is_err());
    }

    #[test]
    fn rejects_tabs() {
        assert!(parse("fn main() =\n\tx = 1\n").is_err());
    }
}
